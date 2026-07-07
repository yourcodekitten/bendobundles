//! Integration tests for the admin-api router.
//!
//! Two categories:
//! - **Pure-mock** (no DynamoDB): tests where the route handler returns before touching the store
//!   (wrong-password login → 401; no-cookie → 401). These use `fake_store()` and run everywhere.
//! - **Store-backed**: tests that need a real DynamoDB-local instance (session creation, links,
//!   games). These use `store_or_skip` and are skipped locally; no local DynamoDB exists on this
//!   box and we never claim otherwise.
use std::sync::Arc;

use admin_api::{AdminInvoker, router};
use async_trait::async_trait;
use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use domain::{Claim, ClaimState, Game, GameStatus, Link, game_id};
use dynamo::{SteamAppCache, Store};
use fulfillment::{FulfillRequest, FulfillResponse};
use steam_client::{RecentReviews, ReviewSummary, SteamApiKey, SteamAppDetail, SteamClient};
use time::macros::datetime;
use tokio::sync::Mutex;
use tower::ServiceExt;

// ── DynamoDB-local helpers ─────────────────────────────────────────────────────

/// Real DynamoDB-local store, one table per test. Returns None if dynamo-local is unreachable.
async fn store_or_skip(test: &str) -> Option<Arc<Store>> {
    let (url, explicit) = match std::env::var("DYNAMODB_LOCAL_URL") {
        Ok(v) => (v, true),
        Err(_) => ("http://localhost:8000".into(), false),
    };
    let config = aws_config::defaults(aws_config::BehaviorVersion::latest())
        .endpoint_url(&url)
        .region("us-east-1")
        .test_credentials()
        .load()
        .await;
    let client = aws_sdk_dynamodb::Client::new(&config);
    if client.list_tables().send().await.is_err() {
        if explicit {
            panic!(
                "DYNAMODB_LOCAL_URL is set but dynamodb-local is unreachable — \
                 refusing to skip (this would forge a green run)"
            );
        }
        eprintln!("SKIP {test}: no dynamodb-local at {url}");
        return None;
    }
    let store = Store::new(client, format!("t-adm-{test}"));
    store.create_table_for_tests().await.unwrap();
    Some(Arc::new(store))
}

/// Fake store for pure-mock tests. The underlying DynamoDB client points at a non-listening port;
/// any actual DynamoDB call would fail, but pure-mock tests never reach the store.
async fn fake_store() -> Arc<Store> {
    let cfg = aws_config::defaults(aws_config::BehaviorVersion::latest())
        .endpoint_url("http://127.0.0.1:0") // nothing here — never called in pure-mock tests
        .region("us-east-1")
        .test_credentials()
        .load()
        .await;
    Arc::new(Store::new(
        aws_sdk_dynamodb::Client::new(&cfg),
        "fake".into(),
    ))
}

// ── Argon2 test helper ─────────────────────────────────────────────────────────

/// Hash `password` with argon2 (random salt) and return the PHC string.
/// Use this to build `admin_hash` in each test.
fn test_admin_hash(password: &str) -> String {
    use argon2::{
        Argon2, PasswordHasher,
        password_hash::{SaltString, rand_core::OsRng},
    };
    let salt = SaltString::generate(&mut OsRng);
    Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .unwrap()
        .to_string()
}

// ── MockAdminInvoker ───────────────────────────────────────────────────────────

struct MockAdminInvoker {
    /// Last request received via `fire`, so tests can prove sync-now actually queued an async
    /// invoke. Stored as Value (FulfillRequest doesn't derive Clone).
    fired: Mutex<Option<serde_json::Value>>,
}

impl MockAdminInvoker {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            fired: Mutex::new(None),
        })
    }

    async fn last_fired(&self) -> Option<FulfillRequest> {
        self.fired
            .lock()
            .await
            .clone()
            .map(|v| serde_json::from_value(v).expect("captured request must deserialize"))
    }
}

#[async_trait]
impl AdminInvoker for MockAdminInvoker {
    async fn fire(&self, req: FulfillRequest) -> Result<(), String> {
        *self.fired.lock().await = Some(serde_json::to_value(&req).unwrap());
        Ok(())
    }

    async fn call(&self, _req: FulfillRequest) -> Result<FulfillResponse, String> {
        // fire-only mock: call is not expected in sync/fire tests
        Err("MockAdminInvoker::call not implemented — use MockCallInvoker".into())
    }
}

// ── Body / cookie helpers ──────────────────────────────────────────────────────

async fn body_json(resp: axum::response::Response) -> serde_json::Value {
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    serde_json::from_slice(&bytes).expect("response body must be JSON")
}

/// POST to /admin/api/login and return the `session=<token>` value from Set-Cookie.
/// Panics if login doesn't succeed — use only in tests where login is expected to work.
async fn admin_login(
    store: &Arc<Store>,
    invoker: &Arc<dyn AdminInvoker>,
    admin_hash: &str,
    password: &str,
) -> String {
    let req = Request::post("/admin/api/login")
        .header("content-type", "application/json")
        .body(Body::from(
            serde_json::to_vec(&serde_json::json!({"password": password})).unwrap(),
        ))
        .unwrap();

    let resp = router(
        Arc::clone(store),
        Arc::clone(invoker),
        admin_hash.to_string(),
        None,
    )
    .oneshot(req)
    .await
    .unwrap();

    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "admin_login helper: login must succeed"
    );

    resp.headers()
        .get(axum::http::header::SET_COOKIE)
        .expect("login must set a cookie")
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .strip_prefix("session=")
        .expect("Set-Cookie must start with 'session='")
        .to_string()
}

// ── Test fixtures ─────────────────────────────────────────────────────────────

fn test_game(n: u32) -> Game {
    Game {
        id: game_id(&format!("gk{n}"), "mn"),
        title: format!("Admin Test Game {n}"),
        bundle: "Test Bundle".into(),
        gamekey: format!("gk{n}"),
        machine_name: "mn".into(),
        key_type: "steam".into(),
        giftable: true,
        hidden: false,
        status: GameStatus::Available,
        claim_id: None,
        artwork_url: None,
        keyindex: n,
        requires_choice: false,
        steam_app_id: None,
        appid_source: None,
        owned_by_ben: false,
    }
}

fn test_link(token: &str) -> Link {
    Link {
        token: token.into(),
        label: "Admin Test Link".into(),
        claims_allowed: 3,
        claims_used: 0,
        revoked: false,
        expires_at: None,
        created_at: datetime!(2026-07-02 00:00 UTC),
    }
}

// ── Pure-mock tests (no DynamoDB) ─────────────────────────────────────────────

/// Wrong password → 401 (500 ms sleep in handler; test accepts this).
/// The store is never touched — handler returns before any DynamoDB call.
#[tokio::test]
async fn login_wrong_password_returns_401() {
    let store = fake_store().await;
    let invoker: Arc<dyn AdminInvoker> = MockAdminInvoker::new();
    let admin_hash = test_admin_hash("correct-pw");

    let req = Request::post("/admin/api/login")
        .header("content-type", "application/json")
        .body(Body::from(r#"{"password":"wrong-pw"}"#))
        .unwrap();

    let resp = router(store, invoker, admin_hash, None)
        .oneshot(req)
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

/// No session cookie → 401 on any protected route. The session middleware rejects before any
/// DynamoDB call — the store is never touched.
#[tokio::test]
async fn no_session_cookie_on_protected_route_returns_401() {
    let store = fake_store().await;
    let invoker: Arc<dyn AdminInvoker> = MockAdminInvoker::new();
    let admin_hash = test_admin_hash("pw");

    let req = Request::get("/admin/api/catalog")
        .body(Body::empty())
        .unwrap();

    let resp = router(store, invoker, admin_hash, None)
        .oneshot(req)
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

/// POST /admin/api/games/:id/self-claim without session cookie → 401.
#[tokio::test]
async fn no_session_cookie_on_self_claim_returns_401() {
    let store = fake_store().await;
    let invoker: Arc<dyn AdminInvoker> = MockAdminInvoker::new();
    let admin_hash = test_admin_hash("pw");

    let req = Request::post("/admin/api/games/some-id/self-claim")
        .header("content-type", "application/json")
        .body(Body::from("{}"))
        .unwrap();

    let resp = router(store, invoker, admin_hash, None)
        .oneshot(req)
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

/// GET /admin/api/claims/self without session cookie → 401.
#[tokio::test]
async fn no_session_cookie_on_claims_self_returns_401() {
    let store = fake_store().await;
    let invoker: Arc<dyn AdminInvoker> = MockAdminInvoker::new();
    let admin_hash = test_admin_hash("pw");

    let req = Request::get("/admin/api/claims/self")
        .body(Body::empty())
        .unwrap();

    let resp = router(store, invoker, admin_hash, None)
        .oneshot(req)
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

// ── Store-backed tests ─────────────────────────────────────────────────────────

/// Login with correct password → Set-Cookie with 64-char session token; subsequent authed
/// request with that cookie → 200 on a protected route.
#[tokio::test]
async fn login_correct_password_sets_cookie_and_enables_auth() {
    let Some(store) = store_or_skip("login-auth").await else {
        return;
    };
    let password = "hunter42";
    let admin_hash = test_admin_hash(password);
    let invoker: Arc<dyn AdminInvoker> = MockAdminInvoker::new();

    let session = admin_login(&store, &invoker, &admin_hash, password).await;
    // Token = two uuid-v4 simple (32 chars each) → 64 hex chars
    assert_eq!(session.len(), 64, "session token must be 64 hex chars");
    assert!(
        session.chars().all(|c| c.is_ascii_hexdigit()),
        "session token must be all hex"
    );

    // Authed GET /admin/api/catalog → 200
    let catalog_req = Request::get("/admin/api/catalog")
        .header("cookie", format!("session={session}"))
        .body(Body::empty())
        .unwrap();

    let resp = router(Arc::clone(&store), Arc::clone(&invoker), admin_hash, None)
        .oneshot(catalog_req)
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
}

/// POST /admin/api/links → token is exactly 64 hex chars; GET /admin/api/links lists the created
/// link with the correct label and claims_allowed.
#[tokio::test]
async fn create_link_token_is_64_chars_and_visible_in_list() {
    let Some(store) = store_or_skip("create-link").await else {
        return;
    };
    let password = "linkpw";
    let admin_hash = test_admin_hash(password);
    let invoker: Arc<dyn AdminInvoker> = MockAdminInvoker::new();

    let session = admin_login(&store, &invoker, &admin_hash, password).await;

    // POST /admin/api/links
    let create_req = Request::post("/admin/api/links")
        .header("content-type", "application/json")
        .header("cookie", format!("session={session}"))
        .body(Body::from(
            serde_json::to_vec(&serde_json::json!({"label": "Dave", "claims_allowed": 2})).unwrap(),
        ))
        .unwrap();

    let create_resp = router(
        Arc::clone(&store),
        Arc::clone(&invoker),
        admin_hash.clone(),
        None,
    )
    .oneshot(create_req)
    .await
    .unwrap();

    assert_eq!(create_resp.status(), StatusCode::OK);
    let j = body_json(create_resp).await;
    let token = j["token"].as_str().unwrap().to_string();
    assert_eq!(token.len(), 64, "token must be 64 hex chars");
    assert!(
        token.chars().all(|c| c.is_ascii_hexdigit()),
        "token must be all hex"
    );
    assert_eq!(j["url_path"], format!("/l/{token}"));

    // GET /admin/api/links — the created link must appear
    let list_req = Request::get("/admin/api/links")
        .header("cookie", format!("session={session}"))
        .body(Body::empty())
        .unwrap();

    let list_resp = router(Arc::clone(&store), Arc::clone(&invoker), admin_hash, None)
        .oneshot(list_req)
        .await
        .unwrap();

    assert_eq!(list_resp.status(), StatusCode::OK);
    let links = body_json(list_resp).await;
    let arr = links.as_array().unwrap();
    let created = arr.iter().find(|l| l["token"] == token).unwrap();
    assert_eq!(created["label"], "Dave");
    assert_eq!(created["claims_allowed"], 2);
    assert_eq!(created["claims_used"], 0);
}

/// POST a create-link body and return the response. Shared by the input-validation tests.
async fn post_create_link(
    store: &Arc<Store>,
    invoker: &Arc<dyn AdminInvoker>,
    admin_hash: &str,
    session: &str,
    body: serde_json::Value,
) -> axum::response::Response {
    let req = Request::post("/admin/api/links")
        .header("content-type", "application/json")
        .header("cookie", format!("session={session}"))
        .body(Body::from(serde_json::to_vec(&body).unwrap()))
        .unwrap();
    router(
        Arc::clone(store),
        Arc::clone(invoker),
        admin_hash.to_string(),
        None,
    )
    .oneshot(req)
    .await
    .unwrap()
}

/// POST /admin/api/links with an absurd expires_days → 422, NOT a panic.
/// `OffsetDateTime + Duration::days(4_000_000_000)` panics (year > 9999) — before validation,
/// this body 502'd the lambda and forced a cold restart.
#[tokio::test]
async fn create_link_absurd_expires_days_returns_422_not_panic() {
    let Some(store) = store_or_skip("link-422-days").await else {
        return;
    };
    let password = "valpw";
    let admin_hash = test_admin_hash(password);
    let invoker: Arc<dyn AdminInvoker> = MockAdminInvoker::new();
    let session = admin_login(&store, &invoker, &admin_hash, password).await;

    let resp = post_create_link(
        &store,
        &invoker,
        &admin_hash,
        &session,
        serde_json::json!({"label": "Overflow", "claims_allowed": 1, "expires_days": 4_000_000_000u32}),
    )
    .await;

    assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let j = body_json(resp).await;
    assert!(
        j["error"].as_str().unwrap().contains("expires_days"),
        "error must name the bad field, got: {j}"
    );
}

/// POST /admin/api/links with claims_allowed: 0 → 422. A zero-claim link is born exhausted —
/// it can never be used and only clutters the list.
#[tokio::test]
async fn create_link_zero_claims_allowed_returns_422() {
    let Some(store) = store_or_skip("link-422-claims").await else {
        return;
    };
    let password = "valpw";
    let admin_hash = test_admin_hash(password);
    let invoker: Arc<dyn AdminInvoker> = MockAdminInvoker::new();
    let session = admin_login(&store, &invoker, &admin_hash, password).await;

    let resp = post_create_link(
        &store,
        &invoker,
        &admin_hash,
        &session,
        serde_json::json!({"label": "Zero", "claims_allowed": 0}),
    )
    .await;

    assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let j = body_json(resp).await;
    assert!(
        j["error"].as_str().unwrap().contains("claims_allowed"),
        "error must name the bad field, got: {j}"
    );
}

/// POST /admin/api/links with an over-long label → 422.
#[tokio::test]
async fn create_link_overlong_label_returns_422() {
    let Some(store) = store_or_skip("link-422-label").await else {
        return;
    };
    let password = "valpw";
    let admin_hash = test_admin_hash(password);
    let invoker: Arc<dyn AdminInvoker> = MockAdminInvoker::new();
    let session = admin_login(&store, &invoker, &admin_hash, password).await;

    let resp = post_create_link(
        &store,
        &invoker,
        &admin_hash,
        &session,
        serde_json::json!({"label": "x".repeat(201), "claims_allowed": 1}),
    )
    .await;

    assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let j = body_json(resp).await;
    assert!(
        j["error"].as_str().unwrap().contains("label"),
        "error must name the bad field, got: {j}"
    );
}

/// Regression guard: a valid create with expires_days at the max bound (3650) still succeeds —
/// validation must reject the absurd, never the legitimate.
#[tokio::test]
async fn create_link_valid_with_max_expires_days_succeeds() {
    let Some(store) = store_or_skip("link-valid-days").await else {
        return;
    };
    let password = "valpw";
    let admin_hash = test_admin_hash(password);
    let invoker: Arc<dyn AdminInvoker> = MockAdminInvoker::new();
    let session = admin_login(&store, &invoker, &admin_hash, password).await;

    let resp = post_create_link(
        &store,
        &invoker,
        &admin_hash,
        &session,
        serde_json::json!({"label": "Decade", "claims_allowed": 100, "expires_days": 3650}),
    )
    .await;

    assert_eq!(resp.status(), StatusCode::OK);
    let j = body_json(resp).await;
    assert_eq!(j["token"].as_str().unwrap().len(), 64);
}

/// The validation bounds are EXACT: the last legal value on each side passes, the first illegal
/// value is rejected. Guards against off-by-one drift in the 1..=MAX ranges.
#[tokio::test]
async fn create_link_bounds_are_exact() {
    let Some(store) = store_or_skip("link-bounds").await else {
        return;
    };
    let password = "valpw";
    let admin_hash = test_admin_hash(password);
    let invoker: Arc<dyn AdminInvoker> = MockAdminInvoker::new();
    let session = admin_login(&store, &invoker, &admin_hash, password).await;

    // (body, expected status, name) — one post per boundary edge.
    let cases = [
        (
            serde_json::json!({"label": "d0", "claims_allowed": 1, "expires_days": 0}),
            StatusCode::UNPROCESSABLE_ENTITY,
            "expires_days = 0 (below min)",
        ),
        (
            serde_json::json!({"label": "d1", "claims_allowed": 1, "expires_days": 1}),
            StatusCode::OK,
            "expires_days = 1 (min legal)",
        ),
        (
            serde_json::json!({"label": "d3651", "claims_allowed": 1, "expires_days": 3651}),
            StatusCode::UNPROCESSABLE_ENTITY,
            "expires_days = 3651 (just past max)",
        ),
        (
            serde_json::json!({"label": "c101", "claims_allowed": 101}),
            StatusCode::UNPROCESSABLE_ENTITY,
            "claims_allowed = 101 (just past max)",
        ),
        (
            serde_json::json!({"label": "x".repeat(200), "claims_allowed": 1}),
            StatusCode::OK,
            "label = 200 chars (max legal)",
        ),
        // The 422 message promises "characters", so the bound MUST count chars, not bytes:
        // 200 × 'é' is 400 utf-8 bytes but exactly 200 chars — a bytes-based check would reject it.
        (
            serde_json::json!({"label": "é".repeat(200), "claims_allowed": 1}),
            StatusCode::OK,
            "label = 200 multibyte chars (max legal — chars, not bytes)",
        ),
        (
            serde_json::json!({"label": "é".repeat(201), "claims_allowed": 1}),
            StatusCode::UNPROCESSABLE_ENTITY,
            "label = 201 multibyte chars (just past max)",
        ),
    ];

    for (body, want, name) in cases {
        let resp = post_create_link(&store, &invoker, &admin_hash, &session, body).await;
        assert_eq!(resp.status(), want, "boundary case: {name}");
    }
}

/// GET /admin/api/links/:token/claims on an unknown token → 404, matching the revoke handler.
/// Before this, "no such link" and "link exists, no claims" were both 200 [].
#[tokio::test]
async fn link_claims_unknown_token_returns_404() {
    let Some(store) = store_or_skip("claims-404").await else {
        return;
    };
    let password = "clmpw";
    let admin_hash = test_admin_hash(password);
    let invoker: Arc<dyn AdminInvoker> = MockAdminInvoker::new();
    let session = admin_login(&store, &invoker, &admin_hash, password).await;

    let req = Request::get("/admin/api/links/no-such-token/claims")
        .header("cookie", format!("session={session}"))
        .body(Body::empty())
        .unwrap();
    let resp = router(Arc::clone(&store), Arc::clone(&invoker), admin_hash, None)
        .oneshot(req)
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

/// GET /admin/api/catalog returns the full game list (including hidden). POST .../hidden toggles
/// the hidden flag and the change is reflected in the next catalog fetch.
#[tokio::test]
async fn catalog_and_hidden_toggle_reflected() {
    let Some(store) = store_or_skip("catalog-hidden").await else {
        return;
    };
    let g = test_game(1);
    let gid = g.id.clone();
    store.put_game(&g).await.unwrap();

    let password = "hidepw";
    let admin_hash = test_admin_hash(password);
    let invoker: Arc<dyn AdminInvoker> = MockAdminInvoker::new();

    let session = admin_login(&store, &invoker, &admin_hash, password).await;

    // GET /admin/api/catalog: game must be present, hidden=false
    let cat1_req = Request::get("/admin/api/catalog")
        .header("cookie", format!("session={session}"))
        .body(Body::empty())
        .unwrap();
    let cat1_resp = router(
        Arc::clone(&store),
        Arc::clone(&invoker),
        admin_hash.clone(),
        None,
    )
    .oneshot(cat1_req)
    .await
    .unwrap();
    assert_eq!(cat1_resp.status(), StatusCode::OK);
    let cat1 = body_json(cat1_resp).await;
    let game_in_cat = cat1
        .as_array()
        .unwrap()
        .iter()
        .find(|g| g["id"] == gid)
        .unwrap();
    assert_eq!(game_in_cat["hidden"], false);

    // POST /admin/api/games/:id/hidden {hidden: true}
    let hide_req = Request::post(format!("/admin/api/games/{gid}/hidden"))
        .header("content-type", "application/json")
        .header("cookie", format!("session={session}"))
        .body(Body::from(r#"{"hidden":true}"#))
        .unwrap();
    let hide_resp = router(
        Arc::clone(&store),
        Arc::clone(&invoker),
        admin_hash.clone(),
        None,
    )
    .oneshot(hide_req)
    .await
    .unwrap();
    assert_eq!(hide_resp.status(), StatusCode::OK);

    // GET /admin/api/catalog again: game must now show hidden=true
    let cat2_req = Request::get("/admin/api/catalog")
        .header("cookie", format!("session={session}"))
        .body(Body::empty())
        .unwrap();
    let cat2_resp = router(Arc::clone(&store), Arc::clone(&invoker), admin_hash, None)
        .oneshot(cat2_req)
        .await
        .unwrap();
    assert_eq!(cat2_resp.status(), StatusCode::OK);
    let cat2 = body_json(cat2_resp).await;
    let game_after = cat2
        .as_array()
        .unwrap()
        .iter()
        .find(|g| g["id"] == gid)
        .unwrap();
    assert_eq!(game_after["hidden"], true);
}

/// POST /admin/api/links/:token/revoke sets revoked=true; confirmed via store.get_link.
#[tokio::test]
async fn revoke_link_is_reflected_in_store() {
    let Some(store) = store_or_skip("revoke-link").await else {
        return;
    };
    let lnk = test_link("test-revoke-tok");
    store.create_link(&lnk).await.unwrap();

    let password = "revokepw";
    let admin_hash = test_admin_hash(password);
    let invoker: Arc<dyn AdminInvoker> = MockAdminInvoker::new();

    let session = admin_login(&store, &invoker, &admin_hash, password).await;

    let revoke_req = Request::post("/admin/api/links/test-revoke-tok/revoke")
        .header("cookie", format!("session={session}"))
        .body(Body::empty())
        .unwrap();
    let revoke_resp = router(Arc::clone(&store), Arc::clone(&invoker), admin_hash, None)
        .oneshot(revoke_req)
        .await
        .unwrap();
    assert_eq!(revoke_resp.status(), StatusCode::OK);

    // Confirm revoked in store
    let stored = store.get_link("test-revoke-tok").await.unwrap().unwrap();
    assert!(
        stored.revoked,
        "link must be revoked in store after POST .../revoke"
    );
}

/// GET /admin/api/status on a store that has NEVER synced → `sync` is JSON
/// null (not a defaulted SyncState). A defaulted object would carry
/// cookie_ok:false and fire the client's red "humble session needs attention"
/// banner on every fresh deploy; null renders the clean "never" state.
#[tokio::test]
async fn status_never_synced_serializes_sync_null() {
    let Some(store) = store_or_skip("status-null").await else {
        return;
    };

    let password = "statuspw";
    let admin_hash = test_admin_hash(password);
    let invoker: Arc<dyn AdminInvoker> = MockAdminInvoker::new();

    let session = admin_login(&store, &invoker, &admin_hash, password).await;

    let req = Request::get("/admin/api/status")
        .header("cookie", format!("session={session}"))
        .body(Body::empty())
        .unwrap();
    let resp = router(Arc::clone(&store), Arc::clone(&invoker), admin_hash, None)
        .oneshot(req)
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let j = body_json(resp).await;
    assert!(
        j["sync"].is_null(),
        "never-synced must serialize sync as null, got: {}",
        j["sync"]
    );
    assert!(j["game_counts"].is_object(), "game_counts always present");
}

/// GET /admin/api/catalog must NOT leak humble order-key material: the raw
/// domain::Game carries gamekey / machine_name / keyindex, which build
/// FulfillRequest::Gift and have no business in a browser network tab.
#[tokio::test]
async fn catalog_does_not_leak_order_key_material() {
    let Some(store) = store_or_skip("catalog-no-leak").await else {
        return;
    };
    store.put_game(&test_game(1)).await.unwrap();

    let password = "leakpw";
    let admin_hash = test_admin_hash(password);
    let invoker: Arc<dyn AdminInvoker> = MockAdminInvoker::new();

    let session = admin_login(&store, &invoker, &admin_hash, password).await;

    let req = Request::get("/admin/api/catalog")
        .header("cookie", format!("session={session}"))
        .body(Body::empty())
        .unwrap();
    let resp = router(Arc::clone(&store), Arc::clone(&invoker), admin_hash, None)
        .oneshot(req)
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let j = body_json(resp).await;
    let game = &j.as_array().unwrap()[0];
    for leaked in ["gamekey", "machine_name", "keyindex"] {
        assert!(
            game.get(leaked).is_none(),
            "catalog must not expose {leaked}"
        );
    }
    // The fields the admin UI actually renders are all present.
    for kept in [
        "id",
        "title",
        "bundle",
        "key_type",
        "giftable",
        "hidden",
        "status",
        "claim_id",
        "artwork_url",
    ] {
        assert!(game.get(kept).is_some(), "catalog must keep {kept}");
    }
}

/// GET /admin/api/links/:token/claims must NOT ship the friend's one-time
/// gift URL to the admin — the wire carries a redacted AdminClaimView with
/// only `issued: bool`. The URL is the friend's bearer secret; the plan says
/// it never reaches the admin surface, and "we just don't render it" is not
/// redaction.
#[tokio::test]
async fn link_claims_redact_gift_url_to_issued_bool() {
    let Some(store) = store_or_skip("claims-redact").await else {
        return;
    };
    let lnk = test_link("aud-tok");
    store.create_link(&lnk).await.unwrap();
    store
        .put_claim(&domain::Claim {
            id: "c-1".into(),
            link_token: "aud-tok".into(),
            game_id: "g-1".into(),
            state: domain::ClaimState::Fulfilled,
            gift_url: Some("https://humble.example/gift?key=SECRET".into()),
            created_at: datetime!(2026-07-03 14:00 UTC),
            choice_pre_tpks: None,
            revealed_key: None,
        })
        .await
        .unwrap();

    let password = "auditpw";
    let admin_hash = test_admin_hash(password);
    let invoker: Arc<dyn AdminInvoker> = MockAdminInvoker::new();

    let session = admin_login(&store, &invoker, &admin_hash, password).await;

    let req = Request::get("/admin/api/links/aud-tok/claims")
        .header("cookie", format!("session={session}"))
        .body(Body::empty())
        .unwrap();
    let resp = router(Arc::clone(&store), Arc::clone(&invoker), admin_hash, None)
        .oneshot(req)
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let raw = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let body_str = std::str::from_utf8(&raw).unwrap();
    assert!(
        !body_str.contains("SECRET"),
        "gift_url must never appear on the admin wire, got: {body_str}"
    );
    let j: serde_json::Value = serde_json::from_str(body_str).unwrap();
    let claim = &j.as_array().unwrap()[0];
    assert_eq!(claim["game_id"], "g-1");
    assert_eq!(claim["state"], "fulfilled");
    assert_eq!(claim["issued"], true);
    assert!(claim.get("gift_url").is_none(), "no gift_url key at all");
}

/// POST /admin/api/sync fires the backfill async and returns 202 immediately —
/// it must NOT block on completion (a full backfill outruns the API Gateway
/// timeout → 504). The mock captures the Sync request to prove it was invoked.
#[tokio::test]
async fn sync_now_fires_async_and_returns_202() {
    let Some(store) = store_or_skip("sync-async").await else {
        return;
    };

    let password = "syncpw";
    let admin_hash = test_admin_hash(password);
    // The trait is fire-only now (the blocking RequestResponse invoke left with the cookie-paste
    // teardown), so "never block through the request path" is enforced by the type system; the
    // capture below proves the Sync request was actually queued.
    let mock = MockAdminInvoker::new();
    let invoker: Arc<dyn AdminInvoker> = mock.clone();

    let session = admin_login(&store, &invoker, &admin_hash, password).await;

    let req = Request::post("/admin/api/sync")
        .header("cookie", format!("session={session}"))
        .body(Body::empty())
        .unwrap();
    let resp = router(Arc::clone(&store), Arc::clone(&invoker), admin_hash, None)
        .oneshot(req)
        .await
        .unwrap();

    assert_eq!(
        resp.status(),
        StatusCode::ACCEPTED,
        "sync-now must return 202 (fire-and-forget), never block for completion"
    );
    let j = body_json(resp).await;
    assert_eq!(j["status"], "started");

    // Sync went through fire() — the async invoke was actually queued.
    assert_eq!(mock.last_fired().await, Some(FulfillRequest::Sync));
}

/// POST /admin/api/sync while a LIVE sync-run marker exists → 409, and the fulfillment lambda
/// is NOT invoked. This is the server-side guard that replaces the serialization the old
/// blocking invoke gave for free (the button re-enables ~1s after the 202 now).
#[tokio::test]
async fn sync_now_refuses_while_run_live() {
    let Some(store) = store_or_skip("sync-409").await else {
        return;
    };

    let password = "syncpw";
    let admin_hash = test_admin_hash(password);
    let mock = MockAdminInvoker::new();
    let invoker: Arc<dyn AdminInvoker> = mock.clone();

    // A run that began just now — definitively live.
    let now = time::OffsetDateTime::now_utc().unix_timestamp();
    store.begin_sync_run(now).await.unwrap();

    let session = admin_login(&store, &invoker, &admin_hash, password).await;

    let req = Request::post("/admin/api/sync")
        .header("cookie", format!("session={session}"))
        .body(Body::empty())
        .unwrap();
    let resp = router(Arc::clone(&store), Arc::clone(&invoker), admin_hash, None)
        .oneshot(req)
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::CONFLICT);
    let j = body_json(resp).await;
    assert_eq!(
        j["error"],
        "a sync is already running — watch the status card"
    );
    assert_eq!(
        mock.last_fired().await,
        None,
        "a refused sync must not queue an invoke"
    );
}

/// POST /admin/api/sync with a STALE run marker (a run that crashed before reporting) → the
/// guard must NOT wedge: it fires a new sync and returns 202.
#[tokio::test]
async fn sync_now_fires_past_stale_run_marker() {
    let Some(store) = store_or_skip("sync-stale").await else {
        return;
    };

    let password = "syncpw";
    let admin_hash = test_admin_hash(password);
    let mock = MockAdminInvoker::new();
    let invoker: Arc<dyn AdminInvoker> = mock.clone();

    // A marker older than any possible live run (fulfillment's hard timeout < staleness cutoff).
    let now = time::OffsetDateTime::now_utc().unix_timestamp();
    store
        .begin_sync_run(now - dynamo::SYNC_RUN_STALE_SECS - 60)
        .await
        .unwrap();

    let session = admin_login(&store, &invoker, &admin_hash, password).await;

    let req = Request::post("/admin/api/sync")
        .header("cookie", format!("session={session}"))
        .body(Body::empty())
        .unwrap();
    let resp = router(Arc::clone(&store), Arc::clone(&invoker), admin_hash, None)
        .oneshot(req)
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::ACCEPTED);
    assert_eq!(mock.last_fired().await, Some(FulfillRequest::Sync));
}

// ── Self-claim test infrastructure ────────────────────────────────────────────

/// Fixed password used by test_app_with_call_invoker and its authed_* helpers.
const TEST_ADMIN_PW: &str = "sc-test-admin-pw";

/// Mock invoker that supports both fire (no-op) and call (returns a configured response,
/// logging every call request). Unlike MockAdminInvoker, this one is built for the
/// self-claim endpoint which needs synchronous RequestResponse invocations.
struct MockCallInvoker {
    response: FulfillResponse,
    log: Arc<std::sync::Mutex<Vec<FulfillRequest>>>,
}

#[async_trait]
impl AdminInvoker for MockCallInvoker {
    async fn fire(&self, _req: FulfillRequest) -> Result<(), String> {
        Ok(()) // no-op — self-claim tests don't trigger fire
    }

    async fn call(&self, req: FulfillRequest) -> Result<FulfillResponse, String> {
        self.log.lock().unwrap().push(req);
        Ok(self.response.clone())
    }
}

/// Build a fully-wired app + fresh DynamoDB table + invoker log, all sharing the same state.
/// Uses a UUID-based table name so concurrent tests don't collide.
/// Panics if DYNAMODB_LOCAL_URL is set but dynamo-local is unreachable.
async fn test_app_with_call_invoker(
    response: FulfillResponse,
) -> (
    axum::Router,
    Arc<Store>,
    Arc<std::sync::Mutex<Vec<FulfillRequest>>>,
) {
    // Use a UUID-derived table name for per-call isolation.
    let uid = uuid::Uuid::new_v4().simple().to_string();
    let store = store_or_skip(&format!("sc{}", &uid[..10]))
        .await
        .expect("DYNAMODB_LOCAL_URL must be set and dynamo-local reachable for self-claim tests");
    let log = Arc::new(std::sync::Mutex::new(Vec::<FulfillRequest>::new()));
    let mock: Arc<dyn AdminInvoker> = Arc::new(MockCallInvoker {
        response,
        log: Arc::clone(&log),
    });
    let admin_hash = test_admin_hash(TEST_ADMIN_PW);
    let app = router(Arc::clone(&store), mock, admin_hash, None);
    (app, store, log)
}

/// Produce a Game with the given `id` (format `"gamekey:machine_name"`), Available status, steam key.
fn sample_game(id: &str) -> Game {
    let mut parts = id.splitn(2, ':');
    let gamekey = parts.next().unwrap_or(id).to_string();
    let machine_name = parts.next().unwrap_or("mn").to_string();
    Game {
        id: id.to_string(),
        title: format!("Sample Game {id}"),
        bundle: "Test Bundle".into(),
        gamekey,
        machine_name,
        key_type: "steam".into(),
        giftable: true,
        hidden: false,
        status: GameStatus::Available,
        claim_id: None,
        artwork_url: None,
        keyindex: 0,
        requires_choice: false,
        steam_app_id: None,
        appid_source: None,
        owned_by_ben: false,
    }
}

/// Write an Available game to the store.
async fn seed_available_game(store: &Arc<Store>, id: &str, title: &str) {
    let mut g = sample_game(id);
    g.title = title.to_string();
    store.put_game(&g).await.unwrap();
}

/// Login via the app's /admin/api/login endpoint and return the session token.
async fn get_session(app: &axum::Router) -> String {
    let req = Request::post("/admin/api/login")
        .header("content-type", "application/json")
        .body(Body::from(
            serde_json::to_vec(&serde_json::json!({"password": TEST_ADMIN_PW})).unwrap(),
        ))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "authed helper: login must succeed"
    );
    resp.headers()
        .get(axum::http::header::SET_COOKIE)
        .expect("login must set a cookie")
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .strip_prefix("session=")
        .expect("Set-Cookie must start with 'session='")
        .to_string()
}

async fn authed_post(app: &axum::Router, path: &str, body: &str) -> axum::response::Response {
    let session = get_session(app).await;
    let req = Request::post(path)
        .header("content-type", "application/json")
        .header("cookie", format!("session={session}"))
        .body(Body::from(body.to_string()))
        .unwrap();
    app.clone().oneshot(req).await.unwrap()
}

async fn authed_get(app: &axum::Router, path: &str) -> axum::response::Response {
    let session = get_session(app).await;
    let req = Request::get(path)
        .header("cookie", format!("session={session}"))
        .body(Body::empty())
        .unwrap();
    app.clone().oneshot(req).await.unwrap()
}

/// Extract the response body as a raw String (for invariant substring checks).
async fn body_string(resp: axum::response::Response) -> String {
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    String::from_utf8(bytes.to_vec()).expect("response body must be valid UTF-8")
}

// ── Self-claim tests ───────────────────────────────────────────────────────────

/// POST /admin/api/games/:id/self-claim with a mock that returns RevealedKey:
/// - Returns 200 with {revealed_key, key_type}
/// - The intake actually happened (claims_for_link(SELF_LINK_TOKEN) has the claim)
/// - The invoke carried the correct game identifiers
#[tokio::test]
async fn self_claim_endpoint_intakes_invokes_and_returns_key() {
    let (app, store, invoker_log) = test_app_with_call_invoker(FulfillResponse::RevealedKey {
        key: "K-123".into(),
    })
    .await;
    seed_available_game(&store, "gkJ:mnJ", "Endpoint Game").await;

    let resp = authed_post(&app, "/admin/api/games/gkJ:mnJ/self-claim", "{}").await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = body_json(resp).await;
    assert_eq!(body["revealed_key"], "K-123");
    assert_eq!(body["key_type"], "steam");

    // Intake really happened:
    let claims = store
        .claims_for_link(domain::SELF_LINK_TOKEN)
        .await
        .unwrap();
    assert_eq!(claims.len(), 1);

    // The invoke carried the right identifiers:
    let sent = invoker_log.lock().unwrap().clone();
    assert!(
        matches!(&sent[0], FulfillRequest::SelfClaim { game_id, .. } if game_id == "gkJ:mnJ"),
        "first call must be SelfClaim for gkJ:mnJ, got: {:?}",
        sent.first()
    );
}

/// POST /admin/api/games/:id/self-claim on a Pending game → 409 (game not available).
#[tokio::test]
async fn self_claim_endpoint_409s_when_game_pending() {
    let (app, store, _) = test_app_with_call_invoker(FulfillResponse::RevealedKey {
        key: "unused".into(),
    })
    .await;
    let mut g = sample_game("gkK:mnK");
    g.status = GameStatus::Pending;
    store.put_game(&g).await.unwrap();

    let resp = authed_post(&app, "/admin/api/games/gkK:mnK/self-claim", "{}").await;
    assert_eq!(resp.status(), 409);
}

/// POST /admin/api/games/:id/self-claim when mock returns Parked → 202 processing.
#[tokio::test]
async fn self_claim_endpoint_202_on_parked() {
    let (app, store, _) =
        test_app_with_call_invoker(FulfillResponse::Parked { reason: "x".into() }).await;
    seed_available_game(&store, "gkL:mnL", "Parked Game").await;

    let resp = authed_post(&app, "/admin/api/games/gkL:mnL/self-claim", "{}").await;
    assert_eq!(resp.status(), 202);
}

/// GET /admin/api/claims/self returns fulfilled self-claims including their revealed_key.
/// Crucially: does NOT 404 even though LINK#SELF has no META item (handle_link_claims would 404).
#[tokio::test]
async fn claims_self_lists_revealed_keys_without_link_precheck() {
    let (app, store, _) = test_app_with_call_invoker(FulfillResponse::RevealedKey {
        key: "unused".into(),
    })
    .await;
    seed_available_game(&store, "gkM:mnM", "Listed Game").await;
    store
        .claim_game_self("gkM:mnM", "c-l1", time::OffsetDateTime::now_utc())
        .await
        .unwrap();
    store
        .fulfill_self_claim("c-l1", "gkM:mnM", "LIST-KEY")
        .await
        .unwrap();

    let resp = authed_get(&app, "/admin/api/claims/self").await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = body_json(resp).await;
    assert_eq!(body[0]["revealed_key"], "LIST-KEY");
}

/// GET /admin/api/catalog includes requires_choice on each game view.
#[tokio::test]
async fn catalog_exposes_requires_choice() {
    let (app, store, _) = test_app_with_call_invoker(FulfillResponse::RevealedKey {
        key: "unused".into(),
    })
    .await;
    let mut g = sample_game("gkN:mnN");
    g.requires_choice = true;
    store.put_game(&g).await.unwrap();

    let resp = authed_get(&app, "/admin/api/catalog").await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = body_json(resp).await;
    // find our game in the list
    let game = body
        .as_array()
        .unwrap()
        .iter()
        .find(|g| g["id"] == "gkN:mnN")
        .expect("gkN:mnN must be in catalog");
    assert_eq!(game["requires_choice"], true);
}

/// The gift-claims surface (GET /admin/api/links/:token/claims → AdminClaimView) must NEVER
/// expose gift_url or revealed_key — raw-JSON substring check, not a typed parse.
/// Regression guard: a new SelfClaimView with revealed_key must not bleed into this endpoint.
#[tokio::test]
async fn gift_link_claims_still_hide_gift_url() {
    let (app, store, _) = test_app_with_call_invoker(FulfillResponse::RevealedKey {
        key: "unused".into(),
    })
    .await;
    // Seed a gift link + fulfilled claim with a real gift_url (same pattern as link_claims_redact_gift_url_to_issued_bool)
    store.create_link(&test_link("tok-inv")).await.unwrap();
    store
        .put_claim(&Claim {
            id: "c-inv-1".into(),
            link_token: "tok-inv".into(),
            game_id: "g-inv-1".into(),
            state: ClaimState::Fulfilled,
            gift_url: Some("https://humble.example/gift?key=SECRETINV".into()),
            created_at: datetime!(2026-07-04 00:00 UTC),
            choice_pre_tpks: None,
            revealed_key: None,
        })
        .await
        .unwrap();

    let resp = authed_get(&app, "/admin/api/links/tok-inv/claims").await;
    assert_eq!(resp.status(), 200);
    let raw = body_string(resp).await;
    assert!(
        !raw.contains("gift_url"),
        "gift surface must not carry gift_url: {raw}"
    );
    assert!(
        !raw.contains("revealed_key"),
        "gift surface must not carry revealed_key: {raw}"
    );
    assert!(
        raw.contains("issued"),
        "sanity: the response is the AdminClaimView shape: {raw}"
    );
}

// ── Steam test infrastructure ─────────────────────────────────────────────────

/// Build a router whose steam client points at the given wiremock base URL.
fn steam_router(
    store: Arc<Store>,
    invoker: Arc<dyn AdminInvoker>,
    admin_hash: String,
    steam_base: &str,
) -> axum::Router {
    let steam = SteamClient::new(
        steam_base,
        steam_base,
        steam_base,
        SteamApiKey::new("TESTKEY".into()),
    )
    .unwrap();
    router(store, invoker, admin_hash, Some(Arc::new(steam)))
}

/// Standard valid 17-digit steamid used across steam tests.
const TEST_STEAMID: &str = "76561198000000001";

/// Build a test app with steam wired to `steam_base`. Returns (app, store).
async fn steam_test_app(steam_base: &str) -> Option<(axum::Router, Arc<Store>)> {
    let uid = uuid::Uuid::new_v4().simple().to_string();
    let store = store_or_skip(&format!("steam{}", &uid[..8])).await?;
    let invoker: Arc<dyn AdminInvoker> = MockAdminInvoker::new();
    let admin_hash = test_admin_hash(TEST_ADMIN_PW);
    let app = steam_router(Arc::clone(&store), invoker, admin_hash, steam_base);
    Some((app, store))
}

/// Register a stub for GET /IPlayerService/GetOwnedGames/v0001/ returning the given body.
async fn mock_owned_games(server: &wiremock::MockServer, body: &str, expect: u64) {
    wiremock::Mock::given(wiremock::matchers::method("GET"))
        .and(wiremock::matchers::path(
            "/IPlayerService/GetOwnedGames/v0001/",
        ))
        .respond_with(wiremock::ResponseTemplate::new(200).set_body_string(body.to_string()))
        .expect(expect)
        .mount(server)
        .await;
}

// ── Steam 401 without session tests (pure-mock, no dynamo) ───────────────────

/// POST /admin/api/steam/identity without session → 401.
#[tokio::test]
async fn steam_identity_post_401_without_session() {
    let store = fake_store().await;
    let invoker: Arc<dyn AdminInvoker> = MockAdminInvoker::new();
    let req = Request::post("/admin/api/steam/identity")
        .header("content-type", "application/json")
        .body(Body::from(
            serde_json::to_vec(&serde_json::json!({"steamid": TEST_STEAMID})).unwrap(),
        ))
        .unwrap();
    let resp = router(store, invoker, test_admin_hash("pw"), None)
        .oneshot(req)
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

/// DELETE /admin/api/steam/identity without session → 401.
#[tokio::test]
async fn steam_identity_delete_401_without_session() {
    let store = fake_store().await;
    let invoker: Arc<dyn AdminInvoker> = MockAdminInvoker::new();
    let req = Request::delete("/admin/api/steam/identity")
        .body(Body::empty())
        .unwrap();
    let resp = router(store, invoker, test_admin_hash("pw"), None)
        .oneshot(req)
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

/// GET /admin/api/steam/identity without session → 401.
#[tokio::test]
async fn steam_identity_get_401_without_session() {
    let store = fake_store().await;
    let invoker: Arc<dyn AdminInvoker> = MockAdminInvoker::new();
    let req = Request::get("/admin/api/steam/identity")
        .body(Body::empty())
        .unwrap();
    let resp = router(store, invoker, test_admin_hash("pw"), None)
        .oneshot(req)
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

/// GET /admin/api/steam/owned/:steamid without session → 401.
#[tokio::test]
async fn steam_owned_proxy_401_without_session() {
    let store = fake_store().await;
    let invoker: Arc<dyn AdminInvoker> = MockAdminInvoker::new();
    let req = Request::get(format!("/admin/api/steam/owned/{TEST_STEAMID}"))
        .body(Body::empty())
        .unwrap();
    let resp = router(store, invoker, test_admin_hash("pw"), None)
        .oneshot(req)
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

/// POST /admin/api/games/:id/steam-app-id without session → 401.
#[tokio::test]
async fn steam_appid_override_401_without_session() {
    let store = fake_store().await;
    let invoker: Arc<dyn AdminInvoker> = MockAdminInvoker::new();
    let req = Request::post("/admin/api/games/some-id/steam-app-id")
        .header("content-type", "application/json")
        .body(Body::from(
            serde_json::to_vec(&serde_json::json!({"app_id": 12345})).unwrap(),
        ))
        .unwrap();
    let resp = router(store, invoker, test_admin_hash("pw"), None)
        .oneshot(req)
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

// ── Steam 503 when not configured ─────────────────────────────────────────────

/// Steam endpoints return 503 {"error":"steam not configured"} when steam is None.
#[tokio::test]
async fn steam_endpoints_503_when_not_configured() {
    let Some(store) = store_or_skip("steam-503").await else {
        return;
    };
    let invoker: Arc<dyn AdminInvoker> = MockAdminInvoker::new();
    let admin_hash = test_admin_hash(TEST_ADMIN_PW);
    // Build router WITHOUT a steam client.
    let app = router(Arc::clone(&store), invoker, admin_hash, None);
    let session = get_session(&app).await;

    for (method, path, body) in [
        (
            "POST",
            "/admin/api/steam/identity",
            r#"{"steamid":"76561198000000001"}"#,
        ),
        ("DELETE", "/admin/api/steam/identity", ""),
        ("GET", "/admin/api/steam/identity", ""),
        (
            "GET",
            format!("/admin/api/steam/owned/{TEST_STEAMID}").as_str(),
            "",
        ),
    ] {
        let req = if method == "GET" || method == "DELETE" {
            Request::builder()
                .method(method)
                .uri(path)
                .header("cookie", format!("session={session}"))
                .body(Body::empty())
                .unwrap()
        } else {
            Request::builder()
                .method(method)
                .uri(path)
                .header("content-type", "application/json")
                .header("cookie", format!("session={session}"))
                .body(Body::from(body.to_string()))
                .unwrap()
        };
        let resp = app.clone().oneshot(req).await.unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::SERVICE_UNAVAILABLE,
            "{method} {path} must return 503 when steam not configured"
        );
        let j = body_json(resp).await;
        assert_eq!(
            j["error"], "steam not configured",
            "{method} {path} must carry steam not configured error"
        );
    }
}

// ── Steam identity round-trip ──────────────────────────────────────────────────

/// POST /admin/api/steam/identity → GET shows steamid → DELETE → GET shows null.
#[tokio::test]
async fn steam_identity_roundtrip() {
    let server = wiremock::MockServer::start().await;
    let Some((app, _store)) = steam_test_app(&server.uri()).await else {
        return;
    };

    // GET before POST → null
    let resp = authed_get(&app, "/admin/api/steam/identity").await;
    assert_eq!(resp.status(), 200);
    let j = body_json(resp).await;
    assert!(j["steamid"].is_null(), "before POST, steamid must be null");

    // POST → 200 {ok: true}
    let resp = authed_post(
        &app,
        "/admin/api/steam/identity",
        &serde_json::to_string(&serde_json::json!({"steamid": TEST_STEAMID})).unwrap(),
    )
    .await;
    assert_eq!(resp.status(), 200);
    let j = body_json(resp).await;
    assert_eq!(j["ok"], true);

    // GET → returns steamid
    let resp = authed_get(&app, "/admin/api/steam/identity").await;
    assert_eq!(resp.status(), 200);
    let j = body_json(resp).await;
    assert_eq!(j["steamid"], TEST_STEAMID);

    // DELETE → 200 {ok: true}
    let session = get_session(&app).await;
    let req = Request::delete("/admin/api/steam/identity")
        .header("cookie", format!("session={session}"))
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), 200);
    let j = body_json(resp).await;
    assert_eq!(j["ok"], true);

    // GET after DELETE → null
    let resp = authed_get(&app, "/admin/api/steam/identity").await;
    assert_eq!(resp.status(), 200);
    let j = body_json(resp).await;
    assert!(j["steamid"].is_null(), "after DELETE, steamid must be null");
}

/// POST /admin/api/steam/identity with a steamid that isn't exactly 17 ASCII digits → 400.
#[tokio::test]
async fn steam_identity_invalid_steamid_returns_400() {
    let server = wiremock::MockServer::start().await;
    let Some((app, _store)) = steam_test_app(&server.uri()).await else {
        return;
    };

    for bad in [
        "1234567890123456",
        "123456789012345678",
        "7656119800000000a",
        "",
    ] {
        let resp = authed_post(
            &app,
            "/admin/api/steam/identity",
            &serde_json::to_string(&serde_json::json!({"steamid": bad})).unwrap(),
        )
        .await;
        assert_eq!(
            resp.status(),
            StatusCode::BAD_REQUEST,
            "steamid='{}' must return 400",
            bad
        );
    }
}

// ── Steam owned proxy tests ────────────────────────────────────────────────────

/// Owned proxy serves the CACHE when `fetched_at` ≤ 24h old — no HTTP to Steam.
#[tokio::test]
async fn steam_owned_proxy_fresh_cache_served_without_hitting_steam() {
    let server = wiremock::MockServer::start().await;
    // Register the mock with expect(0) — any call fails the test.
    mock_owned_games(
        &server,
        r#"{"response":{"game_count":1,"games":[{"appid":440}]}}"#,
        0,
    )
    .await;

    let Some((app, store)) = steam_test_app(&server.uri()).await else {
        return;
    };

    // Seed fresh cache: fetched_at = now - 1 hour (well within 24h).
    let now = time::OffsetDateTime::now_utc().unix_timestamp();
    let fresh_at = now - 3600; // 1 hour ago
    store
        .put_steam_owned(TEST_STEAMID, &[12345, 67890], fresh_at)
        .await
        .unwrap();

    let resp = authed_get(&app, &format!("/admin/api/steam/owned/{TEST_STEAMID}")).await;
    assert_eq!(resp.status(), 200);
    let j = body_json(resp).await;
    let appids = j["appids"].as_array().unwrap();
    assert_eq!(appids.len(), 2);
    assert!(appids.contains(&serde_json::json!(12345)));
    assert!(appids.contains(&serde_json::json!(67890)));

    // expect(0) is verified at server drop — no requests to Steam.
    server.verify().await;
}

/// Owned proxy FETCHES from Steam when cache is stale (> 24h), caches the result.
#[tokio::test]
async fn steam_owned_proxy_stale_cache_fetches_and_caches() {
    let server = wiremock::MockServer::start().await;
    // expect(1): exactly one call must be made.
    mock_owned_games(
        &server,
        r#"{"response":{"game_count":2,"games":[{"appid":730},{"appid":570}]}}"#,
        1,
    )
    .await;

    let Some((app, store)) = steam_test_app(&server.uri()).await else {
        return;
    };

    // Seed stale cache: fetched_at = now - 25 hours (beyond 24h window).
    let now = time::OffsetDateTime::now_utc().unix_timestamp();
    let stale_at = now - (25 * 3600); // 25 hours ago
    store
        .put_steam_owned(TEST_STEAMID, &[99999], stale_at)
        .await
        .unwrap();

    let resp = authed_get(&app, &format!("/admin/api/steam/owned/{TEST_STEAMID}")).await;
    assert_eq!(resp.status(), 200);
    let j = body_json(resp).await;
    let appids = j["appids"].as_array().unwrap();
    assert_eq!(appids.len(), 2, "must return fresh steam data");
    assert!(appids.contains(&serde_json::json!(730)));
    assert!(appids.contains(&serde_json::json!(570)));

    // Cache must have been updated.
    let (cached, cached_at) = store
        .get_steam_owned(TEST_STEAMID)
        .await
        .unwrap()
        .expect("cache must exist after fetch");
    assert!(cached.contains(&730));
    assert!(cached_at > stale_at, "fetched_at must be updated");

    server.verify().await;
}

/// Owned proxy returns {"private":true} when Steam says the library is private,
/// and does NOT overwrite a previously-good cache.
#[tokio::test]
async fn steam_owned_proxy_private_returns_private_and_preserves_cache() {
    let server = wiremock::MockServer::start().await;
    // Private response: no game_count field.
    mock_owned_games(&server, r#"{"response":{}}"#, 1).await;

    let Some((app, store)) = steam_test_app(&server.uri()).await else {
        return;
    };

    // Seed stale cache with real data — it must survive the Private response.
    let now = time::OffsetDateTime::now_utc().unix_timestamp();
    let stale_at = now - (25 * 3600);
    store
        .put_steam_owned(TEST_STEAMID, &[440], stale_at)
        .await
        .unwrap();

    let resp = authed_get(&app, &format!("/admin/api/steam/owned/{TEST_STEAMID}")).await;
    assert_eq!(resp.status(), 200);
    let j = body_json(resp).await;
    assert_eq!(j["private"], true, "Private must return {{private:true}}");
    assert!(
        j.get("appids").is_none(),
        "private response must not carry appids"
    );

    // Old cache must still be intact (fetched_at unchanged = still stale_at).
    let (_, cached_at) = store
        .get_steam_owned(TEST_STEAMID)
        .await
        .unwrap()
        .expect("cache must still exist after Private response");
    assert_eq!(
        cached_at, stale_at,
        "Private must not overwrite cache fetched_at"
    );

    server.verify().await;
}

/// GET /admin/api/steam/owned/:steamid with an invalid steamid (not 17 digits) → 400.
#[tokio::test]
async fn steam_owned_proxy_invalid_steamid_returns_400() {
    let server = wiremock::MockServer::start().await;
    let Some((app, _store)) = steam_test_app(&server.uri()).await else {
        return;
    };

    for bad in ["1234", "765611980000000012", "7656119800000000x"] {
        let resp = authed_get(&app, &format!("/admin/api/steam/owned/{bad}")).await;
        assert_eq!(
            resp.status(),
            StatusCode::BAD_REQUEST,
            "invalid steamid '{bad}' must return 400"
        );
    }
}

// ── Steam appid override tests ─────────────────────────────────────────────────

/// POST /admin/api/games/:id/steam-app-id {app_id: 12345} → {ok:true}, game has Manual appid.
#[tokio::test]
async fn steam_appid_override_sets_manual() {
    let (app, store, _) = test_app_with_call_invoker(FulfillResponse::RevealedKey {
        key: "unused".into(),
    })
    .await;
    let g = sample_game("gk-appid-set:mn");
    let gid = g.id.clone();
    store.put_game(&g).await.unwrap();

    let resp = authed_post(
        &app,
        &format!("/admin/api/games/{gid}/steam-app-id"),
        &serde_json::to_string(&serde_json::json!({"app_id": 12345})).unwrap(),
    )
    .await;
    assert_eq!(resp.status(), 200);
    let j = body_json(resp).await;
    assert_eq!(j["ok"], true);

    let got = store.get_game(&gid).await.unwrap().unwrap();
    assert_eq!(got.steam_app_id, Some(12345));
    assert_eq!(got.appid_source, Some(domain::AppidSource::Manual));
}

/// POST /admin/api/games/:id/steam-app-id {app_id: null} → {ok:true}, clears both fields.
#[tokio::test]
async fn steam_appid_override_null_clears() {
    let (app, store, _) = test_app_with_call_invoker(FulfillResponse::RevealedKey {
        key: "unused".into(),
    })
    .await;
    let mut g = sample_game("gk-appid-clr:mn");
    g.steam_app_id = Some(99999);
    g.appid_source = Some(domain::AppidSource::Manual);
    let gid = g.id.clone();
    store.put_game(&g).await.unwrap();

    let resp = authed_post(
        &app,
        &format!("/admin/api/games/{gid}/steam-app-id"),
        &serde_json::to_string(&serde_json::json!({"app_id": null})).unwrap(),
    )
    .await;
    assert_eq!(resp.status(), 200);
    let j = body_json(resp).await;
    assert_eq!(j["ok"], true);

    let got = store.get_game(&gid).await.unwrap().unwrap();
    assert!(got.steam_app_id.is_none(), "steam_app_id must be cleared");
    assert!(got.appid_source.is_none(), "appid_source must be cleared");
}

/// POST /admin/api/games/:id/steam-app-id on unknown game → 404.
#[tokio::test]
async fn steam_appid_override_unknown_game_returns_404() {
    let (app, _store, _) = test_app_with_call_invoker(FulfillResponse::RevealedKey {
        key: "unused".into(),
    })
    .await;

    let resp = authed_post(
        &app,
        "/admin/api/games/no-such-game/steam-app-id",
        &serde_json::to_string(&serde_json::json!({"app_id": 1})).unwrap(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

// ── Catalog view steam fields ──────────────────────────────────────────────────

/// GET /admin/api/catalog returns steam_app_id and owned_by_ben on each game view.
#[tokio::test]
async fn catalog_view_carries_steam_app_id_and_owned_by_ben() {
    let (app, store, _) = test_app_with_call_invoker(FulfillResponse::RevealedKey {
        key: "unused".into(),
    })
    .await;

    let mut g = sample_game("gk-steam-cat:mn");
    g.steam_app_id = Some(730);
    g.owned_by_ben = true;
    let gid = g.id.clone();
    store.put_game(&g).await.unwrap();

    let resp = authed_get(&app, "/admin/api/catalog").await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = body_json(resp).await;
    let game = body
        .as_array()
        .unwrap()
        .iter()
        .find(|g| g["id"] == gid)
        .expect("game must appear in catalog");

    assert_eq!(game["steam_app_id"], 730, "steam_app_id must be 730");
    assert_eq!(game["owned_by_ben"], true, "owned_by_ben must be true");
}

/// Catalog game with no steam_app_id → steam_app_id is null (not missing), owned_by_ben is false.
#[tokio::test]
async fn catalog_view_steam_fields_absent_when_unset() {
    let (app, store, _) = test_app_with_call_invoker(FulfillResponse::RevealedKey {
        key: "unused".into(),
    })
    .await;

    let g = sample_game("gk-no-steam:mn");
    let gid = g.id.clone();
    store.put_game(&g).await.unwrap();

    let resp = authed_get(&app, "/admin/api/catalog").await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = body_json(resp).await;
    let game = body
        .as_array()
        .unwrap()
        .iter()
        .find(|g| g["id"] == gid)
        .expect("game must appear in catalog");

    assert!(
        game["steam_app_id"].is_null(),
        "unset steam_app_id must serialize as null, not missing"
    );
    assert_eq!(
        game["owned_by_ben"], false,
        "unset owned_by_ben must be false"
    );
}

// ── Task 4: Admin game detail endpoint tests ─────────────────────────────────

/// GET /admin/api/games/:id/detail without session → 401.
#[tokio::test]
async fn admin_game_detail_401_without_session() {
    let store = fake_store().await;
    let invoker: Arc<dyn AdminInvoker> = MockAdminInvoker::new();
    let req = Request::get("/admin/api/games/some-id/detail")
        .body(Body::empty())
        .unwrap();
    let resp = router(store, invoker, test_admin_hash("pw"), None)
        .oneshot(req)
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

/// GET /admin/api/games/:id/detail with session + game with steam cache
/// → 200 with CatalogGameView superset fields AND steam blob.
#[tokio::test]
async fn admin_game_detail_superset_fields_and_steam_blob() {
    let (app, store, _) = test_app_with_call_invoker(FulfillResponse::RevealedKey {
        key: "unused".into(),
    })
    .await;

    let mut g = sample_game("gk-det:mn-det");
    g.steam_app_id = Some(77770);
    g.hidden = true; // to confirm superset (admin can see hidden)
    g.requires_choice = false;
    g.owned_by_ben = true;
    let gid = g.id.clone();
    store.put_game(&g).await.unwrap();

    // Seed steam cache
    store
        .put_steam_app(&SteamAppCache {
            app_id: 77770,
            detail: Some(SteamAppDetail {
                app_id: 77770,
                name: "Detail Test Game".into(),
                developers: vec!["Dev".into()],
                publishers: vec!["Pub".into()],
                genres: vec!["RPG".into()],
                release_date: None,
                short_description: "Admin detail test.".into(),
                header_image: None,
                video_hls_url: None,
                video_thumbnail: None,
            }),
            overall: Some(ReviewSummary {
                desc: "Very Positive".into(),
                total_positive: 900,
                total_negative: 100,
                total_reviews: 1000,
            }),
            recent: Some(RecentReviews {
                percent_positive: 90,
                count: 200,
            }),
            fetched_at: 1_700_000_000,
            reviews_fetched_at: 1_700_000_000,
        })
        .await
        .unwrap();

    let resp = authed_get(&app, &format!("/admin/api/games/{gid}/detail")).await;
    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "admin detail must return 200"
    );
    let j = body_json(resp).await;

    // CatalogGameView superset fields
    for field in [
        "id",
        "title",
        "bundle",
        "key_type",
        "giftable",
        "hidden",
        "status",
        "claim_id",
        "artwork_url",
        "requires_choice",
        "steam_app_id",
        "owned_by_ben",
    ] {
        assert!(
            j["game"].get(field).is_some(),
            "game.{field} must be present"
        );
    }
    assert_eq!(j["game"]["id"], gid);
    assert_eq!(
        j["game"]["hidden"], true,
        "hidden must be true (superset includes hidden)"
    );
    assert_eq!(j["game"]["owned_by_ben"], true);
    assert_eq!(j["game"]["steam_app_id"], 77770);

    // steam blob
    assert!(
        !j["steam"].is_null(),
        "steam must not be null for mapped game with cache"
    );
    assert_eq!(j["steam"]["detail"]["name"], "Detail Test Game");
    assert_eq!(j["steam"]["overall"]["desc"], "Very Positive");
    assert_eq!(j["steam"]["recent"]["percent_positive"], 90);

    // must NOT leak timestamps
    assert!(
        j["steam"].get("fetched_at").is_none(),
        "fetched_at must not leak"
    );
    // must NOT leak order-key material
    assert!(j["game"].get("gamekey").is_none(), "gamekey must not leak");
    assert!(
        j["game"].get("machine_name").is_none(),
        "machine_name must not leak"
    );
}
