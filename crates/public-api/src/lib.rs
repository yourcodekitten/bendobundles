//! Public (friend-facing) HTTP API: link view and claim flow.
//!
//! Routes: `GET /api/l/:token`, `POST /api/l/:token/claim`,
//!         `POST /api/l/:token/thanks`,
//!         `GET /api/steam/login`, `GET /api/steam/return`,
//!         `GET /api/l/:token/steam/owned/:steamid`, fallback 404.
use std::sync::Arc;

use async_trait::async_trait;
use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::{StatusCode, header},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use dynamo::{ClaimTxError, Store};
use fulfillment::{FulfillRequest, FulfillResponse};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use steam_client::{OwnedGames, SteamClient, SteamId64};
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
pub struct AppState {
    store: Arc<Store>,
    invoker: Arc<dyn Invoker>,
    /// Steam client. `None` ⇒ all `/api/steam/*` endpoints return 503.
    steam: Option<Arc<SteamClient>>,
    /// Server-trusted base URL (e.g. "https://bendobundles.com").
    /// Used to reconstruct `expected_return_to` in the OpenID return endpoint
    /// from config — NEVER from Host/X-Forwarded-* headers.
    base_url: String,
}

// ── Response shapes ───────────────────────────────────────────────────────────

#[derive(Serialize)]
struct GameView {
    id: String,
    title: String,
    bundle: String,
    key_type: String,
    artwork_url: Option<String>,
    steam_app_id: Option<u32>,
    /// First ~5 steam genres from the enrichment cache (cache-only,
    /// best-effort). Empty → omitted from the wire. The detail endpoint
    /// always leaves this empty — the modal reads the full steam blob.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    genres: Vec<String>,
    /// Top community tags (popularity order, ≤10) from the enrichment cache — the card
    /// chips (#71). Genres stay as the fallback for tag-less apps AND for deploy-window
    /// back-compat (an older cached SPA bundle still reads `genres`). Empty → omitted.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tags: Vec<String>,
}

impl GameView {
    /// The ONE friend-visible projection of a game — both the list and the
    /// detail endpoint build their `game` objects here so the two wire shapes
    /// can't drift field-by-field. `genres` is the list endpoint's enrichment;
    /// the detail endpoint passes an empty vec (key omitted on the wire — the
    /// modal reads the full steam blob instead).
    fn from_game(g: domain::Game, genres: Vec<String>, tags: Vec<String>) -> Self {
        Self {
            id: g.id,
            title: g.title,
            bundle: g.bundle,
            key_type: g.key_type,
            artwork_url: g.artwork_url,
            steam_app_id: g.steam_app_id,
            genres,
            tags,
        }
    }
}

#[derive(Serialize)]
struct ClaimView {
    game_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    title: Option<String>,
    /// Serialized via domain::ClaimState's own serde (snake_case) — the one
    /// representation, shared with admin-api's AdminClaimView.
    state: domain::ClaimState,
    gift_url: Option<String>,
}

#[derive(Serialize)]
struct LinkView {
    label: String,
    /// Ben's personal note to the friend; rendered in the page-load dialog.
    /// Omitted from the JSON entirely when unset.
    #[serde(skip_serializing_if = "Option::is_none")]
    gift_note: Option<String>,
    /// The friend's own thank-you, echoed back so a revisit renders "sent"
    /// instead of the compose card. Omitted when never thanked — the client
    /// gates on field presence, same as gift_note.
    #[serde(skip_serializing_if = "Option::is_none")]
    thank_note: Option<String>,
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
pub fn router(
    store: Arc<Store>,
    invoker: Arc<dyn Invoker>,
    steam: Option<Arc<SteamClient>>,
    base_url: String,
) -> Router {
    let state = AppState {
        store,
        invoker,
        steam,
        base_url,
    };
    Router::new()
        .route("/api/l/:token", get(handle_get_link))
        .route("/api/l/:token/claim", post(handle_post_claim))
        .route("/api/l/:token/thanks", post(handle_post_thanks))
        .route(
            "/api/l/:token/steam/owned/:steamid",
            get(handle_steam_owned_proxy),
        )
        .route("/api/l/:token/games/:id/detail", get(handle_game_detail))
        .route("/api/steam/login", get(handle_steam_login))
        .route("/api/steam/return", get(handle_steam_return))
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

// ── ctx allowlist ─────────────────────────────────────────────────────────────

/// Validate a `ctx` parameter against the allowlist:
///   - Exactly `/admin`
///   - Or `/admin/` followed by exactly one path segment of one-or-more lowercase ASCII
///     letters `[a-z]+` (the admin SPA subroutes: `catalog`, `links`, `ops`).
///     Equivalent regex `^/admin(/[a-z]+)?$`.  No second slash, no digits, no uppercase,
///     no dots, no backslashes — anything else is rejected.
///   - Or `/l/` followed by exactly 64 lowercase hex characters.
///
/// Returns `true` iff `ctx` is on the allowlist. ONE shared function used by
/// BOTH the login and return endpoints — duplication-safe by construction.
fn ctx_is_allowed(ctx: &str) -> bool {
    if ctx == "/admin" {
        return true;
    }
    if let Some(seg) = ctx.strip_prefix("/admin/") {
        // One segment only: one-or-more lowercase ASCII letters, nothing else.
        return !seg.is_empty()
            && !seg.contains('/')
            && seg.bytes().all(|b: u8| b.is_ascii_lowercase());
    }
    if let Some(token) = ctx.strip_prefix("/l/") {
        return token.len() == 64
            && token
                .bytes()
                .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'));
    }
    false
}

// ── return_to URL helper ──────────────────────────────────────────────────────

/// Build the OpenID `return_to` URL from the server-trusted `base_url` and the
/// (already-validated) `ctx`. Both the login endpoint (emitting it) and the return
/// endpoint (expecting it) call this helper — byte-match by construction.
///
/// Security: `base_url` comes from config (env-threaded into AppState), NEVER
/// from Host/X-Forwarded-* request headers — this is the critical gate.
fn build_return_to(base_url: &str, ctx: &str) -> String {
    format!(
        "{}/api/steam/return?ctx={}",
        base_url,
        urlencoding::encode(ctx)
    )
}

// ── Redirect helper ───────────────────────────────────────────────────────────

/// Build a 302 Found response with the given Location. Panics if `location`
/// contains characters that are invalid in an HTTP header value (not expected
/// for any URL we construct — all are ASCII percent-encoded).
fn redirect_to(location: &str) -> Response {
    let hv = axum::http::HeaderValue::from_str(location)
        .expect("redirect location must be a valid header value");
    (StatusCode::FOUND, [(header::LOCATION, hv)]).into_response()
}

// ── GET /api/steam/login ──────────────────────────────────────────────────────

async fn handle_steam_login(
    State(s): State<AppState>,
    Query(params): Query<HashMap<String, String>>,
) -> Response {
    let ctx = params.get("ctx").cloned().unwrap_or_default();

    // Initiation-side ctx validation (security gate B1): allowlist enforced
    // at login too — not just at return. Bad ctx → 302 / (no fragment).
    if !ctx_is_allowed(&ctx) {
        return redirect_to("/");
    }

    // Guard: steam must be configured. If not, redirect back to ctx with an error fragment
    // so the SPA can show a polite message instead of a dead-end 503 on return.
    if s.steam.is_none() {
        return redirect_to(&format!("{ctx}#steam_error=steam_unreachable"));
    }

    let return_to = build_return_to(&s.base_url, &ctx);
    let redirect_url = steam_client::steam_openid_redirect_url(&s.base_url, &return_to);

    // Redirect to Steam's OpenID endpoint (302 Found).
    redirect_to(&redirect_url)
}

// ── GET /api/steam/return ─────────────────────────────────────────────────────

async fn handle_steam_return(
    State(s): State<AppState>,
    // Vec<(String,String)> preserves duplicate keys — required so that
    // verify_openid_assertion's DUP_GUARD can detect a forged second
    // openid.claimed_id before it reaches Steam.  HashMap would silently
    // collapse duplicates, making the guard dead code at the endpoint level.
    Query(all_params): Query<Vec<(String, String)>>,
) -> Response {
    // Take the FIRST occurrence of `ctx` (first-occurrence semantics, consistent
    // with how the steam-client crate's get() helper works for openid.* params).
    let ctx = all_params
        .iter()
        .find(|(k, _)| k == "ctx")
        .map(|(_, v)| v.clone())
        .unwrap_or_default();

    // ctx allowlist — failure → 302 `/` no fragment.
    if !ctx_is_allowed(&ctx) {
        return redirect_to("/");
    }

    // Require steam client — 503 if unconfigured.
    let steam = match s.steam.as_ref() {
        Some(c) => c,
        None => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(serde_json::json!({"error": "steam not configured"})),
            )
                .into_response();
        }
    };

    // Reconstruct expected_return_to from server-trusted BASE_URL config —
    // NEVER from any request header. Both login and return call build_return_to
    // → byte-match by construction.
    let expected_return_to = build_return_to(&s.base_url, &ctx);

    // Collect all params except `ctx` as the openid.* assertion params.
    let openid_params: Vec<(String, String)> =
        all_params.into_iter().filter(|(k, _)| k != "ctx").collect();

    // Verify the OpenID assertion.
    let steamid = match steam
        .verify_openid_assertion(&openid_params, &expected_return_to)
        .await
    {
        Ok(id) => id,
        Err(steam_client::SteamError::OpenIdRejected(_)) => {
            return redirect_to(&format!("{ctx}#steam_error=verify_failed"));
        }
        Err(
            steam_client::SteamError::Network(_)
            | steam_client::SteamError::Api(_)
            | steam_client::SteamError::RateLimited
            | steam_client::SteamError::KeyRejected
            | steam_client::SteamError::NotFound
            | steam_client::SteamError::Parse(_),
        ) => {
            // Network, API, or other Steam unreachability.
            return redirect_to(&format!("{ctx}#steam_error=steam_unreachable"));
        }
    };

    // Best-effort persona — summary failure ⇒ empty persona, NOT an error.
    // steamids/personas are not secrets; do NOT log persona free-text at info level.
    let persona = match steam.get_player_summary(&steamid).await {
        Ok(p) => p.name,
        Err(
            steam_client::SteamError::Network(_)
            | steam_client::SteamError::Api(_)
            | steam_client::SteamError::RateLimited
            | steam_client::SteamError::KeyRejected
            | steam_client::SteamError::NotFound
            | steam_client::SteamError::Parse(_)
            | steam_client::SteamError::OpenIdRejected(_),
        ) => String::new(),
    };

    // No key material in Location.
    redirect_to(&format!(
        "{ctx}#steam={}&persona={}",
        steamid.0,
        urlencoding::encode(&persona)
    ))
}

// ── GET /api/l/:token/steam/owned/:steamid ────────────────────────────────────

/// Token-scoped proxy to the Steam owned-games endpoint.
///
/// Security: the link token is resolved FIRST. Unknown token → byte-identical 404;
/// dead link (revoked/expired/exhausted) → 409. Never an open proxy.
async fn handle_steam_owned_proxy(
    State(s): State<AppState>,
    Path((token, steamid)): Path<(String, String)>,
) -> Response {
    // Require steam client — 503 if unconfigured.
    let steam = match s.steam.as_ref() {
        Some(c) => c,
        None => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(serde_json::json!({"error": "steam not configured"})),
            )
                .into_response();
        }
    };

    // 1. Resolve link — same 404 shape as any unknown-token 404 (no oracle).
    let link = match s.store.get_link(&token).await {
        Ok(Some(l)) => l,
        Ok(None) => {
            // Byte-identical to the standard unknown-link 404.
            return link_not_found_response();
        }
        Err(_) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": "try again"})),
            )
                .into_response();
        }
    };

    // 2. Liveness gate — dead link → 409 like the claim-path refusals.
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

    // 3. Validate steamid — invariant (8): exactly 17 ASCII digits.
    //    Guard placed AFTER the token-resolution + liveness gate so that an
    //    unknown or dead token always returns the byte-identical 404/409 and
    //    never leaks that the steamid was also malformed (no oracle upgrade).
    if steamid.len() != 17 || !steamid.bytes().all(|b| b.is_ascii_digit()) {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": "steamid must be exactly 17 ASCII digits"})),
        )
            .into_response();
    }

    // 4. Cache-or-fetch exactly like the admin proxy (24h freshness rule).
    let now_epoch = now.unix_timestamp();
    const FRESH_SECS: i64 = 86400;

    match s.store.get_steam_owned(&steamid).await {
        Ok(Some((appids, fetched_at))) if now_epoch - fetched_at <= FRESH_SECS => {
            return (StatusCode::OK, Json(serde_json::json!({"appids": appids}))).into_response();
        }
        Ok(_) => {}  // absent or stale
        Err(_) => {} // read error — fall through
    }

    match steam.get_owned_games(&SteamId64(steamid.clone())).await {
        Ok(OwnedGames::Games(appids)) => {
            let _ = s.store.put_steam_owned(&steamid, &appids, now_epoch).await;
            (StatusCode::OK, Json(serde_json::json!({"appids": appids}))).into_response()
        }
        Ok(OwnedGames::Private) => {
            (StatusCode::OK, Json(serde_json::json!({"private": true}))).into_response()
        }
        Err(
            steam_client::SteamError::Network(_)
            | steam_client::SteamError::Api(_)
            | steam_client::SteamError::RateLimited
            | steam_client::SteamError::KeyRejected
            | steam_client::SteamError::NotFound
            | steam_client::SteamError::Parse(_)
            | steam_client::SteamError::OpenIdRejected(_),
        ) => StatusCode::SERVICE_UNAVAILABLE.into_response(),
    }
}

// ── GET /api/l/:token ─────────────────────────────────────────────────────────

async fn handle_get_link(State(s): State<AppState>, Path(token): Path<String>) -> Response {
    let link = match s.store.get_link(&token).await {
        Ok(Some(l)) => l,
        Ok(None) => {
            // Byte-identical for ANY invalid token — no enumeration oracle.
            return link_not_found_response();
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
    // state + games-gating from ONE exhaustive match over the single can_claim
    // rule — a future refusal variant forces a decision here at compile time
    // instead of silently leaking the catalog through a string comparison.
    // Revoked/expired hide the games (dead link, don't leak catalog);
    // exhausted keeps them visible so the friend can browse (claim buttons
    // are disabled client-side).
    let (state, hide_games) = match link.can_claim(now) {
        Ok(()) => ("active", false),
        Err(domain::ClaimRefusal::Revoked) => ("revoked", true),
        Err(domain::ClaimRefusal::Expired) => ("expired", true),
        Err(domain::ClaimRefusal::Exhausted) => ("exhausted", false),
    };

    // The games list and the claims history are independent reads — run them
    // concurrently. Each degrades on its own (empty grid / empty history).
    // Claims history is ALWAYS returned intact (spec §7); titles come from one
    // BatchGetItem over the claimed ids (claimed games leave the listable set,
    // so the games list can't supply them). A failed lookup degrades to
    // title:None — the client falls back to game_id.
    let (games, claims) = tokio::join!(
        async {
            if hide_games {
                return vec![];
            }
            let gs = match s.store.list_listable_games().await {
                Ok(gs) => gs,
                Err(_) => return vec![],
            };
            // Genres ride the same steam cache the detail endpoint reads, via
            // ONE BatchGetItem over the distinct appids (the games list is the
            // whole listable catalog — N serial GetItems here would put the
            // client's old N+1 inside the lambda). Cache-only: Steam is never
            // called at request time. Best-effort: a failed batch or a
            // missing/stub entry degrades to chip-less cards, never an error.
            let mut app_ids: Vec<u32> = gs.iter().filter_map(|g| g.steam_app_id).collect();
            app_ids.sort_unstable();
            app_ids.dedup();
            let caches = s
                .store
                .batch_get_steam_apps(&app_ids)
                .await
                .unwrap_or_default();
            gs.into_iter()
                .map(|g| {
                    let detail = g
                        .steam_app_id
                        .and_then(|id| caches.get(&id))
                        .and_then(|c| c.detail.as_ref());
                    let genres = detail
                        .map(|d| d.genres.iter().take(5).cloned().collect())
                        .unwrap_or_default();
                    // Stored tags are already capped at 10 — no take() here.
                    let tags = detail.map(|d| d.tags.clone()).unwrap_or_default();
                    GameView::from_game(g, genres, tags)
                })
                .collect()
        },
        async {
            let cs = match s.store.claims_for_link(&token).await {
                Ok(cs) => cs,
                Err(_) => return vec![],
            };
            let ids: Vec<String> = cs.iter().map(|c| c.game_id.clone()).collect();
            let titles = s.store.batch_get_games(&ids).await.unwrap_or_default();
            cs.into_iter()
                .map(|c| ClaimView {
                    title: titles.get(&c.game_id).map(|g| g.title.clone()),
                    game_id: c.game_id,
                    state: c.state,
                    gift_url: c.gift_url,
                })
                .collect::<Vec<_>>()
        }
    );

    (
        StatusCode::OK,
        Json(LinkView {
            label: link.label,
            // The note is strictly more personal than the catalog — a dead
            // (revoked/expired) link must not serve ben's message to whoever
            // holds the URL. Same gate as the games list.
            gift_note: if hide_games { None } else { link.gift_note },
            // Same personal-content gate: a dead link serves neither direction
            // of the correspondence.
            thank_note: if hide_games { None } else { link.thank_note },
            claims_allowed: link.claims_allowed,
            claims_used: link.claims_used,
            state,
            games,
            claims,
        }),
    )
        .into_response()
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
            return link_not_found_response();
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
            // A concurrent claim raced this one at the DDB layer (TransactionConflict /
            // TransactionInProgress). Nothing's wrong with this request — it just lost a
            // timing coin-flip — so it's a retryable 409, not a 500.
            ClaimTxError::TxConflict => (
                StatusCode::CONFLICT,
                Json(serde_json::json!({"error": "someone else is claiming right now, try again"})),
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
        // Rides the same freshly-read Game as gamekey/machine_name — one trust boundary. A choice
        // game flips fulfillment to the choose-then-redeem orchestration.
        requires_choice: game.requires_choice,
    };

    let gift_result = s.invoker.gift(fulfill_req).await;
    // Log the claim's fulfillment outcome (never the gift URL/token). A park
    // here is the friend-visible "processing" — this line says which variant.
    match &gift_result {
        Ok(FulfillResponse::GiftUrl { .. }) => tracing::info!("claim: gifted"),
        Ok(FulfillResponse::AlreadyRedeemed) => tracing::info!("claim: already-redeemed (410)"),
        Ok(other) => tracing::warn!(outcome = ?other, "claim: parked"),
        Err(e) => tracing::warn!(error = %e, "claim: fulfillment invoke failed → parked"),
    }
    match gift_result {
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

// ── POST /api/l/:token/thanks ─────────────────────────────────────────────────

/// Same budget as the gift note it answers (admin-api's `GIFT_NOTE_MAX_CHARS`) —
/// the correspondence is symmetric on purpose.
const THANK_NOTE_MAX_CHARS: usize = 500;

/// Characters that can visually reorder or invisibly pad the note when it renders
/// beside trusted admin chrome — the friend's text sits immediately before the
/// "— label, date" attribution ben reads, and a U+202E override would let it spoof
/// that signature (OMBB, #76 review; display-spoofing, not XSS — React escaping
/// holds). Explicit bidi embeddings/overrides/isolates, the zero-width space, BOM,
/// and the Arabic letter mark. ZWJ/ZWNJ (U+200C/U+200D) are deliberately KEPT:
/// they're load-bearing in emoji sequences and Indic scripts and have no
/// reordering power. Intrinsic RTL text (Arabic/Hebrew letters) is untouched —
/// only the explicit control characters are the spoofing vector.
fn is_spoofing_format_char(c: char) -> bool {
    matches!(
        c,
        '\u{061C}'
            | '\u{200B}'
            | '\u{200E}'
            | '\u{200F}'
            | '\u{202A}'..='\u{202E}'
            | '\u{2066}'..='\u{2069}'
            | '\u{FEFF}'
    )
}

/// Normalize a raw note before validation: whitespace controls (newline, CR, tab)
/// become plain spaces so a multiline paste keeps its word boundaries (both
/// surfaces render the note single-line), and every other control character or
/// spoofing format char is stripped. Runs BEFORE the emptiness/length checks, so
/// a note of nothing but bidi controls is refused as empty, and stripped
/// characters can't smuggle a 501st visible char past the budget.
fn sanitize_note(raw: &str) -> String {
    raw.chars()
        .filter_map(|c| match c {
            '\n' | '\r' | '\t' => Some(' '),
            c if c.is_control() || is_spoofing_format_char(c) => None,
            c => Some(c),
        })
        .collect()
}

#[derive(Deserialize)]
struct ThanksBody {
    note: String,
}

/// The friend's one thank-you back to ben. Write-once (the store's conditional
/// update enforces it — two tabs can't overwrite the first word), link-level
/// (the link IS the friend's identity here, same as the gift note it mirrors),
/// and only meaningful after an unwrap: no claims yet → refused.
async fn handle_post_thanks(
    State(s): State<AppState>,
    Path(token): Path<String>,
    Json(body): Json<ThanksBody>,
) -> Response {
    // 1. Validate before any read. Unlike the admin's gift-note parser, empty is
    //    an error rather than "clear" — there is no clearing a thank-you.
    //    Sanitize first: control/bidi strip precedes emptiness and budget checks.
    let sanitized = sanitize_note(&body.note);
    let note = sanitized.trim();
    if note.is_empty() {
        return (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(serde_json::json!({"error": "a thank-you needs some words"})),
        )
            .into_response();
    }
    if note.chars().count() > THANK_NOTE_MAX_CHARS {
        return (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(serde_json::json!({
                "error": format!("note must be at most {THANK_NOTE_MAX_CHARS} characters")
            })),
        )
            .into_response();
    }

    // 2. Resolve link — same 404 shape as unknown token everywhere else.
    let link = match s.store.get_link(&token).await {
        Ok(Some(l)) => l,
        Ok(None) => return link_not_found_response(),
        Err(_) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": "try again"})),
            )
                .into_response();
        }
    };

    // 3. Liveness gate: dead links don't take mail (same messages as the claim
    //    handler). Exhausted is NOT dead here — a fully-claimed link is exactly
    //    when a friend says thanks — so only Revoked/Expired refuse.
    let now = OffsetDateTime::now_utc();
    match link.can_claim(now) {
        Ok(()) | Err(domain::ClaimRefusal::Exhausted) => {}
        Err(domain::ClaimRefusal::Revoked) => {
            return (
                StatusCode::CONFLICT,
                Json(serde_json::json!({"error": "this link has been revoked"})),
            )
                .into_response();
        }
        Err(domain::ClaimRefusal::Expired) => {
            return (
                StatusCode::CONFLICT,
                Json(serde_json::json!({"error": "this link has expired"})),
            )
                .into_response();
        }
    }

    // 4. Thanks is the echo of an unwrap, not a guestbook: no claim, no note.
    if link.claims_used == 0 {
        return (
            StatusCode::CONFLICT,
            Json(serde_json::json!({"error": "claim a game first"})),
        )
            .into_response();
    }

    // 5. Write-once conditional write. `at` is pre-truncated to whole seconds so
    //    the value we echo back is byte-identical to what a re-read will serve
    //    (storage is epoch seconds).
    let at = OffsetDateTime::from_unix_timestamp(now.unix_timestamp())
        .expect("truncating now() to seconds cannot leave the valid range");
    match s.store.set_link_thanks(&token, note, at).await {
        Ok(dynamo::SetThanksOutcome::Set) => {
            tracing::info!("thanks: landed"); // never the note text
            let ts = at
                .format(&time::format_description::well_known::Rfc3339)
                .expect("rfc3339");
            (
                StatusCode::OK,
                Json(serde_json::json!({"thank_note": note, "thanked_at": ts})),
            )
                .into_response()
        }
        Ok(dynamo::SetThanksOutcome::AlreadyThanked) => (
            StatusCode::CONFLICT,
            Json(serde_json::json!({"error": "thanks already sent"})),
        )
            .into_response(),
        // A revoke raced past the step-3 pre-check and the storage guard caught
        // it — same message as the pre-check, the friend can't tell the paths apart.
        Ok(dynamo::SetThanksOutcome::Revoked) => (
            StatusCode::CONFLICT,
            Json(serde_json::json!({"error": "this link has been revoked"})),
        )
            .into_response(),
        Ok(dynamo::SetThanksOutcome::NotFound) => link_not_found_response(),
        Err(_) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": "try again"})),
        )
            .into_response(),
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

/// Byte-identical 404 used everywhere a token-scope check fails (no enumeration oracle).
/// Any unknown token, unknown game ID, or inaccessible game all return this exact body
/// so callers learn nothing about WHY access was denied.
fn link_not_found_response() -> Response {
    (
        StatusCode::NOT_FOUND,
        Json(serde_json::json!({"error": "unknown link"})),
    )
        .into_response()
}

// ── GET /api/l/:token/games/:id/detail ───────────────────────────────────────

/// Token-scoped game detail endpoint. Friend-facing, cache-only: Steam is never called.
///
/// Access rule (no-oracle): the link must resolve AND the game must be currently
/// listable OR its id must appear in this specific link's claims history.
/// Any other condition → byte-identical 404 so callers learn nothing about why.
///
/// Response shape:
/// ```json
/// { "game": { "id","title","bundle","key_type","artwork_url","steam_app_id" },
///   "steam": { "detail":…|null, "overall":…|null, "recent":…|null } | null }
/// ```
/// `steam: null` ⟺ game has no steam_app_id OR no cache item exists yet.
async fn handle_game_detail(
    State(s): State<AppState>,
    Path((token, game_id)): Path<(String, String)>,
) -> Response {
    // 1. Resolve link — same byte-identical 404 for any failure (no oracle).
    let _link = match s.store.get_link(&token).await {
        Ok(Some(l)) => l,
        Ok(None) => return link_not_found_response(),
        Err(_) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": "try again"})),
            )
                .into_response();
        }
    };

    // 2. Fetch the game — unknown game ID → byte-identical 404 (no oracle).
    let game = match s.store.get_game(&game_id).await {
        Ok(Some(g)) => g,
        Ok(None) => return link_not_found_response(),
        Err(_) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": "try again"})),
            )
                .into_response();
        }
    };

    // 3. Friend access gate: currently listable OR game id in THIS link's claims history.
    //    Inaccessible → same byte-identical 404 (no oracle: friend learns nothing).
    let accessible = if game.is_listable() {
        true
    } else {
        match s.store.claims_for_link(&token).await {
            Ok(claims) => claims.iter().any(|c| c.game_id == game_id),
            Err(_) => false,
        }
    };
    if !accessible {
        return link_not_found_response();
    }

    // 4. Steam cache — cache-only (Steam never called at request time).
    //    No steam_app_id OR no cache entry yet → null.
    let steam = match game.steam_app_id {
        None => serde_json::Value::Null,
        Some(app_id) => match s.store.get_steam_app(app_id).await {
            Ok(Some(cache)) => serde_json::json!({
                "detail": cache.detail,
                "overall": cache.overall,
                "recent": cache.recent,
            }),
            Ok(None) => serde_json::Value::Null,
            Err(_) => serde_json::Value::Null, // degrade gracefully; Steam cache is best-effort
        },
    };

    // Genres and tags deliberately empty (keys omitted on the wire): the modal reads
    // the full steam blob below instead.
    let game_view = GameView::from_game(game, vec![], vec![]);

    (
        StatusCode::OK,
        Json(serde_json::json!({
            "game": game_view,
            "steam": steam,
        })),
    )
        .into_response()
}
