//! Adapter-boundary tests: canned API-Gateway **v1 (REST, payload 1.0)** events through
//! the REAL lambda_http translation into the same `router()` the unit tests use.
//! v1 is what production sends — terraform/aws-apigateway.tf is a REST API; there is
//! no apigatewayv2 resource in the repo (spec 2026-08-12, correction 1).
//!
//! THIS BINARY IS SEPARATE from api_test.rs ON PURPOSE: production sets
//! AWS_LAMBDA_HTTP_IGNORE_STAGE_IN_PATH=true (terraform/aws-lambda.tf:89,:132) and
//! lambda_http reads it from process env per request — and the check is PRESENCE, not
//! value (`env::var(...).is_ok()`, request.rs:408: `"false"` and `""` both activate).
//! Edition-2024 set_var is unsafe and racy against concurrent readers, so it is set
//! exactly once, before any translation, via the Once below — and no other binary
//! inherits it (spec D2). The env-free twin (adapter_stage_control_test.rs) proves the
//! flag is load-bearing; adapter_stage_false_test.rs pins the presence footgun.
//!
//! Fixture provenance (spec D5): fixtures are built to the DOCUMENTED full REST proxy
//! event shape + this repo's terraform config (REST v1, stage `live`, CloudFront
//! origin_path /live — terraform/aws-cloudfront.tf:135-140). The lambda_http shipped
//! corpus was only a starting skeleton and is authority for "parses", never for "is
//! what AWS sends" — it is parser-minimal (no multi-value maps, no body, no
//! requestContext.path). `load_v1_fixture` below is what makes "real-shaped" a
//! machine-checked predicate instead of an adjective.

use std::sync::Arc;
use std::sync::Once;

use async_trait::async_trait;
use fulfillment::{FulfillRequest, FulfillResponse};
use lambda_http::RequestExt;
use public_api::{Invoker, router};
use tower::ServiceExt;

static STAGE_ENV: Once = Once::new();

/// Reproduce production's stage config. Every test calls this before any translation.
fn init_stage_env() {
    STAGE_ENV.call_once(|| {
        // SAFETY: called before any test in this binary performs env reads via
        // lambda_http; Once blocks racing callers until the write completes. No other
        // code in this binary writes env. (Global Constraints forbid set_var elsewhere.)
        unsafe { std::env::set_var("AWS_LAMBDA_HTTP_IGNORE_STAGE_IN_PATH", "true") }
    });
}

/// spec D5: "real-shaped" is a predicate. Every v1 fixture goes through here before
/// from_str ever sees it: (i) full production key-set present, (ii) correlated fields
/// consistent. Closes the hand-edit class, not the instance.
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

/// The predicate must earn its red PER ARM (spec AC4; one planted defect per copy —
/// a both-defects fixture proves only whichever arm panics first, masking the other).
/// Each opens with init_stage_env() even though the predicate never reads the flag:
/// these tests panic BY DESIGN, and std's panic machinery lazily getenv()s
/// RUST_BACKTRACE on the first panic — ungated, that read can race the Once's
/// setenv on another thread, the precise UB edition-2024 set_var warns about
/// (review pass 2, F3).
#[test]
#[should_panic(expected = "fixture missing production key")]
fn degenerate_missing_key_fails_the_keyset_arm() {
    init_stage_env();
    load_v1_fixture(include_str!(
        "fixtures/apigw_v1_degenerate_missing_key.json"
    ));
}

#[test]
#[should_panic(expected = "requestContext.path must equal")]
fn degenerate_inconsistent_path_fails_the_correlation_arm() {
    init_stage_env();
    load_v1_fixture(include_str!(
        "fixtures/apigw_v1_degenerate_inconsistent.json"
    ));
}

#[test]
#[should_panic(expected = "missing from the multi map")]
fn degenerate_header_map_drift_fails_the_dual_map_arm() {
    init_stage_env();
    load_v1_fixture(include_str!(
        "fixtures/apigw_v1_degenerate_header_drift.json"
    ));
}

#[test]
#[should_panic(expected = "must equal the single-map value")]
fn degenerate_query_map_drift_fails_the_dual_map_arm() {
    init_stage_env();
    load_v1_fixture(include_str!(
        "fixtures/apigw_v1_degenerate_query_drift.json"
    ));
}

/// Same store idiom as api_test.rs::store_or_skip, but prefix `t-pubadp-` — this
/// binary runs in parallel with api_test.rs against the same dynamodb-local, and a
/// shared prefix + same test name would collide (Global Constraints).
async fn store_or_skip(test: &str) -> Option<Arc<dynamo::Store>> {
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
    let store = dynamo::Store::new(client, format!("t-pubadp-{test}"));
    store.create_table_for_tests().await.unwrap();
    Some(Arc::new(store))
}

/// Storeless fake for tests that never reach the store (fallback 404).
async fn fake_store() -> Arc<dynamo::Store> {
    let cfg = aws_config::defaults(aws_config::BehaviorVersion::latest())
        .endpoint_url("http://127.0.0.1:0")
        .region("us-east-1")
        .test_credentials()
        .load()
        .await;
    Arc::new(dynamo::Store::new(
        aws_sdk_dynamodb::Client::new(&cfg),
        "fake".into(),
    ))
}

// The real trait (verified at public-api/src/lib.rs:27-30): exactly ONE method,
// `gift`, with fulfillment:: types. Error arm mirrors api_test.rs's MockInvoker.
struct NoInvoker;
#[async_trait]
impl Invoker for NoInvoker {
    async fn gift(&self, _req: FulfillRequest) -> Result<FulfillResponse, String> {
        Err("adapter tests never invoke".into())
    }
}

fn test_router(store: Arc<dynamo::Store>) -> axum::Router {
    router(
        store,
        Arc::new(NoInvoker),
        None,
        "https://bendobundles.com".into(),
    )
}

async fn body_json(resp: axum::response::Response) -> serde_json::Value {
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    serde_json::from_slice(&bytes).expect("response body must be JSON")
}

/// v1 event → translated request: stage `live` must NOT be prepended (flag set),
/// and a path matching no route must land on the router fallback — the generic
/// `{"error":"not found"}`, which is distinct from the matched-route unknown-link
/// shape asserted in the next test. Together they pin path derivation from both sides.
#[tokio::test]
async fn v1_translation_derives_stageless_path_and_falls_back() {
    init_stage_env();
    let req = lambda_http::request::from_str(&load_v1_fixture(include_str!(
        "fixtures/apigw_v1_fallback.json"
    )))
    .expect("v1 fixture must translate");
    assert_eq!(
        req.uri().path(),
        "/api/definitely/no/such/route",
        "stage must not be prepended"
    );
    let resp = test_router(fake_store().await).oneshot(req).await.unwrap();
    assert_eq!(resp.status(), 404);
    assert_eq!(
        body_json(resp).await,
        serde_json::json!({"error": "not found"})
    );
}

/// The automated twin of #186's manual prod probe: unknown token through the REAL
/// translation must reach handle_get_link and answer with the app's own JSON —
/// proof the adapter delivered the event to the Router, the half no test covered.
#[tokio::test]
async fn v1_translation_reaches_link_route_with_unknown_token() {
    init_stage_env();
    let Some(store) = store_or_skip("link-unknown").await else {
        return;
    };
    let req = lambda_http::request::from_str(&load_v1_fixture(include_str!(
        "fixtures/apigw_v1_link_unknown.json"
    )))
    .expect("v1 fixture must translate");
    assert_eq!(req.uri().path(), "/api/l/definitely-not-a-token");
    let resp = test_router(store).oneshot(req).await.unwrap();
    assert_eq!(resp.status(), 404);
    assert_eq!(
        body_json(resp).await,
        serde_json::json!({"error": "unknown link"}),
        "matched-route 404, NOT the fallback shape — path derivation reached the handler"
    );
}

/// multiValueQueryStringParameters survive translation: both values of `a` visible
/// via RequestExt AND in the reconstructed URI (build_request_uri re-serializes the
/// map, preferring the multi-value form — that reconstruction is the assertion target).
#[tokio::test]
async fn v1_translation_preserves_multi_value_query() {
    init_stage_env();
    let req = lambda_http::request::from_str(&load_v1_fixture(include_str!(
        "fixtures/apigw_v1_multi_value_query.json"
    )))
    .expect("v1 fixture must translate");
    let qs = req.query_string_parameters();
    assert_eq!(qs.all("a").expect("a present"), vec!["1", "2"]);
    assert_eq!(qs.first("b"), Some("3"));
    let query = req.uri().query().expect("query must be reconstructed");
    assert!(
        query.contains("a=1") && query.contains("a=2") && query.contains("b=3"),
        "reconstructed URI must carry every value: got {query}"
    );
    // Deliberately NOT asserted: parameter ORDER in the reconstructed URI (map-backed,
    // not part of the property) — recorded as asserted-not-pinned in spec AC1.
}

/// isBase64Encoded body is DECODED by translation (to bytes, non-ASCII intact), and
/// axum's Json extractor parses it — proven by reaching the handler's own link lookup
/// (unknown-link 404) instead of a 4xx extractor refusal. Validation precedes the
/// link read in handle_post_thanks, so a non-empty note is required to get there.
#[tokio::test]
async fn v1_translation_decodes_base64_body_to_reach_json_extractor() {
    init_stage_env();
    let Some(store) = store_or_skip("b64-thanks").await else {
        return;
    };
    let req = lambda_http::request::from_str(&load_v1_fixture(include_str!(
        "fixtures/apigw_v1_base64_thanks.json"
    )))
    .expect("v1 fixture must translate");
    assert_eq!(
        std::str::from_utf8(req.body().as_ref()).expect("decoded body must be the original UTF-8"),
        "{\"note\":\"thank you ben ❤\"}",
    );
    let resp = test_router(store).oneshot(req).await.unwrap();
    assert_eq!(resp.status(), 404);
    assert_eq!(
        body_json(resp).await,
        serde_json::json!({"error": "unknown link"}),
        "the decoded body must parse as JSON and clear note-validation to reach the link read"
    );
}

/// Discriminator guard (spec D4): a v2 event must still parse AS v2 — which
/// simultaneously asserts the v1 arm (tried FIRST, deserializer.rs:29-34) did not
/// capture it. pass_through is OFF, so an unmatched payload errors instead of
/// silently passing through. This fixture deliberately does NOT go through
/// load_v1_fixture — it is not a v1 event and the predicate would rightly reject it.
#[tokio::test]
async fn v2_guard_still_parses_as_v2() {
    init_stage_env();
    let req = lambda_http::request::from_str(include_str!("fixtures/apigw_v2_guard.json"))
        .expect("v2 must remain parseable");
    // v2's rawPath carries no stage; the translated path is the rawPath verbatim.
    assert_eq!(req.uri().path(), "/api/l/x");
    match req.request_context() {
        lambda_http::request::RequestContext::ApiGatewayV2(_) => {}
        other => panic!("v2 fixture parsed as {other:?} — deserializer arm changed"),
    }
}

/// ALB guard (spec D4, OMBB Q4): the cascade has more arms than v1/v2 — one ALB fixture
/// pins the fall-through class. Same pair logic as the v2 guard: parses AS ALB, which
/// simultaneously asserts the v1 arm didn't capture it (v1's requestContext.httpMethod
/// is serde-required and ALB's context carries only `elb`). Not a v1 event — bypasses
/// load_v1_fixture by design.
#[tokio::test]
async fn alb_guard_still_parses_as_alb() {
    init_stage_env();
    let req = lambda_http::request::from_str(include_str!("fixtures/alb_guard.json"))
        .expect("ALB must remain parseable");
    assert_eq!(req.uri().path(), "/api/l/x");
    match req.request_context() {
        lambda_http::request::RequestContext::Alb(_) => {}
        other => panic!("ALB fixture parsed as {other:?} — deserializer arm changed"),
    }
}
