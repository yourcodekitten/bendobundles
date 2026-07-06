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
fn order_json(gamekey: &str, tpks: serde_json::Value) -> serde_json::Value {
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
            ResponseTemplate::new(200).set_body_json(order_json("gk", serde_json::json!([]))),
        )
        .up_to_n_times(1)
        .mount(&humble)
        .await;
    // Re-read: the freshly-minted tpk is present, unredeemed.
    Mock::given(method("GET"))
        .and(path("/api/v1/order/gk"))
        .respond_with(ResponseTemplate::new(200).set_body_json(order_json(
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
        .respond_with(ResponseTemplate::new(200).set_body_json(order_json(
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
            ResponseTemplate::new(200).set_body_json(order_json("gk", serde_json::json!([]))),
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
        .respond_with(ResponseTemplate::new(200).set_body_json(order_json(
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
        .respond_with(ResponseTemplate::new(200).set_body_json(order_json(
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
            ResponseTemplate::new(200).set_body_json(order_json("gk", serde_json::json!([]))),
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
        .respond_with(ResponseTemplate::new(200).set_body_json(order_json(
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
            ResponseTemplate::new(200).set_body_json(order_json("gk", serde_json::json!([]))),
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
        .respond_with(ResponseTemplate::new(200).set_body_json(order_json(
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
        .respond_with(ResponseTemplate::new(200).set_body_json(order_json(
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
            ResponseTemplate::new(200).set_body_json(order_json("gk", serde_json::json!([]))),
        )
        .up_to_n_times(1)
        .mount(&humble)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v1/order/gk"))
        .respond_with(ResponseTemplate::new(200).set_body_json(order_json(
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
            ResponseTemplate::new(200).set_body_json(order_json("gk", serde_json::json!([]))),
        )
        .up_to_n_times(1)
        .mount(&humble)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v1/order/gk"))
        .respond_with(ResponseTemplate::new(200).set_body_json(order_json(
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
