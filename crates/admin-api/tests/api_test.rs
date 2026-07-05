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
use domain::{Game, GameStatus, Link, game_id};
use dynamo::Store;
use fulfillment::FulfillRequest;
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
    let invoker: Arc<dyn AdminInvoker> = MockAdminInvoker::new();
    let admin_hash = test_admin_hash("correct-pw");

    let req = Request::post("/admin/api/login")
        .header("content-type", "application/json")
        .body(Body::from(r#"{"password":"wrong-pw"}"#))
        .unwrap();

    let resp = router(store, invoker, admin_hash)
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

    let resp = router(store, invoker, admin_hash)
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

    let resp = router(Arc::clone(&store), Arc::clone(&invoker), admin_hash)
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

    let create_resp = router(Arc::clone(&store), Arc::clone(&invoker), admin_hash.clone())
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

    let list_resp = router(Arc::clone(&store), Arc::clone(&invoker), admin_hash)
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
    let invoker: Arc<dyn AdminInvoker> = MockAdminInvoker::new();

    let session = admin_login(&store, &invoker, &admin_hash, password).await;

    // GET /admin/api/catalog: game must be present, hidden=false
    let cat1_req = Request::get("/admin/api/catalog")
        .header("cookie", format!("session={session}"))
        .body(Body::empty())
        .unwrap();
    let cat1_resp = router(Arc::clone(&store), Arc::clone(&invoker), admin_hash.clone())
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
    let hide_resp = router(Arc::clone(&store), Arc::clone(&invoker), admin_hash.clone())
        .oneshot(hide_req)
        .await
        .unwrap();
    assert_eq!(hide_resp.status(), StatusCode::OK);

    // GET /admin/api/catalog again: game must now show hidden=true
    let cat2_req = Request::get("/admin/api/catalog")
        .header("cookie", format!("session={session}"))
        .body(Body::empty())
        .unwrap();
    let cat2_resp = router(Arc::clone(&store), Arc::clone(&invoker), admin_hash)
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
    let revoke_resp = router(Arc::clone(&store), Arc::clone(&invoker), admin_hash)
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
    let resp = router(Arc::clone(&store), Arc::clone(&invoker), admin_hash)
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
    let resp = router(Arc::clone(&store), Arc::clone(&invoker), admin_hash)
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
    let resp = router(Arc::clone(&store), Arc::clone(&invoker), admin_hash)
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
    let resp = router(Arc::clone(&store), Arc::clone(&invoker), admin_hash)
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
    let resp = router(Arc::clone(&store), Arc::clone(&invoker), admin_hash)
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
    let resp = router(Arc::clone(&store), Arc::clone(&invoker), admin_hash)
        .oneshot(req)
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::ACCEPTED);
    assert_eq!(mock.last_fired().await, Some(FulfillRequest::Sync));
}
