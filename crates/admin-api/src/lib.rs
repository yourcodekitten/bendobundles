//! Admin (Ben-facing) HTTP API for bendobundles.
//!
//! Routes under `/admin/api/`:
//! - POST  /admin/api/login              — argon2 verify, 7-day session cookie
//! - GET   /admin/api/catalog            — full game catalog (all statuses)
//! - POST  /admin/api/games/:id/hidden   — toggle hidden flag
//! - POST  /admin/api/games/:id/self-claim — intake + synchronous reveal (RequestResponse)
//! - POST  /admin/api/links              — create link (64-char token)
//! - GET   /admin/api/links              — list all links with used/allowed counts
//! - POST  /admin/api/links/:token/revoke
//! - GET   /admin/api/links/:token/claims
//! - GET   /admin/api/claims/self        — Ben's own self-claimed keys (SELF partition)
//! - POST  /admin/api/sync               — trigger catalog sync now
//! - GET   /admin/api/status             — sync state + game counts by status
//!
//! All routes except `/login` require a valid session cookie (`session=<token>`).
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
use dynamo::{ClaimTxError, HiddenWrite, Store};
use fulfillment::{FulfillRequest, FulfillResponse};
use serde::Deserialize;
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
}

// ── Router ────────────────────────────────────────────────────────────────────

/// Build the axum router. `admin_hash` is the argon2 PHC string for the admin password
/// (loaded from SSM at startup by `main.rs`). All routes except `/login` require a valid
/// session cookie set by the login endpoint.
pub fn router(store: Arc<Store>, invoker: Arc<dyn AdminInvoker>, admin_hash: String) -> Router {
    let state = AppState {
        store,
        invoker,
        admin_hash,
    };

    // Protected sub-router: session middleware applied to every route via route_layer.
    // route_layer (vs layer) means 404s from unmatched paths don't hit the session check.
    let protected = Router::new()
        .route("/admin/api/catalog", get(handle_catalog))
        .route("/admin/api/games/:id/hidden", post(handle_game_hidden))
        .route("/admin/api/games/:id/self-claim", post(handle_self_claim))
        .route(
            "/admin/api/links",
            post(handle_create_link).get(handle_list_links),
        )
        .route("/admin/api/links/:token/revoke", post(handle_revoke_link))
        .route("/admin/api/links/:token/claims", get(handle_link_claims))
        .route("/admin/api/claims/self", get(handle_self_claims))
        .route("/admin/api/sync", post(handle_sync))
        .route("/admin/api/status", get(handle_status))
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
}

async fn handle_catalog(State(s): State<AppState>) -> Response {
    match s.store.list_all_games().await {
        Ok(games) => {
            let views: Vec<CatalogGameView> = games
                .into_iter()
                .map(|g| CatalogGameView {
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
                "message": "reveal parked — the key will appear under self-claims when reconcile completes"
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
