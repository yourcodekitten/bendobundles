//! Fulfillment tests.
//!
//! The pure gift-ladder test runs everywhere (it's the real safety guard). The wiremock+dynamo
//! integration tests SKIP locally when there's no dynamodb-local reachable — CI is the receipt,
//! never a local pass.

use domain::{AppidSource, ClaimState, Game, GameStatus, Link, SELF_LINK_TOKEN, game_id};
use dynamo::{Store, SyncState};
use fulfillment::{
    Decision, Deps, FulfillRequest, FulfillResponse, SessionStore, enrich_steam_apps,
    gift_decision, handle, reveal_decision,
};
use humble_client::{HumbleClient, SessionCookie, StepUpCredentials};
use std::sync::Arc;
use time::OffsetDateTime;
use wiremock::matchers::{method, path, query_param};
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
    let store = Store::new(client, format!("t-fulfill-{}-{test}", std::process::id()));
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
        // No Steam client in these handler tests — appid mapper pass is skipped.
        steam: None,
        steam_enrich_disabled: false,
        // Zero pacing in tests: the paced enrichment pass runs instantly against real wiremock I/O.
        steam_enrich_pace: std::time::Duration::ZERO,
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
        steam_app_id: None,
        appid_source: None,
        owned_by_ben: false,
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
        steam: None,
        steam_enrich_disabled: false,
        steam_enrich_pace: std::time::Duration::ZERO,
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
        steam_app_id: None,
        appid_source: None,
        owned_by_ben: false,
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
        steam_app_id: None,
        appid_source: None,
        owned_by_ben: false,
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
// Also: a parked SELF choice claim reconciles (compensates via B1) WITHOUT any choosecontent POST.
// -------------------------------------------------------------------------------------------------
#[tokio::test]
async fn merge_gate_reconcile_redeems_without_choosing() {
    let Some(store) = store_or_skip("choice-mergegate").await else {
        return;
    };
    // Aged claim, snapshot present (empty) — the crash-between-writes state: pick spent, key present.
    let aged = OffsetDateTime::now_utc() - time::Duration::minutes(16);
    let _gid = seed_pending_choice_claim(&store, "gk", OFFERED_ID, TITLE, aged, Some(vec![])).await;

    // SELF choice claim on a separate gamekey — snapshot present, no tpk in order → B1 compensate.
    // No choosecontent route is mounted; a choose attempt would 404 and surface via the gate below.
    let self_gid = game_id("gkM", "offered_m");
    seed_choice_game(&store, &self_gid, "SELF Merge Test").await;
    store
        .claim_game_self(&self_gid, "sc-mg1", aged)
        .await
        .unwrap();
    store
        .record_choice_intent(SELF_LINK_TOKEN, "sc-mg1", vec![])
        .await
        .unwrap();

    let humble = MockServer::start().await;
    mount_gamekeys(&humble, serde_json::json!([{ "gamekey": "gk" }])).await;
    // The order now shows the tpk present + unredeemed (for the Gift claim).
    Mock::given(method("GET"))
        .and(path("/api/v1/order/gk"))
        .respond_with(ResponseTemplate::new(200).set_body_json(choice_order_json(
            "gk",
            serde_json::json!([tpk_json(TPK_MN, TITLE, false)]),
        )))
        .mount(&humble)
        .await;
    // Order for the SELF claim: empty tpks → B1 (snapshot present, no new tpk → compensate).
    Mock::given(method("GET"))
        .and(path("/api/v1/order/gkM"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(choice_order_json("gkM", serde_json::json!([]))),
        )
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
        "reconcile completed the gift claim"
    );
    assert_eq!(
        claim.gift_url.as_deref(),
        Some("https://www.humblebundle.com/gift?key=GIFTTOKEN")
    );
    let game = deps.store.get_game(&gid).await.unwrap().unwrap();
    assert_eq!(game.status, GameStatus::Gifted);

    // SELF claim reconciled without choosing: B1 → compensate_self_claim → Compensated.
    let self_claim = deps
        .store
        .get_claim(SELF_LINK_TOKEN, "sc-mg1")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        self_claim.state,
        ClaimState::Compensated,
        "self claim must be compensated (B1), not choosing"
    );

    let reqs = humble.received_requests().await.unwrap();
    assert_eq!(
        count_path(&reqs, "/humbler/choosecontent"),
        0,
        "THE merge gate: reconcile must NEVER call choosecontent"
    );
    assert_eq!(
        count_path(&reqs, "/humbler/redeemkey"),
        1,
        "exactly one redeem from reconcile (gift claim only)"
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
// The redeemability gate is the membership PAGE, not the list. A month whose per-month read reports
// `canRedeemGames=false` writes nothing — even though it was enumerated and read. (Discovery no longer
// pre-filters on the list flag, precisely because the list is unreliable for the newest months.)
// -------------------------------------------------------------------------------------------------
#[tokio::test]
async fn sync_choice_discovery_skips_page_non_redeemable_month() {
    let Some(store) = store_or_skip("choice-discovery-skip").await else {
        return;
    };

    let humble = MockServer::start().await;
    mount_empty_listing(&humble).await;
    Mock::given(method("GET"))
        .and(path(format!("{CHOICE_LIST_BASE}/")))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "products": [{
                "gamekey": "gkOld", "title": "Old Spent Month",
                "productUrlPath": "old-spent", "productMachineName": "old_spent_choice",
                "usesChoices": true, "isActiveContent": false, "canRedeemGames": true,
                "contentChoiceData": { "game_data": {
                    "some_offered_game": { "title": "Some Offered Game" }
                } }
            }]
        })))
        .mount(&humble)
        .await;
    // The membership PAGE says this month can no longer be redeemed → the write is gated off, even
    // though the read happened.
    let blob = r#"<html><body><script type="application/json" id="webpack-monthly-product-data">
    {"contentChoiceOptions":{
        "gamekey":"gkOld","title":"Old Spent Month","productUrlPath":"old-spent",
        "productMachineName":"old_spent_choice","usesChoices":true,
        "isActiveContent":false,"canRedeemGames":false,
        "contentChoiceData":{"initial":{"content_choices":{"some_offered_game":{"title":"Some Offered Game"}}}},
        "contentChoicesMade":{"initial":{"choices_made":[]}}
    }}</script></body></html>"#;
    Mock::given(method("GET"))
        .and(path("/membership/old-spent"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(blob)
                .append_header("content-type", "text/html"),
        )
        .mount(&humble)
        .await;

    let deps = deps(store, &humble.uri(), None);
    handle(&deps, FulfillRequest::Sync).await;

    // The page's canRedeemGames=false gates the write: nothing surfaced.
    assert!(
        deps.store
            .get_game(&game_id("gkOld", "some_offered_game"))
            .await
            .unwrap()
            .is_none(),
        "a page-non-redeemable month must never yield a requires_choice=true entry"
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

// =================================================================================================
// Task 6: Self-claim bundle path tests.
// =================================================================================================

fn now() -> OffsetDateTime {
    OffsetDateTime::now_utc()
}

/// Seed an Available game with a game_id of the form "gamekey:machine_name" and a given title.
async fn seed_available_game(store: &Store, game_id_str: &str, title: &str) {
    let (gk, mn) = game_id_str
        .split_once(':')
        .expect("game_id must be gamekey:machine_name");
    let g = domain::Game {
        id: game_id_str.into(),
        title: title.into(),
        bundle: "Test Bundle".into(),
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
        steam_app_id: None,
        appid_source: None,
        owned_by_ben: false,
    };
    store.put_game(&g).await.unwrap();
}

fn self_claim_req(
    claim_id: &str,
    game_id: &str,
    gamekey: &str,
    machine_name: &str,
) -> FulfillRequest {
    FulfillRequest::SelfClaim {
        claim_id: claim_id.into(),
        game_id: game_id.into(),
        gamekey: gamekey.into(),
        machine_name: machine_name.into(),
        keyindex: 0,
        requires_choice: false,
    }
}

/// Mount a successful reveal (POST /humbler/redeemkey without gift= → {"key":"…","success":true}).
async fn mount_reveal_success(humble: &MockServer, key: &str) {
    Mock::given(method("POST"))
        .and(path("/humbler/redeemkey"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "key": key,
            "success": true
        })))
        .mount(humble)
        .await;
}

/// Mount an already-redeemed reveal response.
async fn mount_reveal_already_redeemed(humble: &MockServer) {
    Mock::given(method("POST"))
        .and(path("/humbler/redeemkey"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "success": false,
            "errormsg": "This key has already been redeemed."
        })))
        .mount(humble)
        .await;
}

/// Mount an order GET with a tpk that has a redeemed_key_val.
async fn mount_order_with_redeemed_tpk(
    humble: &MockServer,
    gamekey: &str,
    machine_name: &str,
    key_val: &str,
) {
    Mock::given(method("GET"))
        .and(path(format!("/api/v1/order/{gamekey}")))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "gamekey": gamekey,
            "product": { "human_name": "Test Bundle" },
            "tpkd_dict": { "all_tpks": [{
                "machine_name": machine_name,
                "human_name": "Test Game",
                "key_type": "steam",
                "is_expired": false,
                "keyindex": 0,
                "redeemed_key_val": key_val
            }]},
            "subproducts": [],
        })))
        .mount(humble)
        .await;
}

/// Mount an order GET with a tpk that has NO redeemed_key_val.
async fn mount_order_with_redeemed_tpk_no_val(
    humble: &MockServer,
    gamekey: &str,
    machine_name: &str,
) {
    Mock::given(method("GET"))
        .and(path(format!("/api/v1/order/{gamekey}")))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "gamekey": gamekey,
            "product": { "human_name": "Test Bundle" },
            "tpkd_dict": { "all_tpks": [{
                "machine_name": machine_name,
                "human_name": "Test Game",
                "key_type": "steam",
                "is_expired": false,
                "keyindex": 0
            }]},
            "subproducts": [],
        })))
        .mount(humble)
        .await;
}

/// Mount a 500 from the reveal endpoint.
async fn mount_reveal_500(humble: &MockServer) {
    Mock::given(method("POST"))
        .and(path("/humbler/redeemkey"))
        .respond_with(ResponseTemplate::new(500))
        .mount(humble)
        .await;
}

// -------------------------------------------------------------------------------------------------
// Test 1: Happy path — reveal succeeds, key recorded, game flips BenRedeemed.
// -------------------------------------------------------------------------------------------------
#[tokio::test]
async fn self_claim_bundle_reveals_and_records() {
    let Some(store) = store_or_skip("sc-reveals").await else {
        return;
    };
    seed_available_game(&store, "gkA:mnA", "Stardew Valley").await;
    store
        .claim_game_self("gkA:mnA", "sc-1", now())
        .await
        .unwrap();

    let humble = MockServer::start().await;
    mount_reveal_success(&humble, "AAAA-BBBB-CCCC").await;

    let deps = deps(store.clone(), &humble.uri(), None);
    let resp = handle(
        &deps,
        FulfillRequest::SelfClaim {
            claim_id: "sc-1".into(),
            game_id: "gkA:mnA".into(),
            gamekey: "gkA".into(),
            machine_name: "mnA".into(),
            keyindex: 0,
            requires_choice: false,
        },
    )
    .await;

    assert_eq!(
        resp,
        FulfillResponse::RevealedKey {
            key: "AAAA-BBBB-CCCC".into()
        }
    );
    let claim = store
        .get_claim(SELF_LINK_TOKEN, "sc-1")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(claim.revealed_key.as_deref(), Some("AAAA-BBBB-CCCC"));
    assert_eq!(
        store.get_game("gkA:mnA").await.unwrap().unwrap().status,
        domain::GameStatus::BenRedeemed
    );

    // Assert the reveal POST had no gift= param.
    let reqs = humble.received_requests().await.unwrap();
    let reveal_req = reqs
        .iter()
        .find(|r| r.url.path() == "/humbler/redeemkey")
        .unwrap();
    let body = String::from_utf8(reveal_req.body.clone()).unwrap();
    assert!(
        !body.contains("gift="),
        "reveal must not send gift param: {body}"
    );
}

// -------------------------------------------------------------------------------------------------
// Test 2: AlreadyRedeemed → re-read order, recover redeemed_key_val, record it.
// -------------------------------------------------------------------------------------------------
#[tokio::test]
async fn self_claim_already_redeemed_recovers_key_from_order() {
    let Some(store) = store_or_skip("sc-recover").await else {
        return;
    };
    seed_available_game(&store, "gkB:mnB", "Two Point Campus").await;
    store
        .claim_game_self("gkB:mnB", "sc-2", now())
        .await
        .unwrap();

    let humble = MockServer::start().await;
    mount_reveal_already_redeemed(&humble).await;
    mount_order_with_redeemed_tpk(&humble, "gkB", "mnB", "RECOVERED-KEY").await;

    let discord = discord_ok().await;
    let deps = deps(store.clone(), &humble.uri(), Some(discord.uri()));
    let resp = handle(&deps, self_claim_req("sc-2", "gkB:mnB", "gkB", "mnB")).await;

    assert_eq!(
        resp,
        FulfillResponse::RevealedKey {
            key: "RECOVERED-KEY".into()
        }
    );
    let claim = store
        .get_claim(SELF_LINK_TOKEN, "sc-2")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(claim.revealed_key.as_deref(), Some("RECOVERED-KEY"));
}

// -------------------------------------------------------------------------------------------------
// Test 3: AlreadyRedeemed but order has no redeemed_key_val → park + ping, never compensate.
// -------------------------------------------------------------------------------------------------
#[tokio::test]
async fn self_claim_already_redeemed_with_no_key_val_parks() {
    let Some(store) = store_or_skip("sc-noval").await else {
        return;
    };
    seed_available_game(&store, "gkC:mnC", "Mystery Game").await;
    store
        .claim_game_self("gkC:mnC", "sc-3", now())
        .await
        .unwrap();

    let humble = MockServer::start().await;
    mount_reveal_already_redeemed(&humble).await;
    mount_order_with_redeemed_tpk_no_val(&humble, "gkC", "mnC").await;

    let discord = discord_ok().await;
    let deps = deps(store.clone(), &humble.uri(), Some(discord.uri()));
    let resp = handle(&deps, self_claim_req("sc-3", "gkC:mnC", "gkC", "mnC")).await;

    assert!(matches!(resp, FulfillResponse::Parked { .. }));
    let claim = store
        .get_claim(SELF_LINK_TOKEN, "sc-3")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(claim.state, domain::ClaimState::Pending);
}

// -------------------------------------------------------------------------------------------------
// Test 4: Transient failure (500) → park, never compensate.
// -------------------------------------------------------------------------------------------------
#[tokio::test]
async fn self_claim_ambiguous_failure_parks_never_compensates() {
    let Some(store) = store_or_skip("sc-park").await else {
        return;
    };
    seed_available_game(&store, "gkD:mnD", "Park Me").await;
    store
        .claim_game_self("gkD:mnD", "sc-4", now())
        .await
        .unwrap();

    let humble = MockServer::start().await;
    mount_reveal_500(&humble).await;

    let deps = deps(store.clone(), &humble.uri(), None);
    let resp = handle(&deps, self_claim_req("sc-4", "gkD:mnD", "gkD", "mnD")).await;
    assert!(matches!(resp, FulfillResponse::Parked { .. }));
    assert_eq!(
        store
            .get_claim(SELF_LINK_TOKEN, "sc-4")
            .await
            .unwrap()
            .unwrap()
            .state,
        domain::ClaimState::Pending
    );
}

// -------------------------------------------------------------------------------------------------
// Test 5: Key VALUE never appears in logs or pings (M2 log-scrubbing).
// -------------------------------------------------------------------------------------------------

/// A `MakeWriter` that captures all written bytes into a shared buffer.
#[derive(Clone)]
struct CaptureBuf(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);

impl std::io::Write for CaptureBuf {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for CaptureBuf {
    type Writer = CaptureBuf;
    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

/// File-scope mutex to serialize the log-capture test. When set_default(tracing_subscriber) is
/// called in a test with #[tokio::test] (multi-threaded runtime), sibling tests running in parallel
/// can interfere: a tokio worker thread picks up the GLOBAL default subscriber (from a concurrent
/// test's set_default call), not the thread-local one set for THIS test's thread, causing logs to
/// either interleave into the wrong capture buffer or not be captured at all. Holding this lock
/// for the test's entire body prevents concurrent test execution and guarantees that set_default
/// affects only this test's subscriber dispatch — no cross-test pollution.
static LOG_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[tokio::test]
#[allow(clippy::await_holding_lock)] // intentional: LOG_TEST_LOCK guards the whole async body (see its doc)
async fn revealed_key_value_never_appears_in_logs_or_pings() {
    use std::sync::{Arc, Mutex};

    let _lock = LOG_TEST_LOCK.lock().unwrap();

    let Some(store_a) = store_or_skip("sc-scrub-a").await else {
        return;
    };
    let Some(store_b) = store_or_skip("sc-scrub-b").await else {
        return;
    };

    let key = "AAAA-BBBB-CCCC";

    // Set up log capture using our MakeWriter shim.
    let log_buf: Arc<Mutex<Vec<u8>>> = Arc::new(Mutex::new(Vec::new()));
    let capture = CaptureBuf(log_buf.clone());
    let sub = tracing_subscriber::fmt()
        .with_writer(capture)
        .with_ansi(false)
        .finish();
    let _guard = tracing::subscriber::set_default(sub);

    // --- Happy path (store_a) ---
    seed_available_game(&store_a, "gkSA:mnSA", "Scrub Game A").await;
    store_a
        .claim_game_self("gkSA:mnSA", "sc-s1", now())
        .await
        .unwrap();
    let humble_a = MockServer::start().await;
    mount_reveal_success(&humble_a, key).await;
    let discord_a = discord_ok().await;
    let deps_a = deps(store_a.clone(), &humble_a.uri(), Some(discord_a.uri()));
    let _ = handle(
        &deps_a,
        FulfillRequest::SelfClaim {
            claim_id: "sc-s1".into(),
            game_id: "gkSA:mnSA".into(),
            gamekey: "gkSA".into(),
            machine_name: "mnSA".into(),
            keyindex: 0,
            requires_choice: false,
        },
    )
    .await;

    // --- Recover path (store_b) ---
    seed_available_game(&store_b, "gkSB:mnSB", "Scrub Game B").await;
    store_b
        .claim_game_self("gkSB:mnSB", "sc-s2", now())
        .await
        .unwrap();
    let humble_b = MockServer::start().await;
    mount_reveal_already_redeemed(&humble_b).await;
    mount_order_with_redeemed_tpk(&humble_b, "gkSB", "mnSB", key).await;
    let discord_b = discord_ok().await;
    let deps_b = deps(store_b.clone(), &humble_b.uri(), Some(discord_b.uri()));
    let _ = handle(
        &deps_b,
        FulfillRequest::SelfClaim {
            claim_id: "sc-s2".into(),
            game_id: "gkSB:mnSB".into(),
            gamekey: "gkSB".into(),
            machine_name: "mnSB".into(),
            keyindex: 0,
            requires_choice: false,
        },
    )
    .await;

    // --- Assert key value never appeared in logs ---
    let captured = {
        let buf = log_buf.lock().unwrap();
        String::from_utf8_lossy(&buf).to_string()
    };

    // Positive assertion: the capture is non-empty (so the test can't pass vacuously).
    assert!(
        !captured.is_empty(),
        "log capture must be non-empty — the test cannot pass vacuously on an empty capture"
    );
    // Assert the capture includes the happy-path reveal info line AND the recover-path record
    // line — proves the subscriber captured BOTH runs' logging. (No substring fallback: the
    // dispatch line alone contains "self-claim" and would satisfy a weaker check vacuously.)
    assert!(
        captured.contains("self-claim reveal returned a key"),
        "captured logs must include the reveal info line: {captured:.500}"
    );
    assert!(
        captured.contains("redeemed_key_val present"),
        "captured logs must include the recover-path record line: {captured:.500}"
    );
    // The key VALUE must never appear in any log line.
    assert!(
        !captured.contains(key),
        "key value leaked into logs: {captured:.500}"
    );

    // --- Assert key value never appeared in pings ---
    let pings_a = discord_a.received_requests().await.unwrap();
    let pings_b = discord_b.received_requests().await.unwrap();
    for req in pings_a.iter().chain(pings_b.iter()) {
        let body = String::from_utf8_lossy(&req.body).to_string();
        assert!(
            !body.contains(key),
            "key value leaked into a discord ping: {body}"
        );
    }
}

// -------------------------------------------------------------------------------------------------
// Pure decision-ladder test: reveal_decision is identical to gift_decision (same Err classification).
// -------------------------------------------------------------------------------------------------
#[test]
fn reveal_decision_ladder_matches_gift_decision() {
    use humble_client::{GiftUrl, HumbleError as E, RevealedKey};
    assert_eq!(
        reveal_decision(&Ok(RevealedKey("k".into()))),
        Decision::Record
    );
    assert_eq!(
        reveal_decision(&Err(E::AlreadyRedeemed)),
        Decision::Compensate
    );
    assert_eq!(
        reveal_decision(&Err(E::Unauthorized)),
        Decision::ParkCookieDead
    );
    assert_eq!(reveal_decision(&Err(E::AmbiguousRedeem)), Decision::Park);
    assert_eq!(
        reveal_decision(&Err(E::RedeemRefused("x".into()))),
        Decision::Park
    );
    assert_eq!(reveal_decision(&Err(E::RateLimited)), Decision::Park);
    assert_eq!(reveal_decision(&Err(E::Api(500))), Decision::Park);
    assert_eq!(
        reveal_decision(&Err(E::RedeemAuthRejected {
            status: 403,
            csrf_minted: false
        })),
        Decision::Park
    );
    assert_eq!(
        reveal_decision(&Err(E::SecureAreaStepUpFailed { reason: "x".into() })),
        Decision::Park
    );
    assert_eq!(
        reveal_decision(&Err(E::LoginFailed { reason: "x".into() })),
        Decision::Park
    );
    assert_eq!(
        reveal_decision(&Err(E::ChooseFailed { reason: "x".into() })),
        Decision::Park
    );
    // gift_decision and reveal_decision must always agree on Err arms (written out explicitly
    // because HumbleError doesn't implement Clone, so a loop would require reconstruction).
    macro_rules! check_agree {
        ($err:expr) => {{
            assert_eq!(
                gift_decision(&Err::<GiftUrl, _>($err)),
                reveal_decision(&Err::<RevealedKey, _>($err))
            );
        }};
    }
    check_agree!(E::AlreadyRedeemed);
    check_agree!(E::Unauthorized);
    check_agree!(E::AmbiguousRedeem);
    check_agree!(E::RateLimited);
    check_agree!(E::Api(500));
    check_agree!(E::RedeemRefused("y".into()));
    check_agree!(E::RedeemAuthRejected {
        status: 403,
        csrf_minted: true
    });
    check_agree!(E::SecureAreaStepUpFailed { reason: "y".into() });
    check_agree!(E::LoginFailed { reason: "y".into() });
    check_agree!(E::ChooseFailed { reason: "y".into() });
}

// =================================================================================================
// Task 7: Self-claim choice path tests.
// =================================================================================================

/// Seed an Available game with `requires_choice=true` — the self-claim choice variant.
async fn seed_choice_game(store: &Store, game_id_str: &str, title: &str) {
    let (gk, mn) = game_id_str
        .split_once(':')
        .expect("game_id must be gamekey:machine_name");
    let g = domain::Game {
        id: game_id_str.into(),
        title: title.into(),
        bundle: "Test Bundle".into(),
        gamekey: gk.into(),
        machine_name: mn.into(),
        key_type: "steam".into(),
        giftable: true,
        hidden: false,
        status: GameStatus::Available,
        claim_id: None,
        artwork_url: None,
        keyindex: 0,
        requires_choice: true,
        steam_app_id: None,
        appid_source: None,
        owned_by_ben: false,
    };
    store.put_game(&g).await.unwrap();
}

/// Mount GET /api/v1/order/{gamekey} returning no tpks — the pre-choose state.
/// Matched `up_to_n_times(1)` so the subsequent mount (post-choose) can serve the new tpk.
async fn mount_order_pre_choose(humble: &MockServer, gamekey: &str) {
    Mock::given(method("GET"))
        .and(path(format!("/api/v1/order/{gamekey}")))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(choice_order_json(gamekey, serde_json::json!([]))),
        )
        .up_to_n_times(1)
        .mount(humble)
        .await;
}

/// Mount GET /api/v1/order/{gamekey} returning a newly-minted unredeemed tpk.
async fn mount_order_post_choose(
    humble: &MockServer,
    gamekey: &str,
    machine_name: &str,
    _keyindex: u32,
) {
    Mock::given(method("GET"))
        .and(path(format!("/api/v1/order/{gamekey}")))
        .respond_with(ResponseTemplate::new(200).set_body_json(choice_order_json(
            gamekey,
            serde_json::json!([tpk_json(machine_name, machine_name, false)]),
        )))
        .mount(humble)
        .await;
}

/// Mount POST /humbler/choosecontent → success (200, success=true). The "asserting_no_is_gift"
/// in the name documents intent — the actual assertion that `is_gift` is absent is made after the
/// call via `humble.received_requests()`.
async fn mount_choose_success_asserting_no_is_gift(humble: &MockServer, _gamekey: &str) {
    Mock::given(method("POST"))
        .and(path("/humbler/choosecontent"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "success": true, "force_refresh": true
        })))
        .mount(humble)
        .await;
}

/// Mount POST /humbler/choosecontent → 500 (ambiguous: pick MAY be spent).
async fn mount_choose_500(humble: &MockServer, _gamekey: &str) {
    Mock::given(method("POST"))
        .and(path("/humbler/choosecontent"))
        .respond_with(ResponseTemplate::new(500))
        .mount(humble)
        .await;
}

fn self_claim_choice_req(
    claim_id: &str,
    game_id: &str,
    gamekey: &str,
    machine_name: &str,
) -> FulfillRequest {
    FulfillRequest::SelfClaim {
        claim_id: claim_id.into(),
        game_id: game_id.into(),
        gamekey: gamekey.into(),
        machine_name: machine_name.into(),
        keyindex: 0,
        requires_choice: true,
    }
}

// -------------------------------------------------------------------------------------------------
// Test 1: Happy path — pre-read (no tpk) → record intent → choose (no is_gift) → re-read → reveal.
// -------------------------------------------------------------------------------------------------
#[tokio::test]
async fn choice_self_claim_chooses_without_gift_then_reveals() {
    let Some(store) = store_or_skip("sc-choice-happy").await else {
        return;
    };
    seed_choice_game(&store, "gkE:offered_sim", "Construction Simulator").await;
    store
        .claim_game_self("gkE:offered_sim", "sc-c1", now())
        .await
        .unwrap();

    let humble = MockServer::start().await;
    mount_order_pre_choose(&humble, "gkE").await;
    mount_choose_success_asserting_no_is_gift(&humble, "gkE").await;
    mount_order_post_choose(&humble, "gkE", "constructionsim_choice_steam", 0).await;
    mount_reveal_success(&humble, "SIM-KEY-123").await;

    let deps = deps(store.clone(), &humble.uri(), None);
    let resp = handle(
        &deps,
        FulfillRequest::SelfClaim {
            claim_id: "sc-c1".into(),
            game_id: "gkE:offered_sim".into(),
            gamekey: "gkE".into(),
            machine_name: "offered_sim".into(),
            keyindex: 0,
            requires_choice: true,
        },
    )
    .await;

    assert_eq!(
        resp,
        FulfillResponse::RevealedKey {
            key: "SIM-KEY-123".into()
        }
    );
    let claim = store
        .get_claim(SELF_LINK_TOKEN, "sc-c1")
        .await
        .unwrap()
        .unwrap();
    assert!(
        claim.choice_pre_tpks.is_some(),
        "intent snapshot must be recorded before choose"
    );
    assert_eq!(claim.revealed_key.as_deref(), Some("SIM-KEY-123"));

    // Assert is_gift was NOT sent in the choose POST (self-claim must NOT use is_gift).
    let reqs = humble.received_requests().await.unwrap();
    let choose = reqs
        .iter()
        .find(|r| r.url.path() == "/humbler/choosecontent")
        .unwrap();
    let body = String::from_utf8(choose.body.clone()).unwrap();
    assert!(
        !body.contains("is_gift"),
        "self-claim choose must not send is_gift: {body}"
    );
}

// -------------------------------------------------------------------------------------------------
// Test 2: Ambiguous choose (500) → park, no reveal attempted.
// -------------------------------------------------------------------------------------------------
#[tokio::test]
async fn choice_self_claim_ambiguous_choose_parks_no_reveal_attempted() {
    let Some(store) = store_or_skip("sc-choice-park").await else {
        return;
    };
    seed_choice_game(&store, "gkF:offered_x", "Parked Sim").await;
    store
        .claim_game_self("gkF:offered_x", "sc-c2", now())
        .await
        .unwrap();

    let humble = MockServer::start().await;
    mount_order_pre_choose(&humble, "gkF").await;
    mount_choose_500(&humble, "gkF").await;

    let deps = deps(store.clone(), &humble.uri(), None);
    let resp = handle(
        &deps,
        self_claim_choice_req("sc-c2", "gkF:offered_x", "gkF", "offered_x"),
    )
    .await;

    assert!(matches!(resp, FulfillResponse::Parked { .. }));
    // No reveal POST was attempted.
    let reqs = humble.received_requests().await.unwrap();
    assert_eq!(
        count_path(&reqs, "/humbler/redeemkey"),
        0,
        "no reveal should be attempted on ambiguous choose"
    );
}

// =================================================================================================
// Reconcile SELF-claim tests (Task 8)
// =================================================================================================

/// A timestamp old enough to pass the RECONCILE_MIN_AGE gate (15 minutes).
fn old_enough() -> OffsetDateTime {
    hours_ago(1)
}

/// Drive one reconcile pass via handle(Sync). The listing will 404 (no mock mounted),
/// which is treated as a transient failure → reconcile still runs over all parked claims.
async fn run_reconcile(d: &Deps) {
    let _ = handle(d, FulfillRequest::Sync).await;
}

/// Mount GET /api/v1/order/{gamekey} returning a choice order with one unredeemed tpk.
async fn mount_order_with_unredeemed_tpk(
    humble: &MockServer,
    gamekey: &str,
    machine_name: &str,
    _keyindex: u32,
    human_name: &str,
) {
    Mock::given(method("GET"))
        .and(path(format!("/api/v1/order/{gamekey}")))
        .respond_with(ResponseTemplate::new(200).set_body_json(choice_order_json(
            gamekey,
            serde_json::json!([tpk_json(machine_name, human_name, false)]),
        )))
        .mount(humble)
        .await;
}

// -------------------------------------------------------------------------------------------------
// Task 8 – Test 1: SELF choice claim, no intent snapshot → arm A → compensate_self_claim.
// -------------------------------------------------------------------------------------------------
#[tokio::test]
async fn reconcile_self_choice_no_snapshot_compensates_via_self_variant() {
    let Some(store) = store_or_skip("sc-rec-choice-no-snap").await else {
        return;
    };
    seed_choice_game(&store, "gkG:off_g", "Reconcile Me").await;
    store
        .claim_game_self("gkG:off_g", "sc-r1", old_enough())
        .await
        .unwrap();

    let humble = MockServer::start().await;
    // Order has no tpks and no snapshot on the claim → arm A → compensate_self_claim.
    mount_order_pre_choose(&humble, "gkG").await;
    // Deliberately NO choosecontent mock.

    let deps_val = deps(store.clone(), &humble.uri(), None);
    run_reconcile(&deps_val).await;

    let claim = store
        .get_claim(SELF_LINK_TOKEN, "sc-r1")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        claim.state,
        ClaimState::Compensated,
        "self choice arm A must compensate via self-variant"
    );
    assert_eq!(
        store.get_game("gkG:off_g").await.unwrap().unwrap().status,
        GameStatus::Available,
        "game must be re-listed after compensate"
    );
    let reqs = humble.received_requests().await.unwrap();
    assert_eq!(
        count_path(&reqs, "/humbler/choosecontent"),
        0,
        "reconcile must never call choosecontent"
    );
}

// -------------------------------------------------------------------------------------------------
// Task 8 – Test 2: SELF choice claim, B2 (snapshot + unredeemed tpk) → reveal, no choose.
// -------------------------------------------------------------------------------------------------
#[tokio::test]
async fn reconcile_self_choice_b2_reveals_never_chooses() {
    let Some(store) = store_or_skip("sc-rec-choice-b2").await else {
        return;
    };
    seed_choice_game(&store, "gkH:off_h", "Crashed Mid-Claim").await;
    store
        .claim_game_self("gkH:off_h", "sc-r2", old_enough())
        .await
        .unwrap();
    // pre=[] → any tpk present in the order is "new".
    store
        .record_choice_intent(SELF_LINK_TOKEN, "sc-r2", vec![])
        .await
        .unwrap();

    let humble = MockServer::start().await;
    // Order: unredeemed tpk present → B2 → reveal (not redeem, not choose).
    mount_order_with_unredeemed_tpk(&humble, "gkH", "off_h_choice_steam", 0, "Crashed Mid-Claim")
        .await;
    mount_reveal_success(&humble, "RECONCILED-KEY").await;
    // Deliberately NO choosecontent mock.

    let deps_val = deps(store.clone(), &humble.uri(), None);
    run_reconcile(&deps_val).await;

    let claim = store
        .get_claim(SELF_LINK_TOKEN, "sc-r2")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        claim.state,
        ClaimState::Fulfilled,
        "self choice B2 must be fulfilled"
    );
    assert_eq!(claim.revealed_key.as_deref(), Some("RECONCILED-KEY"));
    let reqs = humble.received_requests().await.unwrap();
    assert_eq!(
        count_path(&reqs, "/humbler/choosecontent"),
        0,
        "reconcile must never call choosecontent"
    );
}

// -------------------------------------------------------------------------------------------------
// Task 8 – Test 3: SELF bundle claim, tpk already redeemed → recover_already_redeemed_key.
// -------------------------------------------------------------------------------------------------
#[tokio::test]
async fn reconcile_self_bundle_already_redeemed_recovers_key() {
    let Some(store) = store_or_skip("sc-rec-bundle-redeemed").await else {
        return;
    };
    seed_available_game(&store, "gkI:mnI", "Old Reveal").await;
    store
        .claim_game_self("gkI:mnI", "sc-r3", old_enough())
        .await
        .unwrap();

    let humble = MockServer::start().await;
    // Order shows the tpk already redeemed with a recoverable key value.
    // Mounted unlimited — reconcile reads it once; recover_already_redeemed_key re-reads it.
    mount_order_with_redeemed_tpk(&humble, "gkI", "mnI", "OLD-KEY").await;

    let deps_val = deps(store.clone(), &humble.uri(), None);
    run_reconcile(&deps_val).await;

    let claim = store
        .get_claim(SELF_LINK_TOKEN, "sc-r3")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(claim.revealed_key.as_deref(), Some("OLD-KEY"));
    assert_eq!(
        claim.state,
        ClaimState::Fulfilled,
        "self bundle already-redeemed must be recovered, not pinged"
    );
}

// -------------------------------------------------------------------------------------------------
// Fix I1: SELF bundle claim, tpk NOT redeemed → reveal (not compensate).
// A parked SELF bundle claim must call reveal_claimed_tpk, not compensate_any, so the key appears
// under self-claims. The redeemed arm (Test 3) exercises recovery; this arm exercises reveal.
// -------------------------------------------------------------------------------------------------
#[tokio::test]
async fn reconcile_self_bundle_not_redeemed_reveals() {
    let Some(store) = store_or_skip("sc-rec-bundle-notredeemed").await else {
        return;
    };
    seed_available_game(&store, "gkJ:mnJ", "Parked Self Game").await;
    store
        .claim_game_self("gkJ:mnJ", "sc-r4", old_enough())
        .await
        .unwrap();

    let humble = MockServer::start().await;
    // Order shows tpk NOT redeemed — the reveal never landed on humble.
    Mock::given(method("GET"))
        .and(path("/api/v1/order/gkJ"))
        .respond_with(ResponseTemplate::new(200).set_body_json(order_json("gkJ", "mnJ", false)))
        .mount(&humble)
        .await;
    mount_reveal_success(&humble, "REVEALED-KEY").await;
    // Deliberately NO choosecontent mock — reconcile must never choose.

    let deps_val = deps(store.clone(), &humble.uri(), None);
    run_reconcile(&deps_val).await;

    let claim = store
        .get_claim(SELF_LINK_TOKEN, "sc-r4")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        claim.state,
        ClaimState::Fulfilled,
        "self bundle not-redeemed must reveal and record, not compensate"
    );
    assert_eq!(
        claim.revealed_key.as_deref(),
        Some("REVEALED-KEY"),
        "revealed key must be recorded on the claim"
    );
    assert_eq!(
        store.get_game("gkJ:mnJ").await.unwrap().unwrap().status,
        GameStatus::BenRedeemed,
        "game must flip to BenRedeemed after reconcile reveal"
    );
    let reqs = humble.received_requests().await.unwrap();
    assert_eq!(
        count_path(&reqs, "/humbler/choosecontent"),
        0,
        "reconcile must never call choosecontent"
    );
}

// =================================================================================================
// TASK 6: steam appid mapper — tier-1 walk flow-through + lazy unique-exact-title pass
// =================================================================================================

fn steam_client_at(uri: &str) -> Arc<steam_client::SteamClient> {
    Arc::new(
        steam_client::SteamClient::new(
            uri,
            uri,
            uri,
            steam_client::SteamApiKey::new("test-api-key".into()),
        )
        .unwrap(),
    )
}

async fn seed_steam_game(
    store: &Store,
    gamekey: &str,
    machine_name: &str,
    title: &str,
    steam_app_id: Option<u32>,
    appid_source: Option<AppidSource>,
) -> String {
    let gid = game_id(gamekey, machine_name);
    let g = Game {
        id: gid.clone(),
        title: title.into(),
        bundle: "Some Bundle".into(),
        gamekey: gamekey.into(),
        machine_name: machine_name.into(),
        key_type: "steam".into(),
        giftable: true,
        hidden: false,
        status: GameStatus::Available,
        claim_id: None,
        artwork_url: None,
        keyindex: 0,
        requires_choice: false,
        steam_app_id,
        appid_source,
        owned_by_ben: false,
    };
    store.put_game(&g).await.unwrap();
    gid
}

// -------------------------------------------------------------------------------------------------
// Test 1: Walk carries tier-1 appid.
// An order walk with a KeyEntry that has steam_app_id: Some(570) → the stored game ends up with
// steam_app_id: Some(570) and appid_source: Some(AppidSource::Humble).
// -------------------------------------------------------------------------------------------------
#[tokio::test]
async fn sync_walk_carries_tier1_appid() {
    let Some(store) = store_or_skip("t6-tier1-walk").await else {
        return;
    };

    let humble = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/user/order"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([
            {"gamekey": "gk-tier1"}
        ])))
        .mount(&humble)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v1/order/gk-tier1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "gamekey": "gk-tier1",
            "product": { "human_name": "Test Bundle" },
            "tpkd_dict": { "all_tpks": [{
                "machine_name": "dota2_steam",
                "human_name": "Dota 2",
                "key_type": "steam",
                "is_expired": false,
                "keyindex": 0,
                "steam_app_id": 570
            }] },
            "subproducts": [],
        })))
        .mount(&humble)
        .await;

    // No Steam client — tier-1 flows from the tpk wire data directly, not from the title pass.
    let deps = deps(store, &humble.uri(), None);
    handle(&deps, FulfillRequest::Sync).await;

    let gid = game_id("gk-tier1", "dota2_steam");
    let game = deps
        .store
        .get_game(&gid)
        .await
        .unwrap()
        .expect("game must be written by the order walk");
    assert_eq!(
        game.steam_app_id,
        Some(570),
        "tier-1: steam_app_id must be carried from the tpk wire data"
    );
    assert_eq!(
        game.appid_source,
        Some(AppidSource::Humble),
        "tier-1: appid_source must be Humble"
    );
}

// -------------------------------------------------------------------------------------------------
// Test 2: Title pass maps unique match + leaves ambiguous (duplicate name) unmapped.
// Given two games — one whose title appears exactly once in the Steam app list, one whose title
// appears twice — the unique one gets mapped (appid_source: Title), the dup stays unmapped.
// -------------------------------------------------------------------------------------------------
#[tokio::test]
async fn title_pass_maps_unique_leaves_dup_unmapped() {
    let Some(store) = store_or_skip("t6-title-pass").await else {
        return;
    };

    // Seed two games with no appid — no orders, so the walk won't touch them.
    seed_steam_game(&store, "gk-uniq", "mn-uniq", "Unique Game", None, None).await;
    seed_steam_game(&store, "gk-dup", "mn-dup", "Dup Game", None, None).await;

    let humble = MockServer::start().await;
    mount_empty_listing(&humble).await;

    let steam_mock = MockServer::start().await;
    // "Unique Game" appears once → appid 1001. "Dup Game" appears twice → ambiguous, skip.
    Mock::given(method("GET"))
        .and(path("/IStoreService/GetAppList/v1/"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "response": { "apps": [
                { "appid": 1001, "name": "Unique Game" },
                { "appid": 2001, "name": "Dup Game" },
                { "appid": 2002, "name": "Dup Game" }
            ]}
        })))
        .mount(&steam_mock)
        .await;

    let mut d = deps(store, &humble.uri(), None);
    d.steam = Some(steam_client_at(&steam_mock.uri()));
    handle(&d, FulfillRequest::Sync).await;

    let unique = d
        .store
        .get_game(&game_id("gk-uniq", "mn-uniq"))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        unique.steam_app_id,
        Some(1001),
        "unique exact title match must be mapped by the title pass"
    );
    assert_eq!(
        unique.appid_source,
        Some(AppidSource::Title),
        "appid_source must be Title for a title-pass write"
    );

    let dup = d
        .store
        .get_game(&game_id("gk-dup", "mn-dup"))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        dup.steam_app_id, None,
        "ambiguous title (multiple Steam entries) must remain unmapped"
    );
    assert_eq!(
        dup.appid_source, None,
        "appid_source must remain None for an unmapped game"
    );
}

// -------------------------------------------------------------------------------------------------
// Test 3: Manual appid untouched by both walk and title pass.
// A game with appid_source: Some(Manual) must survive sync unchanged — the walk's merge rule
// and the title pass's guard both refuse to overwrite a Manual-sourced appid.
// -------------------------------------------------------------------------------------------------
#[tokio::test]
async fn manual_appid_untouched_by_walk_and_title_pass() {
    let Some(store) = store_or_skip("t6-manual-guard").await else {
        return;
    };

    // Game with Manual appid — must not be touched.
    let gid = seed_steam_game(
        &store,
        "gk-man",
        "mn-man",
        "Portal",
        Some(400),
        Some(AppidSource::Manual),
    )
    .await;
    // Sentinel: an unmapped game the title pass WILL map. Proves the pass actually ran —
    // without it, an app-list fetch failure (as when ISteamApps/GetAppList died, #48) makes
    // the Manual-guard assertions below pass vacuously.
    seed_steam_game(&store, "gk-sent", "mn-sent", "Half-Life", None, None).await;

    let humble = MockServer::start().await;
    mount_empty_listing(&humble).await;

    let steam_mock = MockServer::start().await;
    // App list returns "Portal" → 9999 (would overwrite if the guard failed) + the sentinel.
    Mock::given(method("GET"))
        .and(path("/IStoreService/GetAppList/v1/"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "response": { "apps": [
                { "appid": 9999, "name": "Portal" },
                { "appid": 70, "name": "Half-Life" }
            ]}
        })))
        .mount(&steam_mock)
        .await;

    let mut d = deps(store, &humble.uri(), None);
    d.steam = Some(steam_client_at(&steam_mock.uri()));
    handle(&d, FulfillRequest::Sync).await;

    let sentinel = d
        .store
        .get_game(&game_id("gk-sent", "mn-sent"))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        sentinel.steam_app_id,
        Some(70),
        "sentinel must be mapped — otherwise the title pass never ran and the Manual-guard \
         assertions below prove nothing"
    );

    let game = d.store.get_game(&gid).await.unwrap().unwrap();
    assert_eq!(
        game.steam_app_id,
        Some(400),
        "Manual appid must not be overwritten by the title pass"
    );
    assert_eq!(
        game.appid_source,
        Some(AppidSource::Manual),
        "Manual appid_source must not be overwritten by the title pass"
    );
}

// -------------------------------------------------------------------------------------------------
// FIX 5: normalize collapses internal whitespace left by ™/® stripping.
// "Cities: Skylines ™ II" → after strip becomes "cities: skylines  ii" (double space),
// which never matched "cities: skylines ii". Fix: split_whitespace().join(" ").
// RED: before the fix, the catalog title with embedded ™ fails to match the Steam app name.
// -------------------------------------------------------------------------------------------------
#[tokio::test]
async fn title_pass_maps_title_with_trademark_symbol() {
    let Some(store) = store_or_skip("t6-trademark-normalize").await else {
        return;
    };

    // Catalog title contains ™ with surrounding spaces (as Humble often formats it).
    seed_steam_game(
        &store,
        "gk-tm",
        "mn-tm",
        "Cities: Skylines ™ II",
        None,
        None,
    )
    .await;

    let humble = MockServer::start().await;
    mount_empty_listing(&humble).await;

    let steam_mock = MockServer::start().await;
    // Steam app list has the same title WITHOUT the ™.
    Mock::given(method("GET"))
        .and(path("/IStoreService/GetAppList/v1/"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "response": { "apps": [
                { "appid": 5555, "name": "Cities: Skylines II" }
            ]}
        })))
        .mount(&steam_mock)
        .await;

    let mut d = deps(store, &humble.uri(), None);
    d.steam = Some(steam_client_at(&steam_mock.uri()));
    handle(&d, FulfillRequest::Sync).await;

    let tm_game = d
        .store
        .get_game(&game_id("gk-tm", "mn-tm"))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        tm_game.steam_app_id,
        Some(5555),
        "title with ™ must match after internal whitespace collapse — FIX 5"
    );
    assert_eq!(
        tm_game.appid_source,
        Some(AppidSource::Title),
        "appid_source must be Title for the trademark-normalized match"
    );
}

// =================================================================================================
// TASK 8: refresh_ben_ownership — owned_by_ben stamping (spec M1)
// =================================================================================================

// -------------------------------------------------------------------------------------------------
// Test 1: Successful fetch stamps intersection + unstamps disjoint.
// Games in Ben's library → owned_by_ben=true; games NOT in his library → owned_by_ben=false.
// Games with no steam_app_id are skipped entirely.
// -------------------------------------------------------------------------------------------------
#[tokio::test]
async fn refresh_ben_ownership_stamps_owned_games() {
    let Some(store) = store_or_skip("t8-owned-games").await else {
        return;
    };

    // game_a: appid 1001, currently NOT owned — should become owned after sync
    let gid_a = seed_steam_game(&store, "gk-a", "mn-a", "Game A", Some(1001), None).await;
    // game_b: appid 1002, currently owned — NOT in fetched library, should become NOT owned
    let gid_b = seed_steam_game(&store, "gk-b", "mn-b", "Game B", Some(1002), None).await;
    store.set_game_owned_by_ben(&gid_b, true).await.unwrap();
    // game_c: no appid — untouched regardless of library contents
    let gid_c = seed_steam_game(&store, "gk-c", "mn-c", "Game C", None, None).await;

    // Plant Ben's Steam identity
    store.put_steam_identity("76561198000000001").await.unwrap();

    let humble = MockServer::start().await;
    mount_empty_listing(&humble).await;

    let steam_mock = MockServer::start().await;
    // Ben owns appid 1001 only; appid 1002 is not in his library.
    Mock::given(method("GET"))
        .and(path("/IPlayerService/GetOwnedGames/v0001/"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "response": {
                "game_count": 1,
                "games": [{ "appid": 1001 }]
            }
        })))
        .mount(&steam_mock)
        .await;

    let mut d = deps(store, &humble.uri(), None);
    d.steam = Some(steam_client_at(&steam_mock.uri()));
    handle(&d, FulfillRequest::Sync).await;

    let game_a = d.store.get_game(&gid_a).await.unwrap().unwrap();
    assert!(
        game_a.owned_by_ben,
        "game_a (appid 1001, in library) must be stamped owned_by_ben=true"
    );

    let game_b = d.store.get_game(&gid_b).await.unwrap().unwrap();
    assert!(
        !game_b.owned_by_ben,
        "game_b (appid 1002, NOT in library) must be unstamped to owned_by_ben=false"
    );

    let game_c = d.store.get_game(&gid_c).await.unwrap().unwrap();
    assert!(
        !game_c.owned_by_ben,
        "game_c (no appid) must remain owned_by_ben=false — no stamp written"
    );
}

// -------------------------------------------------------------------------------------------------
// Test 2: Private response keeps stamps frozen + logs + pings.
// When Ben's library privacy blocks the fetch, existing stamps must be preserved and Ben gets
// a single Discord ping (because a prior successful run is signalled by a STEAMOWN entry).
// -------------------------------------------------------------------------------------------------
#[tokio::test]
async fn refresh_ben_ownership_private_keeps_stamps_and_pings() {
    let Some(store) = store_or_skip("t8-private").await else {
        return;
    };

    const STEAMID: &str = "76561198000000002";

    // game with an existing owned_by_ben=true stamp — must stay true after a Private response.
    let gid = seed_steam_game(&store, "gk-p", "mn-p", "Owned Game", Some(9999), None).await;
    store.set_game_owned_by_ben(&gid, true).await.unwrap();

    store.put_steam_identity(STEAMID).await.unwrap();

    // Seed STEAMOWN to simulate a prior successful fetch — the ping dedupe fires on its presence.
    store
        .put_steam_owned(
            STEAMID,
            &[9999],
            time::OffsetDateTime::now_utc().unix_timestamp(),
        )
        .await
        .unwrap();

    let humble = MockServer::start().await;
    mount_empty_listing(&humble).await;

    let discord = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&discord)
        .await;

    let steam_mock = MockServer::start().await;
    // Private: response object with no game_count field.
    Mock::given(method("GET"))
        .and(path("/IPlayerService/GetOwnedGames/v0001/"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "response": {}
        })))
        .mount(&steam_mock)
        .await;

    let mut d = deps(store, &humble.uri(), Some(discord.uri()));
    d.steam = Some(steam_client_at(&steam_mock.uri()));
    handle(&d, FulfillRequest::Sync).await;

    // Stamp must be frozen — not cleared.
    let game = d.store.get_game(&gid).await.unwrap().unwrap();
    assert!(
        game.owned_by_ben,
        "Private response must NOT clear owned_by_ben stamps — they stay frozen"
    );

    // Exactly one ping must have been sent (dedupe via STEAMOWN presence).
    let pings = discord.received_requests().await.unwrap();
    assert_eq!(
        pings.len(),
        1,
        "exactly one ping must be sent on Private when a prior success exists"
    );
    let body = String::from_utf8(pings[0].body.clone()).unwrap();
    assert!(
        body.contains("privacy") || body.contains("owned badges"),
        "ping body must mention privacy / owned badges"
    );
}

// -------------------------------------------------------------------------------------------------
// Test 3: Transient error keeps stamps + no ping.
// A non-2xx response from GetOwnedGames is a transient failure — stamps stay frozen and NO ping
// is sent (it's not actionable by Ben, so don't noise the channel).
// -------------------------------------------------------------------------------------------------
#[tokio::test]
async fn refresh_ben_ownership_transient_error_keeps_stamps_no_ping() {
    let Some(store) = store_or_skip("t8-error").await else {
        return;
    };

    const STEAMID: &str = "76561198000000003";

    // game with an existing owned_by_ben=true stamp — must stay true after an error response.
    let gid = seed_steam_game(&store, "gk-e", "mn-e", "Error Game", Some(7777), None).await;
    store.set_game_owned_by_ben(&gid, true).await.unwrap();

    store.put_steam_identity(STEAMID).await.unwrap();

    let humble = MockServer::start().await;
    mount_empty_listing(&humble).await;

    let discord = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&discord)
        .await;

    let steam_mock = MockServer::start().await;
    // Transient 500 from GetOwnedGames.
    Mock::given(method("GET"))
        .and(path("/IPlayerService/GetOwnedGames/v0001/"))
        .respond_with(ResponseTemplate::new(500))
        .mount(&steam_mock)
        .await;

    let mut d = deps(store, &humble.uri(), Some(discord.uri()));
    d.steam = Some(steam_client_at(&steam_mock.uri()));
    handle(&d, FulfillRequest::Sync).await;

    // Stamp must be frozen.
    let game = d.store.get_game(&gid).await.unwrap().unwrap();
    assert!(
        game.owned_by_ben,
        "transient error must NOT clear owned_by_ben stamps — they stay frozen"
    );

    // No pings must have been sent on a transient error.
    let pings = discord.received_requests().await.unwrap();
    assert_eq!(pings.len(), 0, "no ping must be sent on a transient error");
}

// =================================================================================================
// TASK 3: enrich_steam_apps — budgeted, politely-paced Steam enrichment pass (spec §3)
//
// Driven by calling enrich_steam_apps directly with an injected tokio deadline. Pacing sleeps are
// virtual under `start_paused` (no real 1.5s waits); staleness uses wall-clock windows (14d/30d)
// that dwarf test runtime, so real `now` is deterministic enough. Storefront bodies are shaped
// from the 2026-07-06 captures.
// =================================================================================================

/// A deadline far enough out that the per-app pacing sleeps never trip it (budget, not deadline,
/// is under test). Under `start_paused` this is virtual time.
fn far_deadline() -> tokio::time::Instant {
    tokio::time::Instant::now() + std::time::Duration::from_secs(3600)
}

fn appdetails_found_body(name: &str) -> serde_json::Value {
    // Key is ignored by the client (it reads the first value); shape mirrors the capture.
    serde_json::json!({
        "0": {
            "success": true,
            "data": {
                "steam_appid": 1,
                "name": name,
                "developers": ["ConcernedApe"],
                "publishers": ["ConcernedApe"],
                "genres": [{ "id": "23", "description": "Indie" }],
                "categories": [{ "id": 2, "description": "Single-player" }],
                "release_date": { "coming_soon": false, "date": "Feb 26, 2016" },
                "short_description": "desc",
                "header_image": "https://img.example/header.jpg",
                "movies": [{
                    "id": 1, "name": "Trailer",
                    "thumbnail": "https://img.example/thumb.jpg",
                    "hls_h264": "https://vid.example/master.m3u8"
                }]
            }
        }
    })
}

fn appdetails_delisted_body() -> serde_json::Value {
    serde_json::json!({ "0": { "success": false } })
}

fn reviews_body() -> serde_json::Value {
    serde_json::json!({
        "success": 1,
        "query_summary": {
            "num_reviews": 0,
            "review_score": 9,
            "review_score_desc": "Overwhelmingly Positive",
            "total_positive": 455578,
            "total_negative": 5303,
            "total_reviews": 460881
        },
        "reviews": []
    })
}

fn histogram_body() -> serde_json::Value {
    serde_json::json!({
        "success": 1,
        "results": {
            "start_date": 0,
            "end_date": 0,
            "weeks": [],
            "rollups": [],
            "recent": [
                { "date": 1, "recommendations_up": 293, "recommendations_down": 3 },
                { "date": 2, "recommendations_up": 285, "recommendations_down": 5 }
            ]
        }
    })
}

/// Mount success storefront mocks (appdetails + appreviews + histogram) for one appid.
async fn mount_steam_ok(steam: &MockServer, app_id: u32) {
    Mock::given(method("GET"))
        .and(path("/api/appdetails"))
        .and(query_param("appids", app_id.to_string()))
        .respond_with(ResponseTemplate::new(200).set_body_json(appdetails_found_body("Game")))
        .mount(steam)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("/appreviews/{app_id}")))
        .respond_with(ResponseTemplate::new(200).set_body_json(reviews_body()))
        .mount(steam)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("/appreviewhistogram/{app_id}")))
        .respond_with(ResponseTemplate::new(200).set_body_json(histogram_body()))
        .mount(steam)
        .await;
}

/// A fully-fresh cache item stamped `now` on both clocks (so it's never in the work list).
fn fresh_cache(app_id: u32, now: i64) -> dynamo::SteamAppCache {
    dynamo::SteamAppCache {
        app_id,
        detail: Some(steam_client::SteamAppDetail {
            app_id,
            name: "Cached".into(),
            developers: vec![],
            publishers: vec![],
            genres: vec![],
            release_date: None,
            short_description: "cached".into(),
            header_image: None,
            video_hls_url: None,
            video_thumbnail: None,
        }),
        overall: Some(steam_client::ReviewSummary {
            desc: "Positive".into(),
            total_positive: 10,
            total_negative: 1,
            total_reviews: 11,
        }),
        recent: Some(steam_client::RecentReviews {
            percent_positive: 90,
            count: 11,
        }),
        fetched_at: now,
        reviews_fetched_at: now,
    }
}

fn days_ago(days: i64) -> i64 {
    OffsetDateTime::now_utc().unix_timestamp() - days * 24 * 60 * 60
}

/// A steam mock that 404s everything, so any storefront call is a countable miss.
async fn steam_mock_empty() -> MockServer {
    MockServer::start().await
}

// -------------------------------------------------------------------------------------------------
// (a) Fresh cache items → ZERO storefront calls.
// -------------------------------------------------------------------------------------------------
#[tokio::test]
async fn enrich_fresh_items_make_zero_storefront_calls() {
    let Some(store) = store_or_skip("t3-fresh-zero").await else {
        return;
    };
    let now = OffsetDateTime::now_utc().unix_timestamp();
    seed_steam_game(&store, "gk-f", "mn-f", "Fresh Game", Some(413150), None).await;
    store
        .put_steam_app(&fresh_cache(413150, now))
        .await
        .unwrap();

    let steam_mock = steam_mock_empty().await;
    let mut d = deps(store, "http://unused", None);
    d.steam = Some(steam_client_at(&steam_mock.uri()));

    enrich_steam_apps(&d, far_deadline()).await;

    let reqs = steam_mock.received_requests().await.unwrap();
    assert_eq!(
        reqs.len(),
        0,
        "fresh cache item must trigger ZERO storefront calls, got {}",
        reqs.len()
    );
}

// -------------------------------------------------------------------------------------------------
// (b) Stale-reviews-only → exactly 2 calls (appreviews + histogram), NO appdetails refetch.
// -------------------------------------------------------------------------------------------------
#[tokio::test]
async fn enrich_stale_reviews_only_skips_appdetails() {
    let Some(store) = store_or_skip("t3-stale-reviews").await else {
        return;
    };
    let now = OffsetDateTime::now_utc().unix_timestamp();
    let app_id = 413150;
    seed_steam_game(&store, "gk-r", "mn-r", "Reviews Game", Some(app_id), None).await;
    // Detail fresh (now), reviews stale (15 days old > 14d window).
    let mut cache = fresh_cache(app_id, now);
    cache.reviews_fetched_at = days_ago(15);
    store.put_steam_app(&cache).await.unwrap();

    let steam_mock = steam_mock_empty().await;
    mount_steam_ok(&steam_mock, app_id).await;

    let mut d = deps(store, "http://unused", None);
    d.steam = Some(steam_client_at(&steam_mock.uri()));

    enrich_steam_apps(&d, far_deadline()).await;

    let reqs = steam_mock.received_requests().await.unwrap();
    assert_eq!(
        count_path(&reqs, "/api/appdetails"),
        0,
        "fresh appdetails clock must NOT be refetched"
    );
    assert_eq!(
        count_path(&reqs, &format!("/appreviews/{app_id}")),
        1,
        "stale reviews must fetch the review summary"
    );
    assert_eq!(
        count_path(&reqs, &format!("/appreviewhistogram/{app_id}")),
        1,
        "stale reviews must fetch the histogram"
    );
    assert_eq!(reqs.len(), 2, "exactly two storefront calls total");
}

// -------------------------------------------------------------------------------------------------
// (c) 429 on the 3rd app aborts — apps 1-2 persisted, 4+ untouched.
// -------------------------------------------------------------------------------------------------
#[tokio::test]
async fn enrich_429_on_third_app_aborts_pass() {
    let Some(store) = store_or_skip("t3-429-abort").await else {
        return;
    };
    // Ascending appid order = processing order: 100, 200, 300, 400.
    seed_steam_game(&store, "gk1", "mn1", "G1", Some(100), None).await;
    seed_steam_game(&store, "gk2", "mn2", "G2", Some(200), None).await;
    seed_steam_game(&store, "gk3", "mn3", "G3", Some(300), None).await;
    seed_steam_game(&store, "gk4", "mn4", "G4", Some(400), None).await;

    let steam_mock = steam_mock_empty().await;
    mount_steam_ok(&steam_mock, 100).await;
    mount_steam_ok(&steam_mock, 200).await;
    // 3rd app: appdetails 429 → whole pass aborts.
    Mock::given(method("GET"))
        .and(path("/api/appdetails"))
        .and(query_param("appids", "300"))
        .respond_with(ResponseTemplate::new(429))
        .mount(&steam_mock)
        .await;
    mount_steam_ok(&steam_mock, 400).await;

    let mut d = deps(store, "http://unused", None);
    d.steam = Some(steam_client_at(&steam_mock.uri()));

    enrich_steam_apps(&d, far_deadline()).await;

    assert!(
        d.store.get_steam_app(100).await.unwrap().is_some(),
        "app 100 must be persisted before the abort"
    );
    assert!(
        d.store.get_steam_app(200).await.unwrap().is_some(),
        "app 200 must be persisted before the abort"
    );
    assert!(
        d.store.get_steam_app(300).await.unwrap().is_none(),
        "app 300 (429) must NOT be persisted"
    );
    assert!(
        d.store.get_steam_app(400).await.unwrap().is_none(),
        "app 400 must be untouched — the pass aborted before reaching it"
    );
    // App 400's storefront must never have been hit.
    let reqs = steam_mock.received_requests().await.unwrap();
    assert_eq!(
        count_path(&reqs, "/appreviews/400"),
        0,
        "app 400 must never be fetched after the 429 abort"
    );
}

// -------------------------------------------------------------------------------------------------
// (d) Delisted → stub written, and NOT refetched on a fresh-window rerun.
// -------------------------------------------------------------------------------------------------
#[tokio::test]
async fn enrich_delisted_writes_stub_and_does_not_refetch() {
    let Some(store) = store_or_skip("t3-delisted").await else {
        return;
    };
    let app_id = 999888;
    seed_steam_game(&store, "gk-d", "mn-d", "Delisted Game", Some(app_id), None).await;

    let steam_mock = steam_mock_empty().await;
    // appdetails → success:false (delisted). No reviews mounts — a delisted app must skip them.
    Mock::given(method("GET"))
        .and(path("/api/appdetails"))
        .and(query_param("appids", app_id.to_string()))
        .respond_with(ResponseTemplate::new(200).set_body_json(appdetails_delisted_body()))
        .mount(&steam_mock)
        .await;

    let mut d = deps(store, "http://unused", None);
    d.steam = Some(steam_client_at(&steam_mock.uri()));

    // First run: writes the negative-cache stub.
    enrich_steam_apps(&d, far_deadline()).await;

    let stub = d
        .store
        .get_steam_app(app_id)
        .await
        .unwrap()
        .expect("delisted app must write a negative-cache stub");
    assert!(
        stub.detail.is_none(),
        "delisted stub must have detail: None"
    );
    assert!(stub.fetched_at > 0, "delisted stub must stamp fetched_at");
    assert!(
        stub.reviews_fetched_at > 0,
        "delisted stub must stamp reviews_fetched_at too, so it isn't retried every sync"
    );

    // Second run on the same fresh window → the stub is fresh, so ZERO further calls.
    enrich_steam_apps(&d, far_deadline()).await;

    let reqs = steam_mock.received_requests().await.unwrap();
    assert_eq!(
        count_path(&reqs, "/api/appdetails"),
        1,
        "delisted stub must NOT be refetched on a fresh-window rerun (exactly one appdetails call total)"
    );
    // And a delisted app never fetches reviews.
    assert_eq!(
        count_path(&reqs, &format!("/appreviews/{app_id}")),
        0,
        "a delisted app must never fetch reviews"
    );
}

// -------------------------------------------------------------------------------------------------
// (e) Budget: 80 mapped games → 75 processed, deferral logged (asserted via persisted item count).
// -------------------------------------------------------------------------------------------------
#[tokio::test]
async fn enrich_budget_caps_at_75_apps() {
    let Some(store) = store_or_skip("t3-budget-75").await else {
        return;
    };
    let steam_mock = steam_mock_empty().await;
    // 80 distinct appids, all missing from cache → all need work. Mount success for every one so
    // the cap (not a fetch failure) is what limits processing.
    for i in 0..80u32 {
        let app_id = 10_000 + i;
        seed_steam_game(
            &store,
            &format!("gk-{i}"),
            &format!("mn-{i}"),
            &format!("Game {i}"),
            Some(app_id),
            None,
        )
        .await;
        mount_steam_ok(&steam_mock, app_id).await;
    }

    let mut d = deps(store, "http://unused", None);
    d.steam = Some(steam_client_at(&steam_mock.uri()));

    enrich_steam_apps(&d, far_deadline()).await;

    // Exactly 75 STEAMAPP items persisted — 5 deferred to the next sync.
    let ids = d.store.list_steam_app_ids().await.unwrap();
    assert_eq!(
        ids.len(),
        75,
        "budget must cap at 75 processed apps per pass, got {}",
        ids.len()
    );
    // The 5 deferred are the tail of the ascending order (10075..10079).
    for i in 75..80u32 {
        assert!(
            !ids.contains(&(10_000 + i)),
            "appid {} must be deferred (beyond the 75 cap)",
            10_000 + i
        );
    }
}

// -------------------------------------------------------------------------------------------------
// Deadline guard: an already-passed deadline processes ZERO apps (behavior 6).
// -------------------------------------------------------------------------------------------------
#[tokio::test]
async fn enrich_deadline_stops_before_starting_apps() {
    let Some(store) = store_or_skip("t3-deadline").await else {
        return;
    };
    seed_steam_game(&store, "gk-dl", "mn-dl", "Deadline Game", Some(555), None).await;

    let steam_mock = steam_mock_empty().await;
    mount_steam_ok(&steam_mock, 555).await;

    let mut d = deps(store, "http://unused", None);
    d.steam = Some(steam_client_at(&steam_mock.uri()));

    // Deadline already reached → no new app is started.
    let past = tokio::time::Instant::now();
    enrich_steam_apps(&d, past).await;

    let reqs = steam_mock.received_requests().await.unwrap();
    assert_eq!(
        reqs.len(),
        0,
        "an already-passed deadline must make ZERO storefront calls"
    );
    assert!(
        d.store.get_steam_app(555).await.unwrap().is_none(),
        "no cache item may be written once the deadline is spent"
    );
}

// -------------------------------------------------------------------------------------------------
// Kill switch: STEAM_ENRICH_DISABLED → ZERO storefront calls even with stale work (behavior 1).
// -------------------------------------------------------------------------------------------------
#[tokio::test]
async fn enrich_kill_switch_skips_pass() {
    let Some(store) = store_or_skip("t3-killswitch").await else {
        return;
    };
    seed_steam_game(&store, "gk-k", "mn-k", "Kill Game", Some(777), None).await;

    let steam_mock = steam_mock_empty().await;
    mount_steam_ok(&steam_mock, 777).await;

    let mut d = deps(store, "http://unused", None);
    d.steam = Some(steam_client_at(&steam_mock.uri()));
    d.steam_enrich_disabled = true;

    enrich_steam_apps(&d, far_deadline()).await;

    let reqs = steam_mock.received_requests().await.unwrap();
    assert_eq!(
        reqs.len(),
        0,
        "kill switch must skip the pass entirely — ZERO storefront calls"
    );
    assert!(
        d.store.get_steam_app(777).await.unwrap().is_none(),
        "kill switch must write nothing"
    );
}

// -------------------------------------------------------------------------------------------------
// steam=None: no client → the pass is a silent no-op (behavior 1).
// -------------------------------------------------------------------------------------------------
#[tokio::test]
async fn enrich_no_steam_client_is_noop() {
    let Some(store) = store_or_skip("t3-none").await else {
        return;
    };
    seed_steam_game(&store, "gk-n", "mn-n", "No Client Game", Some(888), None).await;

    // steam stays None (deps default).
    let d = deps(store, "http://unused", None);
    enrich_steam_apps(&d, far_deadline()).await;

    assert!(
        d.store.get_steam_app(888).await.unwrap().is_none(),
        "with no Steam client the pass must write nothing"
    );
}
