//! Public (friend-facing) HTTP API: link view and claim flow.
//!
//! Routes: `GET /api/l/:token`, `POST /api/l/:token/claim`,
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
            // Genres ride the same steam cache the detail endpoint reads —
            // cache-only, best-effort: any miss/stub/error degrades to no
            // genres for that game. Memoized per appid because duplicate
            // copies of one title are common in the catalog.
            let mut memo: std::collections::HashMap<u32, Vec<String>> =
                std::collections::HashMap::new();
            let mut views = Vec::with_capacity(gs.len());
            for g in gs {
                let genres = match g.steam_app_id {
                    None => Vec::new(),
                    Some(app_id) => {
                        if let Some(known) = memo.get(&app_id) {
                            known.clone()
                        } else {
                            let fetched: Vec<String> = match s.store.get_steam_app(app_id).await {
                                Ok(Some(cache)) => cache
                                    .detail
                                    .map(|d| d.genres.into_iter().take(5).collect())
                                    .unwrap_or_default(),
                                Ok(None) | Err(_) => Vec::new(),
                            };
                            memo.insert(app_id, fetched.clone());
                            fetched
                        }
                    }
                };
                views.push(GameView {
                    id: g.id,
                    title: g.title,
                    bundle: g.bundle,
                    key_type: g.key_type,
                    artwork_url: g.artwork_url,
                    steam_app_id: g.steam_app_id,
                    genres,
                });
            }
            views
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

    let game_view = GameView {
        id: game.id,
        title: game.title,
        bundle: game.bundle,
        key_type: game.key_type,
        artwork_url: game.artwork_url,
        steam_app_id: game.steam_app_id,
        // Deliberately empty (key omitted on the wire): the modal reads
        // steam.detail.genres from the full blob below instead.
        genres: vec![],
    };

    (
        StatusCode::OK,
        Json(serde_json::json!({
            "game": game_view,
            "steam": steam,
        })),
    )
        .into_response()
}
