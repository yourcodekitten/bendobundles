//! Adapter-boundary tests for admin-api. Same design as public-api's adapter_test.rs —
//! see its header comment for the Once/env rationale (spec D2), the presence-triggered
//! flag semantics, and the fixture-provenance contract (spec D5).
//!
//! Helpers below are verbatim mirrors from api_test.rs / the public adapter binary: the
//! repo has no tests/common convention, and a test binary CANNOT import another crate's
//! test binary — mirroring the fn body is the only mechanism. Only stated deltas differ
//! (table prefix `t-admadp-`, so this binary can't collide with api_test.rs's `t-adm-`
//! tables when both run in parallel against the shared dynamodb-local).

use std::sync::Arc;
use std::sync::Once;

use admin_api::{AdminInvoker, router};
use async_trait::async_trait;
use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use domain::Link;
use dynamo::Store;
use fulfillment::{FulfillRequest, FulfillResponse};
use time::macros::datetime;
use tokio::sync::Mutex;
use tower::ServiceExt;

static STAGE_ENV: Once = Once::new();

/// Reproduce production's stage config (see public adapter_test.rs for the full note).
fn init_stage_env() {
    STAGE_ENV.call_once(|| {
        // SAFETY: called before any test in this binary performs env reads via
        // lambda_http; Once blocks racing callers until the write completes.
        unsafe { std::env::set_var("AWS_LAMBDA_HTTP_IGNORE_STAGE_IN_PATH", "true") }
    });
}

/// spec D5/AC4: every v1 fixture — admin's included — passes the key-set +
/// correlated-field predicate. Verbatim mirror of the public binary's loader; the
/// per-arm degenerate-red tests live there (AC4's predicate-red needs one home).
fn load_v1_fixture(raw: &'static str) -> String {
    let v: serde_json::Value = serde_json::from_str(raw).expect("fixture must be JSON");
    for key in [
        "resource",
        "path",
        "httpMethod",
        "headers",
        "multiValueHeaders",
        "queryStringParameters",
        "multiValueQueryStringParameters",
        "pathParameters",
        "stageVariables",
        "requestContext",
        "body",
        "isBase64Encoded",
    ] {
        assert!(
            v.get(key).is_some(),
            "fixture missing production key {key:?} — a corpus-minimal fixture exercises \
             arms production never takes (spec D5-1)"
        );
    }
    let ctx = &v["requestContext"];
    for key in [
        "accountId",
        "resourceId",
        "stage",
        "requestId",
        "requestTimeEpoch",
        "identity",
        "resourcePath",
        "httpMethod",
        "apiId",
        "path",
    ] {
        assert!(
            ctx.get(key).is_some(),
            "requestContext missing {key:?} (spec D5-1)"
        );
    }
    let stage = ctx["stage"].as_str().expect("stage is a string");
    let path = v["path"].as_str().expect("path is a string");
    assert_eq!(
        ctx["path"].as_str().unwrap(),
        format!("/{stage}{path}"),
        "requestContext.path must equal /<stage><path> (spec D5-2 correlated-field check)"
    );
    assert_eq!(
        v["httpMethod"], ctx["httpMethod"],
        "method must agree in both places (D5-2)"
    );
    assert_eq!(
        v["resource"], ctx["resourcePath"],
        "resource must agree in both places (D5-2)"
    );
    // Dual-map correlations — the highest-risk pairs by the spec's own argument:
    // production sends both maps, translation MERGES headers and PREFERS the mv query
    // map, so a hand edit to only one map is invisible to every downstream test.
    // Single-map value = LAST multi-map element (API GW's convention). Review pass 2, F1.
    assert_dual_maps(&v["headers"], &v["multiValueHeaders"], "headers");
    assert_dual_maps(
        &v["queryStringParameters"],
        &v["multiValueQueryStringParameters"],
        "query",
    );
    raw.to_string()
}

/// D5's dual-map law: both present or both null; every single-map key in the multi map
/// with last element equal to the single value; every multi-map key in the single map.
fn assert_dual_maps(single: &serde_json::Value, multi: &serde_json::Value, what: &str) {
    match (single.as_object(), multi.as_object()) {
        (None, None) => {} // both null — legal (e.g. no query string)
        (Some(s), Some(m)) => {
            for (k, v) in s {
                let arr = m.get(k).and_then(|x| x.as_array()).unwrap_or_else(|| {
                    panic!("{what}: single-map key {k:?} missing from the multi map (D5 dual-map)")
                });
                assert_eq!(
                    arr.last().unwrap_or(&serde_json::Value::Null),
                    v,
                    "{what}: multi-map last element for {k:?} must equal the single-map value (D5 dual-map)"
                );
            }
            for k in m.keys() {
                assert!(
                    s.contains_key(k),
                    "{what}: multi-map key {k:?} missing from the single map (D5 dual-map)"
                );
            }
        }
        _ => {
            panic!("{what}: single and multi maps must be both present or both null (D5 dual-map)")
        }
    }
}

/// Mirror of api_test.rs::store_or_skip with prefix `t-admadp-` (stated delta).
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
    let store = Store::new(client, format!("t-admadp-{test}"));
    store.create_table_for_tests().await.unwrap();
    Some(Arc::new(store))
}

/// Mirror of api_test.rs::test_admin_hash.
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

/// Mirror of api_test.rs::MockAdminInvoker.
struct MockAdminInvoker {
    fired: Mutex<Option<serde_json::Value>>,
}

impl MockAdminInvoker {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            fired: Mutex::new(None),
        })
    }
}

#[async_trait]
impl AdminInvoker for MockAdminInvoker {
    async fn fire(&self, req: FulfillRequest) -> Result<(), String> {
        *self.fired.lock().await = Some(serde_json::to_value(&req).unwrap());
        Ok(())
    }

    async fn call(&self, _req: FulfillRequest) -> Result<FulfillResponse, String> {
        Err("MockAdminInvoker::call not implemented — adapter tests never call".into())
    }
}

/// Mirror of api_test.rs::body_json.
async fn body_json(resp: axum::response::Response) -> serde_json::Value {
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    serde_json::from_slice(&bytes).expect("response body must be JSON")
}

/// Mirror of api_test.rs::admin_login — mints a real session via the direct-router
/// path (already covered by api_test.rs; not the subject here).
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

/// Mirror of api_test.rs::test_link.
fn test_link(token: &str) -> Link {
    Link {
        token: token.into(),
        label: "Admin Test Link".into(),
        gift_note: None,
        thank_note: None,
        thanked_at: None,
        claims_allowed: 3,
        claims_used: 0,
        revoked: false,
        expires_at: None,
        unlock_at: None,
        curated_game_ids: None,
        created_at: datetime!(2026-07-02 00:00 UTC),
    }
}

/// The admin session cookie and CSRF header survive translation: a note POST through
/// the REAL v1 event path lands on the middleware, passes both auth layers, and the
/// note is durably written. This is the half of admin auth no test observed (#186).
/// The fixture populates BOTH header maps (real REST events always send both;
/// translation merges them — OMBB Q4 / spec D3).
#[tokio::test]
async fn v1_translation_carries_session_cookie_and_csrf_header() {
    init_stage_env();
    let Some(store) = store_or_skip("note-cookie").await else {
        return;
    };
    let invoker: Arc<dyn AdminInvoker> = MockAdminInvoker::new();
    let hash = test_admin_hash("pw");
    store.create_link(&test_link("fixture-tok")).await.unwrap();

    // Session minted via the already-covered direct-router path (not the subject here).
    let session = admin_login(&store, &invoker, &hash, "pw").await;

    // Predicate BEFORE substitution (spec AC4 — every v1 fixture; the __SESSION__
    // placeholder sits in a header VALUE and touches no predicate-checked key).
    let fixture = load_v1_fixture(include_str!("fixtures/apigw_v1_note_with_cookie.json"))
        .replace("__SESSION__", &session);
    let req = lambda_http::request::from_str(&fixture).expect("v1 fixture must translate");
    assert_eq!(req.uri().path(), "/admin/api/links/fixture-tok/note");
    assert_eq!(
        req.headers()
            .get("cookie")
            .expect("cookie must survive translation"),
        &format!("session={session}"),
    );

    let resp = router(Arc::clone(&store), Arc::clone(&invoker), hash.clone(), None)
        .oneshot(req)
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "both auth layers must pass through translation"
    );
    assert_eq!(body_json(resp).await, serde_json::json!({"ok": true}));

    // Durable proof, not just a 200: read the link back and find the note.
    let link = store
        .get_link("fixture-tok")
        .await
        .unwrap()
        .expect("link exists");
    assert_eq!(link.gift_note.as_deref(), Some("from the adapter side"));
}

/// FULL adapter round-trip (request AND response translation): login through
/// Adapter::from(router) and assert the V1 response shape — Set-Cookie lands in
/// multiValueHeaders (v1 puts ALL headers there and leaves `headers` empty;
/// lambda_http response.rs:78-88 — v2 would hoist cookies into a `cookies` array).
/// Asserted on the SERIALIZED JSON, not the hidden enum's shape, so a lambda_http
/// 2.x reshuffle breaks this test loudly instead of silently changing prod behavior.
/// NOTE: Adapter is #[doc(hidden)] but constructible via the public From impl —
/// accepted with eyes open (spec D7); BOTH halves of these suites ride hidden API
/// (LambdaRequest on the request side too) and there is no other offline seam.
#[tokio::test]
async fn v1_response_translation_puts_set_cookie_in_multi_value_headers() {
    init_stage_env();
    let Some(store) = store_or_skip("resp-cookie").await else {
        return;
    };
    let invoker: Arc<dyn AdminInvoker> = MockAdminInvoker::new();
    let hash = test_admin_hash("pw");

    // lambda_runtime is NOT a dependency of admin-api and must not become one —
    // lambda_http RE-EXPORTS it (lambda_http-1.3.0/src/lib.rs:77), the only
    // sanctioned path here (plan-review M3).
    use lambda_http::lambda_runtime;
    let payload: lambda_http::request::LambdaRequest = serde_json::from_str(&load_v1_fixture(
        include_str!("fixtures/apigw_v1_login.json"),
    ))
    .expect("fixture must deserialize as a LambdaRequest");
    let event = lambda_runtime::LambdaEvent::new(payload, lambda_runtime::Context::default());

    let adapter = lambda_http::Adapter::from(router(store, invoker, hash, None));
    let resp = adapter
        .oneshot(event)
        .await
        .expect("adapter must produce a response");

    let json = serde_json::to_value(&resp).expect("LambdaResponse must serialize");
    // The PROPERTY assert speaks first: behind a status gate it could never go red —
    // the first census's input-side sabotage (wrong password) fired `401 != 200` and
    // the multiValueHeaders extraction had never been observed red (Lilith's census
    // read). Property-first ordering makes AC2 true at property granularity.
    let set_cookie = json["multiValueHeaders"]["set-cookie"]
        .as_array()
        .expect("v1: Set-Cookie must be in multiValueHeaders");
    assert_eq!(json["statusCode"], 200);
    assert!(
        set_cookie[0].as_str().unwrap().starts_with("session="),
        "the session cookie must survive response translation: {set_cookie:?}"
    );
    assert!(
        json["cookies"].is_null(),
        "v1 must NOT hoist cookies into a v2-style cookies array"
    );
    assert_eq!(
        json["isBase64Encoded"], false,
        "a JSON body must not be base64-flagged (spec G2)"
    );
    // v1 puts ALL headers in multiValueHeaders and leaves `headers` empty
    // (response.rs:85-88) — asserted, not just claimed (review pass 2, F5).
    assert!(
        json["headers"]
            .as_object()
            .is_none_or(serde_json::Map::is_empty),
        "v1 response must leave the single-value headers map empty: {:?}",
        json["headers"]
    );
}
