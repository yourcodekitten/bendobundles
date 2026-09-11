//! Unfurl route integration tests. Store-backed via store_or_skip (CI runs them,
//! local skips without dynamodb-local) — see api_test.rs, whose harness this mirrors.
use std::sync::Arc;

use async_trait::async_trait;
use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use domain::Link;
use dynamo::Store;
use fulfillment::{FulfillRequest, FulfillResponse};
use public_api::{Invoker, TemplateError, TemplateSource, router_with_template};
use time::macros::datetime;
use tokio::sync::Mutex;
use tower::ServiceExt;

const TPL: &str =
    "<html><head>\n<!-- og:begin -->\nGENERIC\n<!-- og:end -->\n</head><body></body></html>";

struct StubTemplate(String);
#[async_trait]
impl TemplateSource for StubTemplate {
    async fn fetch(&self) -> Result<String, TemplateError> {
        Ok(self.0.clone())
    }
}
struct FailingTemplate;
#[async_trait]
impl TemplateSource for FailingTemplate {
    async fn fetch(&self) -> Result<String, TemplateError> {
        Err(TemplateError::Unavailable)
    }
}

// ── DynamoDB-local helper (copied from api_test.rs) ───────────────────────────

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
    // one table per test — no cross-test interference
    let store = Store::new(client, format!("t-unfurl-{test}"));
    store.create_table_for_tests().await.unwrap();
    Some(Arc::new(store))
}

// ── Link-seeding helper (copied from api_test.rs) ─────────────────────────────

fn test_link(token: &str) -> Link {
    Link {
        token: token.into(),
        label: "Test Friend".into(),
        gift_note: None,
        thank_note: None,
        thanked_at: None,
        claims_allowed: 1,
        claims_used: 0,
        revoked: false,
        expires_at: None,
        unlock_at: None,
        curated_game_ids: None,
        curated_notes: None,
        friend_id: None,
        created_at: datetime!(2026-07-02 00:00 UTC),
    }
}

// ── MockInvoker (copied from api_test.rs) ─────────────────────────────────────

struct MockInvoker {
    response_json: String,
    captured: Mutex<Option<serde_json::Value>>,
    bells: Mutex<Vec<serde_json::Value>>,
    bell_fails: bool,
}

impl MockInvoker {
    fn new(resp: FulfillResponse) -> Arc<Self> {
        Arc::new(Self {
            response_json: serde_json::to_string(&resp).unwrap(),
            captured: Mutex::new(None),
            bells: Mutex::new(Vec::new()),
            bell_fails: false,
        })
    }
}

#[async_trait]
impl Invoker for MockInvoker {
    async fn gift(&self, req: FulfillRequest) -> Result<FulfillResponse, String> {
        *self.captured.lock().await = Some(serde_json::to_value(&req).unwrap());
        Ok(serde_json::from_str(&self.response_json).unwrap())
    }

    async fn bell(&self, req: FulfillRequest) -> Result<(), String> {
        self.bells
            .lock()
            .await
            .push(serde_json::to_value(&req).unwrap());
        if self.bell_fails {
            return Err("bell invoke exploded".into());
        }
        Ok(())
    }
}

fn mock_invoker() -> Arc<dyn Invoker> {
    MockInvoker::new(FulfillResponse::GiftUrl {
        url: "https://x.com/g".into(),
    })
}

const TEST_BASE_URL: &str = "https://x.example";

#[tokio::test]
async fn unfurl_link_serves_personalized_html_with_cache_header() {
    let Some(store) = store_or_skip("unfurl-personalized").await else {
        return;
    };
    let token = "1".repeat(64);
    let mut lnk = test_link(&token);
    lnk.curated_game_ids = Some(vec!["a".into(), "b".into(), "c".into()]);
    store.create_link(&lnk).await.unwrap();

    let app = router_with_template(
        store,
        mock_invoker(),
        None,
        TEST_BASE_URL.into(),
        Some(Arc::new(StubTemplate(TPL.into()))),
    );
    let res = app
        .oneshot(
            Request::builder()
                .uri(format!("/l/{token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(res.headers()["content-type"], "text/html; charset=utf-8");
    assert_eq!(res.headers()["cache-control"], "public, max-age=60");
    let body = String::from_utf8(
        axum::body::to_bytes(res.into_body(), 1 << 20)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap();
    assert!(body.contains("ben wrapped something for"));
    assert!(body.contains("three treasures inside"));
    assert!(!body.contains("GENERIC"));
}

#[tokio::test]
async fn unfurl_dead_token_serves_template_byte_identical() {
    let Some(store) = store_or_skip("unfurl-dead").await else {
        return;
    };
    let app = router_with_template(
        store,
        mock_invoker(),
        None,
        TEST_BASE_URL.into(),
        Some(Arc::new(StubTemplate(TPL.into()))),
    );
    let res = app
        .oneshot(
            Request::builder()
                .uri(format!("/l/{}", "f".repeat(64)))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body = String::from_utf8(
        axum::body::to_bytes(res.into_body(), 1 << 20)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap();
    assert_eq!(
        body, TPL,
        "generic card must be the template verbatim (spec D5)"
    );
}

#[tokio::test]
async fn unfurl_template_without_markers_serves_generic_and_is_loud() {
    let Some(store) = store_or_skip("unfurl-markerless").await else {
        return;
    };
    let token = "2".repeat(64);
    store.create_link(&test_link(&token)).await.unwrap();

    let app = router_with_template(
        store,
        mock_invoker(),
        None,
        TEST_BASE_URL.into(),
        Some(Arc::new(StubTemplate("<html>no markers</html>".into()))),
    );
    let res = app
        .oneshot(
            Request::builder()
                .uri(format!("/l/{token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body = String::from_utf8(
        axum::body::to_bytes(res.into_body(), 1 << 20)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap();
    assert_eq!(body, "<html>no markers</html>");
    // the EMF witness: unit-tested by shape in unfurl.rs (emf_blob_for_marker_absent
    // test asserting the JSON parses and carries Namespace bendobundles/unfurl +
    // metric UnfurlMarkerAbsent=1) — the handler path here proves the DEGRADE half.
}

/// D5 oracle, executable witness for the REVOKED arm (BLOCKER-2, 2026-09-07
/// final review — the ledger flagged this branch as untested at handler level:
/// only the unknown-token arm had a witness). Seeds a live link, revokes it, and
/// asserts a revoked token's response is byte-identical to an unknown token's —
/// status, content-type, cache-control, AND body — not merely "both equal TPL",
/// so a future edit that special-cases one branch's headers is caught even if it
/// happens to leave the body alone.
#[tokio::test]
async fn revoked_link_unfurl_is_byte_identical_to_unknown() {
    let Some(store) = store_or_skip("unfurl-revoked").await else {
        return;
    };
    let revoked_token = "3".repeat(64);
    let mut lnk = test_link(&revoked_token);
    lnk.revoked = true;
    store.create_link(&lnk).await.unwrap();

    let unknown_token = "e".repeat(64);

    let app = router_with_template(
        store,
        mock_invoker(),
        None,
        TEST_BASE_URL.into(),
        Some(Arc::new(StubTemplate(TPL.into()))),
    );

    let revoked_res = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/l/{revoked_token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let unknown_res = app
        .oneshot(
            Request::builder()
                .uri(format!("/l/{unknown_token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(revoked_res.status(), unknown_res.status());
    assert_eq!(revoked_res.status(), StatusCode::OK);
    assert_eq!(
        revoked_res.headers()["content-type"],
        unknown_res.headers()["content-type"]
    );
    assert_eq!(
        revoked_res.headers()["cache-control"],
        unknown_res.headers()["cache-control"]
    );

    let revoked_body = String::from_utf8(
        axum::body::to_bytes(revoked_res.into_body(), 1 << 20)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap();
    let unknown_body = String::from_utf8(
        axum::body::to_bytes(unknown_res.into_body(), 1 << 20)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap();
    assert_eq!(
        revoked_body, unknown_body,
        "revoked and unknown tokens must be byte-identical (spec D5 oracle)"
    );
    assert_eq!(
        revoked_body, TPL,
        "both arms must also equal the template verbatim"
    );
}

#[tokio::test]
async fn unfurl_source_failure_is_500_json() {
    let Some(store) = store_or_skip("unfurl-fail").await else {
        return;
    };
    let app = router_with_template(
        store,
        mock_invoker(),
        None,
        TEST_BASE_URL.into(),
        Some(Arc::new(FailingTemplate)),
    );
    let res = app
        .oneshot(
            Request::builder()
                .uri(format!("/l/{}", "a".repeat(64)))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::INTERNAL_SERVER_ERROR);
}
