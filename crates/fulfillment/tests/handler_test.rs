//! Fulfillment tests.
//!
//! The pure gift-ladder test runs everywhere (it's the real safety guard). The wiremock+dynamo
//! integration tests SKIP locally when there's no dynamodb-local reachable — CI is the receipt,
//! never a local pass.

use domain::{ClaimState, GameStatus, Link, game_id};
use dynamo::{Store, SyncState};
use fulfillment::{Decision, Deps, FulfillRequest, FulfillResponse, gift_decision, handle};
use humble_client::{HumbleClient, SessionCookie};
use time::OffsetDateTime;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

// ---------------------------------------------------------------------------------------------
// Pure ladder test — the heart. Runs everywhere, no I/O. Exhaustively covers the decision map.
// ---------------------------------------------------------------------------------------------
#[test]
fn gift_decision_ladder_is_exhaustive_and_safe() {
    use humble_client::{GiftUrl, HumbleError as E};
    assert!(matches!(
        gift_decision(&Ok(GiftUrl("u".into()))),
        Decision::Record
    ));
    assert!(matches!(
        gift_decision(&Err(E::AlreadyRedeemed)),
        Decision::Compensate
    ));
    assert!(matches!(
        gift_decision(&Err(E::Unauthorized)),
        Decision::ParkCookieDead
    ));
    // Auth-layer rejection of the redeem WRITE parks plainly: the cookie may be fine (reads own
    // the cookie-health signal), so no cookie_ok flip and no dead-cookie ping from this path.
    assert!(matches!(
        gift_decision(&Err(E::RedeemAuthRejected {
            status: 403,
            csrf_minted: false
        })),
        Decision::Park
    ));
    assert!(matches!(
        gift_decision(&Err(E::AmbiguousRedeem)),
        Decision::Park
    ));
    assert!(matches!(
        gift_decision(&Err(E::RedeemRefused("x".into()))),
        Decision::Park
    ));
    assert!(matches!(
        gift_decision(&Err(E::RateLimited)),
        Decision::Park
    ));
    assert!(matches!(gift_decision(&Err(E::Api(500))), Decision::Park));
    // Network/Parse are constructed only inside humble-client (from reqwest/serde) — the compiler's
    // exhaustiveness check on the no-`_` match in gift_decision is the real guard that they, and any
    // future variant, get a decision. The map above pins every nameable outcome.
}

// ---------------------------------------------------------------------------------------------
// Integration scaffolding (dynamo-gated).
// ---------------------------------------------------------------------------------------------
async fn store_or_skip(test: &str) -> Option<Store> {
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
    let store = Store::new(client, format!("t-fulfill-{test}"));
    store.create_table_for_tests().await.unwrap();
    Some(store)
}

const COOKIE: &str = "sekrit-session-value";

fn humble_at(uri: &str) -> HumbleClient {
    HumbleClient::new(uri, SessionCookie::new(COOKIE.into())).unwrap()
}

fn link(token: &str) -> Link {
    Link {
        token: token.into(),
        label: "dave".into(),
        claims_allowed: 1,
        claims_used: 0,
        revoked: false,
        expires_at: None,
        created_at: OffsetDateTime::now_utc(),
    }
}

fn deps(store: Store, humble_uri: &str, webhook_url: Option<String>) -> Deps {
    Deps {
        store,
        humble: humble_at(humble_uri),
        webhook_url,
        http: reqwest::Client::new(),
        // No self-login in these handler tests — a dead session keeps the flag-and-ping path.
        session_store: None,
    }
}

/// Seed an available game + link + a pending claim, so a Gift request has something to fulfill.
async fn seed_pending_claim(store: &Store, gamekey: &str, machine: &str) -> String {
    let gid = game_id(gamekey, machine);
    let g = domain::Game {
        id: gid.clone(),
        title: "Stardew Valley".into(),
        bundle: "Humble Indie Bundle".into(),
        gamekey: gamekey.into(),
        machine_name: machine.into(),
        key_type: "steam".into(),
        giftable: true,
        hidden: false,
        status: GameStatus::Available,
        claim_id: None,
        artwork_url: None,
        keyindex: 0,
    };
    store.put_game(&g).await.unwrap();
    store.create_link(&link("tok1")).await.unwrap();
    store
        .claim_game("tok1", &gid, "c1", OffsetDateTime::now_utc())
        .await
        .unwrap();
    gid
}

fn gift_req(gid: &str, gamekey: &str, machine: &str) -> FulfillRequest {
    FulfillRequest::Gift {
        claim_id: "c1".into(),
        link_token: "tok1".into(),
        game_id: gid.into(),
        gamekey: gamekey.into(),
        machine_name: machine.into(),
        keyindex: 0,
    }
}

// ---------------------------------------------------------------------------------------------
// Gift happy path: redeem succeeds → URL durable on claim, game flips gifted.
// ---------------------------------------------------------------------------------------------
#[tokio::test]
async fn gift_happy_path_records_url_and_gifts_game() {
    let Some(store) = store_or_skip("gift-happy").await else {
        return;
    };
    let gid = seed_pending_claim(&store, "gk1", "mn").await;

    let humble = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/humbler/redeemkey"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "success": true,
            "giftkey": "GIFTTOKEN"
        })))
        .mount(&humble)
        .await;

    let deps = deps(store, &humble.uri(), None);
    let resp = handle(&deps, gift_req(&gid, "gk1", "mn")).await;

    let expected_url = "https://www.humblebundle.com/gift?key=GIFTTOKEN";
    assert_eq!(
        resp,
        FulfillResponse::GiftUrl {
            url: expected_url.into()
        }
    );

    let claim = deps.store.get_claim("tok1", "c1").await.unwrap().unwrap();
    assert_eq!(claim.state, ClaimState::Fulfilled);
    assert_eq!(claim.gift_url.as_deref(), Some(expected_url));

    let game = deps.store.get_game(&gid).await.unwrap().unwrap();
    assert_eq!(game.status, GameStatus::Gifted);
}

// ---------------------------------------------------------------------------------------------
// Already-redeemed path: humble says the key is gone → compensate (claim compensated, game re-listed).
// ---------------------------------------------------------------------------------------------
#[tokio::test]
async fn already_redeemed_compensates_and_relists() {
    let Some(store) = store_or_skip("gift-already").await else {
        return;
    };
    let gid = seed_pending_claim(&store, "gk1", "mn").await;

    let humble = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/humbler/redeemkey"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "success": false,
            "errormsg": "This key has already been redeemed."
        })))
        .mount(&humble)
        .await;

    let deps = deps(store, &humble.uri(), None);
    let resp = handle(&deps, gift_req(&gid, "gk1", "mn")).await;
    assert_eq!(resp, FulfillResponse::AlreadyRedeemed);

    let claim = deps.store.get_claim("tok1", "c1").await.unwrap().unwrap();
    assert_eq!(claim.state, ClaimState::Compensated);

    // slot returned, game re-listed (available + listable again).
    assert_eq!(
        deps.store
            .get_link("tok1")
            .await
            .unwrap()
            .unwrap()
            .claims_used,
        0
    );
    let game = deps.store.get_game(&gid).await.unwrap().unwrap();
    assert_eq!(game.status, GameStatus::Available);
    assert_eq!(deps.store.list_listable_games().await.unwrap().len(), 1);
}

// ---------------------------------------------------------------------------------------------
// Dead cookie path: the 200-with-HTML login interstitial — the ONE redeem response shape that
// positively identifies a stale session — → park + flag cookie_ok=false + discord ping. The ping
// body must carry the human message and must NEVER contain the session cookie value. A bare
// 401/403/302 on the redeem POST is NOT this path (see the redeem-auth-rejection test below).
// ---------------------------------------------------------------------------------------------
#[tokio::test]
async fn dead_cookie_parks_flags_and_pings_without_leaking_cookie() {
    let Some(store) = store_or_skip("gift-deadcookie").await else {
        return;
    };
    let gid = seed_pending_claim(&store, "gk1", "mn").await;

    let humble = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/humbler/redeemkey"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string("<!DOCTYPE html><html>login</html>")
                .append_header("content-type", "text/html"),
        )
        .mount(&humble)
        .await;

    let discord = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&discord)
        .await;

    let deps = deps(store, &humble.uri(), Some(discord.uri()));
    let resp = handle(&deps, gift_req(&gid, "gk1", "mn")).await;
    assert!(matches!(resp, FulfillResponse::Parked { .. }));

    // cookie flagged dead in sync state.
    let st: SyncState = deps.store.get_sync_state().await.unwrap().unwrap();
    assert!(!st.cookie_ok);

    // claim stays pending (human-owned recovery — never blind-compensated).
    let claim = deps.store.get_claim("tok1", "c1").await.unwrap().unwrap();
    assert_eq!(claim.state, ClaimState::Pending);

    // discord got exactly one ping; body carries the message and NEVER the cookie value.
    let reqs = discord.received_requests().await.unwrap();
    assert_eq!(reqs.len(), 1);
    let body = String::from_utf8(reqs[0].body.clone()).unwrap();
    assert!(body.contains("bendobundles"));
    assert!(body.to_lowercase().contains("cookie"));
    assert!(
        !body.contains(COOKIE),
        "ping body leaked the session cookie"
    );
}

// ---------------------------------------------------------------------------------------------
// Redeem auth-rejection path: a 403 on the redeem WRITE parks — the cookie may be fine (live
// 2026-07-04 capture: redeem 403 while sync walked the whole library on the same cookie), so
// cookie_ok must stay true and the DEAD-COOKIE ping must not fire. But it is NOT silent: a
// distinct, correctly-labeled ping fires instead — without one, a persistent rejection loops
// invisibly (park → reconcile re-lists → re-claim → reject, daily).
// ---------------------------------------------------------------------------------------------
#[tokio::test]
async fn redeem_auth_rejection_parks_and_pings_distinctly_without_flag() {
    let Some(store) = store_or_skip("gift-authreject").await else {
        return;
    };
    let gid = seed_pending_claim(&store, "gk1", "mn").await;

    // A healthy sync state — the redeem 403 must not clobber it.
    let healthy = SyncState {
        last_run_epoch: 1_800_000_000,
        ok: true,
        cookie_ok: true,
        games_written: 5,
        message: "all good".into(),
    };
    store.put_sync_state(&healthy).await.unwrap();

    let humble = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/humbler/redeemkey"))
        .respond_with(ResponseTemplate::new(403))
        .mount(&humble)
        .await;

    let discord = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&discord)
        .await;

    let deps = deps(store, &humble.uri(), Some(discord.uri()));
    let resp = handle(&deps, gift_req(&gid, "gk1", "mn")).await;
    let FulfillResponse::Parked { reason } = resp else {
        panic!("expected Parked, got {resp:?}");
    };
    assert!(
        reason.contains("redeem-auth-rejected"),
        "park reason must name the auth rejection, got: {reason}"
    );

    // cookie NOT flagged dead — the write-layer rejection is not the cookie's fault.
    let st: SyncState = deps.store.get_sync_state().await.unwrap().unwrap();
    assert!(st.cookie_ok, "redeem 403 must not flip cookie_ok");

    // claim stays pending for reconcile.
    let claim = deps.store.get_claim("tok1", "c1").await.unwrap().unwrap();
    assert_eq!(claim.state, ClaimState::Pending);

    // exactly ONE ping, correctly labeled: names the auth-layer block (and, since this test
    // mounts no preflight GET, the failed csrf capture), NOT cookie death — and never leaks
    // the cookie value.
    let reqs = discord.received_requests().await.unwrap();
    assert_eq!(
        reqs.len(),
        1,
        "a redeem auth-rejection must ping distinctly — silence hides the re-list loop"
    );
    let body = String::from_utf8(reqs[0].body.clone()).unwrap();
    assert!(body.contains("auth layer"), "ping must name the real cause");
    assert!(body.contains("c1"), "ping must carry the claim id");
    assert!(
        body.contains("capture FAILED"),
        "with no preflight cookie the ping must surface the minted-fallback signal"
    );
    assert!(
        !body.contains("DEAD") && !body.contains("paste a fresh one"),
        "ping must not carry the dead-cookie message"
    );
    assert!(
        !body.contains(COOKIE),
        "ping body leaked the session cookie"
    );
}

// ---------------------------------------------------------------------------------------------
// ValidateCookie: transient error must NOT write SyncState; Unauthorized must flag dead.
// ---------------------------------------------------------------------------------------------

/// Transient error (429) from humble during ValidateCookie → Error response, SyncState untouched.
/// This is the key regression guard for R1: a rate-limit must not silently mark the cookie dead.
#[tokio::test]
async fn validate_cookie_transient_error_does_not_touch_sync_state() {
    let Some(store) = store_or_skip("validate-transient").await else {
        return;
    };

    // Write a known-good SyncState so we can detect if it gets clobbered.
    let initial_state = dynamo::SyncState {
        last_run_epoch: 1_800_000_000,
        ok: true,
        cookie_ok: true,
        games_written: 5,
        message: "all good".into(),
    };
    store.put_sync_state(&initial_state).await.unwrap();

    // Humble returns 429 (rate-limited) for /api/v1/user/order.
    let humble = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/user/order"))
        .respond_with(ResponseTemplate::new(429))
        .mount(&humble)
        .await;

    let deps = deps(store, &humble.uri(), None);
    let resp = handle(&deps, FulfillRequest::ValidateCookie).await;

    // Must surface as an inconclusive Error, not CookieStatus{ok:false}.
    assert!(
        matches!(resp, FulfillResponse::Error { .. }),
        "transient humble error must return Error, got: {resp:?}"
    );

    // SyncState must be unchanged — we must NOT have written cookie_ok=false.
    let st = deps.store.get_sync_state().await.unwrap().unwrap();
    assert!(
        st.cookie_ok,
        "cookie_ok must be unchanged after a transient humble error"
    );
    assert_eq!(
        st.games_written, 5,
        "SyncState must not have been overwritten"
    );
}

/// Unauthorized from humble during ValidateCookie → CookieStatus{ok:false} and SyncState updated.
#[tokio::test]
async fn validate_cookie_unauthorized_flags_dead() {
    let Some(store) = store_or_skip("validate-unauth").await else {
        return;
    };

    let humble = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/user/order"))
        .respond_with(ResponseTemplate::new(401))
        .mount(&humble)
        .await;

    let deps = deps(store, &humble.uri(), None);
    let resp = handle(&deps, FulfillRequest::ValidateCookie).await;

    assert_eq!(resp, FulfillResponse::CookieStatus { ok: false });
    let st = deps.store.get_sync_state().await.unwrap().unwrap();
    assert!(!st.cookie_ok, "cookie_ok must be false after Unauthorized");
}

/// Success from humble during ValidateCookie → CookieStatus{ok:true} and SyncState updated.
#[tokio::test]
async fn validate_cookie_success_flags_ok() {
    let Some(store) = store_or_skip("validate-ok").await else {
        return;
    };

    let humble = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/user/order"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([])))
        .mount(&humble)
        .await;

    let deps = deps(store, &humble.uri(), None);
    let resp = handle(&deps, FulfillRequest::ValidateCookie).await;

    assert_eq!(resp, FulfillResponse::CookieStatus { ok: true });
    let st = deps.store.get_sync_state().await.unwrap().unwrap();
    assert!(
        st.cookie_ok,
        "cookie_ok must be true after a successful gamekeys call"
    );
}
