//! Admin (Ben-facing) HTTP API for bendobundles.
//!
//! Routes under `/admin/api/`:
//! - POST  /admin/api/login              — argon2 verify, 7-day session cookie
//! - GET   /admin/api/catalog            — full game catalog (all statuses)
//! - POST  /admin/api/games/:id/hidden   — toggle hidden flag
//! - POST  /admin/api/games/:id/self-claim — intake + synchronous reveal (RequestResponse)
//! - POST  /admin/api/games/:id/steam-app-id — admin override for steam_app_id (null clears)
//! - POST  /admin/api/links              — create link (64-char token)
//! - GET   /admin/api/links              — list all links with used/allowed counts
//! - POST  /admin/api/links/:token/revoke
//! - GET   /admin/api/links/:token/claims
//! - GET   /admin/api/claims/self        — Ben's own self-claimed keys (SELF partition)
//! - POST  /admin/api/sync               — trigger catalog sync now
//! - GET   /admin/api/status             — sync state + game counts by status
//! - POST  /admin/api/steam/identity     — set Ben's SteamID (17-digit validation)
//! - DELETE /admin/api/steam/identity    — clear Ben's SteamID
//! - GET   /admin/api/steam/identity     — read Ben's SteamID (null if unset)
//! - GET   /admin/api/steam/owned/:steamid — session-guarded proxy: serve cache (≤24h) or fetch
//!
//! All routes except `/login` require a valid session cookie (`session=<token>`).
//! All `/admin/api/steam/*` routes additionally require a configured steam client; absent → 503.
use std::sync::Arc;

use async_trait::async_trait;
use axum::{
    Json, Router,
    extract::{Path, Request, State},
    http::{StatusCode, header},
    middleware::Next,
    response::{IntoResponse, Response},
    routing::{get, post},
};
use dynamo::{AppidWrite, ClaimTxError, HiddenWrite, Store};
use fulfillment::{FulfillRequest, FulfillResponse};
use serde::Deserialize;
use steam_client::{OwnedGames, SteamClient, SteamId64};
use time::OffsetDateTime;

// ── Traits ────────────────────────────────────────────────────────────────────

/// Bridge to the fulfillment lambda. Deliberately distinct from public-api's `Invoker` to avoid
/// an api→api crate dependency; the shape is intentionally minimal.
#[async_trait]
pub trait AdminInvoker: Send + Sync {
    /// Fire-and-forget invoke (`Event`) — returns as soon as the request is
    /// accepted, not when the work finishes. Used by sync-now: a full backfill
    /// runs for minutes, far past any HTTP timeout, so it MUST NOT be awaited
    /// through the request path.
    async fn fire(&self, req: FulfillRequest) -> Result<(), String>;
    /// Blocking `RequestResponse` invoke — self-claim needs the fulfillment RESULT (the revealed
    /// key) inside the request/response cycle, exactly like public-api's claim path. A reveal is
    /// seconds, not minutes: safe through the HTTP path.
    async fn call(&self, req: FulfillRequest) -> Result<FulfillResponse, String>;
}

// ── App state ─────────────────────────────────────────────────────────────────

#[derive(Clone)]
struct AppState {
    store: Arc<Store>,
    invoker: Arc<dyn AdminInvoker>,
    /// Argon2 PHC string loaded from SSM at lambda boot. Never written to logs.
    admin_hash: String,
    /// Steam client. `None` ⇒ all `/admin/api/steam/*` endpoints return 503.
    steam: Option<Arc<SteamClient>>,
}

// ── Router ────────────────────────────────────────────────────────────────────

/// Build the axum router. `admin_hash` is the argon2 PHC string for the admin password
/// (loaded from SSM at startup by `main.rs`). All routes except `/login` require a valid
/// session cookie set by the login endpoint. `steam` may be `None` — in that case all steam
/// endpoints return 503.
pub fn router(
    store: Arc<Store>,
    invoker: Arc<dyn AdminInvoker>,
    admin_hash: String,
    steam: Option<Arc<SteamClient>>,
) -> Router {
    let state = AppState {
        store,
        invoker,
        admin_hash,
        steam,
    };

    // Protected sub-router: session middleware applied to every route via route_layer.
    // route_layer (vs layer) means 404s from unmatched paths don't hit the session check.
    let protected = Router::new()
        .route("/admin/api/catalog", get(handle_catalog))
        .route("/admin/api/games/:id/detail", get(handle_game_detail))
        .route("/admin/api/games/:id/hidden", post(handle_game_hidden))
        .route("/admin/api/games/:id/self-claim", post(handle_self_claim))
        .route(
            "/admin/api/games/:id/steam-app-id",
            post(handle_game_steam_appid),
        )
        .route(
            "/admin/api/links",
            post(handle_create_link).get(handle_list_links),
        )
        .route("/admin/api/links/:token/revoke", post(handle_revoke_link))
        .route("/admin/api/links/:token/claims", get(handle_link_claims))
        .route("/admin/api/claims/self", get(handle_self_claims))
        .route("/admin/api/sync", post(handle_sync))
        .route("/admin/api/status", get(handle_status))
        .route(
            "/admin/api/steam/identity",
            post(handle_steam_identity_post)
                .delete(handle_steam_identity_delete)
                .get(handle_steam_identity_get),
        )
        .route(
            "/admin/api/steam/owned/:steamid",
            get(handle_steam_owned_proxy),
        )
        .route_layer(axum::middleware::from_fn_with_state(
            state.clone(),
            session_middleware,
        ));

    Router::new()
        .route("/admin/api/login", post(handle_login))
        .merge(protected)
        .with_state(state)
}

// ── Session middleware ─────────────────────────────────────────────────────────

/// Extract the `session=<token>` value from the Cookie header, if present.
fn extract_session_cookie(req: &Request) -> Option<String> {
    req.headers()
        .get(header::COOKIE)
        .and_then(|v| v.to_str().ok())
        .and_then(|cookies| {
            cookies.split(';').find_map(|part| {
                let t = part.trim();
                t.strip_prefix("session=").map(str::to_string)
            })
        })
}

async fn session_middleware(State(s): State<AppState>, request: Request, next: Next) -> Response {
    let Some(token) = extract_session_cookie(&request) else {
        return StatusCode::UNAUTHORIZED.into_response();
    };

    match s.store.get_session(&token).await {
        Ok(Some(expires_epoch)) => {
            if expires_epoch <= OffsetDateTime::now_utc().unix_timestamp() {
                return StatusCode::UNAUTHORIZED.into_response();
            }
        }
        Ok(None) => return StatusCode::UNAUTHORIZED.into_response(),
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }

    next.run(request).await
}

// ── Steam helper ──────────────────────────────────────────────────────────────

/// Validate that `s` is exactly 17 ASCII digit characters — mirrors steam-client's
/// `claimed_id` digit rule from `verify_openid_assertion`.
fn is_valid_steamid(s: &str) -> bool {
    s.len() == 17 && s.bytes().all(|b| b.is_ascii_digit())
}

/// Extract the steam client from state or return a 503 response.
macro_rules! require_steam {
    ($state:expr) => {
        match $state.steam.as_ref() {
            Some(c) => c,
            None => {
                return (
                    StatusCode::SERVICE_UNAVAILABLE,
                    Json(serde_json::json!({"error": "steam not configured"})),
                )
                    .into_response();
            }
        }
    };
}

// ── POST /admin/api/login ─────────────────────────────────────────────────────

#[derive(Deserialize)]
struct LoginBody {
    password: String,
}

async fn handle_login(State(s): State<AppState>, Json(body): Json<LoginBody>) -> Response {
    use argon2::{Argon2, PasswordHash, PasswordVerifier};

    // Verify password against stored PHC string. On failure (bad hash string OR wrong password)
    // sleep 500 ms and return 401 — identical response for all failure modes (no enumeration).
    let ok = PasswordHash::new(&s.admin_hash)
        .ok()
        .and_then(|hash| {
            Argon2::default()
                .verify_password(body.password.as_bytes(), &hash)
                .ok()
        })
        .is_some();

    if !ok {
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        return StatusCode::UNAUTHORIZED.into_response();
    }

    // Token = two uuid-v4 concatenated without hyphens: 32 + 32 = 64 hex chars (≥128 bits).
    let token = format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    );
    let expires = OffsetDateTime::now_utc() + time::Duration::days(7);

    if s.store
        .create_session(&token, expires.unix_timestamp())
        .await
        .is_err()
    {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    }

    let cookie = format!(
        "session={}; HttpOnly; Secure; SameSite=Strict; Path=/admin",
        token
    );
    let cookie_val = axum::http::HeaderValue::from_str(&cookie).expect("cookie is valid header");

    (
        StatusCode::OK,
        [(header::SET_COOKIE, cookie_val)],
        Json(serde_json::json!({"ok": true})),
    )
        .into_response()
}

// ── GET /admin/api/catalog ────────────────────────────────────────────────────

/// Admin catalog view of a game. Deliberately NOT `domain::Game`: the raw
/// struct carries `gamekey`/`machine_name`/`keyindex` — the humble order-key
/// material used to build `FulfillRequest::Gift` — which no client needs and
/// which must not leak into browser network tabs, session-gated or not.
/// Caveat: `id` IS the composite `"{gamekey}:{machine_name}"` (domain
/// `game_id()`), so the gamekey still reaches the client inside the id — a
/// documented-accepted exposure (game-detail-modal spec §4). The field
/// exclusions above keep the order-key FIELDS off the wire, not the id.
#[derive(serde::Serialize)]
struct CatalogGameView {
    id: String,
    title: String,
    bundle: String,
    key_type: String,
    giftable: bool,
    hidden: bool,
    status: domain::GameStatus,
    claim_id: Option<String>,
    artwork_url: Option<String>,
    requires_choice: bool,
    steam_app_id: Option<u32>,
    owned_by_ben: bool,
    steam: Option<SteamSummaryView>,
}

/// Compact steam projection for catalog rows — the toolkit's filter/sort/group
/// data. Deliberately excludes screenshots/video/description (the fat stays on
/// the detail endpoint). `None` fields are individually absent-but-honest.
#[derive(serde::Serialize)]
struct SteamSummaryView {
    genres: Vec<String>,
    developers: Vec<String>,
    publishers: Vec<String>,
    release_date: Option<String>,
    /// "YYYY-MM-DD" parsed server-side (time::Date Display is ISO-8601).
    release_date_iso: Option<String>,
    review_desc: Option<String>,
    /// round(100 * positive / total); None when 0 reviews.
    review_percent: Option<u8>,
    review_count: Option<u64>,
    recent_percent: Option<u8>,
}

/// Project a cache entry to the summary. Returns None for entries with
/// nothing to show (negative-cache stub with no reviews either) so the row
/// serializes `steam: null` rather than an all-null husk.
fn steam_summary(cache: &dynamo::SteamAppCache) -> Option<SteamSummaryView> {
    if cache.detail.is_none() && cache.overall.is_none() && cache.recent.is_none() {
        return None;
    }
    let d = cache.detail.as_ref();
    let release_date = d.and_then(|d| d.release_date.clone());
    let release_date_iso = release_date
        .as_deref()
        .and_then(steam_client::parse_release_date)
        .map(|d| d.to_string());
    let o = cache.overall.as_ref();
    let review_percent = o
        .filter(|o| o.total_reviews > 0)
        .map(|o| ((o.total_positive * 100 + o.total_reviews / 2) / o.total_reviews) as u8);
    Some(SteamSummaryView {
        genres: d.map(|d| d.genres.clone()).unwrap_or_default(),
        developers: d.map(|d| d.developers.clone()).unwrap_or_default(),
        publishers: d.map(|d| d.publishers.clone()).unwrap_or_default(),
        release_date,
        release_date_iso,
        review_desc: o.map(|o| o.desc.clone()),
        review_percent,
        review_count: o.map(|o| o.total_reviews),
        recent_percent: cache.recent.as_ref().map(|r| r.percent_positive),
    })
}

async fn handle_catalog(State(s): State<AppState>) -> Response {
    match s.store.list_all_games().await {
        Ok(games) => {
            // One BatchGetItem over the distinct appids (same idiom as the
            // link view in public-api). Best-effort: a failed batch degrades
            // every row to steam: null — the toolkit shows "unmapped" buckets,
            // never an error.
            let mut app_ids: Vec<u32> = games.iter().filter_map(|g| g.steam_app_id).collect();
            app_ids.sort_unstable();
            app_ids.dedup();
            let caches = s
                .store
                .batch_get_steam_apps(&app_ids)
                .await
                .unwrap_or_default();
            let views: Vec<CatalogGameView> = games
                .into_iter()
                .map(|g| CatalogGameView {
                    steam: g
                        .steam_app_id
                        .and_then(|id| caches.get(&id))
                        .and_then(steam_summary),
                    id: g.id,
                    title: g.title,
                    bundle: g.bundle,
                    key_type: g.key_type,
                    giftable: g.giftable,
                    hidden: g.hidden,
                    status: g.status,
                    claim_id: g.claim_id,
                    artwork_url: g.artwork_url,
                    requires_choice: g.requires_choice,
                    steam_app_id: g.steam_app_id,
                    owned_by_ben: g.owned_by_ben,
                })
                .collect();
            (StatusCode::OK, Json(views)).into_response()
        }
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

// ── POST /admin/api/games/:id/hidden ──────────────────────────────────────────

#[derive(Deserialize)]
struct HiddenBody {
    hidden: bool,
}

/// Toggle a game's `hidden` flag via a guarded conditional write (`store.set_game_hidden`).
/// Returns 200 on success, 404 if the game does not exist, 409 if a concurrent claim owns the
/// game (the admin should retry once the claim completes). The unguarded `put_game` was previously
/// used here but would clobber a live claim's status/claim_id in a mid-claim race.
async fn handle_game_hidden(
    State(s): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<HiddenBody>,
) -> Response {
    match s.store.set_game_hidden(&id, body.hidden).await {
        Ok(HiddenWrite::Written) => {
            (StatusCode::OK, Json(serde_json::json!({"ok": true}))).into_response()
        }
        Ok(HiddenWrite::NotFound) => StatusCode::NOT_FOUND.into_response(),
        Ok(HiddenWrite::Contested) => (
            StatusCode::CONFLICT,
            Json(serde_json::json!({"error": "game is mid-claim — try again in a moment"})),
        )
            .into_response(),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

// ── POST /admin/api/games/:id/steam-app-id ────────────────────────────────────

#[derive(Deserialize)]
struct SteamAppIdBody {
    app_id: Option<u32>,
}

/// Admin override for a game's `steam_app_id`.
/// - `{app_id: <number>}` → sets `steam_app_id = number, appid_source = Manual`.
/// - `{app_id: null}`     → clears both fields; auto-resolution reruns on the next sync walk.
///
/// Uses `set_game_steam_appid_admin`, which bypasses the `Manual` guard (the admin IS the
/// override) and uses the same optimistic-lock-on-status pattern as `set_game_hidden`.
async fn handle_game_steam_appid(
    State(s): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<SteamAppIdBody>,
) -> Response {
    match s.store.set_game_steam_appid_admin(&id, body.app_id).await {
        Ok(AppidWrite::Written) => {
            (StatusCode::OK, Json(serde_json::json!({"ok": true}))).into_response()
        }
        Ok(AppidWrite::NotFound) => StatusCode::NOT_FOUND.into_response(),
        Ok(AppidWrite::Contested) => (
            StatusCode::CONFLICT,
            Json(serde_json::json!({"error": "game is mid-claim — try again in a moment"})),
        )
            .into_response(),
        Ok(AppidWrite::Skipped) => {
            // Should never happen from this path (admin bypasses Manual guard) but handle it.
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

// ── GET /admin/api/games/:id/detail ──────────────────────────────────────────

/// Session-guarded game detail endpoint. Admin superset: any game id (including hidden,
/// non-giftable, non-listable). Cache-only — Steam is never called at request time.
///
/// Response shape:
/// ```json
/// { "game": { …CatalogGameView… },
///   "steam": { "detail":…|null, "overall":…|null, "recent":…|null } | null }
/// ```
/// `steam: null` ⟺ game has no steam_app_id OR no cache item exists yet.
async fn handle_game_detail(State(s): State<AppState>, Path(id): Path<String>) -> Response {
    let game = match s.store.get_game(&id).await {
        Ok(Some(g)) => g,
        Ok(None) => return StatusCode::NOT_FOUND.into_response(),
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };

    // Steam cache — cache-only; degrade gracefully on any read error. Read
    // once, then serve both shapes: the full blob (this endpoint's `steam`)
    // and the compact summary the catalog rows carry (`game.steam`).
    let cache = match game.steam_app_id {
        None => None,
        Some(app_id) => s.store.get_steam_app(app_id).await.ok().flatten(),
    };
    let steam = match &cache {
        Some(cache) => serde_json::json!({
            "detail": cache.detail,
            "overall": cache.overall,
            "recent": cache.recent,
        }),
        None => serde_json::Value::Null,
    };

    let game_view = CatalogGameView {
        steam: cache.as_ref().and_then(steam_summary),
        id: game.id,
        title: game.title,
        bundle: game.bundle,
        key_type: game.key_type,
        giftable: game.giftable,
        hidden: game.hidden,
        status: game.status,
        claim_id: game.claim_id,
        artwork_url: game.artwork_url,
        requires_choice: game.requires_choice,
        steam_app_id: game.steam_app_id,
        owned_by_ben: game.owned_by_ben,
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

// ── POST /admin/api/links ─────────────────────────────────────────────────────

/// Bounds for create-link input. `expires_days` MUST be capped: the handler computes
/// `now + Duration::days(d)`, and `OffsetDateTime + Duration` panics once the result leaves the
/// representable range (year > 9999) — as does the rfc3339 serializer in dynamo's link schema.
/// A panic here is a lambda 502 + cold restart, so absurd input gets a 422 instead.
const EXPIRES_DAYS_MAX: u32 = 3650; // ~10 years — nobody needs a longer-lived gift link
const CLAIMS_ALLOWED_MAX: u32 = 100;
const LABEL_MAX_CHARS: usize = 200;

#[derive(Deserialize)]
struct CreateLinkBody {
    label: String,
    claims_allowed: u32,
    expires_days: Option<u32>,
}

impl CreateLinkBody {
    /// Validate the body before any store or time arithmetic is touched.
    /// Returns a client-facing message on the first violated bound.
    fn validate(&self) -> Result<(), String> {
        if self
            .expires_days
            .is_some_and(|d| !(1..=EXPIRES_DAYS_MAX).contains(&d))
        {
            return Err(format!(
                "expires_days must be between 1 and {EXPIRES_DAYS_MAX}"
            ));
        }
        if !(1..=CLAIMS_ALLOWED_MAX).contains(&self.claims_allowed) {
            return Err(format!(
                "claims_allowed must be between 1 and {CLAIMS_ALLOWED_MAX}"
            ));
        }
        if self.label.chars().count() > LABEL_MAX_CHARS {
            return Err(format!(
                "label must be at most {LABEL_MAX_CHARS} characters"
            ));
        }
        Ok(())
    }
}

async fn handle_create_link(
    State(s): State<AppState>,
    Json(body): Json<CreateLinkBody>,
) -> Response {
    if let Err(msg) = body.validate() {
        return (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(serde_json::json!({"error": msg})),
        )
            .into_response();
    }

    // Token = two uuid-v4 simple-format (no hyphens) concatenated: 32 + 32 = 64 hex chars.
    let token = format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    );

    let now = OffsetDateTime::now_utc();
    let expires_at = body
        .expires_days
        .map(|d| now + time::Duration::days(d as i64));

    let link = domain::Link {
        token: token.clone(),
        label: body.label,
        claims_allowed: body.claims_allowed,
        claims_used: 0,
        revoked: false,
        expires_at,
        created_at: now,
    };

    match s.store.create_link(&link).await {
        Ok(()) => (
            StatusCode::OK,
            Json(serde_json::json!({
                "token": token,
                "url_path": format!("/l/{}", token),
            })),
        )
            .into_response(),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

// ── GET /admin/api/links ──────────────────────────────────────────────────────

async fn handle_list_links(State(s): State<AppState>) -> Response {
    match s.store.list_links().await {
        Ok(links) => (StatusCode::OK, Json(links)).into_response(),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

// ── POST /admin/api/links/:token/revoke ───────────────────────────────────────

async fn handle_revoke_link(State(s): State<AppState>, Path(token): Path<String>) -> Response {
    let mut link = match s.store.get_link(&token).await {
        Ok(Some(l)) => l,
        Ok(None) => return StatusCode::NOT_FOUND.into_response(),
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };

    link.revoked = true;

    match s.store.update_link_meta(&link).await {
        Ok(()) => (StatusCode::OK, Json(serde_json::json!({"ok": true}))).into_response(),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

// ── GET /admin/api/links/:token/claims ────────────────────────────────────────

/// Admin view of a gift claim. Deliberately NOT `domain::Claim`: the friend's
/// one-time gift URL is a bearer secret — it must never reach the admin surface,
/// and the admin only learns THAT one was issued. Self-claims are different by
/// design: `revealed_key` is Ben's own key and is served by `handle_self_claims`
/// ONLY (never on this gift-claim view).
#[derive(serde::Serialize)]
struct AdminClaimView {
    game_id: String,
    state: domain::ClaimState,
    issued: bool,
}

async fn handle_link_claims(State(s): State<AppState>, Path(token): Path<String>) -> Response {
    // Look the link up first: `claims_for_link` on an unknown token yields an empty list, which
    // is indistinguishable from "link exists, no claims yet". Unknown token → 404, matching the
    // revoke handler.
    match s.store.get_link(&token).await {
        Ok(Some(_)) => {}
        Ok(None) => return StatusCode::NOT_FOUND.into_response(),
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }

    match s.store.claims_for_link(&token).await {
        Ok(claims) => {
            let views: Vec<AdminClaimView> = claims
                .into_iter()
                .map(|c| AdminClaimView {
                    game_id: c.game_id,
                    state: c.state,
                    issued: c.gift_url.is_some(),
                })
                .collect();
            (StatusCode::OK, Json(views)).into_response()
        }
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

// ── POST /admin/api/games/:id/self-claim ─────────────────────────────────────

/// Self-claim view of a claim — the ONE admin surface that serves a key value (Ben's own).
#[derive(serde::Serialize)]
struct SelfClaimView {
    game_id: String,
    state: domain::ClaimState,
    revealed_key: Option<String>,
    created_at: String,
}

async fn handle_self_claim(State(s): State<AppState>, Path(id): Path<String>) -> Response {
    // 1. Read the game — need gamekey/machine_name/keyindex/requires_choice for the invoke,
    //    and key_type for the response.
    let game = match s.store.get_game(&id).await {
        Ok(Some(g)) => g,
        Ok(None) => return StatusCode::NOT_FOUND.into_response(),
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };
    if game.status != domain::GameStatus::Available {
        return (
            StatusCode::CONFLICT,
            Json(serde_json::json!({"error": "game is not available"})),
        )
            .into_response();
    }

    // 2. Intake under LINK#SELF (single-winner on the status condition).
    let claim_id = uuid::Uuid::new_v4().to_string();
    if let Err(e) = s
        .store
        .claim_game_self(&id, &claim_id, OffsetDateTime::now_utc())
        .await
    {
        return match e {
            ClaimTxError::GameUnavailable | ClaimTxError::TxConflict => (
                StatusCode::CONFLICT,
                Json(serde_json::json!({"error": "game was just claimed — refresh"})),
            )
                .into_response(),
            _ => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
        };
    }

    // 3. Synchronous fulfillment — the reveal happens now; parks return 202.
    let req = FulfillRequest::SelfClaim {
        claim_id: claim_id.clone(),
        game_id: id.clone(),
        gamekey: game.gamekey.clone(),
        machine_name: game.machine_name.clone(),
        keyindex: game.keyindex,
        requires_choice: game.requires_choice,
    };
    match s.invoker.call(req).await {
        Ok(FulfillResponse::RevealedKey { key }) => (
            StatusCode::OK,
            Json(serde_json::json!({"revealed_key": key, "key_type": game.key_type})),
        )
            .into_response(),
        Ok(FulfillResponse::AlreadyRedeemed) => (
            StatusCode::GONE,
            Json(serde_json::json!({"error": "key was already redeemed"})),
        )
            .into_response(),
        Ok(FulfillResponse::Parked { .. }) => (
            StatusCode::ACCEPTED,
            Json(serde_json::json!({
                "status": "processing",
                "message": "reveal parked — the key will appear under self-claims, or the game will re-list if the claim couldn't complete"
            })),
        )
            .into_response(),
        Ok(_) | Err(_) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": "fulfillment failed — check self-claims later; the claim is recorded"})),
        )
            .into_response(),
    }
}

// ── GET /admin/api/claims/self ────────────────────────────────────────────────

/// Self-claims list. NOTE: deliberately no link-existence pre-check — LINK#SELF has no META item
/// (handle_link_claims' pre-check would 404 this; do not reuse it).
async fn handle_self_claims(State(s): State<AppState>) -> Response {
    match s.store.claims_for_link(domain::SELF_LINK_TOKEN).await {
        Ok(claims) => {
            let views: Vec<SelfClaimView> = claims
                .into_iter()
                .map(|c| SelfClaimView {
                    game_id: c.game_id,
                    state: c.state,
                    revealed_key: c.revealed_key,
                    created_at: c
                        .created_at
                        .format(&time::format_description::well_known::Rfc3339)
                        .unwrap_or_default(),
                })
                .collect();
            (StatusCode::OK, Json(views)).into_response()
        }
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

// ── POST /admin/api/sync ──────────────────────────────────────────────────────

/// Trigger a catalog sync now. Fire-and-forget (`Event` invoke): a full backfill runs for
/// minutes — far past the API Gateway integration timeout — so we must NOT await it through the
/// request path (that 504s). Returns 202 immediately; the admin watches the status card, which
/// fulfillment updates (`put_sync_state`) when the background run finishes.
async fn handle_sync(State(s): State<AppState>) -> Response {
    // Refuse to queue a second backfill while a live run marker exists: concurrent walks double
    // the humble request rate for nothing. This read-then-fire is best-effort UX (a clear 409
    // instead of a silently-skipped duplicate) — the authoritative serialization is fulfillment's
    // conditional `begin_sync_run`. On a marker read error, fire anyway for the same reason.
    let now = OffsetDateTime::now_utc().unix_timestamp();
    let run_live = match s.store.get_sync_run().await {
        Ok(Some(started)) => dynamo::sync_run_is_live(started, now),
        Ok(None) | Err(_) => false,
    };
    if run_live {
        return (
            StatusCode::CONFLICT,
            Json(serde_json::json!({
                "error": "a sync is already running — watch the status card"
            })),
        )
            .into_response();
    }

    match s.invoker.fire(FulfillRequest::Sync).await {
        Ok(()) => (
            StatusCode::ACCEPTED,
            Json(serde_json::json!({
                "status": "started",
                "message": "sync started — watch the status card; a full backfill takes a few minutes"
            })),
        )
            .into_response(),
        Err(_) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": "couldn't start sync — try again"})),
        )
            .into_response(),
    }
}

// ── GET /admin/api/status ─────────────────────────────────────────────────────

/// SyncState + per-status game counts derived from a full `list_all_games` scan.
/// `list_all_games` is a paginated Scan; see `dynamo::Store::list_all_games` for the
/// scan-is-fine-at-this-scale rationale.
async fn handle_status(State(s): State<AppState>) -> Response {
    // Never-run stays None → serialized as JSON null, which is what the client
    // types (`sync: {…} | null`) and renders ("never" + no attention banner).
    // Flattening to SyncState::default() here would fake a failed sync with
    // cookie_ok:false and fire the red banner on every fresh deploy.
    let sync_state = match s.store.get_sync_state().await {
        Ok(st) => st,
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };

    // The run marker drives the client's "sync running" affordances (disabled button, poll
    // loop, running badge). `running` is computed HERE because liveness needs a trustworthy
    // clock — the browser's can't judge staleness against server-written epochs.
    let sync_run = match s.store.get_sync_run().await {
        Ok(r) => r,
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };
    let now = OffsetDateTime::now_utc().unix_timestamp();

    let games = match s.store.list_all_games().await {
        Ok(gs) => gs,
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };

    let mut available = 0u32;
    let mut pending = 0u32;
    let mut gifted = 0u32;
    let mut ben_redeemed = 0u32;
    let mut expired = 0u32;

    for g in &games {
        match g.status {
            domain::GameStatus::Available => available += 1,
            domain::GameStatus::Pending => pending += 1,
            domain::GameStatus::Gifted => gifted += 1,
            domain::GameStatus::BenRedeemed => ben_redeemed += 1,
            domain::GameStatus::Expired => expired += 1,
        }
    }

    (
        StatusCode::OK,
        Json(serde_json::json!({
            "sync": sync_state,
            // null = no marker (idle or a completed run — completion deletes it).
            // running:false with a marker present = a run began but never reported
            // (crash/timeout); the client surfaces that as "likely failed, safe to retry".
            "sync_run": sync_run.map(|started| serde_json::json!({
                "started_epoch": started,
                "running": dynamo::sync_run_is_live(started, now),
            })),
            // Per-status buckets ONLY — the client renders one chip per key,
            // so a folded-in "total" would masquerade as a sixth status and
            // double the apparent catalog size.
            "game_counts": {
                "available": available,
                "pending": pending,
                "gifted": gifted,
                "ben_redeemed": ben_redeemed,
                "expired": expired,
            },
        })),
    )
        .into_response()
}

// ── POST /admin/api/steam/identity ────────────────────────────────────────────

#[derive(Deserialize)]
struct SteamIdentityBody {
    steamid: String,
}

/// Set Ben's Steam identity. Validates that `steamid` is exactly 17 ASCII digits (the Steam
/// 64-bit ID format) — mirrors steam-client's OpenID claimed_id digit rule. Returns 400 on
/// invalid input. Returns 503 if the steam client is not configured.
async fn handle_steam_identity_post(
    State(s): State<AppState>,
    Json(body): Json<SteamIdentityBody>,
) -> Response {
    let _steam = require_steam!(s);

    if !is_valid_steamid(&body.steamid) {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": "steamid must be exactly 17 ASCII digits"})),
        )
            .into_response();
    }

    match s.store.put_steam_identity(&body.steamid).await {
        Ok(()) => (StatusCode::OK, Json(serde_json::json!({"ok": true}))).into_response(),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

// ── DELETE /admin/api/steam/identity ─────────────────────────────────────────

/// Clear Ben's Steam identity. Idempotent — succeeds even if none was set.
/// Returns 503 if the steam client is not configured.
async fn handle_steam_identity_delete(State(s): State<AppState>) -> Response {
    let _steam = require_steam!(s);

    match s.store.delete_steam_identity().await {
        Ok(()) => (StatusCode::OK, Json(serde_json::json!({"ok": true}))).into_response(),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

// ── GET /admin/api/steam/identity ─────────────────────────────────────────────

/// Read Ben's stored Steam identity. Returns `{"steamid": "<17-digit>"}` or
/// `{"steamid": null}` if not yet configured.
/// Returns 503 if the steam client is not configured.
async fn handle_steam_identity_get(State(s): State<AppState>) -> Response {
    let _steam = require_steam!(s);

    match s.store.get_steam_identity().await {
        Ok(steamid) => (
            StatusCode::OK,
            Json(serde_json::json!({"steamid": steamid})),
        )
            .into_response(),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

// ── GET /admin/api/steam/owned/:steamid ───────────────────────────────────────

/// Session-guarded proxy to the Steam owned-games endpoint.
///
/// Freshness rule: serve `get_steam_owned` if `fetched_at` ≤ 24h old; else call
/// `get_owned_games` + `put_steam_owned` + serve. `Private` → `{"private":true}` (do NOT
/// overwrite a previous good cache with a Private response — the cache keeps its old
/// `fetched_at`).
///
/// Returns 503 if the steam client is not configured.
/// Returns 400 if `steamid` is not exactly 17 ASCII digits.
async fn handle_steam_owned_proxy(
    State(s): State<AppState>,
    Path(steamid): Path<String>,
) -> Response {
    let steam = require_steam!(s);

    if !is_valid_steamid(&steamid) {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": "steamid must be exactly 17 ASCII digits"})),
        )
            .into_response();
    }

    let now = OffsetDateTime::now_utc().unix_timestamp();
    const FRESH_SECS: i64 = 86400; // 24 hours

    // Try the cache first.
    match s.store.get_steam_owned(&steamid).await {
        Ok(Some((appids, fetched_at))) if now - fetched_at <= FRESH_SECS => {
            // Cache is fresh — serve it without hitting Steam.
            return (StatusCode::OK, Json(serde_json::json!({"appids": appids}))).into_response();
        }
        Ok(_) => {}  // absent or stale — fall through to fetch
        Err(_) => {} // read error — fall through to fetch (degraded, not fatal)
    }

    // Cache miss or stale: call Steam.
    match steam.get_owned_games(&SteamId64(steamid.clone())).await {
        Ok(OwnedGames::Games(appids)) => {
            // Write-through cache — ignore write errors (degraded cache, not fatal).
            let _ = s.store.put_steam_owned(&steamid, &appids, now).await;
            (StatusCode::OK, Json(serde_json::json!({"appids": appids}))).into_response()
        }
        Ok(OwnedGames::Private) => {
            // Do NOT overwrite a previous good cache — return private signal only.
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
