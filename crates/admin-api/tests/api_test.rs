//! Integration tests for the admin-api router.
//!
//! Two categories:
//! - **Pure-mock** (no DynamoDB): tests where the route handler returns before touching the store
//!   (wrong-password login → 401; no-cookie → 401). These use `fake_store()` and run everywhere.
//! - **Store-backed**: tests that need a real DynamoDB-local instance (session creation, links,
//!   games). These use `store_or_skip` and are skipped locally; no local DynamoDB exists on this
//!   box and we never claim otherwise.
use std::sync::Arc;

use admin_api::{AdminInvoker, SsmPutter, router};
use async_trait::async_trait;
use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use domain::{Game, GameStatus, Link, game_id};
use dynamo::Store;
use fulfillment::{FulfillRequest, FulfillResponse};
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
    /// Pre-serialised response returned for every call.
    response_json: String,
    /// Last request received, stored as Value (FulfillRequest doesn't derive Clone).
    captured: Mutex<Option<serde_json::Value>>,
}

impl MockAdminInvoker {
    fn new(resp: FulfillResponse) -> Arc<Self> {
        Arc::new(Self {
            response_json: serde_json::to_string(&resp).unwrap(),
            captured: Mutex::new(None),
        })
    }

    async fn last_call(&self) -> Option<FulfillRequest> {
        self.captured
            .lock()
            .await
            .clone()
            .map(|v| serde_json::from_value(v).expect("captured request must deserialize"))
    }
}

#[async_trait]
impl AdminInvoker for MockAdminInvoker {
    async fn call(&self, req: FulfillRequest) -> Result<FulfillResponse, String> {
        *self.captured.lock().await = Some(serde_json::to_value(&req).unwrap());
        Ok(serde_json::from_str(&self.response_json).unwrap())
    }
}

// ── MockSsmPutter ──────────────────────────────────────────────────────────────

struct MockSsmPutter {
    /// Existing cookie returned by get_cookie (simulates the current SSM value at test start).
    initial_value: Option<String>,
    /// All values passed to put_cookie in call order.
    puts: Mutex<Vec<String>>,
}

impl MockSsmPutter {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            initial_value: None,
            puts: Mutex::new(vec![]),
        })
    }

    /// Build a mock that already has `value` stored (get_cookie returns Some(value)).
    fn with_existing(value: &str) -> Arc<Self> {
        Arc::new(Self {
            initial_value: Some(value.to_string()),
            puts: Mutex::new(vec![]),
        })
    }

    /// All put_cookie calls in order.
    async fn all_puts(&self) -> Vec<String> {
        self.puts.lock().await.clone()
    }

    /// Convenience: last put_cookie value (backward compat for existing tests).
    async fn last_cookie(&self) -> Option<String> {
        self.puts.lock().await.last().cloned()
    }
}

#[async_trait]
impl SsmPutter for MockSsmPutter {
    async fn put_cookie(&self, value: &str) -> Result<(), String> {
        self.puts.lock().await.push(value.to_string());
        Ok(())
    }

    async fn get_cookie(&self) -> Result<Option<String>, String> {
        Ok(self.initial_value.clone())
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
    ssm: &Arc<dyn SsmPutter>,
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
        Arc::clone(ssm),
        admin_hash.to_string(),
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
    let invoker: Arc<dyn AdminInvoker> = MockAdminInvoker::new(FulfillResponse::SyncDone {
        games_written: 0,
        orders_failed: 0,
    });
    let ssm: Arc<dyn SsmPutter> = MockSsmPutter::new();
    let admin_hash = test_admin_hash("correct-pw");

    let req = Request::post("/admin/api/login")
        .header("content-type", "application/json")
        .body(Body::from(r#"{"password":"wrong-pw"}"#))
        .unwrap();

    let resp = router(store, invoker, ssm, admin_hash)
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
    let invoker: Arc<dyn AdminInvoker> = MockAdminInvoker::new(FulfillResponse::SyncDone {
        games_written: 0,
        orders_failed: 0,
    });
    let ssm: Arc<dyn SsmPutter> = MockSsmPutter::new();
    let admin_hash = test_admin_hash("pw");

    let req = Request::get("/admin/api/catalog")
        .body(Body::empty())
        .unwrap();

    let resp = router(store, invoker, ssm, admin_hash)
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
    let invoker: Arc<dyn AdminInvoker> = MockAdminInvoker::new(FulfillResponse::SyncDone {
        games_written: 0,
        orders_failed: 0,
    });
    let ssm: Arc<dyn SsmPutter> = MockSsmPutter::new();

    let session = admin_login(&store, &invoker, &ssm, &admin_hash, password).await;
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

    let resp = router(
        Arc::clone(&store),
        Arc::clone(&invoker),
        Arc::clone(&ssm),
        admin_hash,
    )
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
    let invoker: Arc<dyn AdminInvoker> = MockAdminInvoker::new(FulfillResponse::SyncDone {
        games_written: 0,
        orders_failed: 0,
    });
    let ssm: Arc<dyn SsmPutter> = MockSsmPutter::new();

    let session = admin_login(&store, &invoker, &ssm, &admin_hash, password).await;

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
        Arc::clone(&ssm),
        admin_hash.clone(),
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

    let list_resp = router(
        Arc::clone(&store),
        Arc::clone(&invoker),
        Arc::clone(&ssm),
        admin_hash,
    )
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
    let invoker: Arc<dyn AdminInvoker> = MockAdminInvoker::new(FulfillResponse::SyncDone {
        games_written: 0,
        orders_failed: 0,
    });
    let ssm: Arc<dyn SsmPutter> = MockSsmPutter::new();

    let session = admin_login(&store, &invoker, &ssm, &admin_hash, password).await;

    // GET /admin/api/catalog: game must be present, hidden=false
    let cat1_req = Request::get("/admin/api/catalog")
        .header("cookie", format!("session={session}"))
        .body(Body::empty())
        .unwrap();
    let cat1_resp = router(
        Arc::clone(&store),
        Arc::clone(&invoker),
        Arc::clone(&ssm),
        admin_hash.clone(),
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
        Arc::clone(&ssm),
        admin_hash.clone(),
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
    let cat2_resp = router(
        Arc::clone(&store),
        Arc::clone(&invoker),
        Arc::clone(&ssm),
        admin_hash,
    )
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
    let invoker: Arc<dyn AdminInvoker> = MockAdminInvoker::new(FulfillResponse::SyncDone {
        games_written: 0,
        orders_failed: 0,
    });
    let ssm: Arc<dyn SsmPutter> = MockSsmPutter::new();

    let session = admin_login(&store, &invoker, &ssm, &admin_hash, password).await;

    let revoke_req = Request::post("/admin/api/links/test-revoke-tok/revoke")
        .header("cookie", format!("session={session}"))
        .body(Body::empty())
        .unwrap();
    let revoke_resp = router(
        Arc::clone(&store),
        Arc::clone(&invoker),
        Arc::clone(&ssm),
        admin_hash,
    )
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

/// POST /admin/api/cookie: MockSsmPutter captures the value, MockAdminInvoker returns
/// CookieStatus {ok:true}, response body is {"ok":true}. The cookie value must NOT appear
/// in the response body (verified by checking the exact response shape).
#[tokio::test]
async fn cookie_paste_captures_value_and_returns_ok_status() {
    let Some(store) = store_or_skip("cookie-paste").await else {
        return;
    };
    let password = "cookiepw";
    let admin_hash = test_admin_hash(password);
    let invoker_mock = MockAdminInvoker::new(FulfillResponse::CookieStatus { ok: true });
    let invoker: Arc<dyn AdminInvoker> = Arc::clone(&invoker_mock) as Arc<dyn AdminInvoker>;
    let ssm_mock = MockSsmPutter::new();
    let ssm: Arc<dyn SsmPutter> = Arc::clone(&ssm_mock) as Arc<dyn SsmPutter>;

    let session = admin_login(&store, &invoker, &ssm, &admin_hash, password).await;

    let cookie_req = Request::post("/admin/api/cookie")
        .header("content-type", "application/json")
        .header("cookie", format!("session={session}"))
        .body(Body::from(r#"{"cookie":"my-super-secret-humble-cookie"}"#))
        .unwrap();

    let resp = router(
        Arc::clone(&store),
        Arc::clone(&invoker),
        Arc::clone(&ssm),
        admin_hash,
    )
    .oneshot(cookie_req)
    .await
    .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp).await;

    // Only {"ok": true} must be in the response — the cookie value must NOT appear.
    assert_eq!(body["ok"], true);
    let body_str = body.to_string();
    assert!(
        !body_str.contains("my-super-secret-humble-cookie"),
        "cookie value must NOT appear in response body: {body_str}"
    );

    // SSM mock must have captured the cookie value.
    let captured = ssm_mock.last_cookie().await;
    assert_eq!(
        captured.as_deref(),
        Some("my-super-secret-humble-cookie"),
        "SSM mock must have received the cookie value"
    );

    // Invoker must have received ValidateCookie.
    let last_call = invoker_mock.last_call().await;
    assert!(
        matches!(last_call, Some(FulfillRequest::ValidateCookie)),
        "invoker must have received ValidateCookie, got: {last_call:?}"
    );
}

/// POST /admin/api/cookie when ValidateCookie returns CookieStatus{ok:false}: SSM must see TWO
/// puts (new cookie, then rollback to old), response is {"ok":false,"restored_previous":true}.
#[tokio::test]
async fn cookie_paste_failed_validate_rolls_back_and_reports() {
    let Some(store) = store_or_skip("cookie-rollback").await else {
        return;
    };
    let password = "rollbackpw";
    let admin_hash = test_admin_hash(password);
    // Invoker returns CookieStatus{ok:false} — the new cookie is dead.
    let invoker_mock = MockAdminInvoker::new(FulfillResponse::CookieStatus { ok: false });
    let invoker: Arc<dyn AdminInvoker> = Arc::clone(&invoker_mock) as Arc<dyn AdminInvoker>;
    // SSM already has an "old-good-cookie" (the existing value to roll back to).
    let ssm_mock = MockSsmPutter::with_existing("old-good-cookie");
    let ssm: Arc<dyn SsmPutter> = Arc::clone(&ssm_mock) as Arc<dyn SsmPutter>;

    let session = admin_login(&store, &invoker, &ssm, &admin_hash, password).await;

    let cookie_req = Request::post("/admin/api/cookie")
        .header("content-type", "application/json")
        .header("cookie", format!("session={session}"))
        .body(Body::from(r#"{"cookie":"bad-new-cookie"}"#))
        .unwrap();

    let resp = router(
        Arc::clone(&store),
        Arc::clone(&invoker),
        Arc::clone(&ssm),
        admin_hash,
    )
    .oneshot(cookie_req)
    .await
    .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp).await;

    // Validation failed → ok:false with rollback indicator.
    assert_eq!(body["ok"], false);
    assert_eq!(
        body["restored_previous"], true,
        "snapshot existed so rollback must have run"
    );
    let body_str = body.to_string();
    assert!(
        !body_str.contains("bad-new-cookie"),
        "new cookie value must NOT appear in response: {body_str}"
    );
    assert!(
        !body_str.contains("old-good-cookie"),
        "snapshot cookie value must NOT appear in response: {body_str}"
    );

    // SSM mock must have seen exactly two puts: new cookie first, then the rollback.
    let puts = ssm_mock.all_puts().await;
    assert_eq!(
        puts.len(),
        2,
        "SSM must see exactly two puts (new then rollback), got: {puts:?}"
    );
    assert_eq!(
        puts[0], "bad-new-cookie",
        "first put must be the new cookie"
    );
    assert_eq!(
        puts[1], "old-good-cookie",
        "second put must be the rollback"
    );
}

/// POST /admin/api/cookie when ValidateCookie succeeds: SSM sees exactly ONE put (no rollback).
/// Response is {"ok":true} with no rollback fields.
#[tokio::test]
async fn cookie_paste_success_single_put_no_rollback() {
    let Some(store) = store_or_skip("cookie-success-put").await else {
        return;
    };
    let password = "successpw";
    let admin_hash = test_admin_hash(password);
    let invoker_mock = MockAdminInvoker::new(FulfillResponse::CookieStatus { ok: true });
    let invoker: Arc<dyn AdminInvoker> = Arc::clone(&invoker_mock) as Arc<dyn AdminInvoker>;
    let ssm_mock = MockSsmPutter::with_existing("prev-cookie");
    let ssm: Arc<dyn SsmPutter> = Arc::clone(&ssm_mock) as Arc<dyn SsmPutter>;

    let session = admin_login(&store, &invoker, &ssm, &admin_hash, password).await;

    let cookie_req = Request::post("/admin/api/cookie")
        .header("content-type", "application/json")
        .header("cookie", format!("session={session}"))
        .body(Body::from(r#"{"cookie":"shiny-new-cookie"}"#))
        .unwrap();

    let resp = router(
        Arc::clone(&store),
        Arc::clone(&invoker),
        Arc::clone(&ssm),
        admin_hash,
    )
    .oneshot(cookie_req)
    .await
    .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp).await;
    assert_eq!(body["ok"], true, "success response must be ok:true");
    // No rollback fields on success.
    assert!(
        body.get("restored_previous").is_none(),
        "no restored_previous on success"
    );

    // Exactly one put.
    let puts = ssm_mock.all_puts().await;
    assert_eq!(puts.len(), 1, "success must produce exactly one SSM put");
    assert_eq!(puts[0], "shiny-new-cookie");
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
    let invoker: Arc<dyn AdminInvoker> = MockAdminInvoker::new(FulfillResponse::SyncDone {
        games_written: 0,
        orders_failed: 0,
    });
    let ssm: Arc<dyn SsmPutter> = MockSsmPutter::new();

    let session = admin_login(&store, &invoker, &ssm, &admin_hash, password).await;

    let req = Request::get("/admin/api/status")
        .header("cookie", format!("session={session}"))
        .body(Body::empty())
        .unwrap();
    let resp = router(
        Arc::clone(&store),
        Arc::clone(&invoker),
        Arc::clone(&ssm),
        admin_hash,
    )
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
    let invoker: Arc<dyn AdminInvoker> = MockAdminInvoker::new(FulfillResponse::SyncDone {
        games_written: 0,
        orders_failed: 0,
    });
    let ssm: Arc<dyn SsmPutter> = MockSsmPutter::new();

    let session = admin_login(&store, &invoker, &ssm, &admin_hash, password).await;

    let req = Request::get("/admin/api/catalog")
        .header("cookie", format!("session={session}"))
        .body(Body::empty())
        .unwrap();
    let resp = router(
        Arc::clone(&store),
        Arc::clone(&invoker),
        Arc::clone(&ssm),
        admin_hash,
    )
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
