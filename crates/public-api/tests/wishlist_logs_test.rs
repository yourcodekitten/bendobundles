//! "No appids in logs" is a promise (spec-wishing-well §4) — this makes it an assertion.
//! Own test binary ⇒ owns the global subscriber; POSITIVE CONTROL first, so a capture that
//! sees nothing cannot pass vacuously.
use std::sync::{Arc, Mutex, OnceLock};

static BUF: OnceLock<Arc<Mutex<Vec<u8>>>> = OnceLock::new();

struct Cap;
impl std::io::Write for Cap {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        BUF.get().unwrap().lock().unwrap().extend_from_slice(b);
        Ok(b.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn install() {
    BUF.get_or_init(|| Arc::new(Mutex::new(Vec::new())));
    // PRODUCTION PARITY: main.rs installs `fmt().with_ansi(false).without_time().init()`, which is
    // INFO across all targets, and that is what reaches CloudWatch. Do NOT raise it to TRACE:
    // aws-smithy-runtime logs full DynamoDB request/response bodies at TRACE
    // (orchestrator.rs:486/:541, orchestrator/http.rs:42), and the seeded game's own
    // `steam_app_id` would appear in them whatever the handler does. That would be a red
    // against a correct handler.
    let _ = tracing_subscriber::fmt()
        .with_ansi(false)
        .with_writer(|| Cap)
        .try_init();
}

fn captured() -> String {
    String::from_utf8_lossy(&BUF.get().unwrap().lock().unwrap()).into_owned()
}

use async_trait::async_trait;
use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use domain::{Game, GameStatus, Link, game_id};
use dynamo::Store;
use fulfillment::{FulfillRequest, FulfillResponse};
use public_api::{Invoker, router};
use steam_client::{SteamApiKey, SteamClient};
use time::macros::datetime;
use tower::ServiceExt;

const TEST_STEAMID: &str = "76561198000000001";
const TEST_BASE_URL: &str = "https://test.bendobundles.com";

/// Copied from api_test.rs:24 — KEEP the explicit-variable panic branch verbatim: it is the
/// only thing that stops a missing store from forging a green here.
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
    let store = Store::new(client, format!("t-pub-{test}"));
    store.create_table_for_tests().await.unwrap();
    Some(Arc::new(store))
}

/// The proxy never invokes fulfillment; this invoker exists only to satisfy `router`.
struct NoInvoker;
#[async_trait]
impl Invoker for NoInvoker {
    async fn gift(&self, _req: FulfillRequest) -> Result<FulfillResponse, String> {
        Err("not used".into())
    }
    async fn bell(&self, _req: FulfillRequest) -> Result<(), String> {
        Ok(())
    }
}

#[tokio::test]
async fn wishlist_proxy_logs_no_appids() {
    install();
    // POSITIVE CONTROL: the capture can see an INFO event on this task.
    tracing::info!("canary-9913377");
    assert!(
        captured().contains("canary-9913377"),
        "capture is blind — the test would be vacuous"
    );

    let Some(store) = store_or_skip("wish-logs").await else {
        return;
    };
    let mut g = Game {
        id: game_id("gk9301", "mn"),
        title: "Game 9301".into(),
        bundle: "Test Bundle".into(),
        gamekey: "gk9301".into(),
        machine_name: "mn".into(),
        key_type: "steam".into(),
        giftable: true,
        hidden: false,
        status: GameStatus::Available,
        claim_id: None,
        artwork_url: None,
        keyindex: 9301,
        requires_choice: false,
        steam_app_id: None,
        appid_source: None,
        owned_by_ben: false,
        hidden_source: None,
        acquired_at: None,
    };
    g.steam_app_id = Some(8675309);
    store.put_game(&g).await.unwrap();
    store
        .create_link(&Link {
            token: "wish-logs-tok".into(),
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
        })
        .await
        .unwrap();

    let server = wiremock::MockServer::start().await;
    wiremock::Mock::given(wiremock::matchers::path("/IWishlistService/GetWishlist/v1/"))
        .respond_with(wiremock::ResponseTemplate::new(200).set_body_string(
            r#"{"response":{"items":[{"appid":8675309,"priority":0,"date_added":1678000000},{"appid":7340033,"priority":0,"date_added":1678000000}]}}"#,
        ))
        .mount(&server)
        .await;
    let steam = SteamClient::new(
        &server.uri(),
        &server.uri(),
        &server.uri(),
        SteamApiKey::new("TESTKEY".into()),
    )
    .unwrap();
    let app = router(
        store,
        Arc::new(NoInvoker),
        Some(Arc::new(steam)),
        TEST_BASE_URL.to_string(),
    );

    let resp = app
        .oneshot(
            Request::get(format!(
                "/api/l/wish-logs-tok/steam/wishlist/{TEST_STEAMID}"
            ))
            .body(Body::empty())
            .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    assert!(
        String::from_utf8_lossy(&body).contains("8675309"),
        "the overlap must be served"
    );

    let logs = captured();
    assert!(
        !logs.contains("8675309"),
        "an overlap appid reached the logs:\n{logs}"
    );
    assert!(
        !logs.contains("7340033"),
        "a wishlist-only appid reached the logs:\n{logs}"
    );
}
