//! Public (friend-facing) HTTP API: link view and claim flow.
//!
//! Routes: `GET /api/l/:token`, `POST /api/l/:token/claim`, fallback 404.
use std::sync::Arc;

use async_trait::async_trait;
use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
};
use dynamo::{ClaimTxError, Store};
use fulfillment::{FulfillRequest, FulfillResponse};
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

// ── Invoker trait ─────────────────────────────────────────────────────────────

/// Synchronous bridge to the fulfillment lambda. `Arc<dyn Invoker>`-friendly.
#[async_trait]
pub trait Invoker: Send + Sync {
    async fn gift(&self, req: FulfillRequest) -> Result<FulfillResponse, String>;
}

// ── LambdaInvoker ─────────────────────────────────────────────────────────────

/// Production invoker: `InvocationType::RequestResponse` to the fulfillment lambda.
pub struct LambdaInvoker {
    pub client: aws_sdk_lambda::Client,
    pub fn_name: String,
}

#[async_trait]
impl Invoker for LambdaInvoker {
    async fn gift(&self, req: FulfillRequest) -> Result<FulfillResponse, String> {
        let payload = serde_json::to_vec(&req).map_err(|e| e.to_string())?;
        let resp = self
            .client
            .invoke()
            .function_name(&self.fn_name)
            .invocation_type(aws_sdk_lambda::types::InvocationType::RequestResponse)
            .payload(aws_sdk_lambda::primitives::Blob::new(payload))
            .send()
            .await
            .map_err(|e| format!("{e:?}"))?;
        let blob = resp
            .payload()
            .ok_or_else(|| "no payload in lambda response".to_string())?;
        serde_json::from_slice(blob.as_ref()).map_err(|e| e.to_string())
    }
}

// ── App state ─────────────────────────────────────────────────────────────────

#[derive(Clone)]
struct AppState {
    store: Arc<Store>,
    invoker: Arc<dyn Invoker>,
}

// ── Response shapes ───────────────────────────────────────────────────────────

#[derive(Serialize)]
struct GameView {
    id: String,
    title: String,
    bundle: String,
    key_type: String,
    artwork_url: Option<String>,
}

#[derive(Serialize)]
struct ClaimView {
    game_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    title: Option<String>,
    state: String,
    gift_url: Option<String>,
}

#[derive(Serialize)]
struct LinkView {
    label: String,
    claims_allowed: u32,
    claims_used: u32,
    /// Explicit link state: "active" | "revoked" | "expired" | "exhausted".
    /// The SINGLE liveness representation on the wire — the client renders
    /// banners and gates claim buttons from this; it must never have to infer
    /// the reason from side signals like games.len().
    state: &'static str,
    games: Vec<GameView>,
    claims: Vec<ClaimView>,
}

// ── Router ────────────────────────────────────────────────────────────────────

/// Build the axum router. `store` is `Arc<Store>` so callers can share one store
/// across multiple oneshot calls in tests.
pub fn router(store: Arc<Store>, invoker: Arc<dyn Invoker>) -> Router {
    let state = AppState { store, invoker };
    Router::new()
        .route("/api/l/:token", get(handle_get_link))
        .route("/api/l/:token/claim", post(handle_post_claim))
        .with_state(state)
        .fallback(handle_not_found)
}

async fn handle_not_found() -> Response {
    (
        StatusCode::NOT_FOUND,
        Json(serde_json::json!({"error": "not found"})),
    )
        .into_response()
}

// ── GET /api/l/:token ─────────────────────────────────────────────────────────

async fn handle_get_link(State(s): State<AppState>, Path(token): Path<String>) -> Response {
    let link = match s.store.get_link(&token).await {
        Ok(Some(l)) => l,
        Ok(None) => {
            // Byte-identical for ANY invalid token — no enumeration oracle.
            return (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({"error": "unknown link"})),
            )
                .into_response();
        }
        Err(_) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": "try again"})),
            )
                .into_response();
        }
    };

    let now = OffsetDateTime::now_utc();
    // state: single can_claim rule — one implementation, never a manual re-derivation
    let state = match link.can_claim(now) {
        Ok(()) => "active",
        Err(domain::ClaimRefusal::Revoked) => "revoked",
        Err(domain::ClaimRefusal::Expired) => "expired",
        Err(domain::ClaimRefusal::Exhausted) => "exhausted",
    };

    // Revoked/expired: show no games (link is dead; don't leak catalog).
    // Exhausted: games stay visible so the friend can browse; claim buttons
    // are disabled client-side via claims_used == claims_allowed.
    let games: Vec<GameView> = if state == "revoked" || state == "expired" {
        vec![]
    } else {
        match s.store.list_listable_games().await {
            Ok(gs) => gs
                .into_iter()
                .map(|g| GameView {
                    id: g.id,
                    title: g.title,
                    bundle: g.bundle,
                    key_type: g.key_type,
                    artwork_url: g.artwork_url,
                })
                .collect(),
            Err(_) => vec![],
        }
    };

    // Claims history is ALWAYS returned intact (spec §7). Titles come from a
    // per-claim game lookup: claimed games leave the listable set, so the
    // `games` list above can't provide them. Claims per link are bounded by
    // claims_allowed (single digits), so the extra gets are fine. A failed
    // lookup degrades to title:None — the client falls back to game_id.
    let claims: Vec<ClaimView> = match s.store.claims_for_link(&token).await {
        Ok(cs) => {
            let mut views = Vec::with_capacity(cs.len());
            for c in cs {
                let title = match s.store.get_game(&c.game_id).await {
                    Ok(Some(g)) => Some(g.title),
                    _ => None,
                };
                views.push(ClaimView {
                    game_id: c.game_id,
                    title,
                    state: claim_state_str(c.state),
                    gift_url: c.gift_url,
                });
            }
            views
        }
        Err(_) => vec![],
    };

    (
        StatusCode::OK,
        Json(LinkView {
            label: link.label,
            claims_allowed: link.claims_allowed,
            claims_used: link.claims_used,
            state,
            games,
            claims,
        }),
    )
        .into_response()
}

fn claim_state_str(state: domain::ClaimState) -> String {
    serde_json::to_value(state)
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_else(|| "unknown".to_string())
}

// ── POST /api/l/:token/claim ──────────────────────────────────────────────────

#[derive(Deserialize)]
struct ClaimBody {
    game_id: String,
}

async fn handle_post_claim(
    State(s): State<AppState>,
    Path(token): Path<String>,
    Json(body): Json<ClaimBody>,
) -> Response {
    // 1. Resolve link — same 404 shape as unknown token.
    let link = match s.store.get_link(&token).await {
        Ok(Some(l)) => l,
        Ok(None) => {
            return (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({"error": "unknown link"})),
            )
                .into_response();
        }
        Err(_) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": "try again"})),
            )
                .into_response();
        }
    };

    // 2. Domain gate (fast pre-check before the DDB transaction).
    let now = OffsetDateTime::now_utc();
    if let Err(refusal) = link.can_claim(now) {
        use domain::ClaimRefusal;
        let msg = match refusal {
            ClaimRefusal::Revoked => "this link has been revoked",
            ClaimRefusal::Expired => "this link has expired",
            ClaimRefusal::Exhausted => "no claims left on this link",
        };
        return (
            StatusCode::CONFLICT,
            Json(serde_json::json!({"error": msg})),
        )
            .into_response();
    }

    // 3. Atomic claim intake: GAME available→pending, LINK counter +1, CLAIM created.
    let claim_id = uuid::Uuid::new_v4().to_string();
    if let Err(e) = s
        .store
        .claim_game(&token, &body.game_id, &claim_id, now)
        .await
    {
        return match e {
            ClaimTxError::GameUnavailable => (
                StatusCode::CONFLICT,
                Json(serde_json::json!({"error": "someone beat you to it"})),
            )
                .into_response(),
            ClaimTxError::LinkNotClaimable => (
                StatusCode::CONFLICT,
                Json(serde_json::json!({"error": "no claims left on this link"})),
            )
                .into_response(),
            // Should be unreachable with uuid v4, but map it loudly.
            ClaimTxError::DuplicateClaim => (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": "unexpected duplicate claim id"})),
            )
                .into_response(),
            ClaimTxError::Store(_) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": "try again"})),
            )
                .into_response(),
        };
    }

    // 4. Read game fields needed for FulfillRequest::Gift. Claim already landed —
    //    any failure here parks (reconcile owns the outcome).
    let game = match s.store.get_game(&body.game_id).await {
        Ok(Some(g)) => g,
        _ => return park_response(),
    };

    // 5. Invoke fulfillment lambda (RequestResponse = synchronous).
    let fulfill_req = FulfillRequest::Gift {
        claim_id,
        link_token: token,
        game_id: body.game_id,
        gamekey: game.gamekey,
        machine_name: game.machine_name,
        keyindex: game.keyindex,
    };

    match s.invoker.gift(fulfill_req).await {
        Ok(FulfillResponse::GiftUrl { url }) => {
            (StatusCode::OK, Json(serde_json::json!({"gift_url": url}))).into_response()
        }
        Ok(FulfillResponse::AlreadyRedeemed) => (
            StatusCode::GONE,
            Json(serde_json::json!({
                "error": "that key was already redeemed on humble — pick another"
            })),
        )
            .into_response(),
        // Parked, Error, transport failure, or any unexpected variant:
        // claim intake succeeded; reconcile owns the fate.
        _ => park_response(),
    }
}

/// 202 "processing" — the claim is recorded; the gift link is coming.
fn park_response() -> Response {
    (
        StatusCode::ACCEPTED,
        Json(serde_json::json!({
            "status": "processing",
            "message": "your claim is recorded — the gift link is taking longer than usual; check back on this page"
        })),
    )
        .into_response()
}
