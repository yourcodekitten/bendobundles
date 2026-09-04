//! Public (friend-facing) HTTP API: link view and claim flow.
//!
//! Routes: `GET /api/l/{token}`, `POST /api/l/{token}/claim`,
//!         `POST /api/l/{token}/thanks`,
//!         `GET /api/steam/login`, `GET /api/steam/return`,
//!         `GET /api/l/{token}/steam/owned/{steamid}`, fallback 404.
use std::sync::Arc;

use async_trait::async_trait;
use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::{StatusCode, header},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use dynamo::{ClaimTxError, OwnedProxyOutcome, Store, StoreError};
use fulfillment::{FulfillRequest, FulfillResponse};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use steam_client::SteamClient;
use time::OffsetDateTime;

// ── Invoker trait ─────────────────────────────────────────────────────────────

/// Synchronous bridge to the fulfillment lambda. `Arc<dyn Invoker>`-friendly.
#[async_trait]
pub trait Invoker: Send + Sync {
    async fn gift(&self, req: FulfillRequest) -> Result<FulfillResponse, String>;
    /// Fire-and-forget bell (spec: docs/spec-attic-bell.md): InvocationType::Event, 202-and-done,
    /// no response to parse. Callers log-and-continue on Err — a bell failure may never fail,
    /// slow, or colour a friend's response.
    async fn bell(&self, req: FulfillRequest) -> Result<(), String>;
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

    async fn bell(&self, req: FulfillRequest) -> Result<(), String> {
        let payload = serde_json::to_vec(&req).map_err(|e| e.to_string())?;
        self.client
            .invoke()
            .function_name(&self.fn_name)
            .invocation_type(aws_sdk_lambda::types::InvocationType::Event)
            .payload(aws_sdk_lambda::primitives::Blob::new(payload))
            .send()
            .await
            .map_err(|e| format!("{e:?}"))?;
        Ok(())
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
    /// Ghost marker (curated links only): this chosen game is in a DECIDED
    /// non-listable state (gifted / ben-redeemed / expired / ungiftable /
    /// hidden). Absent when false so open-shelf payloads stay byte-identical.
    /// Cause-blind by decision (spec §2) — never add the cause here casually.
    #[serde(skip_serializing_if = "is_false")]
    gone: bool,
}

fn is_false(b: &bool) -> bool {
    !*b
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
            gone: false,
        }
    }

    /// A curated pick in a decided non-listable state — real identity fields
    /// (the friend sees what the gift WAS), no enrichment, gone flag on.
    fn ghost(g: domain::Game) -> Self {
        let mut v = Self::from_game(g, Vec::new(), Vec::new());
        v.gone = true;
        v
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
    /// Explicit link state: "active" | "sealed" | "revoked" | "expired" | "exhausted".
    /// The SINGLE liveness representation on the wire — the client renders
    /// banners and gates claim buttons from this; it must never have to infer
    /// the reason from side signals like games.len().
    state: &'static str,
    /// True iff this link carries a curated set. Absent when false (open-shelf
    /// payloads unchanged); the sealed view sets false deliberately — the seal
    /// withholds even the mode.
    #[serde(skip_serializing_if = "is_false")]
    curated: bool,
    games: Vec<GameView>,
    claims: Vec<ClaimView>,
    /// Wrapped gift: seconds until unlock, server-computed and CEILED (never arrives
    /// early; sealed ⇒ >= 1). Present ONLY while sealed — the client counts down from
    /// REMAINING, never by comparing wall clocks (spec 2026-08-05 §2).
    #[serde(skip_serializing_if = "Option::is_none")]
    unlocks_in_seconds: Option<u64>,
    /// Wrapped gift: the unlock instant, rfc3339. Present ONLY while sealed.
    #[serde(skip_serializing_if = "Option::is_none")]
    unlocks_at: Option<String>,
}

/// One unwrapped gift on a friend's shelf — only `title` + `artwork_url` from
/// `Game` (v1 does not touch the steam cache; see `assemble_shelf`).
#[derive(Serialize)]
struct ShelfGift {
    game_id: String,
    title: String,
    artwork_url: Option<String>,
    #[serde(with = "time::serde::rfc3339")]
    unwrapped_at: OffsetDateTime,
    gift_note: Option<String>,
    thank_note: Option<String>,
}

#[derive(Serialize)]
struct ShelfResponse {
    name: String,
    gifts: Vec<ShelfGift>,
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
        .route("/api/l/{token}", get(handle_get_link))
        .route("/api/l/{token}/claim", post(handle_post_claim))
        .route("/api/l/{token}/thanks", post(handle_post_thanks))
        .route(
            "/api/l/{token}/steam/owned/{steamid}",
            get(handle_steam_owned_proxy),
        )
        .route("/api/l/{token}/games/{id}/detail", get(handle_game_detail))
        .route("/api/steam/login", get(handle_steam_login))
        .route("/api/steam/return", get(handle_steam_return))
        .route("/api/s/{token}", get(handle_get_shelf))
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

/// Build the OpenID `return_to` URL from the server-trusted `base_url` and the opaque server-side
/// `state` nonce (#86). The invite `ctx` no longer rides here — only the nonce, which the return
/// endpoint resolves back to `ctx` via `take_oidc_state`. So the 64-hex invite token never crosses
/// to Valve's origin. Both login (emitting the nonce) and return (rebuilding it after resolving the
/// nonce) call this helper → byte-match by construction, which `verify_openid_assertion` pins.
///
/// Security: `base_url` comes from config (env-threaded into AppState), NEVER from
/// Host/X-Forwarded-* request headers — this is the critical gate.
fn build_return_to(base_url: &str, state: &str) -> String {
    format!(
        "{}/api/steam/return?state={}",
        base_url,
        urlencoding::encode(state)
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

/// Typed steam login-flow error, replacing the raw fragment string literals (#47). The web SPA
/// string-matches the `Display` values EXACTLY (`consumeReturnFragment` in steamIdentity.ts;
/// the `verify_failed` branches in LinkPage.tsx/Ops.tsx) — Display IS the wire contract, pinned
/// byte-exact by `steam_login_error_fragments_are_the_wire_contract` below and end-to-end by the
/// api_test fragment tests.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SteamLoginError {
    /// The OpenID assertion did not verify — the identity claim itself was rejected.
    VerifyFailed,
    /// Steam (or our state write) was unavailable — nothing wrong with the identity claim.
    SteamUnreachable,
}

impl std::fmt::Display for SteamLoginError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            SteamLoginError::VerifyFailed => "verify_failed",
            SteamLoginError::SteamUnreachable => "steam_unreachable",
        })
    }
}

/// Build the `ctx`-relative redirect target carrying a steam login error fragment.
fn steam_error_fragment(ctx: &str, err: SteamLoginError) -> String {
    format!("{ctx}#steam_error={err}")
}

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
        return redirect_to(&steam_error_fragment(
            &ctx,
            SteamLoginError::SteamUnreachable,
        ));
    }

    // #86: mint an opaque single-use nonce, store OIDCSTATE#<nonce> → ctx (~5-min TTL), and put ONLY
    // the nonce in return_to. The invite token in `ctx` never crosses to Valve; the return endpoint
    // resolves the nonce back, single-use. A failed state write can't proceed (the return couldn't
    // resolve it) → bounce back with the same error fragment the SPA already handles.
    // Two v4 UUIDs concatenated (64 hex, ≥128 bits of getrandom/CSPRNG entropy) — the same shape as
    // the admin session token. A guessable state is a forgeable state, so this is unguessable by
    // construction, not by hope; a single v4 (122 bits) sits below the codebase's security-token bar.
    let nonce = format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    );
    let expires = OffsetDateTime::now_utc().unix_timestamp() + dynamo::schema::OIDC_STATE_TTL_SECS;
    if s.store.put_oidc_state(&nonce, &ctx, expires).await.is_err() {
        return redirect_to(&steam_error_fragment(
            &ctx,
            SteamLoginError::SteamUnreachable,
        ));
    }

    let return_to = build_return_to(&s.base_url, &nonce);
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
    // #86: the return_to carries an opaque `state` nonce, not the invite `ctx`. Resolve it
    // server-side and SINGLE-USE (take_oidc_state deletes on read) to recover the original ctx. An
    // unknown, expired, or already-consumed nonce — or a store error — → 302 `/` (no fragment), the
    // same dead-end a bad ctx got. First-occurrence semantics for `state`, mirroring the openid.*
    // params. This is the CSRF gate: a return can't be honored without a nonce this server minted.
    let state = all_params
        .iter()
        .find(|(k, _)| k == "state")
        .map(|(_, v)| v.clone())
        .unwrap_or_default();
    let now = OffsetDateTime::now_utc().unix_timestamp();
    let ctx = match s.store.take_oidc_state(&state, now).await {
        Ok(Some(ctx)) => ctx,
        _ => return redirect_to("/"),
    };

    // Defense-in-depth: the stored ctx was allowlisted at login, but re-validate the resolved value —
    // ctx_is_allowed stays the open-redirect guard on whatever actually builds the final redirect.
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

    // Reconstruct expected_return_to from server-trusted BASE_URL config — NEVER from any request
    // header. Login built it from the NONCE, so rebuild it from the same nonce here (not the ctx)
    // → byte-match by construction, which verify_openid_assertion pins.
    let expected_return_to = build_return_to(&s.base_url, &state);

    // Collect all params except our own `state` as the openid.* assertion params.
    let openid_params: Vec<(String, String)> = all_params
        .into_iter()
        .filter(|(k, _)| k != "state")
        .collect();

    // Verify the OpenID assertion.
    let steamid = match steam
        .verify_openid_assertion(&openid_params, &expected_return_to)
        .await
    {
        Ok(id) => id,
        Err(steam_client::SteamError::OpenIdRejected(_)) => {
            return redirect_to(&steam_error_fragment(&ctx, SteamLoginError::VerifyFailed));
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
            return redirect_to(&steam_error_fragment(
                &ctx,
                SteamLoginError::SteamUnreachable,
            ));
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

// ── GET /api/l/{token}/steam/owned/{steamid} ────────────────────────────────────

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
            ClaimRefusal::Sealed => "this gift is still wrapped",
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
    if !steam_client::is_valid_steam_id64(&steamid) {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": steam_client::STEAM_ID64_ERROR_MSG})),
        )
            .into_response();
    }

    // 4. Cache-or-fetch — the shared core with the admin proxy (24h freshness rule, #47);
    //    see Store::cached_owned_or_fetch.
    let now_epoch = now.unix_timestamp();
    match s
        .store
        .cached_owned_or_fetch(steam.as_ref(), &steamid, now_epoch)
        .await
    {
        OwnedProxyOutcome::Games(appids) => (
            StatusCode::OK,
            // Browser-side mirror of the server's freshness rule (#47): the appid list is
            // stable for the same 24h window the STEAMOWN cache serves. `private` — this
            // is a token-scoped, per-friend response; never shared-cacheable.
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

// ── GET /api/l/{token} ─────────────────────────────────────────────────────────

/// THE one liveness computation (spec §2): a game is LIVE on a link iff the
/// grid offers it as a claimable card. The curated partition below and the
/// detail gate (its second caller) both use this — "the gate mirrors the
/// grid, not one id more" is true by construction, not by maintenance. The
/// gate's #154 comment records the last hand-mirrored correspondence that
/// drifted; do not add a third caller-specific rederivation.
fn live_on_link(link: &domain::Link, game: &domain::Game) -> bool {
    match &link.curated_game_ids {
        // Curated: member, and Pending rescues ONLY the status axis — never a
        // deliberate hide or an ungiftable key (spec §2, Lilith's sign-off
        // catch: hidden+Pending has no path back to claimable, so `is_listable
        // || Pending` would pin a permanently-unclaimable card live).
        Some(ids) => {
            ids.iter().any(|id| id == &game.id)
                && game.giftable
                && !game.hidden
                && matches!(
                    game.status,
                    domain::GameStatus::Available | domain::GameStatus::Pending
                )
        }
        // Open shelf: the sparse listable index that feeds the grid IS this
        // predicate — one truth, two spellings, pinned by the gate tests.
        None => game.is_listable(),
    }
}

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
        Err(domain::ClaimRefusal::Sealed) => {
            // Sealed response: no catalog/claims/notes reads AT ALL — the payload is
            // withheld at the source, not filtered (devtools is not a spoiler channel;
            // pinned raw-string by sealed_link_view_withholds_everything_and_counts_down).
            // no-store: a cached sealed 200 outliving the moment would pin a countdown
            // past midnight (family review). Remaining is ceiled: never early, never 0.
            let unlock = link.unlock_at.expect("Sealed refusal implies unlock_at");
            let remaining_ms = (unlock - now).whole_milliseconds().max(1) as u64;
            let remaining = remaining_ms.div_ceil(1000);
            return (
                StatusCode::OK,
                [(header::CACHE_CONTROL, "no-store")],
                Json(LinkView {
                    label: link.label,
                    gift_note: None,
                    thank_note: None,
                    claims_allowed: link.claims_allowed,
                    claims_used: link.claims_used,
                    state: "sealed",
                    curated: false, // withheld, not false — the seal hides the mode too
                    games: vec![],
                    claims: vec![],
                    unlocks_in_seconds: Some(remaining),
                    unlocks_at: Some(
                        unlock
                            .format(&time::format_description::well_known::Rfc3339)
                            .expect("unlock_at formats rfc3339"),
                    ),
                }),
            )
                .into_response();
        }
        Err(domain::ClaimRefusal::Expired) => ("expired", true),
        Err(domain::ClaimRefusal::Exhausted) => ("exhausted", false),
    };

    // The games list and the claims history are independent reads — run them
    // concurrently. Each degrades on its own (empty grid / empty history).
    // Claims history is ALWAYS returned intact (spec §7); titles come from one
    // BatchGetItem over the claimed ids (claimed games leave the listable set,
    // so the games list can't supply them). A failed lookup degrades to
    // title:None — the client falls back to game_id.
    let is_curated = link.curated_game_ids.is_some();
    let (games, claims) = tokio::join!(
        async {
            if hide_games {
                return vec![];
            }
            match &link.curated_game_ids {
                Some(ids) => {
                    // Partition, never filter (spec §2): every stored id becomes a
                    // live card or a ghost, in ben's pick order; an id is skipped
                    // only when the game record no longer exists at all.
                    let found = match s.store.batch_get_games(ids).await {
                        Ok(m) => m,
                        Err(_) => return vec![],
                    };
                    let mut app_ids: Vec<u32> = found
                        .values()
                        .filter(|g| live_on_link(&link, g))
                        .filter_map(|g| g.steam_app_id)
                        .collect();
                    app_ids.sort_unstable();
                    app_ids.dedup();
                    let caches = s
                        .store
                        .batch_get_steam_genres_tags(&app_ids)
                        .await
                        .unwrap_or_default();
                    ids.iter()
                        .filter_map(|id| found.get(id))
                        .map(|g| {
                            // live vs ghost: the ONE computation, shared with the
                            // detail gate. (Membership is tautological here — g came
                            // from the set — but the shared fn is the point.)
                            if live_on_link(&link, g) {
                                let gt = g.steam_app_id.and_then(|id| caches.get(&id));
                                let genres = gt
                                    .map(|c| c.genres.iter().take(5).cloned().collect())
                                    .unwrap_or_default();
                                let tags = gt.map(|c| c.tags.clone()).unwrap_or_default();
                                GameView::from_game(g.clone(), genres, tags)
                            } else {
                                GameView::ghost(g.clone())
                            }
                        })
                        .collect()
                }
                None => {
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
                    // Slim read: only genres+tags per app, not the whole SteamAppCache blob (#64) — the
                    // list never touches detail's heavier fields (reviews, #62's screenshots).
                    let caches = s
                        .store
                        .batch_get_steam_genres_tags(&app_ids)
                        .await
                        .unwrap_or_default();
                    gs.into_iter()
                        .map(|g| {
                            let gt = g.steam_app_id.and_then(|id| caches.get(&id));
                            let genres = gt
                                .map(|c| c.genres.iter().take(5).cloned().collect())
                                .unwrap_or_default();
                            // Stored tags are already capped at 10 — no take() here.
                            let tags = gt.map(|c| c.tags.clone()).unwrap_or_default();
                            GameView::from_game(g, genres, tags)
                        })
                        .collect()
                }
            }
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
            // Same personal-content gate as gift_note/thank_note above: a dead
            // (revoked/expired) link serves no personal content, and the curated
            // mode is personal content too — withheld, not false.
            curated: if hide_games { false } else { is_curated },
            games,
            claims,
            unlocks_in_seconds: None,
            unlocks_at: None,
        }),
    )
        .into_response()
}

// ── POST /api/l/{token}/claim ──────────────────────────────────────────────────

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
            ClaimRefusal::Sealed => "this gift is still wrapped",
            ClaimRefusal::Expired => "this link has expired",
            ClaimRefusal::Exhausted => "no claims left on this link",
        };
        return (
            StatusCode::CONFLICT,
            Json(serde_json::json!({"error": msg})),
        )
            .into_response();
    }

    // 2.5 Curation gate: a curated link claims only its own games. Server-side —
    // the grid never offers the button, but the surface is not the boundary.
    // Pre-check is race-free BECAUSE the set is create-time-only: no edit exists
    // to race the freshly-read link (spec §3).
    if let Some(ids) = &link.curated_game_ids
        && !ids.contains(&body.game_id)
    {
        return (
            StatusCode::CONFLICT,
            Json(serde_json::json!({"error": "that one isn't part of this gift"})),
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
    // cloned BEFORE the request moves them: the bell rings only on GiftUrl below, and it must
    // name the same link+game the gift did.
    let bell_token = token.clone();
    let bell_game_id = body.game_id.clone();
    let bell_choice = game.requires_choice;
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
        Ok(FulfillResponse::KeyDead) => tracing::info!("claim: dead-key (410)"),
        Ok(other) => tracing::warn!(outcome = ?other, "claim: parked"),
        Err(e) => tracing::warn!(error = %e, "claim: fulfillment invoke failed → parked"),
    }
    match gift_result {
        Ok(FulfillResponse::GiftUrl { url }) => {
            // 🔔 the attic bell (spec: docs/spec-attic-bell.md), after durable success and
            // BEFORE the response: an Event invoke is a control-plane 202 (~20ms warm) and a
            // failure is a WARN — it never touches the friend's moment. Fired pre-response
            // because a frozen lambda never finishes a post-response task. The budget is
            // CHOSEN AGAINST THE COLD PATH: this app is low-traffic, so the first claim after
            // a quiet spell pays DNS+TCP+TLS with no pooled connection, and a warm-measured
            // 1s would concentrate misses on exactly the claim that matters. Affordable to
            // bound at all BECAUSE misses are counted (the ring ledger) and pinged (ops).
            let t0 = std::time::Instant::now();
            let mut outcome = "ok";
            match tokio::time::timeout(
                std::time::Duration::from_secs(2),
                s.invoker.bell(FulfillRequest::Bell {
                    event: fulfillment::bell::BellEvent::Unwrap {
                        link_token: bell_token,
                        game_id: bell_game_id,
                        // stamped HERE so the unwrap/ring pair shares one ledger week across
                        // the async hop
                        week: fulfillment::bell::current_week(),
                        choice: bell_choice,
                    },
                }),
            )
            .await
            {
                Ok(Ok(())) => {}
                Ok(Err(e)) => {
                    outcome = "err";
                    tracing::warn!(error = %e, "unwrap bell invoke failed — gift unaffected");
                }
                Err(_) => {
                    outcome = "timeout";
                    tracing::warn!("unwrap bell invoke timed out (2s) — gift unaffected");
                }
            }
            // the budget becomes a distribution — with the OUTCOME beside the duration: a
            // killed request and a 1998ms success write the same-shaped number, so an
            // unlabelled p99 reads the CAP as data and could only ever argue the budget down.
            tracing::info!(
                bell_invoke_ms = t0.elapsed().as_millis() as u64,
                outcome,
                "bell invoke duration"
            );
            (StatusCode::OK, Json(serde_json::json!({"gift_url": url}))).into_response()
        }
        Ok(FulfillResponse::AlreadyRedeemed) => (
            StatusCode::GONE,
            Json(serde_json::json!({
                "error": "that key was already redeemed on humble — pick another"
            })),
        )
            .into_response(),
        Ok(FulfillResponse::KeyDead) => (
            StatusCode::GONE,
            Json(serde_json::json!({
                "error": "that key can't be redeemed anymore — pick another"
            })),
        )
            .into_response(),
        // Parked, Error, transport failure, or any unexpected variant:
        // claim intake succeeded; reconcile owns the fate.
        _ => park_response(),
    }
}

// ── POST /api/l/{token}/thanks ─────────────────────────────────────────────────

/// Same budget as the gift note it answers (admin-api's `GIFT_NOTE_MAX_CHARS`) —
/// the correspondence is symmetric on purpose.
const THANK_NOTE_MAX_CHARS: usize = 500;

/// Characters that can visually reorder or invisibly pad the note when it renders
/// beside trusted admin chrome — the friend's text sits immediately before the
/// "— label, date" attribution ben reads, and a U+202E override would let it spoof
/// that signature (OMBB, #76 review; display-spoofing, not XSS — React escaping
/// holds). This is the Unicode Cf (format) category minus three carve-outs,
/// spelled out because `char::is_control` covers only Cc: bidi
/// embeddings/overrides/isolates, zero-width space, soft hyphen, word joiner +
/// invisible operators + deprecated formatting (the FULL U+2060–206F block —
/// pass 2 caught pass 1 stopping at 2069 and re-opening the invisible-note hole
/// through U+206A–206F; U+2065 is unassigned-and-default-ignorable, swept on
/// purpose), Arabic/Syriac/other prepended marks, interlinear annotation,
/// musical formatting, BOM, and the tag block (U+E0000–E007F — note this
/// degrades RGI subdivision-flag emoji like Scotland's to a plain black flag; an
/// accepted trade-off, the tag block is the canonical invisible-smuggling
/// channel and the base flag survives). Carve-outs, all "load-bearing in real
/// scripts, zero reordering power": ZWJ/ZWNJ (U+200C/D — emoji sequences, Indic)
/// and MVS (U+180E — selects Mongolian final-vowel forms; bidi class BN).
/// Intrinsic RTL text (Arabic/Hebrew letters) is untouched — only the invisible
/// controls are the spoofing vector.
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

/// Kept by the sanitizer (legitimate in real text) but rendering as nothing when
/// standing alone: the ZWJ/ZWNJ/MVS carve-outs, variation selectors, and the
/// blank-note classics — Hangul fillers (U+3164 is *the* "empty message"
/// character; Lo, so no category check catches it) and the Khmer inherent
/// vowels. Used by the emptiness gate: a note made ONLY of these is refused as
/// wordless rather than stored as a visibly blank note that permanently consumes
/// the friend's write-once slot (review pass 2 — the pass-1 guarantee held for
/// Cc/Cf but not for these).
fn is_invisible_standalone(c: char) -> bool {
    matches!(
        c,
        '\u{115F}'
            | '\u{1160}'
            | '\u{17B4}'
            | '\u{17B5}'
            | '\u{180E}'
            | '\u{200C}'
            | '\u{200D}'
            | '\u{3164}'
            | '\u{FE00}'..='\u{FE0F}'
            | '\u{FFA0}'
            | '\u{E0100}'..='\u{E01EF}'
    )
}

/// Normalize a raw note before validation: line/segment separators (newline, CR,
/// tab, VT, FF, NEL, U+2028/U+2029 — everything that breaks lines in some text
/// lineage; a PDF-paste's form feeds are word boundaries too, review pass 2)
/// become plain spaces so a multiline paste keeps its word boundaries, and every
/// other control character or spoofing format char is stripped. Runs BEFORE the
/// emptiness/length checks, so a note of nothing but invisibles is refused as
/// empty, and stripped characters can't smuggle a 501st visible char past the
/// budget.
fn sanitize_note(raw: &str) -> String {
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
    // "Empty" means no visible ink, not just no characters: the sanitizer keeps
    // ZWJ/ZWNJ/MVS/variation-selectors/Hangul-fillers because they're legitimate
    // INSIDE text, but a note made only of them renders blank beside ben's
    // attribution and burns the write-once slot on nothing (review pass 2).
    // Whitespace is inkless too — trim only strips the EDGES, and invisible
    // chars at the edges shield interior spaces from it (ZWJ\nZWJ sanitizes to
    // "ZWJ ZWJ" and sailed through an is_invisible-only check; converge pass).
    if note
        .chars()
        .all(|c| c.is_whitespace() || is_invisible_standalone(c))
    {
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
        // Sealed: refuse, same 409 shape as its neighbors. Unreachable in practice —
        // sealed ⇒ zero claims, so even without this arm the claims-first guard BELOW
        // this match would refuse — but pinned anyway: a ruling without an arm is a
        // guess with a compiler error attached (step-5 gate M1).
        Err(domain::ClaimRefusal::Sealed) => {
            return (
                StatusCode::CONFLICT,
                Json(serde_json::json!({"error": "this gift is still wrapped"})),
            )
                .into_response();
        }
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
            // 🔔 the thanks bell — same shape and same budget as the unwrap bell above. The
            // ring reads the STORED note (never this request body), and thanks rings are
            // deliberately NOT counted in the ledger: they would break the unwraps/rings
            // pairing that makes `rings < unwraps` readable as the suspect direction.
            let t0 = std::time::Instant::now();
            let mut outcome = "ok";
            match tokio::time::timeout(
                std::time::Duration::from_secs(2),
                s.invoker.bell(FulfillRequest::Bell {
                    event: fulfillment::bell::BellEvent::Thanks {
                        link_token: token.clone(),
                    },
                }),
            )
            .await
            {
                Ok(Ok(())) => {}
                Ok(Err(e)) => {
                    outcome = "err";
                    tracing::warn!(error = %e, "thanks bell invoke failed — thanks unaffected");
                }
                Err(_) => {
                    outcome = "timeout";
                    tracing::warn!("thanks bell invoke timed out (2s) — thanks unaffected");
                }
            }
            tracing::info!(
                bell_invoke_ms = t0.elapsed().as_millis() as u64,
                outcome,
                "bell invoke duration"
            );
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
        // Steps 3-4 raced past the pre-checks and the storage guards caught them —
        // same messages as the pre-checks, the friend can't tell the paths apart.
        Ok(dynamo::SetThanksOutcome::Revoked) => (
            StatusCode::CONFLICT,
            Json(serde_json::json!({"error": "this link has been revoked"})),
        )
            .into_response(),
        Ok(dynamo::SetThanksOutcome::Expired) => (
            StatusCode::CONFLICT,
            Json(serde_json::json!({"error": "this link has expired"})),
        )
            .into_response(),
        Ok(dynamo::SetThanksOutcome::NoClaims) => (
            StatusCode::CONFLICT,
            Json(serde_json::json!({"error": "claim a game first"})),
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

// ── GET /api/l/{token}/games/{id}/detail ───────────────────────────────────────

/// Token-scoped game detail endpoint. Friend-facing, cache-only: Steam is never called.
///
/// Access rule (no-oracle): the link must resolve AND (`live_on_link(&link, &game)` —
/// the same shared liveness computation the games grid uses — OR the game id appears
/// in this specific link's claims history, the friend's receipt for something they
/// already claimed off this link). Any other condition → byte-identical 404 so callers
/// learn nothing about why. On curated links this is a deliberate tightening: a
/// listable game that is NOT one of the link's curated picks is not `live_on_link`, so
/// it 404s here too — a curated token cannot use this endpoint to enumerate the whole
/// catalog (spec §2).
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

    // 1b. Liveness gate (#154 — this endpoint predated the exhaustive-match socket and
    //     never consulted can_claim): detail serves iff the games GRID is visible on this
    //     link — active + exhausted. Refusals reuse the endpoint's byte-identical 404 (no
    //     oracle: a dead link must not grow a distinguishable "something's here" response),
    //     and the refusing path does the same link lookup then strictly less work, so the
    //     404 is indistinguishable from true not-found in timing too (spec §2).
    //     Pinned by game_detail_refuses_revoked_link_154.
    let now = OffsetDateTime::now_utc();
    match link.can_claim(now) {
        Ok(()) | Err(domain::ClaimRefusal::Exhausted) => {}
        Err(
            domain::ClaimRefusal::Revoked
            | domain::ClaimRefusal::Expired
            | domain::ClaimRefusal::Sealed,
        ) => {
            return link_not_found_response();
        }
    }

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

    // 3. Friend access gate: live on THIS link's grid (the shared live_on_link
    //    computation — the gate serves exactly what the grid offers, by
    //    construction) OR in THIS link's claims history. Everything else is the
    //    byte-identical 404, no oracle. NOTE the deliberate tightening on curated
    //    links: a listable NON-member is not on this grid, so it 404s here — a
    //    curated token cannot enumerate the whole catalog's details (spec §2).
    let accessible = if live_on_link(&link, &game) {
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

// ── GET /api/s/{token} — the gift shelf ─────────────────────────────────────────

/// Resolve a bearer shelf token to its friend's shelf. Unknown AND revoked tokens
/// share the byte-identical 404 (no oracle) via `link_not_found_response()`. Any
/// store error anywhere — INCLUDING a gsi3 query error during the deploy backfill
/// window — surfaces as 500; a partial or soft-empty shelf would be a lie.
async fn handle_get_shelf(State(s): State<AppState>, Path(token): Path<String>) -> Response {
    let friend = match s.store.get_friend_by_shelf_token(&token).await {
        Ok(Some(f)) => f,
        Ok(None) => return link_not_found_response(),
        Err(_) => return shelf_error_response(),
    };
    match assemble_shelf(&s.store, friend).await {
        Ok(shelf) => (StatusCode::OK, Json(shelf)).into_response(),
        Err(_) => shelf_error_response(),
    }
}

fn shelf_error_response() -> Response {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(serde_json::json!({"error": "the shelf slipped — try again"})),
    )
        .into_response()
}

/// Build a friend's shelf: every Fulfilled claim across every link currently
/// assigned to them (Pending isn't a gift yet, Failed never was, Compensated was
/// taken back), oldest-unwrapped-first. No detail-assembly extraction in v1 — the
/// shelf needs only `title` + `artwork_url` off `Game`, never the steam cache.
///
/// Every fallible step propagates with `?`: a gsi3 query error (deploy window), a
/// claims-query error, or a batch-get error all become the caller's 500, never an
/// `Ok(vec![])`. A claim whose game record went missing is `StoreError::Corrupt` —
/// a partial shelf is a lie, not a skipped row.
async fn assemble_shelf(
    store: &Store,
    friend: domain::Friend,
) -> Result<ShelfResponse, StoreError> {
    // Friend-scoped by construction: `list_links_for_friend` queries gsi3 for
    // exactly this friend's links, so another friend's claims can never bleed in.
    // A gsi3 query error (deploy-window fault injection) propagates via `?` — no
    // `unwrap_or_default`, no `Ok(vec![])`.
    let links = store.list_links_for_friend(&friend.id).await?;

    // (claim, gift_note, thank_note) for every Fulfilled claim across those links —
    // Pending isn't a gift yet, Failed never was, Compensated was taken back.
    let mut fulfilled: Vec<(domain::Claim, Option<String>, Option<String>)> = Vec::new();
    for link in &links {
        let claims = store.claims_for_link(&link.token).await?;
        for c in claims {
            if c.state == domain::ClaimState::Fulfilled {
                fulfilled.push((c, link.gift_note.clone(), link.thank_note.clone()));
            }
        }
    }

    // ONE batch_get_games call for every game on the shelf — never N serial gets.
    let game_ids: Vec<String> = fulfilled
        .iter()
        .map(|(c, _, _)| c.game_id.clone())
        .collect();
    let games = store.batch_get_games(&game_ids).await?;

    let mut gifts = Vec::with_capacity(fulfilled.len());
    for (c, gift_note, thank_note) in fulfilled {
        // A claim whose game record is missing is corruption, not an empty slot —
        // a partial shelf would be a lie, so this is an error, not a skipped row.
        let game = games.get(&c.game_id).ok_or(StoreError::Corrupt(
            "shelf: fulfilled claim references a game record that no longer exists",
        ))?;
        gifts.push(ShelfGift {
            game_id: c.game_id,
            title: game.title.clone(),
            artwork_url: game.artwork_url.clone(),
            unwrapped_at: c.created_at,
            gift_note,
            thank_note,
        });
    }
    gifts.sort_by_key(|g| g.unwrapped_at);

    Ok(ShelfResponse {
        name: friend.name,
        gifts,
    })
}

#[cfg(test)]
mod steam_login_error_tests {
    use super::*;

    /// Display IS the wire contract: the SPA string-matches these fragment values
    /// (steamIdentity.ts / LinkPage.tsx / Ops.tsx). Byte-exact, forever.
    #[test]
    fn steam_login_error_fragments_are_the_wire_contract() {
        assert_eq!(SteamLoginError::VerifyFailed.to_string(), "verify_failed");
        assert_eq!(
            SteamLoginError::SteamUnreachable.to_string(),
            "steam_unreachable"
        );
        assert_eq!(
            steam_error_fragment("/l/tok", SteamLoginError::VerifyFailed),
            "/l/tok#steam_error=verify_failed"
        );
    }
}
