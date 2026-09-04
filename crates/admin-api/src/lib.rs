//! Admin (Ben-facing) HTTP API for bendobundles.
//!
//! Routes under `/admin/api/`:
//! - POST  /admin/api/login              — argon2 verify, 7-day session cookie
//! - POST  /admin/api/logout             — revoke session server-side + clear cookie (idempotent)
//! - GET   /admin/api/catalog            — full game catalog (all statuses)
//! - POST  /admin/api/games/{id}/hidden   — toggle hidden flag
//! - POST  /admin/api/games/{id}/self-claim — intake + synchronous reveal (RequestResponse)
//! - POST  /admin/api/games/{id}/steam-app-id — admin override for steam_app_id (null clears)
//! - POST  /admin/api/links              — create link (64-char token)
//! - GET   /admin/api/links              — list all links with used/allowed counts
//! - POST  /admin/api/links/{token}/revoke
//! - GET   /admin/api/links/{token}/claims
//! - POST  /admin/api/links/{token}/friend — assign/clear the link's owning friend
//! - POST  /admin/api/friends            — create a friend + mint their shelf token
//! - GET   /admin/api/friends            — list all friends
//! - POST  /admin/api/friends/{id}       — rename OR reissue OR revoke (exactly one)
//! - GET   /admin/api/claims/self        — Ben's own self-claimed keys (SELF partition)
//! - POST  /admin/api/sync               — trigger catalog sync now
//! - GET   /admin/api/status             — sync state + game counts by status
//! - POST  /admin/api/steam/identity     — set Ben's SteamID (17-digit validation)
//! - DELETE /admin/api/steam/identity    — clear Ben's SteamID
//! - GET   /admin/api/steam/identity     — read Ben's SteamID (null if unset)
//! - GET   /admin/api/steam/owned/{steamid} — session-guarded proxy: serve cache (≤24h) or fetch
//!
//! All routes except `/login` and `/logout` require a valid session cookie (`session=<token>`).
//! State-changing routes (POST/DELETE/…) additionally require the `X-Admin-Request` header —
//! CSRF defense-in-depth, an independent second layer under `SameSite=Strict` (#83).
//! All `/admin/api/steam/*` routes additionally require a configured steam client; absent → 503.
use std::sync::Arc;

use async_trait::async_trait;
use axum::{
    Json, Router,
    extract::{Path, Request, State},
    http::{HeaderMap, Method, StatusCode, header},
    middleware::Next,
    response::{IntoResponse, Response},
    routing::{get, post},
};
use dynamo::{AppidWrite, ClaimTxError, HiddenWrite, OwnedProxyOutcome, Store};
use fulfillment::{FulfillRequest, FulfillResponse};
use serde::Deserialize;
use steam_client::SteamClient;
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
        .route("/admin/api/games/{id}/detail", get(handle_game_detail))
        .route("/admin/api/games/{id}/hidden", post(handle_game_hidden))
        .route("/admin/api/games/{id}/self-claim", post(handle_self_claim))
        .route(
            "/admin/api/games/{id}/steam-app-id",
            post(handle_game_steam_appid),
        )
        .route(
            "/admin/api/links",
            post(handle_create_link).get(handle_list_links),
        )
        .route("/admin/api/links/{token}/revoke", post(handle_revoke_link))
        .route("/admin/api/links/{token}/note", post(handle_set_link_note))
        .route(
            "/admin/api/links/{token}/unlock",
            post(handle_set_link_unlock).delete(handle_delete_link_unlock),
        )
        .route("/admin/api/links/{token}/claims", get(handle_link_claims))
        .route(
            "/admin/api/links/{token}/friend",
            post(handle_set_link_friend),
        )
        .route(
            "/admin/api/friends",
            post(handle_create_friend).get(handle_list_friends),
        )
        .route("/admin/api/friends/{id}", post(handle_patch_friend))
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
            "/admin/api/steam/owned/{steamid}",
            get(handle_steam_owned_proxy),
        )
        .route_layer(axum::middleware::from_fn_with_state(
            state.clone(),
            session_middleware,
        ));

    Router::new()
        .route("/admin/api/login", post(handle_login))
        // Logout sits OUTSIDE the session middleware, a sibling of login. Revoking a session
        // must not itself require a live session — an expired or already-deleted token still
        // needs to clear the browser cookie and get a clean 204, which is impossible if the
        // middleware 401s the request first.
        .route("/admin/api/logout", post(handle_logout))
        .merge(protected)
        .with_state(state)
}

// ── Session middleware ─────────────────────────────────────────────────────────

/// Extract the `session=<token>` value from the Cookie header, if present.
fn extract_session_cookie(headers: &HeaderMap) -> Option<String> {
    headers
        .get(header::COOKIE)
        .and_then(|v| v.to_str().ok())
        .and_then(|cookies| {
            cookies.split(';').find_map(|part| {
                let t = part.trim();
                t.strip_prefix("session=").map(str::to_string)
            })
        })
}

/// The custom header every state-changing admin request must carry — the independent CSRF layer
/// (#83). Unforgeable cross-site: a browser can't set a custom header on a cross-origin request
/// without a CORS preflight, and no `CorsLayer` exists anywhere in `crates/` to grant one.
const ADMIN_REQUEST_HEADER: &str = "x-admin-request";

/// Methods that mutate server state — the ones that need CSRF protection. Read-only methods are
/// exempt (a cross-site GET can't change anything, and it lands cookie-less under SameSite anyway).
fn is_state_changing(method: &Method) -> bool {
    !matches!(*method, Method::GET | Method::HEAD | Method::OPTIONS)
}

async fn session_middleware(State(s): State<AppState>, request: Request, next: Next) -> Response {
    let Some(token) = extract_session_cookie(request.headers()) else {
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

    // #83: CSRF defense-in-depth. Beyond a valid session, a STATE-CHANGING request must also carry
    // the `X-Admin-Request` custom header. This is the independent second layer under
    // `SameSite=Strict` for the bodyless POSTs (`/sync`, `/revoke`, `/self-claim`) that lack the
    // `Json` Content-Type barrier — and it holds even if a future subdomain ever weakened SameSite
    // (a cross-site request still can't forge the header). Read-only methods are exempt.
    if is_state_changing(request.method()) && !request.headers().contains_key(ADMIN_REQUEST_HEADER)
    {
        return StatusCode::FORBIDDEN.into_response();
    }

    next.run(request).await
}

// ── Steam helper ──────────────────────────────────────────────────────────────

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

    let token = mint_token();
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

// ── POST /admin/api/logout ────────────────────────────────────────────────────

/// Revoke the current admin session and clear the browser cookie. Reads the token from the
/// Cookie header and deletes it server-side (`delete_session` is idempotent), so an unknown or
/// already-expired token is a clean no-op. The store is only touched when a cookie is actually
/// present — an unauthenticated request with no session cookie clears nothing and writes nothing.
/// Always 204 on success (there is nothing to return); the cookie is the credential, so `Path`
/// and all attributes mirror login's cookie exactly or the browser won't overwrite it.
async fn handle_logout(State(s): State<AppState>, headers: HeaderMap) -> Response {
    // Delete only when a cookie is actually present (short-circuits before touching the store).
    // If the store delete genuinely fails, do NOT clear the cookie and claim success — the
    // session is still live server-side, so report it honestly.
    if let Some(token) = extract_session_cookie(&headers)
        && s.store.delete_session(&token).await.is_err()
    {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    }

    let cookie = "session=; Max-Age=0; HttpOnly; Secure; SameSite=Strict; Path=/admin";
    let cookie_val = axum::http::HeaderValue::from_static(cookie);

    (StatusCode::NO_CONTENT, [(header::SET_COOKIE, cookie_val)]).into_response()
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
    /// Provenance of `hidden` — "sync" rows get the "auto-hidden: adult content" label (#71).
    hidden_source: Option<domain::HiddenSource>,
    steam: Option<SteamSummaryView>,
}

/// Compact steam projection for catalog rows — the toolkit's filter/sort/group
/// data. Deliberately excludes screenshots/video/description (the fat stays on
/// the detail endpoint). `None` fields are individually absent-but-honest.
#[derive(serde::Serialize)]
struct SteamSummaryView {
    genres: Vec<String>,
    /// Top community tags (popularity order, ≤10) — the toolkit's chips + tag filter (#71).
    tags: Vec<String>,
    /// Raw content descriptor ids — the 🔞 badge ({1,3,4} ∩) and mature filter are
    /// client-side policy over these (#71).
    content_descriptor_ids: Vec<u32>,
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
        tags: d.map(|d| d.tags.clone()).unwrap_or_default(),
        content_descriptor_ids: d
            .map(|d| d.content_descriptor_ids.clone())
            .unwrap_or_default(),
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
                    hidden_source: g.hidden_source,
                })
                .collect();
            (StatusCode::OK, Json(views)).into_response()
        }
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

// ── POST /admin/api/games/{id}/hidden ──────────────────────────────────────────

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
            Json(serde_json::json!({"error": "game changed underneath this edit (a claim or sync raced it) — try again in a moment"})),
        )
            .into_response(),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

// ── POST /admin/api/games/{id}/steam-app-id ────────────────────────────────────

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
            Json(serde_json::json!({"error": "game changed underneath this edit (a claim or sync raced it) — try again in a moment"})),
        )
            .into_response(),
        Ok(AppidWrite::Skipped) => {
            // Should never happen from this path (admin bypasses Manual guard) but handle it.
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

// ── GET /admin/api/games/{id}/detail ──────────────────────────────────────────

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
        hidden_source: game.hidden_source,
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
const UNLOCK_MAX_DAYS: i64 = 370; // a typo'd year must not seal a gift forever
const CLAIMS_ALLOWED_MAX: u32 = 100;
const LABEL_MAX_CHARS: usize = 200;
const GIFT_NOTE_MAX_CHARS: usize = 500; // fits the friend page's dialog box without scrolling
const CURATED_GAMES_MAX: usize = 100; // an unbounded admin array is still an unbounded array

/// Single owner of the gift-note input rules — create and edit-after-the-fact both
/// call this, so the two paths can never drift apart, and validation can't be
/// skipped by a call site that only wanted normalization.
///
/// Trims, bounds-checks the TRIMMED text (trailing whitespace shouldn't eat
/// budget), and collapses empty/whitespace-only to `None` so the friend page can
/// gate rendering on plain field presence (and blank means "clear" on edit).
fn parse_gift_note(raw: Option<&str>) -> Result<Option<String>, String> {
    let trimmed = raw.map(str::trim).filter(|n| !n.is_empty());
    if trimmed.is_some_and(|n| n.chars().count() > GIFT_NOTE_MAX_CHARS) {
        return Err(format!(
            "gift_note must be at most {GIFT_NOTE_MAX_CHARS} characters"
        ));
    }
    Ok(trimmed.map(String::from))
}

/// Mint a bearer token: two uuid-v4 simple-format (no hyphens) concatenated — 32 + 32 = 64
/// lowercase hex chars (≥128 bits). Shared by login sessions, link tokens, and friend shelf
/// tokens — three call sites now, one implementation, so the idiom can't silently drift between
/// them.
fn mint_token() -> String {
    format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    )
}

/// The crate's one 422 shape — `{"error": msg}` — which the web client's
/// `throwIfValidation422` parses; both validated endpoints build it here.
fn unprocessable(msg: String) -> Response {
    (
        StatusCode::UNPROCESSABLE_ENTITY,
        Json(serde_json::json!({ "error": msg })),
    )
        .into_response()
}

/// Parse an absolute rfc3339 unlock instant and bound it to (now, now + UNLOCK_MAX_DAYS].
/// Shared by create and the edit verb. The browser already resolved ben's local pick to
/// an instant — a bare datetime without offset is a client bug and parses as Err here.
fn parse_unlock_at(raw: &str, now: OffsetDateTime) -> Result<OffsetDateTime, String> {
    let t = OffsetDateTime::parse(raw, &time::format_description::well_known::Rfc3339)
        .map_err(|_| "unlock_at must be an rfc3339 instant with offset".to_string())?;
    if t <= now {
        return Err("unlock_at must be in the future".to_string());
    }
    if t > now + time::Duration::days(UNLOCK_MAX_DAYS) {
        return Err(format!("unlock_at must be within {UNLOCK_MAX_DAYS} days"));
    }
    Ok(t)
}

#[derive(Deserialize)]
struct CreateLinkBody {
    label: String,
    claims_allowed: u32,
    expires_days: Option<u32>,
    gift_note: Option<String>,
    /// Wrapped gift: absolute rfc3339 instant (WITH offset — the browser resolved ben's
    /// local pick at write time; a bare datetime is a client bug and parses as Err).
    /// Optional; a seal is create-time-only (spec 2026-08-05 §4).
    unlock_at: Option<String>,
    /// Curated shelf: the admin's pick order, preserved verbatim (never sorted/deduped-by-
    /// reorder — order IS meaning, storage and wire). Omitted/absent means open-shelf.
    game_ids: Option<Vec<String>>,
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
        parse_gift_note(self.gift_note.as_deref())?;
        if let Some(ids) = &self.game_ids {
            if ids.is_empty() {
                return Err(
                    "game_ids must not be empty when provided — omit it for an open-shelf link"
                        .into(),
                );
            }
            if ids.len() > CURATED_GAMES_MAX {
                return Err(format!(
                    "game_ids must be at most {CURATED_GAMES_MAX} games"
                ));
            }
            let mut seen = std::collections::HashSet::new();
            if let Some(dup) = ids.iter().find(|id| !seen.insert(id.as_str())) {
                return Err(format!("game_ids contains a duplicate: {dup}"));
            }
            if self.claims_allowed as usize > ids.len() {
                return Err(format!(
                    "claims_allowed ({}) exceeds the {} curated games — the link would promise more than it can deliver",
                    self.claims_allowed,
                    ids.len()
                ));
            }
        }
        Ok(())
    }
}

async fn handle_create_link(
    State(s): State<AppState>,
    Json(body): Json<CreateLinkBody>,
) -> Response {
    if let Err(msg) = body.validate() {
        return unprocessable(msg);
    }

    // Store-backed validation: every curated id must exist and be listable NOW.
    // (It can stop being listable later — the friend surface ghosts it; spec §2.)
    if let Some(ids) = &body.game_ids {
        let found = match s.store.batch_get_games(ids).await {
            Ok(m) => m,
            Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
        };
        let unknown: Vec<&str> = ids
            .iter()
            .filter(|id| !found.contains_key(*id))
            .map(String::as_str)
            .collect();
        if !unknown.is_empty() {
            return unprocessable(format!("unknown game ids: {}", unknown.join(", ")));
        }
        let unlistable: Vec<&str> = ids
            .iter()
            .filter(|id| found.get(*id).is_some_and(|g| !g.is_listable()))
            .map(String::as_str)
            .collect();
        if !unlistable.is_empty() {
            return unprocessable(format!(
                "not claimable right now: {}",
                unlistable.join(", ")
            ));
        }
    }

    let token = mint_token();

    let now = OffsetDateTime::now_utc();
    let expires_at = body
        .expires_days
        .map(|d| now + time::Duration::days(d as i64));

    // Wrapped gift: parse + bound the unlock instant, then cross-check it precedes
    // expiry — a link that expires before it unwraps is a gift nobody can ever open.
    let unlock_at = match body.unlock_at.as_deref() {
        None => None,
        Some(raw) => match parse_unlock_at(raw, now) {
            Ok(t) => {
                if expires_at.is_some_and(|exp| t >= exp) {
                    return unprocessable("unlock_at must be before the link expires".into());
                }
                Some(t)
            }
            Err(msg) => return unprocessable(msg),
        },
    };

    let link = domain::Link {
        token: token.clone(),
        gift_note: parse_gift_note(body.gift_note.as_deref())
            .expect("gift_note bound checked by validate() above"),
        // Thanks only ever arrive post-creation, from the friend, via the public
        // API's scoped write — a fresh link has none by definition.
        thank_note: None,
        thanked_at: None,
        label: body.label,
        claims_allowed: body.claims_allowed,
        claims_used: 0,
        revoked: false,
        expires_at,
        unlock_at,
        curated_game_ids: body.game_ids.clone(),
        friend_id: None,
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

// ── POST /admin/api/links/{token}/revoke ───────────────────────────────────────

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

// ── POST /admin/api/links/{token}/note ─────────────────────────────────────────

#[derive(Deserialize)]
struct SetLinkNoteBody {
    /// The new note. Blank/whitespace-only (or null) CLEARS the note — one
    /// endpoint covers set, edit, and remove.
    gift_note: Option<String>,
}

/// Set/replace/clear a link's gift note after creation. NOT the revoke-shaped
/// read-modify-write: `set_link_gift_note` is a single-attribute SET/REMOVE, so
/// this handler holds no snapshot of the enforcer fields and a save racing a
/// revoke/expiry change cannot write anything stale back (the old shape could
/// silently un-revoke a link). One round-trip; the condition doubles as the 404.
async fn handle_set_link_note(
    State(s): State<AppState>,
    Path(token): Path<String>,
    Json(body): Json<SetLinkNoteBody>,
) -> Response {
    // Validation precedes existence (matching create): an over-length note on an
    // unknown token 422s rather than 404s — the single-RTT design never reads
    // first, so field errors are diagnosed before the write's condition runs.
    let note = match parse_gift_note(body.gift_note.as_deref()) {
        Ok(n) => n,
        Err(msg) => return unprocessable(msg),
    };

    match s.store.set_link_gift_note(&token, note.as_deref()).await {
        Ok(true) => (StatusCode::OK, Json(serde_json::json!({"ok": true}))).into_response(),
        Ok(false) => StatusCode::NOT_FOUND.into_response(),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

// ── /admin/api/links/{token}/unlock — the two seal verbs (spec 2026-08-05 §4) ─

#[derive(Deserialize)]
struct SetUnlockBody {
    /// Required: unseal is DELETE, never a null set (family review — a null that
    /// means "unseal" is a fat-finger away from a set).
    unlock_at: String,
}

const NOT_SEALED_MSG: &str =
    "link is not sealed — seals are create-time-only and end at the unlock moment";

async fn handle_set_link_unlock(
    State(s): State<AppState>,
    Path(token): Path<String>,
    body: Result<Json<SetUnlockBody>, axum::extract::rejection::JsonRejection>,
) -> Response {
    let Ok(Json(body)) = body else {
        return unprocessable(
            "unlock_at (rfc3339 instant) is required — to unseal, DELETE instead".into(),
        );
    };
    let now = OffsetDateTime::now_utc();
    let unlock = match parse_unlock_at(&body.unlock_at, now) {
        Ok(t) => t,
        Err(msg) => return unprocessable(msg),
    };
    // Cross-check expiry from a read (benign: the SEAL rules are enforced atomically in
    // the store condition; expiry ordering is admin-input hygiene, not enforcement).
    match s.store.get_link(&token).await {
        Ok(Some(l)) => {
            if l.expires_at.is_some_and(|exp| unlock >= exp) {
                return unprocessable("unlock_at must be before the link expires".into());
            }
        }
        Ok(None) => return StatusCode::NOT_FOUND.into_response(),
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
    match s.store.set_link_unlock(&token, unlock, now).await {
        Ok(true) => (StatusCode::OK, Json(serde_json::json!({"ok": true}))).into_response(),
        Ok(false) => (
            StatusCode::CONFLICT,
            Json(serde_json::json!({"error": NOT_SEALED_MSG})),
        )
            .into_response(),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

/// Deliberate asymmetry with the POST verb (plan, gate minor 3): POST distinguishes
/// unknown-token (404) from not-sealed (409) because it already reads the link for the
/// expiry cross-check; DELETE does no read — unknown and not-sealed both mean "nothing
/// to unseal" and collapse into one 409. Admin-only surface; no oracle concern.
async fn handle_delete_link_unlock(
    State(s): State<AppState>,
    Path(token): Path<String>,
) -> Response {
    let now = OffsetDateTime::now_utc();
    match s.store.remove_link_unlock(&token, now).await {
        Ok(true) => (StatusCode::OK, Json(serde_json::json!({"ok": true}))).into_response(),
        Ok(false) => (
            StatusCode::CONFLICT,
            Json(serde_json::json!({"error": NOT_SEALED_MSG})),
        )
            .into_response(),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

// ── GET /admin/api/links/{token}/claims ────────────────────────────────────────

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

// ── POST /admin/api/links/{token}/friend ───────────────────────────────────────

#[derive(Deserialize)]
struct AssignFriendBody {
    /// `Some(id)` assigns; `None` clears. The friend must exist when non-null — an
    /// assignment pointing at nobody is a worse failure mode than a 422.
    friend_id: Option<String>,
}

/// Assign or clear a link's owning friend. Friend existence is checked BEFORE the store write
/// (matching create/note's validate-before-touch shape): a non-null `friend_id` that doesn't
/// resolve is a client error (422), not a 404 on the LINK. `set_link_friend`'s own
/// `attribute_exists(pk)` condition is what tells an unknown link apart from an unknown friend —
/// `Ok(false)` only happens once the friend side is already known-good.
async fn handle_set_link_friend(
    State(s): State<AppState>,
    Path(token): Path<String>,
    Json(body): Json<AssignFriendBody>,
) -> Response {
    if let Some(fid) = &body.friend_id {
        match s.store.get_friend(fid).await {
            Ok(Some(_)) => {}
            Ok(None) => return unprocessable("unknown friend".into()),
            Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
        }
    }

    match s
        .store
        .set_link_friend(&token, body.friend_id.as_deref())
        .await
    {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => StatusCode::NOT_FOUND.into_response(),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

// ── POST /admin/api/friends ─────────────────────────────────────────────────────

const FRIEND_NAME_MAX_CHARS: usize = 64;

/// Same category public-api's `sanitize_note` strips from a thank-you note (Unicode Cf
/// format chars minus the ZWJ/ZWNJ/MVS carve-outs — bidi overrides/embeddings/isolates,
/// zero-width space, word joiner, BOM, the tag block, etc.) — duplicated here rather than
/// imported because admin-api does not depend on public-api (review finding: the friend
/// `name` renders on the UNAUTHENTICATED `/s/{token}` shelf header just like the note
/// renders beside ben's trusted chrome, so it carries the same U+202E spoofing risk and
/// needs the same treatment at write time). Keep in sync with
/// `public-api::is_spoofing_format_char` if that category ever changes.
fn is_spoofing_format_char(c: char) -> bool {
    matches!(
        c,
        '\u{00AD}'
            | '\u{0600}'..='\u{0605}'
            | '\u{061C}'
            | '\u{06DD}'
            | '\u{070F}'
            | '\u{0890}'..='\u{0891}'
            | '\u{08E2}'
            | '\u{200B}'
            | '\u{200E}'
            | '\u{200F}'
            | '\u{202A}'..='\u{202E}'
            | '\u{2060}'..='\u{206F}'
            | '\u{FEFF}'
            | '\u{FFF9}'..='\u{FFFB}'
            | '\u{110BD}'
            | '\u{110CD}'
            | '\u{13430}'..='\u{1343F}'
            | '\u{1BCA0}'..='\u{1BCA3}'
            | '\u{1D173}'..='\u{1D17A}'
            | '\u{E0000}'..='\u{E007F}'
    )
}

/// Same treatment `sanitize_note` gives a thank-you note, applied to a friend name at
/// write time (create + rename): line/segment separators fold to a space, every other
/// control or spoofing-format char is dropped — strip, not reject, matching the note
/// path's behavior exactly. Runs BEFORE the length/emptiness check below so a name that
/// sanitizes down to nothing is refused as empty, and stripped chars can't smuggle extra
/// visible length past `FRIEND_NAME_MAX_CHARS`.
fn sanitize_friend_name(raw: &str) -> String {
    raw.chars()
        .filter_map(|c| match c {
            '\n' | '\r' | '\t' | '\u{000B}' | '\u{000C}' | '\u{0085}' | '\u{2028}' | '\u{2029}' => {
                Some(' ')
            }
            c if c.is_control() || is_spoofing_format_char(c) => None,
            c => Some(c),
        })
        .collect()
}

#[derive(Deserialize)]
struct CreateFriendBody {
    name: String,
}

/// Create a friend + mint their first shelf token in one shot. `id` is a fresh uuid-v4
/// (simple format, no hyphens); `shelf_token` is `mint_token()` — the same 64-hex idiom
/// login sessions and link tokens use.
async fn handle_create_friend(
    State(s): State<AppState>,
    Json(body): Json<CreateFriendBody>,
) -> Response {
    let name = sanitize_friend_name(&body.name);
    let name = name.trim();
    // chars().count(), not len(): the constant and the 422 message both promise
    // CHARACTERS, and a byte count refuses multibyte names far short of the cap
    if name.is_empty() || name.chars().count() > FRIEND_NAME_MAX_CHARS {
        return unprocessable(format!("name must be 1-{FRIEND_NAME_MAX_CHARS} characters"));
    }

    let id = uuid::Uuid::new_v4().simple().to_string();
    let token = mint_token();
    let friend = domain::Friend {
        id: id.clone(),
        name: name.to_string(),
        shelf_token: token.clone(),
        created_at: OffsetDateTime::now_utc(),
    };

    match s.store.create_friend(&friend).await {
        Ok(()) => (
            StatusCode::OK,
            Json(serde_json::json!({
                "id": id,
                "name": friend.name,
                "shelf_token": token,
                "shelf_url_path": format!("/s/{token}"),
            })),
        )
            .into_response(),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

// ── GET /admin/api/friends ───────────────────────────────────────────────────────

/// `domain::Friend` already serializes to exactly the wire shape this route promises
/// (`{id, name, shelf_token, created_at}`, `shelf_token: ""` when revoked) — no view struct
/// needed.
async fn handle_list_friends(State(s): State<AppState>) -> Response {
    match s.store.list_friends().await {
        Ok(friends) => (StatusCode::OK, Json(friends)).into_response(),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

// ── POST /admin/api/friends/{id} ─────────────────────────────────────────────────

#[derive(Deserialize)]
struct PatchFriendBody {
    name: Option<String>,
    reissue: Option<bool>,
    revoke: Option<bool>,
}

/// Rename OR reissue OR revoke — exactly one of the three per request. Counting the set ones
/// (rather than an if/else-if chain) is what makes "two at once" a distinguishable 422 instead
/// of one silently winning.
async fn handle_patch_friend(
    State(s): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<PatchFriendBody>,
) -> Response {
    let set_count = [
        body.name.is_some(),
        body.reissue == Some(true),
        body.revoke == Some(true),
    ]
    .into_iter()
    .filter(|set| *set)
    .count();
    if set_count != 1 {
        return unprocessable("provide exactly one of: name, reissue, revoke".into());
    }

    if let Some(name) = body.name.as_deref() {
        let name = sanitize_friend_name(name);
        let name = name.trim();
        // chars().count(), not len() — same character-cap contract as create
        if name.is_empty() || name.chars().count() > FRIEND_NAME_MAX_CHARS {
            return unprocessable(format!("name must be 1-{FRIEND_NAME_MAX_CHARS} characters"));
        }
        return match s.store.rename_friend(&id, name).await {
            Ok(true) => (StatusCode::OK, Json(serde_json::json!({"ok": true}))).into_response(),
            Ok(false) => StatusCode::NOT_FOUND.into_response(),
            Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
        };
    }

    // reissue and revoke both need the friend's CURRENT token first — reissue to hand it to
    // the transaction as the old-pointer key, revoke for the same reason. This read also
    // supplies the 404 for an unknown id (neither store call below returns a bool). A
    // REVOKED friend reads back shelf_token == "" — both store methods branch on that:
    // reissue skips the pointer delete (there is none), revoke becomes an idempotent no-op.
    let friend = match s.store.get_friend(&id).await {
        Ok(Some(f)) => f,
        Ok(None) => return StatusCode::NOT_FOUND.into_response(),
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };

    if body.reissue == Some(true) {
        let new_token = mint_token();
        return match s
            .store
            .reissue_shelf_token(&id, &friend.shelf_token, &new_token)
            .await
        {
            Ok(()) => (
                StatusCode::OK,
                Json(serde_json::json!({
                    "shelf_token": new_token,
                    "shelf_url_path": format!("/s/{new_token}"),
                })),
            )
                .into_response(),
            Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
        };
    }

    // revoke (the only remaining option, given set_count == 1)
    match s.store.revoke_shelf_token(&id, &friend.shelf_token).await {
        Ok(()) => (StatusCode::OK, Json(serde_json::json!({"shelf_token": ""}))).into_response(),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

// ── POST /admin/api/games/{id}/self-claim ─────────────────────────────────────

/// Self-claim view of a claim — the ONE admin surface that serves a key value (Ben's own).
#[derive(serde::Serialize)]
struct SelfClaimView {
    // The claim id — the stable React key. game_id repeats across a
    // claim→compensate→re-claim cycle, so the web list must key on this, not game_id (#44).
    id: String,
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
        Ok(FulfillResponse::KeyDead) => (
            StatusCode::GONE,
            Json(serde_json::json!({
                "error": "key is dead on humble's side — claim failed terminally, reason recorded on the claim"
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
                    id: c.id,
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

    if !steam_client::is_valid_steam_id64(&body.steamid) {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": steam_client::STEAM_ID64_ERROR_MSG})),
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

// ── GET /admin/api/steam/owned/{steamid} ───────────────────────────────────────

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

    if !steam_client::is_valid_steam_id64(&steamid) {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": steam_client::STEAM_ID64_ERROR_MSG})),
        )
            .into_response();
    }

    let now = OffsetDateTime::now_utc().unix_timestamp();
    // 24h cache-or-fetch — the shared core (#47); see Store::cached_owned_or_fetch.
    match s
        .store
        .cached_owned_or_fetch(steam.as_ref(), &steamid, now)
        .await
    {
        OwnedProxyOutcome::Games(appids) => (
            StatusCode::OK,
            // Browser-side mirror of the server's freshness rule (#47): the appid list is
            // stable for the same 24h window the STEAMOWN cache serves. `private` — this
            // is a session-guarded, per-admin response; never shared-cacheable.
            [(
                header::CACHE_CONTROL,
                format!(
                    "private, max-age={}",
                    dynamo::schema::STEAM_OWNED_FRESH_SECS
                ),
            )],
            Json(serde_json::json!({"appids": appids})),
        )
            .into_response(),
        OwnedProxyOutcome::Private => {
            (StatusCode::OK, Json(serde_json::json!({"private": true}))).into_response()
        }
        OwnedProxyOutcome::Unavailable => StatusCode::SERVICE_UNAVAILABLE.into_response(),
    }
}

#[cfg(test)]
mod friend_name_sanitize_tests {
    use super::*;

    /// The friend name renders on the UNAUTHENTICATED `/s/{token}` shelf header
    /// ("ben's shelf for {name}") the same way a thank-you note renders beside ben's
    /// trusted "— label, date" attribution — same U+202E spoofing threat, so the name
    /// gets the same strip-not-reject treatment public-api's `sanitize_note` gives notes
    /// (review finding: it previously did not).
    #[test]
    fn strips_bidi_override_and_other_spoofing_format_chars() {
        assert_eq!(
            sanitize_friend_name("sarah\u{202E}"),
            "sarah",
            "a trailing bidi override must be stripped, not stored"
        );
        assert_eq!(
            sanitize_friend_name("\u{202E}sarah\u{200B}"),
            "sarah",
            "leading override + zero-width space both stripped"
        );
        // ordinary names are untouched
        assert_eq!(sanitize_friend_name("Sarah O'Brien"), "Sarah O'Brien");
    }

    #[test]
    fn folds_line_separators_to_a_space_like_sanitize_note_does() {
        assert_eq!(sanitize_friend_name("sarah\nsmith"), "sarah smith");
    }

    #[test]
    fn a_name_that_sanitizes_to_nothing_is_empty_after_trim() {
        // mirrors sanitize_note's ordering: sanitize runs BEFORE the emptiness check
        // in the handlers, so an all-invisible name is refused as empty, not stored.
        assert_eq!(sanitize_friend_name("\u{202E}\u{200B}").trim(), "");
    }
}
