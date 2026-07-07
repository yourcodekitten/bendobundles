//! Integration tests for the public-api router.
//!
//! Store-backed tests use `store_or_skip` and require a DynamoDB-local instance;
//! they are skipped locally and run in CI. The MockInvoker is used for all tests —
//! it records the FulfillRequest it received so we can assert field correctness.
use std::sync::Arc;

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
use tokio::sync::Mutex;
use tower::ServiceExt;

// ── DynamoDB-local helper ─────────────────────────────────────────────────────

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
    let store = Store::new(client, format!("t-pub-{test}"));
    store.create_table_for_tests().await.unwrap();
    Some(Arc::new(store))
}

// ── Test fixtures ─────────────────────────────────────────────────────────────

fn test_game(n: u32) -> Game {
    Game {
        id: game_id(&format!("gk{n}"), "mn"),
        title: format!("Game {n}"),
        bundle: "Test Bundle".into(),
        gamekey: format!("gk{n}"),
        machine_name: "mn".into(),
        key_type: "steam".into(),
        giftable: true,
        hidden: false,
        status: GameStatus::Available,
        claim_id: None,
        artwork_url: Some(format!("art{n}.png")),
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
        label: "Test Friend".into(),
        claims_allowed: 1,
        claims_used: 0,
        revoked: false,
        expires_at: None,
        created_at: datetime!(2026-07-02 00:00 UTC),
    }
}

/// A hex token suitable as a valid ctx path `/l/<token>` — exactly 64 lowercase hex chars.
const CTX_TOKEN: &str = "00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff";

/// Steam ID used across OpenID tests.
const TEST_STEAMID: &str = "76561198000000001";

/// Base URL used for all router instances in tests.
const TEST_BASE_URL: &str = "https://test.bendobundles.com";

// ── MockInvoker ───────────────────────────────────────────────────────────────

struct MockInvoker {
    /// Serialised FulfillResponse so we can return it without Clone.
    response_json: String,
    /// Last FulfillRequest received, stored as Value so we can read it later.
    captured: Mutex<Option<serde_json::Value>>,
}

impl MockInvoker {
    fn new(resp: FulfillResponse) -> Arc<Self> {
        Arc::new(Self {
            response_json: serde_json::to_string(&resp).unwrap(),
            captured: Mutex::new(None),
        })
    }

    async fn captured_request(&self) -> Option<FulfillRequest> {
        self.captured
            .lock()
            .await
            .clone()
            .map(|v| serde_json::from_value(v).expect("captured request must deserialise"))
    }
}

#[async_trait]
impl Invoker for MockInvoker {
    async fn gift(&self, req: FulfillRequest) -> Result<FulfillResponse, String> {
        *self.captured.lock().await = Some(serde_json::to_value(&req).unwrap());
        Ok(serde_json::from_str(&self.response_json).unwrap())
    }
}

// ── Router builder helpers ────────────────────────────────────────────────────

/// Build a plain router (no steam client) for tests that don't need steam.
fn plain_router(store: Arc<Store>, invoker: Arc<dyn Invoker>) -> axum::Router {
    router(store, invoker, None, TEST_BASE_URL.to_string())
}

/// Build a router with a steam client pointed at a wiremock server.
fn steam_router(store: Arc<Store>, invoker: Arc<dyn Invoker>, steam_base: &str) -> axum::Router {
    let steam = SteamClient::new(
        steam_base,
        steam_base,
        steam_base,
        SteamApiKey::new("TESTKEY".into()),
    )
    .unwrap();
    router(
        store,
        invoker,
        Some(Arc::new(steam)),
        TEST_BASE_URL.to_string(),
    )
}

// ── Body helper ───────────────────────────────────────────────────────────────

async fn body_json(resp: axum::response::Response) -> serde_json::Value {
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    serde_json::from_slice(&bytes).expect("response body must be JSON")
}

// ── Tests ─────────────────────────────────────────────────────────────────────

/// Unknown token → 404 {"error":"unknown link"}, indistinguishable from any
/// other invalid token (no enumeration oracle).
#[tokio::test]
async fn unknown_token_is_404() {
    let Some(store) = store_or_skip("unknown-token").await else {
        return;
    };
    let mock = MockInvoker::new(FulfillResponse::GiftUrl {
        url: "https://x.com/g".into(),
    });

    let req = Request::get("/api/l/no-such-token")
        .body(Body::empty())
        .unwrap();
    let resp = plain_router(store, mock).oneshot(req).await.unwrap();

    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    let j = body_json(resp).await;
    assert_eq!(j["error"], "unknown link");
}

/// Revoked link → 200, state:"revoked", games:[] (dead link shows no catalog),
/// but claims history is intact (even if empty here).
#[tokio::test]
async fn revoked_link_active_false_games_empty() {
    let Some(store) = store_or_skip("revoked-link").await else {
        return;
    };
    // Seed a listable game so the empty-games assertion is meaningful.
    store.put_game(&test_game(1)).await.unwrap();
    let mut lnk = test_link("rev-tok");
    lnk.revoked = true;
    store.create_link(&lnk).await.unwrap();

    let mock = MockInvoker::new(FulfillResponse::GiftUrl {
        url: "https://x.com/g".into(),
    });

    let req = Request::get("/api/l/rev-tok").body(Body::empty()).unwrap();
    let resp = plain_router(Arc::clone(&store), mock.clone())
        .oneshot(req)
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let j = body_json(resp).await;
    assert_eq!(j["state"], "revoked");
    assert_eq!(j["games"], serde_json::json!([]));
    assert!(j["claims"].as_array().is_some());
}

/// Exhausted link → 200, state:"exhausted", games STILL visible
/// (friend can browse; claim buttons disabled client-side). The explicit state
/// field is what lets the client tell exhausted from revoked without guessing
/// from games.len().
#[tokio::test]
async fn exhausted_link_state_exhausted_games_visible() {
    let Some(store) = store_or_skip("exhausted-link").await else {
        return;
    };
    store.put_game(&test_game(1)).await.unwrap();
    let mut lnk = test_link("exh-tok");
    lnk.claims_used = lnk.claims_allowed;
    store.create_link(&lnk).await.unwrap();

    let mock = MockInvoker::new(FulfillResponse::GiftUrl {
        url: "https://x.com/g".into(),
    });

    let req = Request::get("/api/l/exh-tok").body(Body::empty()).unwrap();
    let resp = plain_router(Arc::clone(&store), mock.clone())
        .oneshot(req)
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let j = body_json(resp).await;
    assert_eq!(j["state"], "exhausted");
    assert_eq!(j["games"].as_array().unwrap().len(), 1);
}

/// Expired link → 200, state:"expired", games hidden like revoked.
#[tokio::test]
async fn expired_link_state_expired_games_empty() {
    let Some(store) = store_or_skip("expired-link").await else {
        return;
    };
    store.put_game(&test_game(1)).await.unwrap();
    let mut lnk = test_link("exp-tok");
    lnk.expires_at = Some(datetime!(2026-07-01 00:00 UTC));
    store.create_link(&lnk).await.unwrap();

    let mock = MockInvoker::new(FulfillResponse::GiftUrl {
        url: "https://x.com/g".into(),
    });

    let req = Request::get("/api/l/exp-tok").body(Body::empty()).unwrap();
    let resp = plain_router(Arc::clone(&store), mock.clone())
        .oneshot(req)
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let j = body_json(resp).await;
    assert_eq!(j["state"], "expired");
    assert_eq!(j["games"], serde_json::json!([]));
}

/// Happy path: seed game + link, claim with MockInvoker returning GiftUrl →
/// 200 with gift_url; a second GET shows the game gone from the games list;
/// MockInvoker received FulfillRequest::Gift with the correct keyindex / gamekey
/// / machine_name from the seeded game.
#[tokio::test]
async fn happy_claim_gift_url_and_fields_verified() {
    let Some(store) = store_or_skip("happy-claim").await else {
        return;
    };
    let g = test_game(1); // keyindex=1, gamekey="gk1", machine_name="mn"
    let gid = g.id.clone();
    store.put_game(&g).await.unwrap();
    store.create_link(&test_link("happy-tok")).await.unwrap();

    let mock: Arc<MockInvoker> = MockInvoker::new(FulfillResponse::GiftUrl {
        url: "https://humblebundle.com/gift?key=abc".into(),
    });
    let invoker: Arc<dyn Invoker> = mock.clone();

    // POST /api/l/happy-tok/claim
    let claim_body = serde_json::json!({"game_id": gid});
    let post_req = Request::post("/api/l/happy-tok/claim")
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_vec(&claim_body).unwrap()))
        .unwrap();
    let post_resp = plain_router(Arc::clone(&store), Arc::clone(&invoker))
        .oneshot(post_req)
        .await
        .unwrap();
    assert_eq!(post_resp.status(), StatusCode::OK);
    let post_j = body_json(post_resp).await;
    assert_eq!(post_j["gift_url"], "https://humblebundle.com/gift?key=abc");

    // Second GET: game gone from listable (it's now Pending after claim_game).
    let get_req = Request::get("/api/l/happy-tok")
        .body(Body::empty())
        .unwrap();
    let get_resp = plain_router(Arc::clone(&store), Arc::clone(&invoker))
        .oneshot(get_req)
        .await
        .unwrap();
    assert_eq!(get_resp.status(), StatusCode::OK);
    let get_j = body_json(get_resp).await;
    assert_eq!(
        get_j["games"],
        serde_json::json!([]),
        "game must be removed from listable after claim"
    );
    let claims = get_j["claims"].as_array().unwrap();
    assert_eq!(claims.len(), 1, "claim must appear in history");
    assert_eq!(claims[0]["game_id"], gid);
    assert_eq!(
        claims[0]["title"], "Game 1",
        "claim history must carry the game title (friends see names, not ids)"
    );

    // Assert MockInvoker received correct game fields.
    let captured = mock
        .captured_request()
        .await
        .expect("invoker must have been called");
    if let FulfillRequest::Gift {
        gamekey,
        machine_name,
        keyindex,
        ..
    } = captured
    {
        assert_eq!(gamekey, "gk1", "gamekey from seeded game");
        assert_eq!(machine_name, "mn", "machine_name from seeded game");
        assert_eq!(
            keyindex, 1u32,
            "keyindex from seeded game (test_game(1) → keyindex=1)"
        );
    } else {
        panic!("expected FulfillRequest::Gift");
    }
}

/// Race loser: two links try to claim the same game; the second attempt gets a
/// 409 "someone beat you to it".
#[tokio::test]
async fn race_loser_gets_409() {
    let Some(store) = store_or_skip("race-loser").await else {
        return;
    };
    let g = test_game(2);
    let gid = g.id.clone();
    store.put_game(&g).await.unwrap();
    store.create_link(&test_link("race-tok1")).await.unwrap();
    let mut lnk2 = test_link("race-tok2");
    lnk2.claims_allowed = 5; // not exhausted so the gate is only GameUnavailable
    store.create_link(&lnk2).await.unwrap();

    let mock = MockInvoker::new(FulfillResponse::GiftUrl {
        url: "https://x.com/gift".into(),
    });

    let claim_body = serde_json::json!({"game_id": gid});

    // First claim wins.
    let r1 = Request::post("/api/l/race-tok1/claim")
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_vec(&claim_body).unwrap()))
        .unwrap();
    let resp1 = plain_router(Arc::clone(&store), mock.clone())
        .oneshot(r1)
        .await
        .unwrap();
    assert_eq!(resp1.status(), StatusCode::OK, "first claim must succeed");

    // Second claim loses.
    let r2 = Request::post("/api/l/race-tok2/claim")
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_vec(&claim_body).unwrap()))
        .unwrap();
    let resp2 = plain_router(Arc::clone(&store), mock.clone())
        .oneshot(r2)
        .await
        .unwrap();
    assert_eq!(
        resp2.status(),
        StatusCode::CONFLICT,
        "race loser must get 409"
    );
    let j = body_json(resp2).await;
    assert!(
        j["error"].as_str().unwrap().contains("beat you"),
        "error must name the race: {j}"
    );
}

/// Parked: MockInvoker returns Parked → 202 processing body; claim is visible
/// in the GET claims list with state "pending" (MockInvoker didn't call
/// fulfill_claim, so the store still has the intake state).
#[tokio::test]
async fn parked_claim_returns_202_and_appears_pending() {
    let Some(store) = store_or_skip("parked-claim").await else {
        return;
    };
    let g = test_game(3);
    let gid = g.id.clone();
    store.put_game(&g).await.unwrap();
    store.create_link(&test_link("park-tok")).await.unwrap();

    let mock = MockInvoker::new(FulfillResponse::Parked {
        reason: "humble call inconclusive".into(),
    });

    let claim_body = serde_json::json!({"game_id": gid});
    let post_req = Request::post("/api/l/park-tok/claim")
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_vec(&claim_body).unwrap()))
        .unwrap();
    let post_resp = plain_router(Arc::clone(&store), mock.clone())
        .oneshot(post_req)
        .await
        .unwrap();
    assert_eq!(post_resp.status(), StatusCode::ACCEPTED, "parked → 202");
    let post_j = body_json(post_resp).await;
    assert_eq!(post_j["status"], "processing");
    assert!(
        post_j["message"].as_str().unwrap().contains("check back"),
        "message must tell user to check back: {post_j}"
    );

    // GET: claim visible in pending state (fulfillment didn't complete).
    let get_req = Request::get("/api/l/park-tok").body(Body::empty()).unwrap();
    let get_resp = plain_router(Arc::clone(&store), mock.clone())
        .oneshot(get_req)
        .await
        .unwrap();
    let get_j = body_json(get_resp).await;
    let claims = get_j["claims"].as_array().unwrap();
    assert_eq!(claims.len(), 1, "parked claim must appear in history");
    assert_eq!(claims[0]["state"], "pending");
    assert_eq!(
        claims[0]["gift_url"],
        serde_json::Value::Null,
        "gift_url must be null while parked"
    );
}

/// Fallback route returns 404 for any unmatched path.
#[tokio::test]
async fn unknown_route_is_404() {
    let Some(store) = store_or_skip("unknown-route").await else {
        return;
    };
    let mock = MockInvoker::new(FulfillResponse::GiftUrl {
        url: "https://x.com/g".into(),
    });
    let req = Request::get("/not/a/route").body(Body::empty()).unwrap();
    let resp = plain_router(store, mock).oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

/// Reserved token SELF → 404 with byte-identical body to unknown token
/// (no enumeration oracle, no special indication of reserved status).
#[tokio::test]
async fn self_reserved_token_is_a_plain_404() {
    let Some(store) = store_or_skip("self-404").await else {
        return;
    };
    let mock = MockInvoker::new(FulfillResponse::GiftUrl {
        url: "https://x.com/g".into(),
    });

    let req_self = Request::get("/api/l/SELF").body(Body::empty()).unwrap();
    let resp_self = plain_router(Arc::clone(&store), mock.clone())
        .oneshot(req_self)
        .await
        .unwrap();

    let req_other =
        Request::get("/api/l/nonexistent0000000000000000000000000000000000000000000000000000")
            .body(Body::empty())
            .unwrap();
    let resp_other = plain_router(Arc::clone(&store), mock.clone())
        .oneshot(req_other)
        .await
        .unwrap();

    assert_eq!(resp_self.status(), StatusCode::NOT_FOUND);
    assert_eq!(resp_other.status(), StatusCode::NOT_FOUND);

    let bytes_self = axum::body::to_bytes(resp_self.into_body(), usize::MAX)
        .await
        .unwrap();
    let bytes_other = axum::body::to_bytes(resp_other.into_body(), usize::MAX)
        .await
        .unwrap();
    assert_eq!(
        bytes_self, bytes_other,
        "SELF and unknown token must return byte-identical 404 (no oracle)"
    );
}

// ── Task 10: Steam tests ───────────────────────────────────────────────────────

/// Build assertion params for a wiremock-backed OpenID check_authentication test.
fn assertion_params(claimed: &str, return_to: &str) -> Vec<(String, String)> {
    vec![
        (
            "openid.ns".into(),
            "http://specs.openid.net/auth/2.0".into(),
        ),
        ("openid.mode".into(), "id_res".into()),
        ("openid.claimed_id".into(), claimed.into()),
        ("openid.identity".into(), claimed.into()),
        ("openid.return_to".into(), return_to.into()),
        (
            "openid.response_nonce".into(),
            "2026-07-06T00:00:00Znonce".into(),
        ),
        ("openid.assoc_handle".into(), "h".into()),
        (
            "openid.signed".into(),
            "signed,op_endpoint,claimed_id,identity,return_to,response_nonce,assoc_handle".into(),
        ),
        ("openid.sig".into(), "sig".into()),
    ]
}

/// Owned proxy: unknown token → byte-identical 404 (no oracle).
/// Security gate B1: unknown token → same body as any unknown-token 404.
#[tokio::test]
async fn owned_proxy_404s_without_live_link_byte_identical() {
    let Some(store) = store_or_skip("owned-proxy-404").await else {
        return;
    };
    let server = wiremock::MockServer::start().await;
    let mock = MockInvoker::new(FulfillResponse::GiftUrl {
        url: "https://x.com/g".into(),
    });
    let app = steam_router(Arc::clone(&store), mock.clone(), &server.uri());

    // Request the owned proxy with a non-existent token.
    let proxy_req = Request::get(format!("/api/l/{CTX_TOKEN}/steam/owned/{TEST_STEAMID}"))
        .body(Body::empty())
        .unwrap();
    let proxy_resp = app.clone().oneshot(proxy_req).await.unwrap();

    // Request standard unknown-link 404.
    let link_req = Request::get(format!("/api/l/{CTX_TOKEN}"))
        .body(Body::empty())
        .unwrap();
    let link_resp = app.clone().oneshot(link_req).await.unwrap();

    assert_eq!(proxy_resp.status(), StatusCode::NOT_FOUND);
    assert_eq!(link_resp.status(), StatusCode::NOT_FOUND);

    let proxy_bytes = axum::body::to_bytes(proxy_resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let link_bytes = axum::body::to_bytes(link_resp.into_body(), usize::MAX)
        .await
        .unwrap();
    assert_eq!(
        proxy_bytes, link_bytes,
        "owned proxy unknown-token 404 must be byte-identical to standard unknown-link 404"
    );
}

/// Owned proxy: fresh cache served without hitting Steam (wiremock expect(0) discipline).
/// Stale cache: exactly one Steam fetch, cache refreshed.
#[tokio::test]
async fn owned_proxy_serves_cache_then_fetches_on_stale() {
    let Some(store) = store_or_skip("owned-proxy-cache").await else {
        return;
    };
    let server = wiremock::MockServer::start().await;

    // Register owned-games mock with expect(1) — exactly one call for the stale phase.
    wiremock::Mock::given(wiremock::matchers::method("GET"))
        .and(wiremock::matchers::path(
            "/IPlayerService/GetOwnedGames/v0001/",
        ))
        .respond_with(wiremock::ResponseTemplate::new(200).set_body_string(
            r#"{"response":{"game_count":2,"games":[{"appid":730},{"appid":440}]}}"#,
        ))
        .expect(1)
        .mount(&server)
        .await;

    let mock = MockInvoker::new(FulfillResponse::GiftUrl {
        url: "https://x.com/g".into(),
    });
    // Seed a live link.
    let token = CTX_TOKEN;
    let lnk = test_link(token);
    store.create_link(&lnk).await.unwrap();

    // Phase 1: seed a FRESH cache — no Steam call expected.
    let now_epoch = time::OffsetDateTime::now_utc().unix_timestamp();
    let fresh_at = now_epoch - 3600; // 1 hour ago
    store
        .put_steam_owned(TEST_STEAMID, &[12345], fresh_at)
        .await
        .unwrap();

    let app = steam_router(Arc::clone(&store), mock.clone(), &server.uri());
    let resp = app
        .clone()
        .oneshot(
            Request::get(format!("/api/l/{token}/steam/owned/{TEST_STEAMID}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let j = body_json(resp).await;
    assert!(
        j["appids"]
            .as_array()
            .unwrap()
            .contains(&serde_json::json!(12345)),
        "fresh cache must be served: {j}"
    );

    // Phase 2: make cache stale, force a fetch.
    let stale_at = now_epoch - (25 * 3600);
    store
        .put_steam_owned(TEST_STEAMID, &[99999], stale_at)
        .await
        .unwrap();

    let resp2 = app
        .clone()
        .oneshot(
            Request::get(format!("/api/l/{token}/steam/owned/{TEST_STEAMID}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp2.status(), StatusCode::OK);
    let j2 = body_json(resp2).await;
    let appids2 = j2["appids"].as_array().unwrap();
    assert!(
        appids2.contains(&serde_json::json!(730)),
        "stale cache must trigger fetch: {j2}"
    );

    server.verify().await;
}

/// Steam return: valid assertion → 302 Location `{ctx}#steam=<id64>&persona=<urlencoded>`.
#[tokio::test]
async fn steam_return_valid_redirects_with_fragment() {
    let Some(store) = store_or_skip("steam-return-valid").await else {
        return;
    };
    let server = wiremock::MockServer::start().await;

    // check_authentication mock: is_valid:true
    wiremock::Mock::given(wiremock::matchers::method("POST"))
        .and(wiremock::matchers::path("/openid/login"))
        .and(wiremock::matchers::body_string_contains(
            "openid.mode=check_authentication",
        ))
        .respond_with(
            wiremock::ResponseTemplate::new(200)
                .set_body_string("ns:http://specs.openid.net/auth/2.0\nis_valid:true\n"),
        )
        .mount(&server)
        .await;

    // Persona mock.
    wiremock::Mock::given(wiremock::matchers::method("GET"))
        .and(wiremock::matchers::path("/ISteamUser/GetPlayerSummaries/v0002/"))
        .respond_with(wiremock::ResponseTemplate::new(200).set_body_string(
            r#"{"response":{"players":[{"steamid":"76561198000000001","personaname":"bendoerr","avatarfull":null}]}}"#,
        ))
        .mount(&server)
        .await;

    let mock = MockInvoker::new(FulfillResponse::GiftUrl {
        url: "https://x.com/g".into(),
    });
    let app = steam_router(Arc::clone(&store), mock.clone(), &server.uri());

    let ctx = format!("/l/{CTX_TOKEN}");
    let expected_return_to = format!(
        "{TEST_BASE_URL}/api/steam/return?ctx={}",
        urlencoding::encode(&ctx)
    );

    // Build the assertion params with the expected_return_to.
    let params = assertion_params(
        &format!("https://steamcommunity.com/openid/id/{TEST_STEAMID}"),
        &expected_return_to,
    );

    // Build query string for GET /api/steam/return
    let mut qs = format!("ctx={}", urlencoding::encode(&ctx));
    for (k, v) in &params {
        qs.push('&');
        qs.push_str(&urlencoding::encode(k));
        qs.push('=');
        qs.push_str(&urlencoding::encode(v));
    }

    let resp = app
        .oneshot(
            Request::get(format!("/api/steam/return?{qs}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::FOUND, "must redirect");
    let loc = resp.headers().get("location").unwrap().to_str().unwrap();
    assert!(
        loc.starts_with(&format!("/l/{CTX_TOKEN}#steam=")),
        "location must start with ctx + #steam=; got: {loc}"
    );
    assert!(
        loc.contains(TEST_STEAMID),
        "location must contain steamid; got: {loc}"
    );
    assert!(
        loc.contains("persona=bendoerr"),
        "location must contain persona=bendoerr; got: {loc}"
    );
}

/// Steam return: bad ctx → 302 `/` (no fragment).
#[tokio::test]
async fn steam_return_bad_ctx_redirects_root_no_fragment() {
    let Some(store) = store_or_skip("steam-return-bad-ctx").await else {
        return;
    };
    let server = wiremock::MockServer::start().await;
    let mock = MockInvoker::new(FulfillResponse::GiftUrl {
        url: "https://x.com/g".into(),
    });
    let app = steam_router(Arc::clone(&store), mock.clone(), &server.uri());

    let resp = app
        .oneshot(
            Request::get("/api/steam/return?ctx=%2Fevil")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::FOUND);
    let loc = resp.headers().get("location").unwrap().to_str().unwrap();
    assert_eq!(loc, "/", "bad ctx must redirect to /; got: {loc}");
}

/// Steam return: is_valid:false → 302 `{ctx}#steam_error=verify_failed`.
#[tokio::test]
async fn steam_return_invalid_assertion_gets_steam_error_fragment() {
    let Some(store) = store_or_skip("steam-return-invalid").await else {
        return;
    };
    let server = wiremock::MockServer::start().await;

    // check_authentication: is_valid:false
    wiremock::Mock::given(wiremock::matchers::method("POST"))
        .and(wiremock::matchers::path("/openid/login"))
        .and(wiremock::matchers::body_string_contains(
            "openid.mode=check_authentication",
        ))
        .respond_with(
            wiremock::ResponseTemplate::new(200)
                .set_body_string("ns:http://specs.openid.net/auth/2.0\nis_valid:false\n"),
        )
        .mount(&server)
        .await;

    let mock = MockInvoker::new(FulfillResponse::GiftUrl {
        url: "https://x.com/g".into(),
    });
    let app = steam_router(Arc::clone(&store), mock.clone(), &server.uri());

    let ctx = format!("/l/{CTX_TOKEN}");
    let expected_return_to = format!(
        "{TEST_BASE_URL}/api/steam/return?ctx={}",
        urlencoding::encode(&ctx)
    );

    let params = assertion_params(
        &format!("https://steamcommunity.com/openid/id/{TEST_STEAMID}"),
        &expected_return_to,
    );

    let mut qs = format!("ctx={}", urlencoding::encode(&ctx));
    for (k, v) in &params {
        qs.push('&');
        qs.push_str(&urlencoding::encode(k));
        qs.push('=');
        qs.push_str(&urlencoding::encode(v));
    }

    let resp = app
        .oneshot(
            Request::get(format!("/api/steam/return?{qs}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::FOUND);
    let loc = resp.headers().get("location").unwrap().to_str().unwrap();
    assert!(
        loc.contains("#steam_error=verify_failed"),
        "invalid assertion must get #steam_error=verify_failed; got: {loc}"
    );
    assert!(
        loc.starts_with(&format!("/l/{CTX_TOKEN}")),
        "must redirect back to ctx; got: {loc}"
    );
}

/// GameView carries steam_app_id field.
#[tokio::test]
async fn game_view_carries_steam_app_id() {
    let Some(store) = store_or_skip("game-view-appid").await else {
        return;
    };
    let mut g = test_game(1);
    g.steam_app_id = Some(413150);
    store.put_game(&g).await.unwrap();
    let lnk = test_link("appid-tok");
    store.create_link(&lnk).await.unwrap();

    let mock = MockInvoker::new(FulfillResponse::GiftUrl {
        url: "https://x.com/g".into(),
    });
    let req = Request::get("/api/l/appid-tok")
        .body(Body::empty())
        .unwrap();
    let resp = plain_router(Arc::clone(&store), mock.clone())
        .oneshot(req)
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let j = body_json(resp).await;
    let games = j["games"].as_array().unwrap();
    assert_eq!(games.len(), 1, "must have one game");
    assert_eq!(
        games[0]["steam_app_id"], 413150,
        "GameView must carry steam_app_id; got: {}",
        games[0]
    );
}

// ── I1: steamid validation on public owned proxy ──────────────────────────────

/// I1-RED: live link + 16-digit steamid → 400 (must fail before fix).
/// Security invariant 8: steamid must be validated as exactly 17 ASCII digits.
#[tokio::test]
async fn owned_proxy_steamid_16digit_gets_400() {
    let Some(store) = store_or_skip("owned-proxy-i1-16digit").await else {
        return;
    };
    let server = wiremock::MockServer::start().await;
    let mock = MockInvoker::new(FulfillResponse::GiftUrl {
        url: "https://x.com/g".into(),
    });
    // Seed a live (active) link.
    store.create_link(&test_link(CTX_TOKEN)).await.unwrap();
    let app = steam_router(Arc::clone(&store), mock, &server.uri());

    let resp = app
        .oneshot(
            Request::get(format!("/api/l/{CTX_TOKEN}/steam/owned/1234567890123456"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(
        resp.status(),
        StatusCode::BAD_REQUEST,
        "16-digit steamid on live link must get 400"
    );
    let j = body_json(resp).await;
    assert_eq!(
        j["error"], "steamid must be exactly 17 ASCII digits",
        "error message must match admin twin: {j}"
    );
}

/// I1-RED: live link + steamid containing non-digit → 400.
#[tokio::test]
async fn owned_proxy_steamid_nondigit_gets_400() {
    let Some(store) = store_or_skip("owned-proxy-i1-nondigit").await else {
        return;
    };
    let server = wiremock::MockServer::start().await;
    let mock = MockInvoker::new(FulfillResponse::GiftUrl {
        url: "https://x.com/g".into(),
    });
    store.create_link(&test_link(CTX_TOKEN)).await.unwrap();
    let app = steam_router(Arc::clone(&store), mock, &server.uri());

    let resp = app
        .oneshot(
            Request::get(format!("/api/l/{CTX_TOKEN}/steam/owned/7656119800000000x"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(
        resp.status(),
        StatusCode::BAD_REQUEST,
        "non-digit steamid on live link must get 400"
    );
    let j = body_json(resp).await;
    assert_eq!(
        j["error"], "steamid must be exactly 17 ASCII digits",
        "error message must match admin twin: {j}"
    );
}

/// I1-ordering-pin: unknown token + bad steamid → byte-identical 404 (not 400).
/// The token liveness check must run BEFORE steamid validation — a bad steamid
/// on a dead/unknown token must never reveal that the steamid was rejected.
#[tokio::test]
async fn owned_proxy_unknown_token_bad_steamid_is_still_404() {
    let Some(store) = store_or_skip("owned-proxy-i1-order").await else {
        return;
    };
    let server = wiremock::MockServer::start().await;
    let mock = MockInvoker::new(FulfillResponse::GiftUrl {
        url: "https://x.com/g".into(),
    });
    // No link seeded — unknown token.
    let app = steam_router(Arc::clone(&store), mock.clone(), &server.uri());

    // Request with a KNOWN-BAD steamid (16 digits) but unknown token.
    let proxy_req = Request::get(format!("/api/l/{CTX_TOKEN}/steam/owned/1234567890123456"))
        .body(Body::empty())
        .unwrap();
    let proxy_resp = app.clone().oneshot(proxy_req).await.unwrap();

    // Request the standard unknown-link 404 for byte comparison.
    let link_req = Request::get(format!("/api/l/{CTX_TOKEN}"))
        .body(Body::empty())
        .unwrap();
    let link_resp = app.clone().oneshot(link_req).await.unwrap();

    assert_eq!(
        proxy_resp.status(),
        StatusCode::NOT_FOUND,
        "unknown token + bad steamid must still return 404, not 400"
    );
    assert_eq!(link_resp.status(), StatusCode::NOT_FOUND);

    let proxy_bytes = axum::body::to_bytes(proxy_resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let link_bytes = axum::body::to_bytes(link_resp.into_body(), usize::MAX)
        .await
        .unwrap();
    assert_eq!(
        proxy_bytes, link_bytes,
        "unknown-token + bad-steamid 404 must be byte-identical to standard unknown-link 404 \
         (ordering proof: token gate fires before steamid validation)"
    );
}

// ── I2: preserve duplicate query params so DUP_GUARD fires ────────────────────

/// I2-RED: a genuine-shaped assertion with a SECOND `openid.claimed_id` appended
/// must be rejected with `#steam_error=verify_failed` (DUP_GUARD firing through
/// the endpoint), NOT succeed. WireMock answers is_valid:true — rejection must
/// happen locally before the Steam roundtrip.
#[tokio::test]
async fn steam_return_duplicate_claimed_id_gets_verify_failed() {
    let Some(store) = store_or_skip("steam-return-i2-dup").await else {
        return;
    };
    let server = wiremock::MockServer::start().await;

    // Mount check_authentication to answer is_valid:true — so any success would
    // mean the DUP_GUARD is dead; we want to prove it fires BEFORE this call.
    wiremock::Mock::given(wiremock::matchers::method("POST"))
        .and(wiremock::matchers::path("/openid/login"))
        .and(wiremock::matchers::body_string_contains(
            "openid.mode=check_authentication",
        ))
        .respond_with(
            wiremock::ResponseTemplate::new(200)
                .set_body_string("ns:http://specs.openid.net/auth/2.0\nis_valid:true\n"),
        )
        .expect(0) // DUP_GUARD must reject locally — Steam must NOT be called.
        .mount(&server)
        .await;

    let mock = MockInvoker::new(FulfillResponse::GiftUrl {
        url: "https://x.com/g".into(),
    });
    let app = steam_router(Arc::clone(&store), mock.clone(), &server.uri());

    let ctx = format!("/l/{CTX_TOKEN}");
    let expected_return_to = format!(
        "{TEST_BASE_URL}/api/steam/return?ctx={}",
        urlencoding::encode(&ctx)
    );

    // Build the standard assertion params.
    let params = assertion_params(
        &format!("https://steamcommunity.com/openid/id/{TEST_STEAMID}"),
        &expected_return_to,
    );

    // Build query string with a DUPLICATE openid.claimed_id injected at the end.
    let mut qs = format!("ctx={}", urlencoding::encode(&ctx));
    for (k, v) in &params {
        qs.push('&');
        qs.push_str(&urlencoding::encode(k));
        qs.push('=');
        qs.push_str(&urlencoding::encode(v));
    }
    // Inject a second claimed_id (attacker's forgery attempt).
    qs.push_str(
        "&openid.claimed_id=https%3A%2F%2Fsteamcommunity.com%2Fopenid%2Fid%2F99999999999999999",
    );

    let resp = app
        .oneshot(
            Request::get(format!("/api/steam/return?{qs}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::FOUND, "must redirect");
    let loc = resp.headers().get("location").unwrap().to_str().unwrap();
    assert!(
        loc.contains("#steam_error=verify_failed"),
        "duplicate claimed_id must fire the DUP_GUARD → verify_failed; got: {loc}"
    );
    assert!(
        loc.starts_with(&format!("/l/{CTX_TOKEN}")),
        "must redirect back to ctx; got: {loc}"
    );

    // Verify Steam was NOT called (DUP_GUARD rejected locally).
    server.verify().await;
}

/// Login endpoint rejects a bad ctx (allowlist enforced initiation-side too).
#[tokio::test]
async fn steam_login_rejects_bad_ctx() {
    let Some(store) = store_or_skip("steam-login-bad-ctx").await else {
        return;
    };
    let server = wiremock::MockServer::start().await;
    let mock = MockInvoker::new(FulfillResponse::GiftUrl {
        url: "https://x.com/g".into(),
    });
    let app = steam_router(Arc::clone(&store), mock.clone(), &server.uri());

    for bad_ctx in [
        "/evil",
        "/l/",
        "/l/tooshort",
        "/l/UPPERCASE1234567890123456789012345678901234567890123456789012",
    ] {
        let resp = app
            .clone()
            .oneshot(
                Request::get(format!(
                    "/api/steam/login?ctx={}",
                    urlencoding::encode(bad_ctx)
                ))
                .body(Body::empty())
                .unwrap(),
            )
            .await
            .unwrap();

        let loc = resp.headers().get("location").unwrap().to_str().unwrap();
        assert_eq!(
            loc, "/",
            "bad ctx '{bad_ctx}' must redirect to /; got: {loc}"
        );
    }
}
