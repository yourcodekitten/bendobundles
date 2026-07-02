use domain::{Claim, ClaimState, Game, GameStatus, Link, game_id};
use dynamo::{ClaimTxError, Store};
use time::macros::datetime;

async fn store_or_skip(test: &str) -> Option<Store> {
    let url =
        std::env::var("DYNAMODB_LOCAL_URL").unwrap_or_else(|_| "http://localhost:8000".into());
    let config = aws_config::defaults(aws_config::BehaviorVersion::latest())
        .endpoint_url(&url)
        .region("us-east-1")
        .test_credentials()
        .load()
        .await;
    let client = aws_sdk_dynamodb::Client::new(&config);
    if client.list_tables().send().await.is_err() {
        eprintln!("SKIP {test}: no dynamodb-local at {url}");
        return None;
    }
    // one table per test = no cross-test interference
    let store = Store::new(client, format!("t-{test}"));
    store.create_table_for_tests().await.unwrap();
    Some(store)
}

fn game(n: u32, listable: bool) -> Game {
    Game {
        id: game_id(&format!("gk{n}"), "mn"),
        title: format!("Game {n}"),
        bundle: "B".into(),
        gamekey: format!("gk{n}"),
        machine_name: "mn".into(),
        key_type: "steam".into(),
        giftable: listable,
        hidden: false,
        status: GameStatus::Available,
        claim_id: None,
        artwork_url: None,
    }
}

fn link(token: &str) -> Link {
    Link {
        token: token.into(),
        label: "dave".into(),
        claims_allowed: 1,
        claims_used: 0,
        revoked: false,
        expires_at: None,
        created_at: datetime!(2026-07-02 00:00 UTC),
    }
}

#[tokio::test]
async fn game_roundtrip_and_listable_index() {
    let Some(store) = store_or_skip("game-roundtrip").await else {
        return;
    };
    store.put_game(&game(1, true)).await.unwrap();
    store.put_game(&game(2, false)).await.unwrap();

    let got = store
        .get_game(&game_id("gk1", "mn"))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(got, game(1, true));
    assert_eq!(store.get_game("nope").await.unwrap(), None);

    let listable = store.list_listable_games().await.unwrap();
    assert_eq!(listable.len(), 1);
    assert_eq!(listable[0].id, game_id("gk1", "mn"));
}

#[tokio::test]
async fn link_and_claim_roundtrip() {
    let Some(store) = store_or_skip("link-claim").await else {
        return;
    };
    store.put_link(&link("tok1")).await.unwrap();
    assert_eq!(store.get_link("tok1").await.unwrap().unwrap(), link("tok1"));

    let claim = Claim {
        id: "c1".into(),
        link_token: "tok1".into(),
        game_id: game_id("gk1", "mn"),
        state: ClaimState::Pending,
        gift_url: None,
        created_at: datetime!(2026-07-02 01:00 UTC),
    };
    store.put_claim(&claim).await.unwrap();
    assert_eq!(store.get_claim("tok1", "c1").await.unwrap().unwrap(), claim);
    assert_eq!(store.claims_for_link("tok1").await.unwrap(), vec![claim]);
}

#[tokio::test]
async fn claim_happy_path_then_race_loses() {
    let Some(store) = store_or_skip("claim-race").await else {
        return;
    };
    store.put_game(&game(1, true)).await.unwrap();
    store.put_link(&link("tok1")).await.unwrap(); // claims_allowed = 1
    let now = datetime!(2026-07-02 12:00 UTC);
    let gid = game_id("gk1", "mn");

    store.claim_game("tok1", &gid, "c1", now).await.unwrap();

    // game is now pending + off the listable index; link slot consumed
    assert_eq!(store.list_listable_games().await.unwrap(), vec![]);
    let g = store.get_game(&gid).await.unwrap().unwrap();
    assert_eq!(g.status, GameStatus::Pending);
    assert_eq!(g.claim_id.as_deref(), Some("c1"));

    // second claim on the same game: game already pending → unavailable
    store.put_link(&link("tok2")).await.unwrap();
    let err = store.claim_game("tok2", &gid, "c2", now).await.unwrap_err();
    assert!(matches!(err, ClaimTxError::GameUnavailable));

    // exhausted link: tok1 had exactly 1 claim
    store.put_game(&game(3, true)).await.unwrap();
    let err = store
        .claim_game("tok1", &game_id("gk3", "mn"), "c3", now)
        .await
        .unwrap_err();
    assert!(matches!(err, ClaimTxError::LinkNotClaimable));
}

#[tokio::test]
async fn fulfill_writes_gift_url_then_flips_game() {
    let Some(store) = store_or_skip("fulfill").await else {
        return;
    };
    store.put_game(&game(1, true)).await.unwrap();
    store.put_link(&link("tok1")).await.unwrap();
    let now = datetime!(2026-07-02 12:00 UTC);
    let gid = game_id("gk1", "mn");
    store.claim_game("tok1", &gid, "c1", now).await.unwrap();

    store
        .fulfill_claim(
            "tok1",
            "c1",
            &gid,
            "https://www.humblebundle.com/gift?key=x",
        )
        .await
        .unwrap();

    let c = store.get_claim("tok1", "c1").await.unwrap().unwrap();
    assert_eq!(c.state, ClaimState::Fulfilled);
    assert_eq!(
        c.gift_url.as_deref(),
        Some("https://www.humblebundle.com/gift?key=x")
    );
    let g = store.get_game(&gid).await.unwrap().unwrap();
    assert_eq!(g.status, GameStatus::Gifted);
}

#[tokio::test]
async fn compensate_returns_everything() {
    let Some(store) = store_or_skip("compensate").await else {
        return;
    };
    store.put_game(&game(1, true)).await.unwrap();
    store.put_link(&link("tok1")).await.unwrap();
    let now = datetime!(2026-07-02 12:00 UTC);
    let gid = game_id("gk1", "mn");
    store.claim_game("tok1", &gid, "c1", now).await.unwrap();

    store.compensate_claim("tok1", "c1", &gid).await.unwrap();

    // game listable again, link slot returned, claim marked compensated
    assert_eq!(store.list_listable_games().await.unwrap().len(), 1);
    let l = store.get_link("tok1").await.unwrap().unwrap();
    assert_eq!(l.claims_used, 0);
    let c = store.get_claim("tok1", "c1").await.unwrap().unwrap();
    assert_eq!(c.state, ClaimState::Compensated);
}

#[tokio::test]
async fn hidden_game_is_unclaimable() {
    let Some(store) = store_or_skip("claim-hidden").await else {
        return;
    };
    // Available + giftable but hidden → is_listable() false → no sparse gsi1pk marker, even though
    // status is still "available". The race-free listability gate must reject the claim.
    let mut g = game(1, true);
    g.hidden = true;
    store.put_game(&g).await.unwrap();
    store.put_link(&link("tok1")).await.unwrap();
    let now = datetime!(2026-07-02 12:00 UTC);

    let err = store
        .claim_game("tok1", &game_id("gk1", "mn"), "c1", now)
        .await
        .unwrap_err();
    assert!(matches!(err, ClaimTxError::GameUnavailable));
}

#[tokio::test]
async fn compensate_is_idempotent() {
    let Some(store) = store_or_skip("compensate-idem").await else {
        return;
    };
    // Link with 2 slots so a single compensate vs a double compensate is distinguishable in the
    // counter: claim A (used=1), claim B (used=2), compensate A (used=1), compensate A again → 1.
    let mut lnk = link("tok1");
    lnk.claims_allowed = 2;
    store.put_link(&lnk).await.unwrap();
    store.put_game(&game(1, true)).await.unwrap(); // A
    store.put_game(&game(2, true)).await.unwrap(); // B
    let now = datetime!(2026-07-02 12:00 UTC);
    let a = game_id("gk1", "mn");
    let b = game_id("gk2", "mn");

    store.claim_game("tok1", &a, "cA", now).await.unwrap();
    store.claim_game("tok1", &b, "cB", now).await.unwrap();
    assert_eq!(
        store.get_link("tok1").await.unwrap().unwrap().claims_used,
        2
    );

    // first compensate: counter 2 → 1, game A re-listed
    store.compensate_claim("tok1", "cA", &a).await.unwrap();
    assert_eq!(
        store.get_link("tok1").await.unwrap().unwrap().claims_used,
        1
    );

    // second compensate on the SAME claim: Ok, but must NOT decrement again (retry-after-success)
    store.compensate_claim("tok1", "cA", &a).await.unwrap();
    assert_eq!(
        store.get_link("tok1").await.unwrap().unwrap().claims_used,
        1,
        "second compensate must not double-decrement the link counter"
    );

    // game A is listable again exactly once; B is still pending (not listable)
    let listable = store.list_listable_games().await.unwrap();
    assert_eq!(listable.len(), 1, "game A re-listed exactly once");
    assert_eq!(listable[0].id, a);
}

#[tokio::test]
async fn claim_concurrent_counter_is_authoritative() {
    let Some(store) = store_or_skip("claim-concurrent").await else {
        return;
    };

    // Link with 2 slots; two available games.
    let mut lnk = link("tok-cc");
    lnk.claims_allowed = 2;
    store.put_link(&lnk).await.unwrap();
    store.put_game(&game(20, true)).await.unwrap();
    store.put_game(&game(21, true)).await.unwrap();

    let gid20 = game_id("gk20", "mn");
    let gid21 = game_id("gk21", "mn");
    let now = datetime!(2026-07-02 12:00 UTC);

    // Two concurrent claim_game calls on different games via the same link.
    let (r1, r2) = tokio::join!(
        store.claim_game("tok-cc", &gid20, "cc1", now),
        store.claim_game("tok-cc", &gid21, "cc2", now),
    );
    assert!(r1.is_ok(), "first concurrent claim failed: {r1:?}");
    assert!(r2.is_ok(), "second concurrent claim failed: {r2:?}");

    // get_link must read the authoritative top-level counter, not stale body JSON.
    let l = store.get_link("tok-cc").await.unwrap().unwrap();
    assert_eq!(
        l.claims_used, 2,
        "top-level counter must reflect both atomic increments"
    );

    // Compensating one claim atomically decrements the counter.
    store
        .compensate_claim("tok-cc", "cc1", &gid20)
        .await
        .unwrap();
    let l = store.get_link("tok-cc").await.unwrap().unwrap();
    assert_eq!(l.claims_used, 1, "counter must be 1 after one compensation");
}
