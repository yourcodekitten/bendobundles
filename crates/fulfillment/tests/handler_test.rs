//! Fulfillment tests.
//!
//! The pure gift-ladder test runs everywhere (it's the real safety guard). The wiremock+dynamo
//! integration tests SKIP locally when there's no dynamodb-local reachable — CI is the receipt,
//! never a local pass.

use domain::{ClaimState, GameStatus, Link, game_id};
use dynamo::{Store, SyncState};
use fulfillment::{
    Decision, Deps, FulfillRequest, FulfillResponse, SessionStore, gift_decision, handle,
};
use humble_client::{HumbleClient, SessionCookie, StepUpCredentials};
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
        requires_choice: false,
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
        requires_choice: false,
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
        !body.contains("DEAD") && !body.contains("break-glass"),
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

// ---------------------------------------------------------------------------------------------
// Gift in-line self-heal scaffolding: a Deps with self-login configured (step-up credentials on
// the client + a SessionStore pointed at a mock SSM), so a dead session on a redeem heals and
// retries in-line instead of parking.
// ---------------------------------------------------------------------------------------------

/// Any valid base32 seed works — the mock `/processlogin` never checks the code (RFC 6238 test
/// vector seed, same one humble-client's unit tests use).
const TOTP_SEED: &str = "GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ";

async fn ssm_at(uri: &str) -> aws_sdk_ssm::Client {
    let config = aws_config::defaults(aws_config::BehaviorVersion::latest())
        .endpoint_url(uri)
        .region("us-east-1")
        .test_credentials()
        .load()
        .await;
    aws_sdk_ssm::Client::new(&config)
}

async fn deps_with_selfheal(
    store: Store,
    humble_uri: &str,
    webhook_url: Option<String>,
    ssm_uri: &str,
) -> Deps {
    Deps {
        store,
        humble: humble_at(humble_uri).with_step_up(StepUpCredentials::new(
            "bot@example.com".into(),
            "hunter2".into(),
            TOTP_SEED.into(),
        )),
        webhook_url,
        http: reqwest::Client::new(),
        session_store: Some(SessionStore {
            ssm: ssm_at(ssm_uri).await,
            cookie_param: "/bendobundles/humble-cookie".into(),
        }),
    }
}

/// Mock SSM that accepts any PutParameter (AWS JSON 1.1 shape).
async fn mock_ssm() -> MockServer {
    let ssm = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(
            r#"{"Version":1,"Tier":"Advanced"}"#,
            "application/x-amz-json-1.1",
        ))
        .mount(&ssm)
        .await;
    ssm
}

// ---------------------------------------------------------------------------------------------
// Dead session + self-login configured: the gift path heals IN-LINE and retries the redeem once —
// the friend gets their URL on this very request instead of parking until the next scheduled run.
// Burn-safety: the first attempt's 200-with-HTML interstitial is humble's session check answering
// BEFORE the redeem handler runs, so the key was provably untouched and the healed retry is the
// first attempt that can burn it (see `selfheal_once` in the fulfillment crate).
// ---------------------------------------------------------------------------------------------
#[tokio::test]
async fn dead_session_gift_heals_inline_and_succeeds() {
    let Some(store) = store_or_skip("gift-heal-ok").await else {
        return;
    };
    let gid = seed_pending_claim(&store, "gk1", "mn").await;

    let humble = MockServer::start().await;
    // GET / serves BOTH the redeem csrf preflight and the login bootstrap: offer a csrf_cookie
    // and an anonymous session, exactly what `login()` needs.
    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(
            ResponseTemplate::new(200)
                .append_header("set-cookie", "csrf_cookie=csrfval; Path=/")
                .append_header("set-cookie", "_simpleauth_sess=ANONSESS; Path=/"),
        )
        .mount(&humble)
        .await;
    // First redeem: the 200-with-HTML login interstitial — the ONE dead-session redeem shape.
    Mock::given(method("POST"))
        .and(path("/humbler/redeemkey"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string("<!DOCTYPE html><html>login</html>")
                .append_header("content-type", "text/html"),
        )
        .up_to_n_times(1)
        .mount(&humble)
        .await;
    // After the heal: the retry succeeds.
    Mock::given(method("POST"))
        .and(path("/humbler/redeemkey"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "success": true,
            "giftkey": "GIFTTOKEN"
        })))
        .mount(&humble)
        .await;
    // Self-login: /processlogin accepts and rotates the session.
    Mock::given(method("POST"))
        .and(path("/processlogin"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(serde_json::json!({"goto": "/home/keys"}))
                .append_header("set-cookie", "_simpleauth_sess=NEWSESS; Path=/"),
        )
        .mount(&humble)
        .await;

    let ssm = mock_ssm().await;
    let discord = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&discord)
        .await;

    let deps = deps_with_selfheal(store, &humble.uri(), Some(discord.uri()), &ssm.uri()).await;
    let resp = handle(&deps, gift_req(&gid, "gk1", "mn")).await;

    // The friend saw a gift URL, not a park.
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

    // The heal persisted the rotated session to SSM — exactly once.
    let ssm_reqs = ssm.received_requests().await.unwrap();
    assert_eq!(ssm_reqs.len(), 1, "exactly one SSM PutParameter");
    let ssm_body = String::from_utf8(ssm_reqs[0].body.clone()).unwrap();
    assert!(
        ssm_body.contains("NEWSESS"),
        "the persisted cookie must be the rotated session"
    );

    // Durable heal ⇒ cookie_ok=true recorded (the sync-walk bookkeeping, mirrored in-line).
    let st: SyncState = deps.store.get_sync_state().await.unwrap().unwrap();
    assert!(st.cookie_ok, "a persisted heal must record cookie_ok=true");

    // Exactly TWO redeem POSTs — the ladder's retry is bounded at once.
    let redeems = humble
        .received_requests()
        .await
        .unwrap()
        .iter()
        .filter(|r| r.url.path() == "/humbler/redeemkey")
        .count();
    assert_eq!(redeems, 2, "exactly one heal-retry of the redeem");

    // One ping: the healed notice — never the dead, anon, or fresh session value.
    let reqs = discord.received_requests().await.unwrap();
    assert_eq!(reqs.len(), 1, "a heal pings once (the healed notice)");
    let body = String::from_utf8(reqs[0].body.clone()).unwrap();
    assert!(body.contains("self-login refreshed"));
    assert!(
        !body.contains(COOKIE) && !body.contains("NEWSESS") && !body.contains("ANONSESS"),
        "ping body leaked a session value"
    );
}

// ---------------------------------------------------------------------------------------------
// Dead session + self-login configured, but the heal FAILS → the pre-selfheal park semantics,
// with truthful messaging: the self-login failure ping fires first (from refresh_session), then
// the parked ping tells the operator the in-line heal already lost (paste = break-glass).
// NOTE: refresh_session retries a failed login once after the TOTP window rolls, so this test
// sleeps up to ~31s of wall clock — the price of exercising the real retry path against real
// sockets (tokio's paused clock would fast-forward the AWS SDK's request timeouts mid-flight).
// ---------------------------------------------------------------------------------------------
#[tokio::test]
async fn dead_session_gift_heal_failure_parks_flags_and_pings() {
    let Some(store) = store_or_skip("gift-heal-fail").await else {
        return;
    };
    let gid = seed_pending_claim(&store, "gk1", "mn").await;

    let humble = MockServer::start().await;
    // GET / offers NO csrf_cookie: the redeem preflight mints a fallback (fine), but the login
    // bootstrap REQUIRES one — so the in-line heal fails, both attempts.
    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&humble)
        .await;
    Mock::given(method("POST"))
        .and(path("/humbler/redeemkey"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string("<!DOCTYPE html><html>login</html>")
                .append_header("content-type", "text/html"),
        )
        .mount(&humble)
        .await;

    let ssm = mock_ssm().await;
    let discord = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&discord)
        .await;

    let deps = deps_with_selfheal(store, &humble.uri(), Some(discord.uri()), &ssm.uri()).await;
    let resp = handle(&deps, gift_req(&gid, "gk1", "mn")).await;
    assert!(matches!(resp, FulfillResponse::Parked { .. }));

    // Pre-selfheal park semantics, unchanged: claim pending, cookie flagged dead.
    let claim = deps.store.get_claim("tok1", "c1").await.unwrap().unwrap();
    assert_eq!(claim.state, ClaimState::Pending);
    let st: SyncState = deps.store.get_sync_state().await.unwrap().unwrap();
    assert!(!st.cookie_ok);

    // No login succeeded ⇒ nothing was persisted to SSM.
    assert!(ssm.received_requests().await.unwrap().is_empty());

    // ONE redeem POST only — a failed heal must never retry the write.
    let redeems = humble
        .received_requests()
        .await
        .unwrap()
        .iter()
        .filter(|r| r.url.path() == "/humbler/redeemkey")
        .count();
    assert_eq!(redeems, 1, "no redeem retry without a usable heal");

    // Two pings, in order: the self-login failure (with its reason), then the parked notice
    // that points back at it — and neither leaks the session cookie.
    let reqs = discord.received_requests().await.unwrap();
    assert_eq!(reqs.len(), 2, "failure ping + parked ping");
    let first = String::from_utf8(reqs[0].body.clone()).unwrap();
    let second = String::from_utf8(reqs[1].body.clone()).unwrap();
    assert!(
        first.contains("self-login FAILED"),
        "first ping must name the self-login failure: {first}"
    );
    assert!(
        second.contains("could not revive") && second.contains("break-glass"),
        "parked ping must say the in-line heal already lost: {second}"
    );
    assert!(
        !second.contains("next scheduled"),
        "parked ping must not promise a scheduled heal that already failed: {second}"
    );
    assert!(!first.contains(COOKIE) && !second.contains(COOKIE));
}

// ---------------------------------------------------------------------------------------------
// RedeemAuthRejected must NOT trigger the heal — even with self-login fully configured. A
// CSRF-layer 403 comes from a LIVE session (healing fixes nothing) and, unlike Unauthorized, is
// not proof the redeem handler was never reached — so the ladder must not touch it: no login,
// no SSM write, no redeem retry, cookie_ok untouched.
// ---------------------------------------------------------------------------------------------
#[tokio::test]
async fn redeem_auth_rejection_never_triggers_selfheal() {
    let Some(store) = store_or_skip("gift-authreject-noheal").await else {
        return;
    };
    let gid = seed_pending_claim(&store, "gk1", "mn").await;
    let healthy = SyncState {
        last_run_epoch: 1_800_000_000,
        ok: true,
        cookie_ok: true,
        games_written: 5,
        message: "all good".into(),
    };
    store.put_sync_state(&healthy).await.unwrap();

    let humble = MockServer::start().await;
    // A working csrf preflight, so a login WOULD succeed if (wrongly) attempted — the
    // no-/processlogin assertion below is what proves it never was.
    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(
            ResponseTemplate::new(200).append_header("set-cookie", "csrf_cookie=csrfval; Path=/"),
        )
        .mount(&humble)
        .await;
    // Plain 403 on the write: no secureArea redirect, no login_required body → RedeemAuthRejected.
    Mock::given(method("POST"))
        .and(path("/humbler/redeemkey"))
        .respond_with(ResponseTemplate::new(403))
        .mount(&humble)
        .await;

    let ssm = mock_ssm().await;
    let discord = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&discord)
        .await;

    let deps = deps_with_selfheal(store, &humble.uri(), Some(discord.uri()), &ssm.uri()).await;
    let resp = handle(&deps, gift_req(&gid, "gk1", "mn")).await;
    let FulfillResponse::Parked { reason } = resp else {
        panic!("expected Parked, got {resp:?}");
    };
    assert!(reason.contains("redeem-auth-rejected"));

    // The heal ladder never engaged: no login, no SSM write, no redeem retry.
    let humble_reqs = humble.received_requests().await.unwrap();
    assert!(
        humble_reqs.iter().all(|r| r.url.path() != "/processlogin"),
        "a CSRF-layer rejection must never trigger self-login"
    );
    assert_eq!(
        humble_reqs
            .iter()
            .filter(|r| r.url.path() == "/humbler/redeemkey")
            .count(),
        1,
        "RedeemAuthRejected must never be retried"
    );
    assert!(ssm.received_requests().await.unwrap().is_empty());

    // Park semantics unchanged: claim pending, cookie_ok NOT flipped.
    let claim = deps.store.get_claim("tok1", "c1").await.unwrap().unwrap();
    assert_eq!(claim.state, ClaimState::Pending);
    let st: SyncState = deps.store.get_sync_state().await.unwrap().unwrap();
    assert!(st.cookie_ok, "redeem 403 must not flip cookie_ok");

    // One ping: the auth-layer one, not a cookie-death or self-login message.
    let reqs = discord.received_requests().await.unwrap();
    assert_eq!(reqs.len(), 1);
    let body = String::from_utf8(reqs[0].body.clone()).unwrap();
    assert!(body.contains("auth layer"));
    assert!(!body.contains("self-login") && !body.contains("DEAD"));
}

// =============================================================================================
// RECONCILE + SYNC-WALK MATRIX
// ---------------------------------------------------------------------------------------------
// reconcile + the sync walk had ZERO tests before this suite — phase 3 (Humble Choice) is about
// to stand on this exact path, so it gets a net first. Every test drives the real code through
// `handle(FulfillRequest::Sync)`: it takes the sync-run marker, self-heals the listing, runs
// `reconcile`, then walks orders. We isolate reconcile by returning an EMPTY gamekey listing
// (GET /api/v1/user/order → []) — the order walk then loops over nothing, while reconcile still
// fetches each parked claim's order independently (GET /api/v1/order/<gamekey>).
// =============================================================================================

/// An empty gamekey listing — isolates reconcile from the order walk (the walk sees no orders,
/// reconcile still fetches per parked-claim gamekey on its own).
async fn mount_empty_listing(humble: &MockServer) {
    Mock::given(method("GET"))
        .and(path("/api/v1/user/order"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([])))
        .mount(humble)
        .await;
}

/// One order with a single key. `redeemed` toggles `redeemed_key_val` (present ⇒ redeemed).
fn order_json(gamekey: &str, machine: &str, redeemed: bool) -> serde_json::Value {
    let mut tpk = serde_json::json!({
        "machine_name": machine,
        "human_name": "Test Game",
        "key_type": "steam",
        "is_expired": false,
        "keyindex": 0,
    });
    if redeemed {
        tpk["redeemed_key_val"] = serde_json::json!("REDEEMED-KEY-VALUE");
    }
    serde_json::json!({
        "gamekey": gamekey,
        "product": { "human_name": "Test Bundle" },
        "tpkd_dict": { "all_tpks": [tpk] },
        "subproducts": [],
    })
}

/// Seed a parked (Pending) claim with a controllable `created_at` (via `claim_game`'s `now`), plus
/// its game (Available) and link (fresh, one slot). `gid` is the game_id stored on the claim —
/// pass a colonless string to exercise the unsplittable-game_id reconcile arm.
async fn seed_aged_pending(
    store: &Store,
    gid: &str,
    token: &str,
    claim_id: &str,
    created: OffsetDateTime,
) {
    let (gk, mn) = gid.split_once(':').unwrap_or((gid, gid));
    let g = domain::Game {
        id: gid.into(),
        title: "Stardew Valley".into(),
        bundle: "Humble Indie Bundle".into(),
        gamekey: gk.into(),
        machine_name: mn.into(),
        key_type: "steam".into(),
        giftable: true,
        hidden: false,
        status: GameStatus::Available,
        claim_id: None,
        artwork_url: None,
        keyindex: 0,
        requires_choice: false,
    };
    store.put_game(&g).await.unwrap();
    store.create_link(&link(token)).await.unwrap();
    store
        .claim_game(token, gid, claim_id, created)
        .await
        .unwrap();
}

fn hours_ago(h: i64) -> OffsetDateTime {
    OffsetDateTime::now_utc() - time::Duration::hours(h)
}

async fn discord_ok() -> MockServer {
    let discord = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&discord)
        .await;
    discord
}

// ---------------------------------------------------------------------------------------------
// reconcile: key shows REDEEMED on humble but no URL recorded → ping + LEAVE pending (never blind
// compensate). This is the crash-after-gift case: the gift WAS generated, so a human recovers the
// URL from humble's gift-history page; compensating would risk a lost, recoverable gift URL.
// ---------------------------------------------------------------------------------------------
#[tokio::test]
async fn reconcile_redeemed_pings_and_leaves_pending() {
    let Some(store) = store_or_skip("recon-redeemed").await else {
        return;
    };
    let gid = game_id("gkR", "mnR");
    seed_aged_pending(&store, &gid, "tokR", "cR", hours_ago(2)).await;

    let humble = MockServer::start().await;
    mount_empty_listing(&humble).await;
    Mock::given(method("GET"))
        .and(path("/api/v1/order/gkR"))
        .respond_with(ResponseTemplate::new(200).set_body_json(order_json("gkR", "mnR", true)))
        .mount(&humble)
        .await;
    let discord = discord_ok().await;

    let deps = deps(store, &humble.uri(), Some(discord.uri()));
    assert_eq!(
        handle(&deps, FulfillRequest::Sync).await,
        FulfillResponse::SyncDone
    );

    // claim stays PENDING — human-owned URL recovery, never blind-compensated.
    let claim = deps.store.get_claim("tokR", "cR").await.unwrap().unwrap();
    assert_eq!(claim.state, ClaimState::Pending);
    // game NOT re-listed (still owned by the claim).
    let game = deps.store.get_game(&gid).await.unwrap().unwrap();
    assert_eq!(game.status, GameStatus::Pending);

    // exactly one ping, naming the manual gift-history recovery, no key value leaked.
    let reqs = discord.received_requests().await.unwrap();
    assert_eq!(reqs.len(), 1);
    let body = String::from_utf8(reqs[0].body.clone()).unwrap();
    assert!(body.contains("cR"), "ping carries the claim id");
    assert!(
        body.contains("gift-history"),
        "ping names the recovery path"
    );
    assert!(
        !body.contains("REDEEMED-KEY-VALUE"),
        "ping must not leak a key"
    );
}

// ---------------------------------------------------------------------------------------------
// reconcile: key NOT redeemed on humble → the redeem never landed → compensate (slot returns,
// game re-lists). The ping fires so a compensate of an actually-gifted key (recoverable lost URL)
// is caught.
// ---------------------------------------------------------------------------------------------
#[tokio::test]
async fn reconcile_not_redeemed_compensates_and_relists() {
    let Some(store) = store_or_skip("recon-notredeemed").await else {
        return;
    };
    let gid = game_id("gkN", "mnN");
    seed_aged_pending(&store, &gid, "tokN", "cN", hours_ago(2)).await;

    let humble = MockServer::start().await;
    mount_empty_listing(&humble).await;
    Mock::given(method("GET"))
        .and(path("/api/v1/order/gkN"))
        .respond_with(ResponseTemplate::new(200).set_body_json(order_json("gkN", "mnN", false)))
        .mount(&humble)
        .await;
    let discord = discord_ok().await;

    let deps = deps(store, &humble.uri(), Some(discord.uri()));
    handle(&deps, FulfillRequest::Sync).await;

    // claim compensated, slot returned, game re-listed + listable.
    let claim = deps.store.get_claim("tokN", "cN").await.unwrap().unwrap();
    assert_eq!(claim.state, ClaimState::Compensated);
    assert_eq!(
        deps.store
            .get_link("tokN")
            .await
            .unwrap()
            .unwrap()
            .claims_used,
        0,
        "compensate returns the slot"
    );
    let game = deps.store.get_game(&gid).await.unwrap().unwrap();
    assert_eq!(game.status, GameStatus::Available);
    assert_eq!(deps.store.list_listable_games().await.unwrap().len(), 1);

    // the compensate ping fired (recoverable-lost-URL checkpoint).
    let reqs = discord.received_requests().await.unwrap();
    assert_eq!(reqs.len(), 1);
    let body = String::from_utf8(reqs[0].body.clone()).unwrap();
    assert!(body.contains("compensated") && body.contains("cN"));
}

// ---------------------------------------------------------------------------------------------
// reconcile: a claim younger than RECONCILE_MIN_AGE (15m) is left alone — a live redeem may still
// be recording its URL. No humble order fetch, no state change.
// ---------------------------------------------------------------------------------------------
#[tokio::test]
async fn reconcile_min_age_skips_fresh_claim() {
    let Some(store) = store_or_skip("recon-minage").await else {
        return;
    };
    let gid = game_id("gkF", "mnF");
    // 1 minute old — well under the 15m floor.
    seed_aged_pending(
        &store,
        &gid,
        "tokF",
        "cF",
        OffsetDateTime::now_utc() - time::Duration::minutes(1),
    )
    .await;

    let humble = MockServer::start().await;
    mount_empty_listing(&humble).await;
    // NOTE: intentionally NO /api/v1/order/gkF mock — reconcile must not fetch a too-fresh claim.
    let discord = discord_ok().await;

    let deps = deps(store, &humble.uri(), Some(discord.uri()));
    handle(&deps, FulfillRequest::Sync).await;

    // untouched: still pending, no order fetch, no ping.
    let claim = deps.store.get_claim("tokF", "cF").await.unwrap().unwrap();
    assert_eq!(claim.state, ClaimState::Pending);
    let order_hits = humble
        .received_requests()
        .await
        .unwrap()
        .iter()
        .filter(|r| r.url.path() == "/api/v1/order/gkF")
        .count();
    assert_eq!(order_hits, 0, "a too-fresh claim must not be fetched");
    assert!(discord.received_requests().await.unwrap().is_empty());
}

// ---------------------------------------------------------------------------------------------
// reconcile: session dies mid-pass (order fetch → 401) with no self-login → stop LOUDLY (flag
// cookie_ok=false) rather than silently skip every remaining claim. The claim stays pending.
// ---------------------------------------------------------------------------------------------
#[tokio::test]
async fn reconcile_dead_session_aborts_loudly_and_flags_cookie() {
    let Some(store) = store_or_skip("recon-dead").await else {
        return;
    };
    let gid = game_id("gkD", "mnD");
    seed_aged_pending(&store, &gid, "tokD", "cD", hours_ago(2)).await;

    let humble = MockServer::start().await;
    mount_empty_listing(&humble).await;
    Mock::given(method("GET"))
        .and(path("/api/v1/order/gkD"))
        .respond_with(ResponseTemplate::new(401))
        .mount(&humble)
        .await;
    let discord = discord_ok().await;

    let deps = deps(store, &humble.uri(), Some(discord.uri()));
    handle(&deps, FulfillRequest::Sync).await;

    // cookie flagged dead; claim untouched (NOT blind-compensated).
    let st: SyncState = deps.store.get_sync_state().await.unwrap().unwrap();
    assert!(
        !st.cookie_ok,
        "a dead session mid-reconcile must flag cookie_ok=false"
    );
    let claim = deps.store.get_claim("tokD", "cD").await.unwrap().unwrap();
    assert_eq!(claim.state, ClaimState::Pending);
}

// ---------------------------------------------------------------------------------------------
// reconcile: a transient (429) order fetch skips THAT claim and retries next pass — never
// compensates on ambiguity. Claim stays pending, cookie_ok not flipped.
// ---------------------------------------------------------------------------------------------
#[tokio::test]
async fn reconcile_transient_order_error_skips_claim() {
    let Some(store) = store_or_skip("recon-transient").await else {
        return;
    };
    let gid = game_id("gkT", "mnT");
    seed_aged_pending(&store, &gid, "tokT", "cT", hours_ago(2)).await;

    let humble = MockServer::start().await;
    mount_empty_listing(&humble).await;
    Mock::given(method("GET"))
        .and(path("/api/v1/order/gkT"))
        .respond_with(ResponseTemplate::new(429))
        .mount(&humble)
        .await;
    let discord = discord_ok().await;

    let deps = deps(store, &humble.uri(), Some(discord.uri()));
    handle(&deps, FulfillRequest::Sync).await;

    let claim = deps.store.get_claim("tokT", "cT").await.unwrap().unwrap();
    assert_eq!(
        claim.state,
        ClaimState::Pending,
        "transient error must not compensate"
    );
    // a transient reconcile skip is silent (no stuck-alert ping — the claim IS reconcilable).
    assert!(discord.received_requests().await.unwrap().is_empty());
}

// ---------------------------------------------------------------------------------------------
// LOUD-SKIP (new): an unreconcilable parked claim younger than RECONCILE_STUCK_ALERT_AGE (24h)
// stays SILENT — the machine_name mismatch may be a mid-deploy artifact the next sync corrects.
// ---------------------------------------------------------------------------------------------
#[tokio::test]
async fn reconcile_unreconcilable_under_threshold_stays_silent() {
    let Some(store) = store_or_skip("recon-stuck-young").await else {
        return;
    };
    // claim's machine_name is "mnGHOST", but the order only ever lists "mnREAL" → unreconcilable.
    let gid = game_id("gkS", "mnGHOST");
    seed_aged_pending(&store, &gid, "tokS", "cS", hours_ago(3)).await; // >15m, <24h

    let humble = MockServer::start().await;
    mount_empty_listing(&humble).await;
    Mock::given(method("GET"))
        .and(path("/api/v1/order/gkS"))
        .respond_with(ResponseTemplate::new(200).set_body_json(order_json("gkS", "mnREAL", false)))
        .mount(&humble)
        .await;
    let discord = discord_ok().await;

    let deps = deps(store, &humble.uri(), Some(discord.uri()));
    handle(&deps, FulfillRequest::Sync).await;

    // claim untouched, and NO ping yet — under the loud-skip threshold.
    let claim = deps.store.get_claim("tokS", "cS").await.unwrap().unwrap();
    assert_eq!(claim.state, ClaimState::Pending);
    assert!(
        discord.received_requests().await.unwrap().is_empty(),
        "under the age threshold the skip must stay log-only"
    );
}

// ---------------------------------------------------------------------------------------------
// LOUD-SKIP (new): the SAME unreconcilable claim, now past RECONCILE_STUCK_ALERT_AGE → the eternal
// silent skip goes LOUD: warn + exactly one discord ping (claim id + game_id, no secrets). The
// skip itself is unchanged (reconcile still decides nothing — the claim stays pending).
// ---------------------------------------------------------------------------------------------
#[tokio::test]
async fn reconcile_unreconcilable_over_threshold_pings_once() {
    let Some(store) = store_or_skip("recon-stuck-old").await else {
        return;
    };
    let gid = game_id("gkS2", "mnGHOST");
    seed_aged_pending(&store, &gid, "tokS2", "cS2", hours_ago(30)).await; // > 24h

    let humble = MockServer::start().await;
    mount_empty_listing(&humble).await;
    Mock::given(method("GET"))
        .and(path("/api/v1/order/gkS2"))
        .respond_with(ResponseTemplate::new(200).set_body_json(order_json("gkS2", "mnREAL", false)))
        .mount(&humble)
        .await;
    let discord = discord_ok().await;

    let deps = deps(store, &humble.uri(), Some(discord.uri()));
    handle(&deps, FulfillRequest::Sync).await;

    // skip UNCHANGED: claim still pending (reconcile decided nothing) …
    let claim = deps.store.get_claim("tokS2", "cS2").await.unwrap().unwrap();
    assert_eq!(claim.state, ClaimState::Pending);
    // … but now it's LOUD: exactly one ping, carrying claim id + game_id, no secret.
    let reqs = discord.received_requests().await.unwrap();
    assert_eq!(
        reqs.len(),
        1,
        "past the threshold, the stuck claim must ping exactly once"
    );
    let body = String::from_utf8(reqs[0].body.clone()).unwrap();
    assert!(body.contains("cS2"), "ping carries the claim id");
    assert!(body.contains(&gid), "ping carries the game_id");
    assert!(
        body.to_lowercase().contains("stuck"),
        "ping names the stuck condition"
    );
}

// ---------------------------------------------------------------------------------------------
// LOUD-SKIP (new): a parked claim whose game_id has no `gamekey:machine_name` split can never be
// checked against an order at all. Past the threshold it pings loudly (and never fetches humble —
// there's no gamekey to fetch).
// ---------------------------------------------------------------------------------------------
#[tokio::test]
async fn reconcile_unsplittable_game_id_over_threshold_pings() {
    let Some(store) = store_or_skip("recon-nosplit").await else {
        return;
    };
    // A colonless game_id: split_once(':') yields None → the unreconcilable arm, no order to check.
    seed_aged_pending(&store, "colonlessgameid", "tokX", "cX", hours_ago(30)).await;

    let humble = MockServer::start().await;
    mount_empty_listing(&humble).await;
    let discord = discord_ok().await;

    let deps = deps(store, &humble.uri(), Some(discord.uri()));
    handle(&deps, FulfillRequest::Sync).await;

    let claim = deps.store.get_claim("tokX", "cX").await.unwrap().unwrap();
    assert_eq!(claim.state, ClaimState::Pending);
    let reqs = discord.received_requests().await.unwrap();
    assert_eq!(
        reqs.len(),
        1,
        "an unsplittable game_id must ping once past the threshold"
    );
    let body = String::from_utf8(reqs[0].body.clone()).unwrap();
    assert!(body.contains("cX") && body.contains("game_id"));
}

// ---------------------------------------------------------------------------------------------
// ONE-HEAL-PER-RUN CAP: with self-login configured, the run's single heal is spent on the FIRST
// dead-session order fetch (claim A heals + reconciles); the SECOND dead order fetch (claim B) may
// NOT heal again — it aborts the pass loudly. Proof: exactly ONE /processlogin for the whole run.
// ---------------------------------------------------------------------------------------------
#[tokio::test]
async fn reconcile_one_heal_per_run_cap() {
    let Some(store) = store_or_skip("recon-healcap").await else {
        return;
    };
    // A is OLDER than B, so list_pending_claims (oldest-first) processes A then B.
    seed_aged_pending(&store, &game_id("gkA", "mnA"), "tokA", "cA", hours_ago(5)).await;
    seed_aged_pending(&store, &game_id("gkB", "mnB"), "tokB", "cB", hours_ago(2)).await;

    let humble = MockServer::start().await;
    mount_empty_listing(&humble).await;
    // GET / serves the redeem csrf preflight + the login bootstrap (csrf + anon session).
    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(
            ResponseTemplate::new(200)
                .append_header("set-cookie", "csrf_cookie=csrfval; Path=/")
                .append_header("set-cookie", "_simpleauth_sess=ANONSESS; Path=/"),
        )
        .mount(&humble)
        .await;
    // claim A's order: first 401 (dead), then after the heal a 200 not-redeemed → compensate.
    Mock::given(method("GET"))
        .and(path("/api/v1/order/gkA"))
        .respond_with(ResponseTemplate::new(401))
        .up_to_n_times(1)
        .mount(&humble)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v1/order/gkA"))
        .respond_with(ResponseTemplate::new(200).set_body_json(order_json("gkA", "mnA", false)))
        .mount(&humble)
        .await;
    // claim B's order: always dead — but the run's heal is already spent, so no second login.
    Mock::given(method("GET"))
        .and(path("/api/v1/order/gkB"))
        .respond_with(ResponseTemplate::new(401))
        .mount(&humble)
        .await;
    // Self-login rotates the session once.
    Mock::given(method("POST"))
        .and(path("/processlogin"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(serde_json::json!({"goto": "/home/keys"}))
                .append_header("set-cookie", "_simpleauth_sess=NEWSESS; Path=/"),
        )
        .mount(&humble)
        .await;

    let ssm = mock_ssm().await;
    let discord = discord_ok().await;
    let deps = deps_with_selfheal(store, &humble.uri(), Some(discord.uri()), &ssm.uri()).await;
    handle(&deps, FulfillRequest::Sync).await;

    // THE cap: exactly one login for the whole run.
    let logins = humble
        .received_requests()
        .await
        .unwrap()
        .iter()
        .filter(|r| r.url.path() == "/processlogin")
        .count();
    assert_eq!(
        logins, 1,
        "the run's single heal must be spent once, never twice"
    );

    // A healed + reconciled (not redeemed → compensated); B stayed pending (pass aborted).
    let a = deps.store.get_claim("tokA", "cA").await.unwrap().unwrap();
    assert_eq!(
        a.state,
        ClaimState::Compensated,
        "claim A healed and reconciled"
    );
    let b = deps.store.get_claim("tokB", "cB").await.unwrap().unwrap();
    assert_eq!(
        b.state,
        ClaimState::Pending,
        "claim B left for the next run (heal cap hit)"
    );
}

// ---------------------------------------------------------------------------------------------
// SYNC-RUN MARKER: a second sync while one holds the marker is a no-op (SyncDone), does NOT walk,
// and never touches humble — the mutex that makes concurrent walks impossible.
// ---------------------------------------------------------------------------------------------
#[tokio::test]
async fn sync_run_marker_blocks_concurrent_walk() {
    let Some(store) = store_or_skip("recon-marker").await else {
        return;
    };
    let gid = game_id("gkM", "mnM");
    seed_aged_pending(&store, &gid, "tokM", "cM", hours_ago(2)).await;
    // Hold a LIVE marker — as if another run owns the walk right now.
    assert_eq!(
        store
            .begin_sync_run(OffsetDateTime::now_utc().unix_timestamp())
            .await
            .unwrap(),
        dynamo::SyncBegin::Started
    );

    let humble = MockServer::start().await;
    mount_empty_listing(&humble).await;
    // Order for gkM would compensate if the walk ran — it must NOT.
    Mock::given(method("GET"))
        .and(path("/api/v1/order/gkM"))
        .respond_with(ResponseTemplate::new(200).set_body_json(order_json("gkM", "mnM", false)))
        .mount(&humble)
        .await;

    let deps = deps(store, &humble.uri(), None);
    assert_eq!(
        handle(&deps, FulfillRequest::Sync).await,
        FulfillResponse::SyncDone
    );

    // The walk was skipped: claim untouched, and humble was never called at all.
    let claim = deps.store.get_claim("tokM", "cM").await.unwrap().unwrap();
    assert_eq!(
        claim.state,
        ClaimState::Pending,
        "a blocked run must not reconcile"
    );
    assert!(
        humble.received_requests().await.unwrap().is_empty(),
        "a blocked run must not touch humble"
    );
}

// ---------------------------------------------------------------------------------------------
// TRANSIENT LISTING STILL RECONCILES: a 429 on the gamekey LISTING must not also cost a pass of
// parked-claim recovery — reconcile runs even when the listing failed (it doesn't need the list).
// ---------------------------------------------------------------------------------------------
#[tokio::test]
async fn transient_listing_still_reconciles() {
    let Some(store) = store_or_skip("recon-listing429").await else {
        return;
    };
    let gid = game_id("gkL", "mnL");
    seed_aged_pending(&store, &gid, "tokL", "cL", hours_ago(2)).await;

    let humble = MockServer::start().await;
    // The LISTING is rate-limited …
    Mock::given(method("GET"))
        .and(path("/api/v1/user/order"))
        .respond_with(ResponseTemplate::new(429))
        .mount(&humble)
        .await;
    // … but reconcile can still fetch the parked claim's order and act (not redeemed → compensate).
    Mock::given(method("GET"))
        .and(path("/api/v1/order/gkL"))
        .respond_with(ResponseTemplate::new(200).set_body_json(order_json("gkL", "mnL", false)))
        .mount(&humble)
        .await;
    let discord = discord_ok().await;

    let deps = deps(store, &humble.uri(), Some(discord.uri()));
    handle(&deps, FulfillRequest::Sync).await;

    // reconcile ran despite the failed listing: the claim was compensated.
    let claim = deps.store.get_claim("tokL", "cL").await.unwrap().unwrap();
    assert_eq!(
        claim.state,
        ClaimState::Compensated,
        "reconcile must run even when the listing 429s"
    );
    // and the run recorded the listing failure in its summary.
    let st: SyncState = deps.store.get_sync_state().await.unwrap().unwrap();
    assert!(
        st.message.contains("failed listing"),
        "summary names the listing failure: {}",
        st.message
    );
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

// =================================================================================================
// Humble Choice phase-3: the choose-then-redeem gift orchestration + its reconcile branch.
// =================================================================================================

const OFFERED_ID: &str = "octopathtravelerii";
const TITLE: &str = "Octopath Traveler II";
const TPK_MN: &str = "octopathtraveler2_row_choice_steam";

/// A choice tpk JSON in the order's `all_tpks`. `redeemed=true` stamps `redeemed_key_val` (which is
/// how `order()` derives `redeemed`).
fn tpk_json(machine_name: &str, human_name: &str, redeemed: bool) -> serde_json::Value {
    let mut t = serde_json::json!({
        "machine_name": machine_name,
        "human_name": human_name,
        "key_type": "steam",
        "is_expired": false,
        "keyindex": 0,
    });
    if redeemed {
        t["redeemed_key_val"] = serde_json::json!("STEAMKEY-XXXX");
    }
    t
}

/// A `/api/v1/order/<gamekey>` body carrying the given tpks.
fn choice_order_json(gamekey: &str, tpks: serde_json::Value) -> serde_json::Value {
    serde_json::json!({
        "gamekey": gamekey,
        "product": { "human_name": "May 2026 Humble Choice" },
        "tpkd_dict": { "all_tpks": tpks },
        "subproducts": [],
    })
}

/// Mount the gamekeys listing (`GET /api/v1/user/order`) so a `handle(Sync)` reconcile has an order
/// to walk.
async fn mount_gamekeys(humble: &MockServer, gamekeys: serde_json::Value) {
    Mock::given(method("GET"))
        .and(path("/api/v1/user/order"))
        .respond_with(ResponseTemplate::new(200).set_body_json(gamekeys))
        .mount(humble)
        .await;
}

/// Seed an Available `requires_choice` game + link + a Pending claim, then backdate the claim's
/// `created_at` and set its `choice_pre_tpks` snapshot directly (so reconcile tests can seed an aged
/// claim with a known intent snapshot). `machine_name` on the game = the OFFERED id (there is no tpk
/// yet). Returns the game id.
async fn seed_pending_choice_claim(
    store: &Store,
    gamekey: &str,
    offered_id: &str,
    title: &str,
    created_at: OffsetDateTime,
    pre: Option<Vec<String>>,
) -> String {
    let gid = game_id(gamekey, offered_id);
    let g = domain::Game {
        id: gid.clone(),
        title: title.into(),
        bundle: "May 2026 Humble Choice".into(),
        gamekey: gamekey.into(),
        machine_name: offered_id.into(),
        key_type: "steam".into(),
        giftable: true,
        hidden: false,
        status: GameStatus::Available,
        claim_id: None,
        artwork_url: None,
        keyindex: 0,
        requires_choice: true,
    };
    store.put_game(&g).await.unwrap();
    store.create_link(&link("tok1")).await.unwrap();
    store
        .claim_game("tok1", &gid, "c1", OffsetDateTime::now_utc())
        .await
        .unwrap();
    // Overwrite the claim body to age it + stamp the intent snapshot. State stays Pending, so the
    // gsi2pk reconcile marker survives (claim_item re-adds it for a Pending claim).
    let mut claim = store.get_claim("tok1", "c1").await.unwrap().unwrap();
    claim.created_at = created_at;
    claim.choice_pre_tpks = pre;
    store.put_claim(&claim).await.unwrap();
    gid
}

fn choice_gift_req(gid: &str, gamekey: &str, offered_id: &str) -> FulfillRequest {
    FulfillRequest::Gift {
        claim_id: "c1".into(),
        link_token: "tok1".into(),
        game_id: gid.into(),
        gamekey: gamekey.into(),
        machine_name: offered_id.into(),
        keyindex: 0,
        requires_choice: true,
    }
}

fn count_path(reqs: &[wiremock::Request], p: &str) -> usize {
    reqs.iter().filter(|r| r.url.path() == p).count()
}

// -------------------------------------------------------------------------------------------------
// Happy path: pre-read (no tpk) → record intent → choose → re-read (tpk) → redeem → gift URL.
// -------------------------------------------------------------------------------------------------
#[tokio::test]
async fn choice_happy_path_chooses_then_redeems() {
    let Some(store) = store_or_skip("choice-happy").await else {
        return;
    };
    let gid = seed_pending_choice_claim(
        &store,
        "gk",
        OFFERED_ID,
        TITLE,
        OffsetDateTime::now_utc(),
        None,
    )
    .await;

    let humble = MockServer::start().await;
    // Pre-read: no tpk yet (the pick isn't spent). up_to_n_times(1) so the re-read gets the next.
    Mock::given(method("GET"))
        .and(path("/api/v1/order/gk"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(choice_order_json("gk", serde_json::json!([]))),
        )
        .up_to_n_times(1)
        .mount(&humble)
        .await;
    // Re-read: the freshly-minted tpk is present, unredeemed.
    Mock::given(method("GET"))
        .and(path("/api/v1/order/gk"))
        .respond_with(ResponseTemplate::new(200).set_body_json(choice_order_json(
            "gk",
            serde_json::json!([tpk_json(TPK_MN, TITLE, false)]),
        )))
        .mount(&humble)
        .await;
    Mock::given(method("POST"))
        .and(path("/humbler/choosecontent"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "success": true, "force_refresh": true
        })))
        .mount(&humble)
        .await;
    Mock::given(method("POST"))
        .and(path("/humbler/redeemkey"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "success": true, "giftkey": "GIFTTOKEN"
        })))
        .mount(&humble)
        .await;

    let deps = deps(store, &humble.uri(), None);
    let resp = handle(&deps, choice_gift_req(&gid, "gk", OFFERED_ID)).await;

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
    // The intent snapshot was recorded (empty pre-read) and survives fulfill.
    assert_eq!(claim.choice_pre_tpks, Some(vec![]));

    let game = deps.store.get_game(&gid).await.unwrap().unwrap();
    assert_eq!(game.status, GameStatus::Gifted);

    let reqs = humble.received_requests().await.unwrap();
    assert_eq!(
        count_path(&reqs, "/humbler/choosecontent"),
        1,
        "exactly one pick spent"
    );
    assert_eq!(
        count_path(&reqs, "/humbler/redeemkey"),
        1,
        "exactly one redeem"
    );
    // The choose carried is_gift=true and the offered id in the array field.
    let choose = reqs
        .iter()
        .find(|r| r.url.path() == "/humbler/choosecontent")
        .unwrap();
    let body = String::from_utf8(choose.body.clone()).unwrap();
    assert!(body.contains("is_gift=true"), "choose body: {body}");
    assert!(
        body.contains(&format!("chosen_identifiers%5B%5D={OFFERED_ID}")),
        "choose body must carry the offered id in chosen_identifiers[]: {body}"
    );
}

// -------------------------------------------------------------------------------------------------
// MERGE GATE: crash after choose, before redeem → reconcile redeems WITHOUT ever choosing.
// -------------------------------------------------------------------------------------------------
#[tokio::test]
async fn merge_gate_reconcile_redeems_without_choosing() {
    let Some(store) = store_or_skip("choice-mergegate").await else {
        return;
    };
    // Aged claim, snapshot present (empty) — the crash-between-writes state: pick spent, key present.
    let aged = OffsetDateTime::now_utc() - time::Duration::minutes(16);
    let _gid = seed_pending_choice_claim(&store, "gk", OFFERED_ID, TITLE, aged, Some(vec![])).await;

    let humble = MockServer::start().await;
    mount_gamekeys(&humble, serde_json::json!([{ "gamekey": "gk" }])).await;
    // The order now shows the tpk present + unredeemed.
    Mock::given(method("GET"))
        .and(path("/api/v1/order/gk"))
        .respond_with(ResponseTemplate::new(200).set_body_json(choice_order_json(
            "gk",
            serde_json::json!([tpk_json(TPK_MN, TITLE, false)]),
        )))
        .mount(&humble)
        .await;
    Mock::given(method("POST"))
        .and(path("/humbler/redeemkey"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "success": true, "giftkey": "GIFTTOKEN"
        })))
        .mount(&humble)
        .await;
    // NOTE: deliberately NO /humbler/choosecontent mock — reconcile must never call it.

    let deps = deps(store, &humble.uri(), None);
    let resp = handle(&deps, FulfillRequest::Sync).await;
    assert_eq!(resp, FulfillResponse::SyncDone);

    let gid = game_id("gk", OFFERED_ID);
    let claim = deps.store.get_claim("tok1", "c1").await.unwrap().unwrap();
    assert_eq!(
        claim.state,
        ClaimState::Fulfilled,
        "reconcile completed the claim"
    );
    assert_eq!(
        claim.gift_url.as_deref(),
        Some("https://www.humblebundle.com/gift?key=GIFTTOKEN")
    );
    let game = deps.store.get_game(&gid).await.unwrap().unwrap();
    assert_eq!(game.status, GameStatus::Gifted);

    let reqs = humble.received_requests().await.unwrap();
    assert_eq!(
        count_path(&reqs, "/humbler/choosecontent"),
        0,
        "THE merge gate: reconcile must NEVER call choosecontent"
    );
    assert_eq!(
        count_path(&reqs, "/humbler/redeemkey"),
        1,
        "exactly one redeem from reconcile"
    );
}

// -------------------------------------------------------------------------------------------------
// GUARD (divergence a): the game row is missing at fulfillment → park BEFORE any humble call.
// The title read is the first step of handle_gift_choice; a miss must fail-safe (park, zero spend),
// never fail-dangerous. No game seeded → get_game returns None.
// -------------------------------------------------------------------------------------------------
#[tokio::test]
async fn choice_get_game_missing_parks_without_spending() {
    let Some(store) = store_or_skip("choice-game-missing").await else {
        return;
    };
    // Deliberately seed NOTHING — the very first step (get_game for the title) returns None.
    let humble = MockServer::start().await; // zero mocks: any humble call 404s and is counted.
    let gid = game_id("gk", OFFERED_ID);

    let deps = deps(store, &humble.uri(), None);
    let resp = handle(&deps, choice_gift_req(&gid, "gk", OFFERED_ID)).await;

    assert!(
        matches!(resp, FulfillResponse::Parked { .. }),
        "game missing at fulfillment must park, got {resp:?}"
    );
    let reqs = humble.received_requests().await.unwrap();
    // The proof it fails-safe: it parked BEFORE touching humble at all — no pick spent, no redeem.
    assert!(
        reqs.is_empty(),
        "game-missing parks before any humble call, got: {reqs:?}"
    );
    assert_eq!(
        count_path(&reqs, "/humbler/choosecontent"),
        0,
        "game-missing: no pick spent"
    );
}

// -------------------------------------------------------------------------------------------------
// GUARD: the intent write (step 3) hits its `attribute_exists(gsi2pk)` condition on a settled claim
// → Corrupt → park, and choose (step 4) is NEVER reached. This is the gate that stops a re-choose on
// a claim that already settled (a stale retry racing its own reconcile). Settle the claim out-of-band
// so its pending marker is gone, then drive a Gift through the pre-write steps.
// -------------------------------------------------------------------------------------------------
#[tokio::test]
async fn choice_intent_write_on_settled_claim_refuses_to_choose() {
    let Some(store) = store_or_skip("choice-intent-ccf").await else {
        return;
    };
    let gid = seed_pending_choice_claim(
        &store,
        "gk",
        OFFERED_ID,
        TITLE,
        OffsetDateTime::now_utc(),
        None,
    )
    .await;
    // Settle it out-of-band: compensate drops the gsi2pk pending marker (state → Compensated).
    store.compensate_claim("tok1", "c1", &gid).await.unwrap();

    let humble = MockServer::start().await;
    // Pre-read shows no tpk (pick not spent) so the flow proceeds all the way to the intent write.
    Mock::given(method("GET"))
        .and(path("/api/v1/order/gk"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(choice_order_json("gk", serde_json::json!([]))),
        )
        .mount(&humble)
        .await;
    // NOTE: deliberately NO /humbler/choosecontent mock — a settled claim must NEVER be chosen for.

    let deps = deps(store, &humble.uri(), None);
    let resp = handle(&deps, choice_gift_req(&gid, "gk", OFFERED_ID)).await;

    assert!(
        matches!(resp, FulfillResponse::Parked { .. }),
        "intent write on a settled claim must park, got {resp:?}"
    );
    let reqs = humble.received_requests().await.unwrap();
    assert_eq!(
        count_path(&reqs, "/humbler/choosecontent"),
        0,
        "THE intent gate: a settled claim must never reach choosecontent"
    );
    assert_eq!(
        count_path(&reqs, "/humbler/redeemkey"),
        0,
        "settled claim: no redeem either"
    );
    // The failed intent write does not resurrect or re-settle the claim — it stays Compensated.
    let claim = deps.store.get_claim("tok1", "c1").await.unwrap().unwrap();
    assert_eq!(claim.state, ClaimState::Compensated);
}

// -------------------------------------------------------------------------------------------------
// choose fails (success=false: no picks / already chosen / refused) → park, no spend, distinct ping.
// -------------------------------------------------------------------------------------------------
#[tokio::test]
async fn choice_choose_refused_parks_without_redeeming() {
    let Some(store) = store_or_skip("choice-refused").await else {
        return;
    };
    let gid = seed_pending_choice_claim(
        &store,
        "gk",
        OFFERED_ID,
        TITLE,
        OffsetDateTime::now_utc(),
        None,
    )
    .await;

    let humble = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/order/gk"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(choice_order_json("gk", serde_json::json!([]))),
        )
        .mount(&humble)
        .await;
    Mock::given(method("POST"))
        .and(path("/humbler/choosecontent"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "success": false, "errormsg": "no choices remaining"
        })))
        .mount(&humble)
        .await;

    let discord = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&discord)
        .await;

    let deps = deps(store, &humble.uri(), Some(discord.uri()));
    let resp = handle(&deps, choice_gift_req(&gid, "gk", OFFERED_ID)).await;
    assert!(
        matches!(resp, FulfillResponse::Parked { .. }),
        "got {resp:?}"
    );

    let claim = deps.store.get_claim("tok1", "c1").await.unwrap().unwrap();
    assert_eq!(claim.state, ClaimState::Pending);
    let game = deps.store.get_game(&gid).await.unwrap().unwrap();
    assert_eq!(
        game.status,
        GameStatus::Pending,
        "not compensated — game stays pending"
    );
    // No compensate: the link slot is still used.
    assert_eq!(
        deps.store
            .get_link("tok1")
            .await
            .unwrap()
            .unwrap()
            .claims_used,
        1
    );

    let reqs = humble.received_requests().await.unwrap();
    assert_eq!(
        count_path(&reqs, "/humbler/redeemkey"),
        0,
        "no redeem after a refused choose"
    );
    assert_eq!(count_path(&reqs, "/humbler/choosecontent"), 1);

    // cookie_ok untouched (no sync state written by a plain choose refusal).
    assert!(deps.store.get_sync_state().await.unwrap().is_none());
    // Exactly one distinct ping — the choose-refused notice, NOT a dead-cookie message.
    let dreqs = discord.received_requests().await.unwrap();
    assert_eq!(dreqs.len(), 1);
    let body = String::from_utf8(dreqs[0].body.clone()).unwrap();
    assert!(
        body.contains("refused the pick"),
        "ping must name the refusal: {body}"
    );
    assert!(!body.contains("DEAD"), "not a dead-cookie ping");
}

// -------------------------------------------------------------------------------------------------
// double-choose window: humble says "already chosen" (success=false) → ChooseFailed → park w/ snapshot.
// -------------------------------------------------------------------------------------------------
#[tokio::test]
async fn choice_double_choose_already_chosen_parks_with_snapshot() {
    let Some(store) = store_or_skip("choice-double").await else {
        return;
    };
    let gid = seed_pending_choice_claim(
        &store,
        "gk",
        OFFERED_ID,
        TITLE,
        OffsetDateTime::now_utc(),
        None,
    )
    .await;

    let humble = MockServer::start().await;
    // Pre-read carries an UNRELATED claimed tpk (so the pre-check doesn't title-match and resume).
    Mock::given(method("GET"))
        .and(path("/api/v1/order/gk"))
        .respond_with(ResponseTemplate::new(200).set_body_json(choice_order_json(
            "gk",
            serde_json::json!([tpk_json("unrelated_choice_steam", "Some Other Game", true)]),
        )))
        .mount(&humble)
        .await;
    Mock::given(method("POST"))
        .and(path("/humbler/choosecontent"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "success": false, "errormsg": "you have already chosen this content"
        })))
        .mount(&humble)
        .await;

    let deps = deps(store, &humble.uri(), None);
    let resp = handle(&deps, choice_gift_req(&gid, "gk", OFFERED_ID)).await;
    assert!(
        matches!(resp, FulfillResponse::Parked { .. }),
        "got {resp:?}"
    );

    let claim = deps.store.get_claim("tok1", "c1").await.unwrap().unwrap();
    assert_eq!(claim.state, ClaimState::Pending);
    // Snapshot recorded before the choose (the crash-recovery hinge) — reconcile owns it now.
    assert_eq!(
        claim.choice_pre_tpks,
        Some(vec!["unrelated_choice_steam".to_string()])
    );

    let reqs = humble.received_requests().await.unwrap();
    assert_eq!(count_path(&reqs, "/humbler/choosecontent"), 1);
    assert_eq!(count_path(&reqs, "/humbler/redeemkey"), 0);
}

// -------------------------------------------------------------------------------------------------
// pre-check resumes: the pick is already spent (tpk present unredeemed) → redeem WITHOUT choosing.
// -------------------------------------------------------------------------------------------------
#[tokio::test]
async fn choice_precheck_resumes_without_choosing() {
    let Some(store) = store_or_skip("choice-precheck").await else {
        return;
    };
    let gid = seed_pending_choice_claim(
        &store,
        "gk",
        OFFERED_ID,
        TITLE,
        OffsetDateTime::now_utc(),
        None,
    )
    .await;

    let humble = MockServer::start().await;
    // Pre-read already carries the game's key (human_name == title), unredeemed → resume.
    Mock::given(method("GET"))
        .and(path("/api/v1/order/gk"))
        .respond_with(ResponseTemplate::new(200).set_body_json(choice_order_json(
            "gk",
            serde_json::json!([tpk_json(TPK_MN, TITLE, false)]),
        )))
        .mount(&humble)
        .await;
    Mock::given(method("POST"))
        .and(path("/humbler/redeemkey"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "success": true, "giftkey": "GIFTTOKEN"
        })))
        .mount(&humble)
        .await;

    let deps = deps(store, &humble.uri(), None);
    let resp = handle(&deps, choice_gift_req(&gid, "gk", OFFERED_ID)).await;
    assert_eq!(
        resp,
        FulfillResponse::GiftUrl {
            url: "https://www.humblebundle.com/gift?key=GIFTTOKEN".into()
        }
    );

    let claim = deps.store.get_claim("tok1", "c1").await.unwrap().unwrap();
    assert_eq!(claim.state, ClaimState::Fulfilled);
    let game = deps.store.get_game(&gid).await.unwrap().unwrap();
    assert_eq!(game.status, GameStatus::Gifted);

    let reqs = humble.received_requests().await.unwrap();
    assert_eq!(
        count_path(&reqs, "/humbler/choosecontent"),
        0,
        "resume must NOT choose"
    );
    assert_eq!(count_path(&reqs, "/humbler/redeemkey"), 1);
}

// -------------------------------------------------------------------------------------------------
// 5xx after choose → Api (ambiguous, maybe-spent) → park; reconcile finishes. TOTAL choose POSTs == 1.
// -------------------------------------------------------------------------------------------------
#[tokio::test]
async fn choice_5xx_after_choose_parks_then_reconcile_finishes() {
    let Some(store) = store_or_skip("choice-5xx").await else {
        return;
    };
    // Aged from the start (phase 1 handle(Gift) ignores age; phase 2 handle(Sync) reconciles it).
    let aged = OffsetDateTime::now_utc() - time::Duration::minutes(16);
    let gid = seed_pending_choice_claim(&store, "gk", OFFERED_ID, TITLE, aged, None).await;

    // ---- Phase 1: choose 500 → park. ----
    let humble1 = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/order/gk"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(choice_order_json("gk", serde_json::json!([]))),
        )
        .mount(&humble1)
        .await;
    Mock::given(method("POST"))
        .and(path("/humbler/choosecontent"))
        .respond_with(ResponseTemplate::new(500))
        .mount(&humble1)
        .await;

    let deps1 = deps(store, &humble1.uri(), None);
    let resp = handle(&deps1, choice_gift_req(&gid, "gk", OFFERED_ID)).await;
    assert!(
        matches!(resp, FulfillResponse::Parked { .. }),
        "got {resp:?}"
    );
    let claim = deps1.store.get_claim("tok1", "c1").await.unwrap().unwrap();
    assert_eq!(claim.state, ClaimState::Pending);
    assert_eq!(
        claim.choice_pre_tpks,
        Some(vec![]),
        "snapshot durable before the ambiguous choose"
    );
    let reqs1 = humble1.received_requests().await.unwrap();
    assert_eq!(count_path(&reqs1, "/humbler/choosecontent"), 1);
    assert_eq!(count_path(&reqs1, "/humbler/redeemkey"), 0);

    // ---- Phase 2: reconcile sees the pick DID commit (tpk present) and finishes — never chooses. ----
    let store = deps1.store; // reuse the same table
    let humble2 = MockServer::start().await;
    mount_gamekeys(&humble2, serde_json::json!([{ "gamekey": "gk" }])).await;
    Mock::given(method("GET"))
        .and(path("/api/v1/order/gk"))
        .respond_with(ResponseTemplate::new(200).set_body_json(choice_order_json(
            "gk",
            serde_json::json!([tpk_json(TPK_MN, TITLE, false)]),
        )))
        .mount(&humble2)
        .await;
    Mock::given(method("POST"))
        .and(path("/humbler/redeemkey"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "success": true, "giftkey": "GIFTTOKEN"
        })))
        .mount(&humble2)
        .await;

    let deps2 = deps(store, &humble2.uri(), None);
    let resp = handle(&deps2, FulfillRequest::Sync).await;
    assert_eq!(resp, FulfillResponse::SyncDone);

    let claim = deps2.store.get_claim("tok1", "c1").await.unwrap().unwrap();
    assert_eq!(claim.state, ClaimState::Fulfilled);
    let game = deps2.store.get_game(&gid).await.unwrap().unwrap();
    assert_eq!(game.status, GameStatus::Gifted);
    let reqs2 = humble2.received_requests().await.unwrap();
    assert_eq!(
        count_path(&reqs2, "/humbler/choosecontent"),
        0,
        "phase 2 reconcile must not choose — total choose POSTs across both phases stays 1"
    );
    assert_eq!(count_path(&reqs2, "/humbler/redeemkey"), 1);
}

// -------------------------------------------------------------------------------------------------
// reconcile: snapshot present but order diff empty (pick not spent) → compensate, no humble writes.
// -------------------------------------------------------------------------------------------------
#[tokio::test]
async fn reconcile_choice_not_spent_compensates() {
    let Some(store) = store_or_skip("choice-recon-comp").await else {
        return;
    };
    let aged = OffsetDateTime::now_utc() - time::Duration::minutes(16);
    let gid = seed_pending_choice_claim(&store, "gk", OFFERED_ID, TITLE, aged, Some(vec![])).await;

    let humble = MockServer::start().await;
    mount_gamekeys(&humble, serde_json::json!([{ "gamekey": "gk" }])).await;
    // Empty order — no new tpk vs the empty snapshot → pick provably not spent.
    Mock::given(method("GET"))
        .and(path("/api/v1/order/gk"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(choice_order_json("gk", serde_json::json!([]))),
        )
        .mount(&humble)
        .await;
    let discord = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&discord)
        .await;

    let deps = deps(store, &humble.uri(), Some(discord.uri()));
    handle(&deps, FulfillRequest::Sync).await;

    let claim = deps.store.get_claim("tok1", "c1").await.unwrap().unwrap();
    assert_eq!(claim.state, ClaimState::Compensated);
    let game = deps.store.get_game(&gid).await.unwrap().unwrap();
    assert_eq!(game.status, GameStatus::Available);
    assert_eq!(
        deps.store.list_listable_games().await.unwrap().len(),
        1,
        "game re-listed"
    );
    assert_eq!(
        deps.store
            .get_link("tok1")
            .await
            .unwrap()
            .unwrap()
            .claims_used,
        0,
        "slot returned"
    );

    let reqs = humble.received_requests().await.unwrap();
    assert_eq!(
        count_path(&reqs, "/humbler/choosecontent"),
        0,
        "compensate does no humble writes"
    );
    assert_eq!(count_path(&reqs, "/humbler/redeemkey"), 0);
    // A compensate ping fired.
    let dreqs = discord.received_requests().await.unwrap();
    assert!(dreqs.iter().any(|r| {
        String::from_utf8(r.body.clone())
            .unwrap()
            .contains("compensated choice claim")
    }));
}

// -------------------------------------------------------------------------------------------------
// reconcile: NO snapshot (intent never landed) → compensate (choose provably never ran).
// -------------------------------------------------------------------------------------------------
#[tokio::test]
async fn reconcile_choice_no_snapshot_compensates() {
    let Some(store) = store_or_skip("choice-recon-nosnap").await else {
        return;
    };
    let aged = OffsetDateTime::now_utc() - time::Duration::minutes(16);
    let gid = seed_pending_choice_claim(&store, "gk", OFFERED_ID, TITLE, aged, None).await;

    let humble = MockServer::start().await;
    mount_gamekeys(&humble, serde_json::json!([{ "gamekey": "gk" }])).await;
    // Order even carries some unrelated tpk — irrelevant: no snapshot ⇒ choose never ran.
    Mock::given(method("GET"))
        .and(path("/api/v1/order/gk"))
        .respond_with(ResponseTemplate::new(200).set_body_json(choice_order_json(
            "gk",
            serde_json::json!([tpk_json("unrelated_choice_steam", "Other Game", true)]),
        )))
        .mount(&humble)
        .await;

    let deps = deps(store, &humble.uri(), None);
    handle(&deps, FulfillRequest::Sync).await;

    let claim = deps.store.get_claim("tok1", "c1").await.unwrap().unwrap();
    assert_eq!(claim.state, ClaimState::Compensated);
    let game = deps.store.get_game(&gid).await.unwrap().unwrap();
    assert_eq!(game.status, GameStatus::Available);

    let reqs = humble.received_requests().await.unwrap();
    assert_eq!(count_path(&reqs, "/humbler/choosecontent"), 0);
    assert_eq!(count_path(&reqs, "/humbler/redeemkey"), 0);
}

// -------------------------------------------------------------------------------------------------
// reconcile: key present but ALREADY redeemed, URL unrecorded → human-recover ping, stays pending.
// -------------------------------------------------------------------------------------------------
#[tokio::test]
async fn reconcile_choice_redeemed_unrecorded_pings_human() {
    let Some(store) = store_or_skip("choice-recon-redeemed").await else {
        return;
    };
    let aged = OffsetDateTime::now_utc() - time::Duration::minutes(16);
    let gid = seed_pending_choice_claim(&store, "gk", OFFERED_ID, TITLE, aged, Some(vec![])).await;

    let humble = MockServer::start().await;
    mount_gamekeys(&humble, serde_json::json!([{ "gamekey": "gk" }])).await;
    // New tpk present AND redeemed → spent + burned, URL unrecorded.
    Mock::given(method("GET"))
        .and(path("/api/v1/order/gk"))
        .respond_with(ResponseTemplate::new(200).set_body_json(choice_order_json(
            "gk",
            serde_json::json!([tpk_json(TPK_MN, TITLE, true)]),
        )))
        .mount(&humble)
        .await;
    let discord = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&discord)
        .await;

    let deps = deps(store, &humble.uri(), Some(discord.uri()));
    handle(&deps, FulfillRequest::Sync).await;

    let claim = deps.store.get_claim("tok1", "c1").await.unwrap().unwrap();
    assert_eq!(
        claim.state,
        ClaimState::Pending,
        "left pending for human recovery"
    );
    let _ = game_id("gk", OFFERED_ID);
    let game = deps.store.get_game(&gid).await.unwrap().unwrap();
    assert_eq!(game.status, GameStatus::Pending, "not re-listed");

    let reqs = humble.received_requests().await.unwrap();
    assert_eq!(
        count_path(&reqs, "/humbler/redeemkey"),
        0,
        "already-redeemed key is never re-redeemed"
    );

    let dreqs = discord.received_requests().await.unwrap();
    let ping = dreqs
        .iter()
        .map(|r| String::from_utf8(r.body.clone()).unwrap())
        .find(|b| b.contains("c1"))
        .expect("a ping mentioning the claim id");
    assert!(
        ping.contains("recover"),
        "ping must point at manual recovery: {ping}"
    );
    assert!(
        !ping.contains("STEAMKEY"),
        "ping must NEVER carry a key value: {ping}"
    );
}

// -------------------------------------------------------------------------------------------------
// happy-path re-read yields TWO new tpks the title can't split → ambiguous → park, no redeem.
// -------------------------------------------------------------------------------------------------
#[tokio::test]
async fn choice_ambiguous_multi_new_tpk_parks() {
    let Some(store) = store_or_skip("choice-ambiguous").await else {
        return;
    };
    let gid = seed_pending_choice_claim(
        &store,
        "gk",
        OFFERED_ID,
        TITLE,
        OffsetDateTime::now_utc(),
        None,
    )
    .await;

    let humble = MockServer::start().await;
    // Pre-read empty; re-read has two new tpks, NEITHER human_name == title.
    Mock::given(method("GET"))
        .and(path("/api/v1/order/gk"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(choice_order_json("gk", serde_json::json!([]))),
        )
        .up_to_n_times(1)
        .mount(&humble)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v1/order/gk"))
        .respond_with(ResponseTemplate::new(200).set_body_json(choice_order_json(
            "gk",
            serde_json::json!([
                tpk_json("alpha_choice_steam", "Alpha Game", false),
                tpk_json("beta_choice_steam", "Beta Game", false),
            ]),
        )))
        .mount(&humble)
        .await;
    Mock::given(method("POST"))
        .and(path("/humbler/choosecontent"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(serde_json::json!({ "success": true })),
        )
        .mount(&humble)
        .await;
    let discord = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&discord)
        .await;

    let deps = deps(store, &humble.uri(), Some(discord.uri()));
    let resp = handle(&deps, choice_gift_req(&gid, "gk", OFFERED_ID)).await;
    assert!(
        matches!(resp, FulfillResponse::Parked { .. }),
        "got {resp:?}"
    );

    let claim = deps.store.get_claim("tok1", "c1").await.unwrap().unwrap();
    assert_eq!(claim.state, ClaimState::Pending);
    let reqs = humble.received_requests().await.unwrap();
    assert_eq!(
        count_path(&reqs, "/humbler/redeemkey"),
        0,
        "never guess which key to burn"
    );
    assert_eq!(count_path(&reqs, "/humbler/choosecontent"), 1);
}

// -------------------------------------------------------------------------------------------------
// AlreadyRedeemed on a CHOICE redeem → NOT compensated (pick spent) → park + human recover ping.
// -------------------------------------------------------------------------------------------------
#[tokio::test]
async fn choice_redeem_already_redeemed_is_not_compensated() {
    let Some(store) = store_or_skip("choice-redeem-already").await else {
        return;
    };
    let gid = seed_pending_choice_claim(
        &store,
        "gk",
        OFFERED_ID,
        TITLE,
        OffsetDateTime::now_utc(),
        None,
    )
    .await;

    let humble = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/order/gk"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(choice_order_json("gk", serde_json::json!([]))),
        )
        .up_to_n_times(1)
        .mount(&humble)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v1/order/gk"))
        .respond_with(ResponseTemplate::new(200).set_body_json(choice_order_json(
            "gk",
            serde_json::json!([tpk_json(TPK_MN, TITLE, false)]),
        )))
        .mount(&humble)
        .await;
    Mock::given(method("POST"))
        .and(path("/humbler/choosecontent"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(serde_json::json!({ "success": true })),
        )
        .mount(&humble)
        .await;
    // The redeem says the key is already redeemed.
    Mock::given(method("POST"))
        .and(path("/humbler/redeemkey"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "success": false, "errormsg": "This key has already been redeemed."
        })))
        .mount(&humble)
        .await;
    let discord = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&discord)
        .await;

    let deps = deps(store, &humble.uri(), Some(discord.uri()));
    let resp = handle(&deps, choice_gift_req(&gid, "gk", OFFERED_ID)).await;
    // NOT FulfillResponse::AlreadyRedeemed (which would 410 + compensate) — a plain park.
    assert!(
        matches!(resp, FulfillResponse::Parked { .. }),
        "must park, not 410: {resp:?}"
    );

    let claim = deps.store.get_claim("tok1", "c1").await.unwrap().unwrap();
    assert_eq!(
        claim.state,
        ClaimState::Pending,
        "the spent pick must NOT be compensated"
    );
    let game = deps.store.get_game(&gid).await.unwrap().unwrap();
    assert_eq!(
        game.status,
        GameStatus::Pending,
        "game must NOT be re-listed"
    );
    assert_eq!(
        deps.store
            .get_link("tok1")
            .await
            .unwrap()
            .unwrap()
            .claims_used,
        1,
        "slot not returned"
    );

    let dreqs = discord.received_requests().await.unwrap();
    let ping = dreqs
        .iter()
        .map(|r| String::from_utf8(r.body.clone()).unwrap())
        .find(|b| b.contains("c1"))
        .expect("a human-recover ping");
    assert!(
        ping.contains("already-redeemed")
            || ping.contains("already spent")
            || ping.contains("gift-history"),
        "ping must guide human recovery: {ping}"
    );
}

// =================================================================================================
// PHASE-4: choice-discovery ingest — run_sync writes still-claimable OFFERED games as
// requires_choice=true (the sole intended writer per the domain trust contract).
// =================================================================================================

/// Base path for the Choice-months cursor walk (`choice_months`).
const CHOICE_LIST_BASE: &str =
    "/api/v1/subscriptions/humble_monthly/subscription_products_with_gamekeys";

/// Build a `/membership/<slug>` page carrying the `webpack-monthly-product-data` blob the
/// single-month `choice_month` read parses — `usesChoices`/`canRedeemGames` on, the given offered
/// games, and the given already-chosen machine_names. Claimable = offered − chosen.
fn membership_html(
    slug: &str,
    gamekey: &str,
    title: &str,
    offered: &[(&str, &str)],
    chosen: &[&str],
) -> String {
    let mut content_choices = serde_json::Map::new();
    for (mn, t) in offered {
        content_choices.insert((*mn).to_string(), serde_json::json!({ "title": t }));
    }
    let blob = serde_json::json!({
        "contentChoiceOptions": {
            "gamekey": gamekey,
            "title": title,
            "productUrlPath": slug,
            "productMachineName": format!("{}_choice", slug.replace('-', "_")),
            "usesChoices": true,
            "isActiveContent": false,
            "canRedeemGames": true,
            "contentChoiceData": { "initial": {
                "total_choices": offered.len(),
                "content_choices": serde_json::Value::Object(content_choices),
            } },
            "contentChoicesMade": { "initial": { "choices_made": chosen } },
        }
    });
    format!(
        "<html><body><script type=\"application/json\" id=\"webpack-monthly-product-data\">{blob}</script></body></html>"
    )
}

// -------------------------------------------------------------------------------------------------
// The happy path: a live month with an unspent pick → its still-claimable offered games land in the
// catalog as requires_choice=true / Available; the already-chosen one does NOT.
// -------------------------------------------------------------------------------------------------
#[tokio::test]
async fn sync_discovers_offered_choice_games_as_requires_choice_true() {
    let Some(store) = store_or_skip("choice-discovery-writes").await else {
        return;
    };

    let humble = MockServer::start().await;
    mount_empty_listing(&humble).await; // no order-walk games — isolate the discovery pass.
    // choice_months walk: one live month (usesChoices + canRedeemGames), single page (no cursor).
    Mock::given(method("GET"))
        .and(path(format!("{CHOICE_LIST_BASE}/")))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "products": [{
                "gamekey": "gkJun26", "title": "June 2026 Humble Choice",
                "productUrlPath": "june-2026", "productMachineName": "june_2026_choice",
                "usesChoices": true, "isActiveContent": false, "canRedeemGames": true,
                "contentChoiceData": { "game_data": {
                    "construction_simulator": { "title": "Construction Simulator" }
                } }
            }]
        })))
        .mount(&humble)
        .await;
    // single-month read: 3 offered, 1 already chosen → 2 claimable.
    Mock::given(method("GET"))
        .and(path("/membership/june-2026"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(membership_html(
                    "june-2026",
                    "gkJun26",
                    "June 2026 Humble Choice",
                    &[
                        ("construction_simulator", "Construction Simulator"),
                        ("another_offer", "Another Offer"),
                        ("already_picked", "Already Picked"),
                    ],
                    &["already_picked"],
                ))
                .append_header("content-type", "text/html"),
        )
        .mount(&humble)
        .await;

    let deps = deps(store, &humble.uri(), None);
    handle(&deps, FulfillRequest::Sync).await;

    // The two still-claimable offered games are written as claimable choice entries.
    let cs = deps
        .store
        .get_game(&game_id("gkJun26", "construction_simulator"))
        .await
        .unwrap()
        .expect("construction sim written as a claimable choice game");
    assert!(
        cs.requires_choice,
        "offered game must be requires_choice=true"
    );
    assert_eq!(cs.status, GameStatus::Available);
    assert_eq!(cs.title, "Construction Simulator");
    assert_eq!(cs.bundle, "June 2026 Humble Choice");
    assert_eq!(cs.machine_name, "construction_simulator");
    assert!(cs.giftable && !cs.hidden);

    assert!(
        deps.store
            .get_game(&game_id("gkJun26", "another_offer"))
            .await
            .unwrap()
            .is_some_and(|g| g.requires_choice),
        "the other unspent offer is also written"
    );

    // The already-chosen game is NOT re-listed as claimable (offered − chosen removed it).
    assert!(
        deps.store
            .get_game(&game_id("gkJun26", "already_picked"))
            .await
            .unwrap()
            .is_none(),
        "an already-chosen game must not be written as a claimable choice entry"
    );
}

// -------------------------------------------------------------------------------------------------
// The trust-contract guard: a month the list-walk surfaces but which is NOT still-redeemable
// (canRedeemGames=false) is filtered out BEFORE the single-month read — so its offered games are
// never written as requires_choice=true. Proves the list-walk (claimed set unknown) is never a
// source of `true`: no /membership mock is mounted, and reaching it would 404.
// -------------------------------------------------------------------------------------------------
#[tokio::test]
async fn sync_choice_discovery_skips_non_redeemable_month() {
    let Some(store) = store_or_skip("choice-discovery-skip").await else {
        return;
    };

    let humble = MockServer::start().await;
    mount_empty_listing(&humble).await;
    // A month that can no longer be redeemed → must be skipped (no single-month read, no write).
    Mock::given(method("GET"))
        .and(path(format!("{CHOICE_LIST_BASE}/")))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "products": [{
                "gamekey": "gkOld", "title": "Old Spent Month",
                "productUrlPath": "old-spent", "productMachineName": "old_spent_choice",
                "usesChoices": true, "isActiveContent": false, "canRedeemGames": false,
                "contentChoiceData": { "game_data": {
                    "some_offered_game": { "title": "Some Offered Game" }
                } }
            }]
        })))
        .mount(&humble)
        .await;
    // NOTE: deliberately NO /membership/old-spent mock — the filter must skip it before any read.

    let deps = deps(store, &humble.uri(), None);
    handle(&deps, FulfillRequest::Sync).await;

    // Nothing from a non-redeemable month is written as a claimable choice entry.
    assert!(
        deps.store
            .get_game(&game_id("gkOld", "some_offered_game"))
            .await
            .unwrap()
            .is_none(),
        "a non-redeemable month must never yield a requires_choice=true entry"
    );
}

// -------------------------------------------------------------------------------------------------
// The CLAIM-ALL tier (usesChoices=false, "Get My Games"): the month has no `initial` block and lists
// its games under `game_data`. Discovery must (a) NOT filter it out (an earlier build required
// uses_choices=true), and (b) surface its un-chosen offers as requires_choice=true. Regression guard
// for the June-2026 live miss.
// -------------------------------------------------------------------------------------------------
#[tokio::test]
async fn sync_discovers_claim_all_tier_offers() {
    let Some(store) = store_or_skip("choice-discovery-claim-all").await else {
        return;
    };

    let humble = MockServer::start().await;
    mount_empty_listing(&humble).await;
    // List-walk: a claim-all month (usesChoices=false, canRedeemGames=true).
    Mock::given(method("GET"))
        .and(path(format!("{CHOICE_LIST_BASE}/")))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "products": [{
                "gamekey": "gkJun26", "title": "June 2026 Humble Choice",
                "productUrlPath": "june-2026", "productMachineName": "june_2026_choice",
                "usesChoices": false, "isActiveContent": false, "canRedeemGames": true,
                "contentChoiceData": { "game_data": {
                    "constructionsimulator": { "title": "Construction Simulator" }
                } }
            }]
        })))
        .mount(&humble)
        .await;
    // Single-month read: claim-all blob — NO `initial` block, games under `game_data`, one chosen.
    let blob = r#"<html><body><script type="application/json" id="webpack-monthly-product-data">
    {"contentChoiceOptions":{
        "gamekey":"gkJun26","title":"June 2026","productUrlPath":"june-2026",
        "productMachineName":"june_2026_choice","usesChoices":false,
        "isActiveContent":false,"canRedeemGames":true,
        "contentChoiceData":{"game_data":{
            "constructionsimulator":{"title":"Construction Simulator"},
            "octopathtravelerii":{"title":"OCTOPATH TRAVELER II"}
        }},
        "contentChoicesMade":{"initial":{"choices_made":["octopathtravelerii"]}}
    }}</script></body></html>"#;
    Mock::given(method("GET"))
        .and(path("/membership/june-2026"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(blob)
                .append_header("content-type", "text/html"),
        )
        .mount(&humble)
        .await;

    let deps = deps(store, &humble.uri(), None);
    handle(&deps, FulfillRequest::Sync).await;

    // The un-chosen claim-all offer is surfaced as a claimable choice game.
    let cs = deps
        .store
        .get_game(&game_id("gkJun26", "constructionsimulator"))
        .await
        .unwrap()
        .expect("claim-all offer written as a claimable choice game");
    assert!(cs.requires_choice);
    assert_eq!(cs.status, GameStatus::Available);
    assert_eq!(cs.title, "Construction Simulator");

    // The already-chosen game is NOT re-surfaced as claimable.
    assert!(
        deps.store
            .get_game(&game_id("gkJun26", "octopathtravelerii"))
            .await
            .unwrap()
            .is_none(),
        "an already-chosen claim-all game must not be written as claimable"
    );
}
