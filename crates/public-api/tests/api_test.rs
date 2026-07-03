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
    let resp = router(store, mock).oneshot(req).await.unwrap();

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
    let resp = router(Arc::clone(&store), mock.clone())
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
    let resp = router(Arc::clone(&store), mock.clone())
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
    let resp = router(Arc::clone(&store), mock.clone())
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
    let post_resp = router(Arc::clone(&store), Arc::clone(&invoker))
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
    let get_resp = router(Arc::clone(&store), Arc::clone(&invoker))
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
    let resp1 = router(Arc::clone(&store), mock.clone())
        .oneshot(r1)
        .await
        .unwrap();
    assert_eq!(resp1.status(), StatusCode::OK, "first claim must succeed");

    // Second claim loses.
    let r2 = Request::post("/api/l/race-tok2/claim")
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_vec(&claim_body).unwrap()))
        .unwrap();
    let resp2 = router(Arc::clone(&store), mock.clone())
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
    let post_resp = router(Arc::clone(&store), mock.clone())
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
    let get_resp = router(Arc::clone(&store), mock.clone())
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
    let resp = router(store, mock).oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}
