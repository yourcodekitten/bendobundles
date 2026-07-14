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
use domain::{Claim, ClaimState, Game, GameStatus, Link, game_id};
use dynamo::{SteamAppCache, Store};
use fulfillment::{FulfillRequest, FulfillResponse};
use public_api::{Invoker, router};
use steam_client::{RecentReviews, ReviewSummary, SteamApiKey, SteamAppDetail, SteamClient};
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
        hidden_source: None,
    }
}

fn test_link(token: &str) -> Link {
    Link {
        token: token.into(),
        label: "Test Friend".into(),
        gift_note: None,
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

/// gift_note passes through to the friend view when set, and is OMITTED from the
/// JSON (not null) when unset — the client gates the note dialog on field presence.
#[tokio::test]
async fn link_view_carries_gift_note_and_omits_when_unset() {
    let Some(store) = store_or_skip("gift-note-link").await else {
        return;
    };
    let mut noted = test_link("note-tok");
    noted.gift_note = Some("picked these with you in mind ♡".into());
    store.create_link(&noted).await.unwrap();
    store.create_link(&test_link("plain-tok")).await.unwrap();

    let mock = MockInvoker::new(FulfillResponse::GiftUrl {
        url: "https://x.com/g".into(),
    });

    let req = Request::get("/api/l/note-tok").body(Body::empty()).unwrap();
    let resp = plain_router(Arc::clone(&store), mock.clone())
        .oneshot(req)
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let j = body_json(resp).await;
    assert_eq!(j["gift_note"], "picked these with you in mind ♡");

    let req = Request::get("/api/l/plain-tok")
        .body(Body::empty())
        .unwrap();
    let resp = plain_router(Arc::clone(&store), mock.clone())
        .oneshot(req)
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let j = body_json(resp).await;
    assert!(
        j.get("gift_note").is_none(),
        "unset gift_note must be omitted, got: {j}"
    );
}

/// A dead (revoked/expired) link must not serve the gift note: the note is
/// strictly more personal than the catalog, and the catalog is already hidden
/// for exactly these states. Revoking a leaked URL has to take the personal
/// message with it.
#[tokio::test]
async fn dead_link_omits_gift_note() {
    let Some(store) = store_or_skip("dead-note-link").await else {
        return;
    };
    let mut revoked = test_link("revoked-note-tok");
    revoked.gift_note = Some("just for you ♡".into());
    revoked.revoked = true;
    store.create_link(&revoked).await.unwrap();
    let mut expired = test_link("expired-note-tok");
    expired.gift_note = Some("just for you ♡".into());
    expired.expires_at = Some(datetime!(2020-01-01 00:00 UTC));
    store.create_link(&expired).await.unwrap();

    let mock = MockInvoker::new(FulfillResponse::GiftUrl {
        url: "https://x.com/g".into(),
    });

    for (token, state) in [
        ("revoked-note-tok", "revoked"),
        ("expired-note-tok", "expired"),
    ] {
        let req = Request::get(format!("/api/l/{token}"))
            .body(Body::empty())
            .unwrap();
        let resp = plain_router(Arc::clone(&store), mock.clone())
            .oneshot(req)
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let j = body_json(resp).await;
        assert_eq!(j["state"], state);
        assert!(
            j.get("gift_note").is_none(),
            "{state} link must not serve the gift note, got: {j}"
        );
    }
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
    lnk.gift_note = Some("you got them all ♡".into());
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

// ── C1: admin/ops ctx allowlist bridging tests ────────────────────────────────

/// C1-GREEN: login with ctx=/admin/ops → 302 to Steam (not to /).
/// The allowlist must accept `/admin/<subroute>` — the fix that was missing.
#[tokio::test]
async fn steam_login_admin_ops_ctx_accepted() {
    let Some(store) = store_or_skip("steam-login-admin-ops").await else {
        return;
    };
    let server = wiremock::MockServer::start().await;
    let mock = MockInvoker::new(FulfillResponse::GiftUrl {
        url: "https://x.com/g".into(),
    });
    let app = steam_router(Arc::clone(&store), mock.clone(), &server.uri());

    let resp = app
        .oneshot(
            Request::get("/api/steam/login?ctx=%2Fadmin%2Fops")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::FOUND, "admin/ops ctx must 302");
    let loc = resp.headers().get("location").unwrap().to_str().unwrap();
    assert!(
        loc.contains("steamcommunity.com") || loc.contains("steampowered.com"),
        "ctx=/admin/ops must redirect to Steam, not to /; got: {loc}"
    );
}

/// C1-GREEN: return endpoint with valid assertion and ctx=/admin/ops →
/// 302 to /admin/ops#steam=... (not to /).
#[tokio::test]
async fn steam_return_admin_ops_ctx_valid_assertion() {
    let Some(store) = store_or_skip("steam-return-admin-ops").await else {
        return;
    };
    let server = wiremock::MockServer::start().await;

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

    let ctx = "/admin/ops";
    let expected_return_to = format!(
        "{TEST_BASE_URL}/api/steam/return?ctx={}",
        urlencoding::encode(ctx)
    );

    let params = assertion_params(
        &format!("https://steamcommunity.com/openid/id/{TEST_STEAMID}"),
        &expected_return_to,
    );

    let mut qs = format!("ctx={}", urlencoding::encode(ctx));
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
        loc.starts_with("/admin/ops#steam="),
        "return with ctx=/admin/ops must redirect to /admin/ops#steam=...; got: {loc}"
    );
    assert!(
        loc.contains(TEST_STEAMID),
        "location must contain steamid; got: {loc}"
    );
}

/// FIX 6: handle_steam_login with steam=None must redirect to `{ctx}#steam_error=steam_unreachable`
/// instead of forwarding to Steam and then returning a 503.
/// RED: before FIX 6, handle_steam_login redirected to steamcommunity.com even with steam=None.
#[tokio::test]
async fn steam_login_unconfigured_redirects_to_steam_unreachable_fragment() {
    let Some(store) = store_or_skip("steam-login-none").await else {
        return;
    };
    let mock = MockInvoker::new(FulfillResponse::GiftUrl {
        url: "https://x.com/g".into(),
    });
    // plain_router has steam=None
    let app = plain_router(Arc::clone(&store), mock);

    // Use a valid ctx so the ctx allowlist check passes.
    let ctx = "/l/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    let resp = app
        .oneshot(
            Request::get(format!("/api/steam/login?ctx={}", urlencoding::encode(ctx)))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::FOUND);
    let loc = resp.headers().get("location").unwrap().to_str().unwrap();
    assert!(
        loc.contains("#steam_error=steam_unreachable"),
        "unconfigured steam must redirect to ctx#steam_error=steam_unreachable; got: {loc}"
    );
    assert!(
        !loc.contains("steamcommunity.com"),
        "must NOT redirect to Steam when steam client is None; got: {loc}"
    );
}

// ── Task 4: Game detail endpoint tests ───────────────────────────────────────

/// Build a minimal SteamAppCache for seeding in tests.
fn test_steam_cache(app_id: u32) -> SteamAppCache {
    SteamAppCache {
        app_id,
        detail: Some(SteamAppDetail {
            app_id,
            name: format!("Test Game {app_id}"),
            developers: vec!["Dev Inc".into()],
            publishers: vec!["Pub Ltd".into()],
            genres: vec!["Action".into()],
            release_date: None,
            short_description: "A test game for detail tests.".into(),
            header_image: None,
            video_hls_url: None,
            video_thumbnail: None,
            screenshots: vec![],
            tags: vec![],
            content_descriptor_ids: vec![],
            content_notes: None,
        }),
        overall: Some(ReviewSummary {
            desc: "Mostly Positive".into(),
            total_positive: 100,
            total_negative: 20,
            total_reviews: 120,
        }),
        recent: Some(RecentReviews {
            percent_positive: 83,
            count: 50,
        }),
        fetched_at: 1_700_000_000,
        reviews_fetched_at: 1_700_000_000,
    }
}

/// GET /api/l/:token/games/:id/detail — listable game with steam cache → 200.
/// The response carries `game` with the friend-visible fields and a `steam` object
/// with detail/overall/recent all populated.
#[tokio::test]
async fn game_detail_listable_200_with_steam_blob() {
    let uid = uuid::Uuid::new_v4().simple().to_string();
    let Some(store) = store_or_skip(&format!("gdl{}", &uid[..10])).await else {
        return;
    };
    let mut g = test_game(50);
    g.steam_app_id = Some(99001);
    let gid = g.id.clone();
    store.put_game(&g).await.unwrap();
    store.put_steam_app(&test_steam_cache(99001)).await.unwrap();

    let tok = format!("gdl{}", &uid[..28]);
    let lnk = test_link(&tok);
    store.create_link(&lnk).await.unwrap();

    let mock = MockInvoker::new(FulfillResponse::GiftUrl {
        url: "https://x.com/g".into(),
    });
    let req = Request::get(format!("/api/l/{tok}/games/{gid}/detail"))
        .body(Body::empty())
        .unwrap();
    let resp = plain_router(Arc::clone(&store), mock)
        .oneshot(req)
        .await
        .unwrap();

    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "listable game must return 200"
    );
    let j = body_json(resp).await;

    // game object has the friend-visible shape
    assert_eq!(j["game"]["id"], gid);
    assert_eq!(j["game"]["title"], "Game 50");
    assert!(
        j["game"].get("bundle").is_some(),
        "game.bundle must be present"
    );
    assert!(
        j["game"].get("key_type").is_some(),
        "game.key_type must be present"
    );
    assert_eq!(j["game"]["steam_app_id"], 99001);

    // steam blob present and populated
    assert!(
        !j["steam"].is_null(),
        "steam must not be null for a mapped game with cache"
    );
    assert!(
        !j["steam"]["detail"].is_null(),
        "steam.detail must be present"
    );
    // screenshots key rides the wire (issue #61) — a serde skip attr would drop it silently
    assert!(
        j["steam"]["detail"]["screenshots"].is_array(),
        "steam.detail.screenshots must be serialized as an array"
    );
    assert_eq!(j["steam"]["overall"]["desc"], "Mostly Positive");
    assert_eq!(j["steam"]["recent"]["percent_positive"], 83);

    // must NOT leak timestamps or order-key material
    assert!(
        j["steam"].get("fetched_at").is_none(),
        "fetched_at must not leak"
    );
    assert!(j["game"].get("gamekey").is_none(), "gamekey must not leak");
}

/// Hidden game → 404 byte-identical to unknown-token 404 (no oracle).
/// The detail endpoint never reveals WHY access was denied.
#[tokio::test]
async fn game_detail_hidden_game_404_byte_identical() {
    let uid = uuid::Uuid::new_v4().simple().to_string();
    let Some(store) = store_or_skip(&format!("gdh{}", &uid[..10])).await else {
        return;
    };
    let mut g = test_game(51);
    g.hidden = true; // hidden → not listable
    let gid = g.id.clone();
    store.put_game(&g).await.unwrap();

    let tok = format!("gdh{}", &uid[..28]);
    let lnk = test_link(&tok);
    store.create_link(&lnk).await.unwrap();

    let mock = MockInvoker::new(FulfillResponse::GiftUrl {
        url: "https://x.com/g".into(),
    });

    // detail request for hidden game
    let detail_req = Request::get(format!("/api/l/{tok}/games/{gid}/detail"))
        .body(Body::empty())
        .unwrap();
    let detail_resp = plain_router(Arc::clone(&store), mock.clone())
        .oneshot(detail_req)
        .await
        .unwrap();

    // reference: unknown-token 404 (unknown token — never stored)
    let ref_tok = format!("ref{}", &uid[..28]);
    let ref_req2 = Request::get(format!("/api/l/{ref_tok}"))
        .body(Body::empty())
        .unwrap();
    let ref_resp = plain_router(Arc::clone(&store), mock.clone())
        .oneshot(ref_req2)
        .await
        .unwrap();

    assert_eq!(
        detail_resp.status(),
        StatusCode::NOT_FOUND,
        "hidden game must yield 404"
    );
    assert_eq!(ref_resp.status(), StatusCode::NOT_FOUND);

    let detail_bytes = axum::body::to_bytes(detail_resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let ref_bytes = axum::body::to_bytes(ref_resp.into_body(), usize::MAX)
        .await
        .unwrap();
    assert_eq!(
        detail_bytes, ref_bytes,
        "hidden-game 404 must be byte-identical to unknown-token 404 (no oracle)"
    );
}

/// Game in THIS link's claims history (but currently not listable) → 200.
/// A friend who previously claimed a game (now Gifted/non-listable) can still view its detail.
#[tokio::test]
async fn game_detail_claimed_by_this_link_200() {
    let uid = uuid::Uuid::new_v4().simple().to_string();
    let Some(store) = store_or_skip(&format!("gdcl{}", &uid[..10])).await else {
        return;
    };
    // Gifted game — not listable
    let mut g = test_game(52);
    g.status = GameStatus::Gifted;
    let gid = g.id.clone();
    store.put_game(&g).await.unwrap();

    let tok = format!("gdcl{}", &uid[..27]);
    let lnk = test_link(&tok);
    store.create_link(&lnk).await.unwrap();

    // Seed a claim under THIS link for this game
    let claim_id = format!("cl{}", &uid[..10]);
    store
        .put_claim(&Claim {
            id: claim_id,
            link_token: tok.clone(),
            game_id: gid.clone(),
            state: ClaimState::Fulfilled,
            gift_url: Some("https://humble.com/g".into()),
            created_at: datetime!(2026-07-07 00:00 UTC),
            choice_pre_tpks: None,
            revealed_key: None,
        })
        .await
        .unwrap();

    let mock = MockInvoker::new(FulfillResponse::GiftUrl {
        url: "https://x.com/g".into(),
    });
    let req = Request::get(format!("/api/l/{tok}/games/{gid}/detail"))
        .body(Body::empty())
        .unwrap();
    let resp = plain_router(Arc::clone(&store), mock)
        .oneshot(req)
        .await
        .unwrap();

    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "game in this link's claims history must return 200"
    );
    let j = body_json(resp).await;
    assert_eq!(j["game"]["id"], gid);
    // steam_app_id was None → steam: null
    assert!(
        j["steam"].is_null(),
        "game with no steam_app_id must give steam: null"
    );
}

/// Game claimed by a DIFFERENT link → 404 (no-oracle, not in this link's history).
#[tokio::test]
async fn game_detail_other_links_claimed_game_404() {
    let uid = uuid::Uuid::new_v4().simple().to_string();
    let Some(store) = store_or_skip(&format!("gdo{}", &uid[..10])).await else {
        return;
    };
    // Gifted game — not listable
    let mut g = test_game(53);
    g.status = GameStatus::Gifted;
    let gid = g.id.clone();
    store.put_game(&g).await.unwrap();

    // Link A: the one we'll query (no claims)
    let tok_a = format!("gdoa{}", &uid[..27]);
    store.create_link(&test_link(&tok_a)).await.unwrap();
    // Link B: the one that has the claim
    let tok_b = format!("gdob{}", &uid[..27]);
    store.create_link(&test_link(&tok_b)).await.unwrap();

    // Claim under link B
    let claim_id = format!("co{}", &uid[..10]);
    store
        .put_claim(&Claim {
            id: claim_id,
            link_token: tok_b.clone(),
            game_id: gid.clone(),
            state: ClaimState::Fulfilled,
            gift_url: Some("https://humble.com/g".into()),
            created_at: datetime!(2026-07-07 00:00 UTC),
            choice_pre_tpks: None,
            revealed_key: None,
        })
        .await
        .unwrap();

    let mock = MockInvoker::new(FulfillResponse::GiftUrl {
        url: "https://x.com/g".into(),
    });

    // Query link A → 404
    let detail_req = Request::get(format!("/api/l/{tok_a}/games/{gid}/detail"))
        .body(Body::empty())
        .unwrap();
    let detail_resp = plain_router(Arc::clone(&store), mock.clone())
        .oneshot(detail_req)
        .await
        .unwrap();

    // Reference: unknown-token 404 (a token that was never stored)
    let ref_tok = format!("ref{}", &uid[..28]);
    let ref_req = Request::get(format!("/api/l/{ref_tok}"))
        .body(Body::empty())
        .unwrap();
    let ref_resp = plain_router(Arc::clone(&store), mock.clone())
        .oneshot(ref_req)
        .await
        .unwrap();

    assert_eq!(
        detail_resp.status(),
        StatusCode::NOT_FOUND,
        "other-link claimed game must yield 404 on this link"
    );
    let detail_bytes = axum::body::to_bytes(detail_resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let ref_bytes = axum::body::to_bytes(ref_resp.into_body(), usize::MAX)
        .await
        .unwrap();
    assert_eq!(
        detail_bytes, ref_bytes,
        "other-link 404 must be byte-identical to unknown-token 404"
    );
}

/// Listable game with no steam_app_id → 200 with `steam: null`.
#[tokio::test]
async fn game_detail_unmapped_game_steam_null() {
    let uid = uuid::Uuid::new_v4().simple().to_string();
    let Some(store) = store_or_skip(&format!("gdna{}", &uid[..10])).await else {
        return;
    };
    // steam_app_id = None (unmapped)
    let g = test_game(54);
    let gid = g.id.clone();
    store.put_game(&g).await.unwrap();

    let tok = format!("gdna{}", &uid[..27]);
    let lnk = test_link(&tok);
    store.create_link(&lnk).await.unwrap();

    let mock = MockInvoker::new(FulfillResponse::GiftUrl {
        url: "https://x.com/g".into(),
    });
    let req = Request::get(format!("/api/l/{tok}/games/{gid}/detail"))
        .body(Body::empty())
        .unwrap();
    let resp = plain_router(Arc::clone(&store), mock)
        .oneshot(req)
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let j = body_json(resp).await;
    assert_eq!(j["game"]["id"], gid);
    assert!(j["steam"].is_null(), "unmapped game must yield steam: null");
}

/// C1-REJECT: admin subroute rejections — the widening must not enable open redirect.
/// `/admin//evil`, `/admin/ops/x`, `/admin/Ops`, `/admin/ops2`, `/admin/../etc` all → /.
#[tokio::test]
async fn steam_login_admin_subroute_rejections() {
    let Some(store) = store_or_skip("steam-login-admin-reject").await else {
        return;
    };
    let server = wiremock::MockServer::start().await;
    let mock = MockInvoker::new(FulfillResponse::GiftUrl {
        url: "https://x.com/g".into(),
    });
    let app = steam_router(Arc::clone(&store), mock.clone(), &server.uri());

    for bad_ctx in [
        "/admin//evil",
        "/admin/ops/x",
        "/admin/Ops",
        "/admin/ops2",
        "/admin/../etc",
        "/admin/",
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
            "bad admin ctx '{bad_ctx}' must redirect to /; got: {loc}"
        );
    }
}

/// GET /api/l/:token — games in the list payload carry `genres` from the steam
/// cache (first 5, cache-only), games without an appid omit the key entirely,
/// and the detail endpoint's `game` object stays wire-identical (no `genres`).
#[tokio::test]
async fn link_list_carries_genres_from_steam_cache() {
    let uid = uuid::Uuid::new_v4().simple().to_string();
    let Some(store) = store_or_skip(&format!("gnr{}", &uid[..10])).await else {
        return;
    };

    // game A: steam appid + warm cache with 6 genres (proves the 5-cap)
    let mut a = test_game(60);
    a.steam_app_id = Some(99101);
    let aid = a.id.clone();
    store.put_game(&a).await.unwrap();
    let mut cache = test_steam_cache(99101);
    cache.detail.as_mut().unwrap().genres = vec![
        "Action".into(),
        "Indie".into(),
        "Platformer".into(),
        "Adventure".into(),
        "Casual".into(),
        "Sports".into(),
    ];
    store.put_steam_app(&cache).await.unwrap();

    // game B: no steam appid → genres key must be absent
    let b = test_game(61);
    let bid = b.id.clone();
    store.put_game(&b).await.unwrap();

    // game C: appid but cache-cold (no put_steam_app) → genres key absent too
    let mut c = test_game(62);
    c.steam_app_id = Some(99102);
    let cid = c.id.clone();
    store.put_game(&c).await.unwrap();

    let tok = format!("gnr{}", &uid[..28]);
    store.create_link(&test_link(&tok)).await.unwrap();

    let mock = MockInvoker::new(FulfillResponse::GiftUrl {
        url: "https://x.com/g".into(),
    });
    let req = Request::get(format!("/api/l/{tok}"))
        .body(Body::empty())
        .unwrap();
    let resp = plain_router(Arc::clone(&store), mock.clone())
        .oneshot(req)
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let j = body_json(resp).await;

    let games = j["games"].as_array().expect("games must be an array");
    let ga = games
        .iter()
        .find(|g| g["id"] == aid.as_str())
        .expect("game A must be in the list");
    let gb = games
        .iter()
        .find(|g| g["id"] == bid.as_str())
        .expect("game B must be in the list");

    assert_eq!(
        ga["genres"],
        serde_json::json!(["Action", "Indie", "Platformer", "Adventure", "Casual"]),
        "cache-warm game carries the first 5 genres, in cache order"
    );
    assert!(
        gb.get("genres").is_none(),
        "game without appid must omit the genres key entirely"
    );
    let gc = games
        .iter()
        .find(|g| g["id"] == cid.as_str())
        .expect("game C must be in the list");
    assert!(
        gc.get("genres").is_none(),
        "appid with a cold cache must degrade to no genres key (best-effort)"
    );

    // detail endpoint wire shape unchanged: game object has NO genres key,
    // and the modal still reads the full steam blob.
    let dreq = Request::get(format!("/api/l/{tok}/games/{aid}/detail"))
        .body(Body::empty())
        .unwrap();
    let dresp = plain_router(Arc::clone(&store), mock)
        .oneshot(dreq)
        .await
        .unwrap();
    assert_eq!(dresp.status(), StatusCode::OK);
    let dj = body_json(dresp).await;
    assert!(
        dj["game"].get("genres").is_none(),
        "detail game object must stay wire-identical (no genres key)"
    );
    assert_eq!(dj["steam"]["detail"]["genres"][0], "Action");
}

/// GET /api/l/:token — games carry community `tags` from the steam cache (#71);
/// empty-tag caches omit the key (genre fallback stays client-side).
#[tokio::test]
async fn link_list_carries_tags_from_steam_cache() {
    let uid = uuid::Uuid::new_v4().simple().to_string();
    let Some(store) = store_or_skip(&format!("tag{}", &uid[..10])).await else {
        return;
    };

    // game A: warm cache with tags
    let mut a = test_game(70);
    a.steam_app_id = Some(99201);
    let aid = a.id.clone();
    store.put_game(&a).await.unwrap();
    let mut cache = test_steam_cache(99201);
    cache.detail.as_mut().unwrap().tags = vec!["Roguelike".into(), "Sci-fi".into()];
    store.put_steam_app(&cache).await.unwrap();

    // game B: warm cache, EMPTY tags (gated/pre-backfill) → tags key absent
    let mut b = test_game(71);
    b.steam_app_id = Some(99202);
    let bid = b.id.clone();
    store.put_game(&b).await.unwrap();
    store.put_steam_app(&test_steam_cache(99202)).await.unwrap();

    let tok = format!("tag{}", &uid[..28]);
    store.create_link(&test_link(&tok)).await.unwrap();

    let mock = MockInvoker::new(FulfillResponse::GiftUrl {
        url: "https://x.com/g".into(),
    });
    let req = Request::get(format!("/api/l/{tok}"))
        .body(Body::empty())
        .unwrap();
    let resp = plain_router(Arc::clone(&store), mock)
        .oneshot(req)
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let j = body_json(resp).await;

    let games = j["games"].as_array().expect("games must be an array");
    let ga = games
        .iter()
        .find(|g| g["id"] == aid.as_str())
        .expect("game A must be in the list");
    let gb = games
        .iter()
        .find(|g| g["id"] == bid.as_str())
        .expect("game B must be in the list");

    assert_eq!(
        ga["tags"],
        serde_json::json!(["Roguelike", "Sci-fi"]),
        "cache-warm game carries community tags in popularity order"
    );
    assert!(
        gb.get("tags").is_none(),
        "empty-tag cache must omit the tags key entirely (genre fallback is client-side)"
    );
}
