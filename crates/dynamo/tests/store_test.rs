use aws_sdk_dynamodb::types::AttributeValue;
use domain::{AppidSource, Claim, ClaimState, Game, GameStatus, Link, SELF_LINK_TOKEN, game_id};
use dynamo::{
    AppidWrite, ClaimTxError, HiddenWrite, OwnedWrite, SYNC_RUN_STALE_SECS, Store, SyncBegin,
    SyncState, SyncWrite, sync_run_is_live,
};
use std::collections::HashMap;
use time::macros::datetime;

/// A raw dynamodb client + resolved table name for the given test, matching how `store_or_skip`
/// wires the Store. Lets a test craft an item whose top-level attrs deliberately disagree with
/// its serialized `body` — impossible via the Store API, which keeps them in lockstep.
async fn raw_client(test: &str) -> aws_sdk_dynamodb::Client {
    let url =
        std::env::var("DYNAMODB_LOCAL_URL").unwrap_or_else(|_| "http://localhost:8000".into());
    let config = aws_config::defaults(aws_config::BehaviorVersion::latest())
        .endpoint_url(&url)
        .region("us-east-1")
        .test_credentials()
        .load()
        .await;
    let _ = test;
    aws_sdk_dynamodb::Client::new(&config)
}

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
        keyindex: 0,
        requires_choice: false,
        steam_app_id: None,
        appid_source: None,
        owned_by_ben: false,
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
    store.create_link(&link("tok1")).await.unwrap();
    assert_eq!(store.get_link("tok1").await.unwrap().unwrap(), link("tok1"));

    let claim = Claim {
        id: "c1".into(),
        link_token: "tok1".into(),
        game_id: game_id("gk1", "mn"),
        state: ClaimState::Pending,
        gift_url: None,
        created_at: datetime!(2026-07-02 01:00 UTC),
        choice_pre_tpks: None,
        revealed_key: None,
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
    store.create_link(&link("tok1")).await.unwrap(); // claims_allowed = 1
    let now = datetime!(2026-07-02 12:00 UTC);
    let gid = game_id("gk1", "mn");

    store.claim_game("tok1", &gid, "c1", now).await.unwrap();

    // game is now pending + off the listable index; link slot consumed
    assert_eq!(store.list_listable_games().await.unwrap(), vec![]);
    let g = store.get_game(&gid).await.unwrap().unwrap();
    assert_eq!(g.status, GameStatus::Pending);
    assert_eq!(g.claim_id.as_deref(), Some("c1"));

    // second claim on the same game: game already pending → unavailable
    store.create_link(&link("tok2")).await.unwrap();
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
    store.create_link(&link("tok1")).await.unwrap();
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
    store.create_link(&link("tok1")).await.unwrap();
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
    store.create_link(&link("tok1")).await.unwrap();
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
    store.create_link(&lnk).await.unwrap();
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
async fn expired_link_rejected_numerically() {
    let Some(store) = store_or_skip("expired-link").await else {
        return;
    };
    store.put_game(&game(1, true)).await.unwrap();
    // expires_at in the past, stored as epoch-seconds N; claim_game's numeric `expires_at > :now`
    // gate must reject it (the old lexicographic RFC3339 compare was the bug this guards).
    let mut lnk = link("tok1");
    lnk.expires_at = Some(datetime!(2020-01-01 00:00 UTC));
    store.create_link(&lnk).await.unwrap();
    let now = datetime!(2026-07-02 12:00 UTC);

    let err = store
        .claim_game("tok1", &game_id("gk1", "mn"), "c1", now)
        .await
        .unwrap_err();
    assert!(matches!(err, ClaimTxError::LinkNotClaimable));
}

#[tokio::test]
async fn fulfill_flip_requires_ownership() {
    let Some(store) = store_or_skip("fulfill-ownership").await else {
        return;
    };
    store.put_game(&game(1, true)).await.unwrap();
    store.create_link(&link("tok1")).await.unwrap();
    let now = datetime!(2026-07-02 12:00 UTC);
    let gid = game_id("gk1", "mn");
    store.claim_game("tok1", &gid, "c1", now).await.unwrap();

    // compensate returns the game to the pool: available, claim_id cleared, listable again.
    store.compensate_claim("tok1", "c1", &gid).await.unwrap();
    assert_eq!(store.list_listable_games().await.unwrap().len(), 1);

    // a stale fulfill for the now-compensated claim must NOT flip the re-listed game to gifted.
    let res = store
        .fulfill_claim(
            "tok1",
            "c1",
            &gid,
            "https://www.humblebundle.com/gift?key=x",
        )
        .await;
    assert!(
        res.is_err(),
        "stale fulfill after compensate must take the loud path, not silently flip: {res:?}"
    );
    let g = store.get_game(&gid).await.unwrap().unwrap();
    assert_eq!(
        g.status,
        GameStatus::Available,
        "re-listed game must stay available, never gifted"
    );
    assert_eq!(g.claim_id, None);
}

#[tokio::test]
async fn compensate_transaction_is_atomic_idempotent() {
    let Some(store) = store_or_skip("compensate-atomic").await else {
        return;
    };
    store.put_game(&game(1, true)).await.unwrap();
    store.create_link(&link("tok1")).await.unwrap(); // claims_allowed = 1
    let now = datetime!(2026-07-02 12:00 UTC);
    let gid = game_id("gk1", "mn");
    store.claim_game("tok1", &gid, "c1", now).await.unwrap();
    assert_eq!(
        store.get_link("tok1").await.unwrap().unwrap().claims_used,
        1
    );

    // compensate, then compensate again: both Ok, counter decremented EXACTLY once, game listable
    // EXACTLY once — the all-or-nothing transaction makes the retry a clean idempotent no-op.
    store.compensate_claim("tok1", "c1", &gid).await.unwrap();
    store.compensate_claim("tok1", "c1", &gid).await.unwrap();

    let l = store.get_link("tok1").await.unwrap().unwrap();
    assert_eq!(l.claims_used, 0, "counter decremented exactly once");
    let listable = store.list_listable_games().await.unwrap();
    assert_eq!(listable.len(), 1, "game listable exactly once");
    assert_eq!(listable[0].id, gid);
    let c = store.get_claim("tok1", "c1").await.unwrap().unwrap();
    assert_eq!(c.state, ClaimState::Compensated);
}

#[tokio::test]
async fn claim_concurrent_counter_is_authoritative() {
    let Some(store) = store_or_skip("claim-concurrent").await else {
        return;
    };

    // Link with 2 slots; two available games.
    let mut lnk = link("tok-cc");
    lnk.claims_allowed = 2;
    store.create_link(&lnk).await.unwrap();
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

#[tokio::test]
async fn sync_upsert_respects_ownership() {
    let Some(store) = store_or_skip("sync-upsert").await else {
        return;
    };
    // new game → Written
    let g = game(1, true);
    assert!(matches!(
        store.upsert_game_from_sync(g.clone()).await.unwrap(),
        SyncWrite::Written
    ));
    // unchanged → Unchanged
    assert!(matches!(
        store.upsert_game_from_sync(g.clone()).await.unwrap(),
        SyncWrite::Unchanged
    ));
    // hidden survives a humble-side change
    let mut hidden = g.clone();
    hidden.hidden = true;
    store.put_game(&hidden).await.unwrap();
    let mut fresh = g.clone();
    fresh.title = "Renamed".into();
    assert!(matches!(
        store.upsert_game_from_sync(fresh).await.unwrap(),
        SyncWrite::Written
    ));
    let got = store.get_game(&g.id).await.unwrap().unwrap();
    assert!(got.hidden);
    assert_eq!(got.title, "Renamed");
    // pending game: sync may refresh cosmetics but never the status.
    // NOTE: g is hidden at this point (asserted above), and hidden games are unclaimable by
    // design (claim_game's gsi1pk listability gate) — so this phase uses a SECOND, visible
    // game. This exact line once claimed the hidden g and failed in CI: the gate worked.
    let g2 = game(2, true);
    store.put_game(&g2).await.unwrap();
    store.create_link(&link("tok1")).await.unwrap();
    store
        .claim_game("tok1", &g2.id, "c1", datetime!(2026-07-02 12:00 UTC))
        .await
        .unwrap();
    let mut fresh2 = g2.clone();
    fresh2.status = GameStatus::BenRedeemed;
    fresh2.title = "Renamed Again".into();
    let w = store.upsert_game_from_sync(fresh2).await.unwrap();
    assert!(matches!(w, SyncWrite::Written | SyncWrite::SkippedInFlight));
    let after = store.get_game(&g2.id).await.unwrap().unwrap();
    assert_eq!(after.status, GameStatus::Pending); // status untouched either way
}

#[tokio::test]
async fn pending_claims_and_sync_state_and_sessions() {
    let Some(store) = store_or_skip("pending-state-sessions").await else {
        return;
    };
    store.put_game(&game(1, true)).await.unwrap();
    store.create_link(&link("tok1")).await.unwrap();
    store
        .claim_game(
            "tok1",
            &game_id("gk1", "mn"),
            "c1",
            datetime!(2026-07-02 12:00 UTC),
        )
        .await
        .unwrap();
    let pending = store.list_pending_claims().await.unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].id, "c1");

    let st = SyncState {
        last_run_epoch: 1_800_000_000,
        ok: true,
        cookie_ok: true,
        games_written: 3,
        message: "ok".into(),
    };
    store.put_sync_state(&st).await.unwrap();
    assert_eq!(store.get_sync_state().await.unwrap().unwrap(), st);

    store.create_session("sess1", 2_000_000_000).await.unwrap();
    assert_eq!(
        store.get_session("sess1").await.unwrap(),
        Some(2_000_000_000)
    );
    store.delete_session("sess1").await.unwrap();
    assert_eq!(store.get_session("sess1").await.unwrap(), None);
}

/// The sync-run marker is a mutex: begin takes it, a second begin is refused while the first is
/// live, a stale marker (crashed run) may be taken over, and end releases it. This conditional
/// put is the ONLY thing serializing concurrent sync walks — the semantics here are load-bearing.
#[tokio::test]
async fn sync_run_marker_serializes_runs() {
    let Some(store) = store_or_skip("sync-run").await else {
        return;
    };
    let now = 1_800_000_000;

    // No marker → nothing running, first begin takes ownership.
    assert_eq!(store.get_sync_run().await.unwrap(), None);
    assert_eq!(store.begin_sync_run(now).await.unwrap(), SyncBegin::Started);
    assert_eq!(store.get_sync_run().await.unwrap(), Some(now));

    // Live marker → concurrent begin refused; the marker keeps the FIRST run's start time.
    assert_eq!(
        store.begin_sync_run(now + 5).await.unwrap(),
        SyncBegin::AlreadyRunning
    );
    assert_eq!(store.get_sync_run().await.unwrap(), Some(now));

    // Stale marker (older than any possible live run) → takeover allowed.
    let later = now + SYNC_RUN_STALE_SECS + 1;
    assert!(!sync_run_is_live(now, later));
    assert_eq!(
        store.begin_sync_run(later).await.unwrap(),
        SyncBegin::Started
    );
    assert_eq!(store.get_sync_run().await.unwrap(), Some(later));

    // End releases → marker gone → begin works again immediately.
    store.end_sync_run().await.unwrap();
    assert_eq!(store.get_sync_run().await.unwrap(), None);
    assert_eq!(
        store.begin_sync_run(later + 5).await.unwrap(),
        SyncBegin::Started
    );
}

/// `list_all_games` must return every GAME# META item, including non-listable ones (hidden,
/// non-giftable, non-Available). The listable GSI only covers Available+giftable+unhidden; admin
/// needs the whole picture including games ben has hidden or that are in mid-claim state.
#[tokio::test]
async fn list_all_games_includes_non_listable() {
    let Some(store) = store_or_skip("list-all-games").await else {
        return;
    };
    // Game 1: giftable + available (listable)
    store.put_game(&game(1, true)).await.unwrap();
    // Game 2: giftable + available but hidden (NOT listable; admin still needs it)
    let mut hidden_g = game(2, true);
    hidden_g.hidden = true;
    store.put_game(&hidden_g).await.unwrap();

    let all = store.list_all_games().await.unwrap();
    assert_eq!(all.len(), 2, "list_all_games must return both games");

    let ids: std::collections::HashSet<_> = all.iter().map(|g| g.id.as_str()).collect();
    assert!(
        ids.contains(game_id("gk1", "mn").as_str()),
        "game 1 must be present"
    );
    assert!(
        ids.contains(game_id("gk2", "mn").as_str()),
        "hidden game 2 must be present"
    );

    // The listable GSI covers only game 1.
    let listable = store.list_listable_games().await.unwrap();
    assert_eq!(
        listable.len(),
        1,
        "listable GSI must still only return game 1"
    );
    assert_eq!(listable[0].id, game_id("gk1", "mn"));
}

/// `list_links` must return every LINK# META item with the authoritative top-level counter.
/// CLAIM# sub-items share the same pk prefix but have sk = "CLAIM#..." — they must be excluded.
#[tokio::test]
async fn list_links_returns_all_with_authoritative_counter() {
    let Some(store) = store_or_skip("list-links").await else {
        return;
    };
    // Seed two links; allow 1 claim each.
    store.create_link(&link("tok-la")).await.unwrap();
    let mut lnk_b = link("tok-lb");
    lnk_b.claims_allowed = 5;
    store.create_link(&lnk_b).await.unwrap();

    // Claim a game via tok-la to drive the authoritative ADD counter.
    store.put_game(&game(10, true)).await.unwrap();
    let now = datetime!(2026-07-02 12:00 UTC);
    store
        .claim_game("tok-la", &game_id("gk10", "mn"), "cla1", now)
        .await
        .unwrap();

    let links = store.list_links().await.unwrap();
    assert_eq!(
        links.len(),
        2,
        "both links must be returned; no CLAIM# items"
    );

    let la = links.iter().find(|l| l.token == "tok-la").unwrap();
    assert_eq!(
        la.claims_used, 1,
        "authoritative counter must reflect the ADD"
    );

    let lb = links.iter().find(|l| l.token == "tok-lb").unwrap();
    assert_eq!(lb.claims_used, 0);
    assert_eq!(lb.claims_allowed, 5);
}

/// `set_game_hidden` basic: seed a game, toggle hidden → Written, re-read confirms hidden=true.
/// Unknown id → NotFound.
#[tokio::test]
async fn set_game_hidden_basic() {
    let Some(store) = store_or_skip("set-game-hidden-basic").await else {
        return;
    };
    let g = game(1, true);
    let gid = g.id.clone();
    store.put_game(&g).await.unwrap();

    // Set hidden=true → Written
    let result = store.set_game_hidden(&gid, true).await.unwrap();
    assert!(
        matches!(result, HiddenWrite::Written),
        "set_game_hidden on available unclaimed game must be Written"
    );

    // Re-read: hidden must be true
    let got = store.get_game(&gid).await.unwrap().unwrap();
    assert!(
        got.hidden,
        "game must be hidden after set_game_hidden(true)"
    );

    // Unknown id → NotFound
    let nf = store.set_game_hidden("no-such-id", true).await.unwrap();
    assert!(
        matches!(nf, HiddenWrite::NotFound),
        "unknown game id must return NotFound"
    );

    // Gifted game (has claim_id from fulfill) must also allow set_game_hidden → Written.
    // This was the bug: the old `attribute_not_exists(claim_id)` guard permanently blocked
    // gifted games since fulfill_claim leaves claim_id on the DynamoDB item.
    let g2 = game(2, true);
    let gid2 = g2.id.clone();
    store.put_game(&g2).await.unwrap();
    let mut lnk2 = link("tok-hide-gifted");
    lnk2.claims_allowed = 1;
    store.create_link(&lnk2).await.unwrap();
    let now = datetime!(2026-07-02 12:00 UTC);
    store
        .claim_game("tok-hide-gifted", &gid2, "c-hide", now)
        .await
        .unwrap();
    store
        .fulfill_claim(
            "tok-hide-gifted",
            "c-hide",
            &gid2,
            "https://www.humblebundle.com/gift?key=TESTKEYXYZ",
        )
        .await
        .unwrap();
    // Gifted game still carries claim_id — set_game_hidden must now succeed (Written).
    let gifted = store.get_game(&gid2).await.unwrap().unwrap();
    assert_eq!(gifted.status, GameStatus::Gifted);
    assert!(gifted.claim_id.is_some(), "gifted game retains claim_id");
    let result2 = store.set_game_hidden(&gid2, true).await.unwrap();
    assert!(
        matches!(result2, HiddenWrite::Written),
        "set_game_hidden on a gifted game must be Written (not Contested), got {result2:?}"
    );
    let after2 = store.get_game(&gid2).await.unwrap().unwrap();
    assert!(
        after2.hidden,
        "gifted game must be hidden after set_game_hidden(true)"
    );
}

/// `set_game_hidden` contested: a Pending game (mid-claim) triggers an early Contested return
/// before the put even runs — the Pending status is the gate, not claim_id presence.
#[tokio::test]
async fn set_game_hidden_contested() {
    let Some(store) = store_or_skip("set-game-hidden-contested").await else {
        return;
    };
    let g = game(1, true);
    let gid = g.id.clone();
    store.put_game(&g).await.unwrap();
    store.create_link(&link("tok1")).await.unwrap();
    let now = datetime!(2026-07-02 12:00 UTC);

    // Claim the game — it is now Pending with a top-level claim_id attribute.
    store.claim_game("tok1", &gid, "c1", now).await.unwrap();
    let claimed = store.get_game(&gid).await.unwrap().unwrap();
    assert_eq!(claimed.status, GameStatus::Pending);
    assert!(claimed.claim_id.is_some());

    // set_game_hidden must detect Pending status and return Contested early.
    let result = store.set_game_hidden(&gid, true).await.unwrap();
    assert!(
        matches!(result, HiddenWrite::Contested),
        "set_game_hidden on a Pending game must return Contested, got {result:?}"
    );

    // The game's claim must be intact (no clobber).
    let after = store.get_game(&gid).await.unwrap().unwrap();
    assert_eq!(
        after.status,
        GameStatus::Pending,
        "status must still be Pending"
    );
    assert!(after.claim_id.is_some(), "claim_id must be preserved");
}

/// batch_get_games: found ids come back keyed by id; missing ids are simply
/// absent (no error); empty input short-circuits to an empty map with no I/O.
#[tokio::test]
async fn batch_get_games_found_and_missing() {
    let Some(store) = store_or_skip("batch-get-games").await else {
        return;
    };
    let g1 = game(1, true);
    let g2 = game(2, true);
    store.put_game(&g1).await.unwrap();
    store.put_game(&g2).await.unwrap();

    let ids = vec![g1.id.clone(), g2.id.clone(), "GAMEKEY:nope".to_string()];
    let map = store.batch_get_games(&ids).await.unwrap();

    assert_eq!(map.len(), 2, "two found, one missing");
    assert_eq!(map.get(&g1.id).unwrap().title, "Game 1");
    assert_eq!(map.get(&g2.id).unwrap().title, "Game 2");
    assert!(!map.contains_key("GAMEKEY:nope"));

    let empty = store.batch_get_games(&[]).await.unwrap();
    assert!(empty.is_empty());
}

/// `list_listable_games` must exhaust every page. Force a 1-item Query page via the test seam so
/// three listable games span three pages: a single-page read would truncate to the first item.
#[tokio::test]
async fn list_listable_games_paginates() {
    let Some(store) = store_or_skip("list-listable-paginate").await else {
        return;
    };
    for n in 0..3 {
        store.put_game(&game(n, true)).await.unwrap();
    }

    // default (full-page) read returns all three
    let all = store.list_listable_games().await.unwrap();
    assert_eq!(all.len(), 3, "full read must return every listable game");

    // 1 item per Query page → the loop must follow last_evaluated_key across 3 pages
    let paged = store.list_listable_games_paged(Some(1)).await.unwrap();
    assert_eq!(
        paged.len(),
        3,
        "paginated read must not truncate at the first page"
    );
    let ids: std::collections::HashSet<_> = paged.iter().map(|g| g.id.clone()).collect();
    for n in 0..3 {
        assert!(
            ids.contains(&game_id(&format!("gk{n}"), "mn")),
            "game {n} present"
        );
    }
}

/// `list_pending_claims` feeds reconcile completeness — a truncated page parks claims forever.
/// Force a 1-item page across three pending claims and assert all three come back, oldest-first
/// ordering preserved across the page boundaries.
#[tokio::test]
async fn list_pending_claims_paginates_and_keeps_order() {
    let Some(store) = store_or_skip("list-pending-paginate").await else {
        return;
    };
    let mut lnk = link("tok-pp");
    lnk.claims_allowed = 3;
    store.create_link(&lnk).await.unwrap();
    // three claims with distinct, increasing created_at so ordering is observable
    for n in 0..3u32 {
        store.put_game(&game(n, true)).await.unwrap();
        let now = datetime!(2026-07-02 12:00 UTC) + time::Duration::minutes(n as i64);
        store
            .claim_game(
                "tok-pp",
                &game_id(&format!("gk{n}"), "mn"),
                &format!("c{n}"),
                now,
            )
            .await
            .unwrap();
    }

    let all = store.list_pending_claims().await.unwrap();
    assert_eq!(all.len(), 3, "full read must return every pending claim");

    let paged = store.list_pending_claims_paged(Some(1)).await.unwrap();
    assert_eq!(
        paged.len(),
        3,
        "paginated read must exhaust all pages — a dropped claim would be parked forever"
    );
    // gsi2sk is created_at ascending; the loop must preserve that order across pages
    let ids: Vec<_> = paged.iter().map(|c| c.id.clone()).collect();
    assert_eq!(
        ids,
        vec!["c0", "c1", "c2"],
        "oldest-first order preserved across pages"
    );
}

/// `get_link` / `list_links` must take EVERY enforcer field (claims_used, claims_allowed,
/// revoked, expires_at) from the authoritative top-level attributes, never the serialized body.
/// We craft an item whose body deliberately disagrees with its top-level attrs — the exact
/// lost-update an edit-link endpoint would create — and assert the top-level values win.
#[tokio::test]
async fn get_link_overrides_all_enforcer_fields_from_top_level() {
    let Some(store) = store_or_skip("enforcer-override").await else {
        return;
    };
    let client = raw_client("enforcer-override").await;
    let table = "t-enforcer-override";

    // Body carries STALE enforcer values: allowed=1, used=9, revoked=true, expires far past.
    let stale_body = Link {
        token: "tok-e".into(),
        label: "dave".into(),
        claims_allowed: 1,
        claims_used: 9,
        revoked: true,
        expires_at: Some(datetime!(2020-01-01 00:00 UTC)),
        created_at: datetime!(2026-07-02 00:00 UTC),
    };
    // Top-level attrs are the AUTHORITATIVE truth: allowed=5, used=2, revoked=false, no expiry.
    let item = HashMap::from([
        ("pk".to_string(), AttributeValue::S("LINK#tok-e".into())),
        ("sk".to_string(), AttributeValue::S("META".into())),
        (
            "body".to_string(),
            AttributeValue::S(serde_json::to_string(&stale_body).unwrap()),
        ),
        ("claims_allowed".to_string(), AttributeValue::N("5".into())),
        ("claims_used".to_string(), AttributeValue::N("2".into())),
        ("revoked".to_string(), AttributeValue::Bool(false)),
        // expires_at intentionally ABSENT at top level → authoritative "never expires"
    ]);
    client
        .put_item()
        .table_name(table)
        .set_item(Some(item))
        .send()
        .await
        .unwrap();

    let got = store.get_link("tok-e").await.unwrap().unwrap();
    assert_eq!(got.claims_used, 2, "claims_used from top-level attr");
    assert_eq!(
        got.claims_allowed, 5,
        "claims_allowed from top-level attr, not stale body 1"
    );
    assert!(
        !got.revoked,
        "revoked from top-level attr, not stale body true"
    );
    assert_eq!(
        got.expires_at, None,
        "absent top-level expires_at is authoritative 'never' — stale body value must NOT leak"
    );

    // list_links must apply the identical override.
    let links = store.list_links().await.unwrap();
    let l = links.iter().find(|l| l.token == "tok-e").unwrap();
    assert_eq!(l.claims_allowed, 5);
    assert_eq!(l.claims_used, 2);
    assert!(!l.revoked);
    assert_eq!(l.expires_at, None);
}

#[tokio::test]
async fn self_claim_intake_accepts_non_giftable_and_hidden() {
    let Some(store) = store_or_skip("sc-nongiftable").await else {
        return;
    };
    // A game that is available but NOT listable: giftable=false AND hidden=true — no gsi1pk.
    // claim_game would reject this (gsi1pk absent); claim_game_self must accept it.
    let mut g = game(1, false); // giftable=false
    g.hidden = true;
    store.put_game(&g).await.unwrap();

    store
        .claim_game_self(
            &game_id("gk1", "mn"),
            "claim-1",
            time::OffsetDateTime::now_utc(),
        )
        .await
        .expect("non-giftable+hidden must be self-claimable");

    let after = store
        .get_game(&game_id("gk1", "mn"))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(after.status, GameStatus::Pending);
    assert_eq!(after.claim_id.as_deref(), Some("claim-1"));
    let claim = store
        .get_claim(SELF_LINK_TOKEN, "claim-1")
        .await
        .unwrap()
        .expect("claim recorded under LINK#SELF");
    assert_eq!(claim.state, ClaimState::Pending);
    assert_eq!(claim.link_token, SELF_LINK_TOKEN);
}

#[tokio::test]
async fn self_claim_intake_single_winner_on_race() {
    let Some(store) = store_or_skip("sc-race").await else {
        return;
    };
    store.put_game(&game(1, true)).await.unwrap();
    let gid = game_id("gk1", "mn");
    let now = time::OffsetDateTime::now_utc();
    // Sequential calls: first wins, second refuses on the status condition.
    let a = store.claim_game_self(&gid, "claim-a", now).await;
    let b = store.claim_game_self(&gid, "claim-b", now).await;
    assert!(a.is_ok());
    assert!(matches!(b, Err(ClaimTxError::GameUnavailable)));
}

#[tokio::test]
async fn gift_vs_self_claim_race_single_winner() {
    let Some(store) = store_or_skip("sc-gift-vs-self").await else {
        return;
    };
    store.put_game(&game(1, true)).await.unwrap();
    let lnk = link("tok-race");
    store.create_link(&lnk).await.unwrap();
    let gid = game_id("gk1", "mn");
    let now = time::OffsetDateTime::now_utc();
    // Gift claim wins first…
    store
        .claim_game("tok-race", &gid, "claim-g", now)
        .await
        .unwrap();
    // …self-claim then refuses on the same status condition.
    let s = store.claim_game_self(&gid, "claim-s", now).await;
    assert!(matches!(s, Err(ClaimTxError::GameUnavailable)));
}

#[tokio::test]
async fn fulfill_self_claim_writes_key_then_flips_ben_redeemed() {
    let Some(store) = store_or_skip("sc-fulfill-1").await else {
        return;
    };
    let gid = game_id("gk2", "mn");
    store.put_game(&game(2, true)).await.unwrap();
    store
        .claim_game_self(&gid, "c-f1", time::OffsetDateTime::now_utc())
        .await
        .unwrap();

    store
        .fulfill_self_claim("c-f1", &gid, "AAAA-BBBB-CCCC")
        .await
        .unwrap();

    let claim = store
        .get_claim(SELF_LINK_TOKEN, "c-f1")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(claim.state, ClaimState::Fulfilled);
    assert_eq!(claim.revealed_key.as_deref(), Some("AAAA-BBBB-CCCC"));
    let g = store.get_game(&gid).await.unwrap().unwrap();
    assert_eq!(g.status, GameStatus::BenRedeemed);
}

#[tokio::test]
async fn fulfill_self_claim_is_idempotent_on_retry() {
    let Some(store) = store_or_skip("sc-fulfill-2").await else {
        return;
    };
    let gid = game_id("gk3", "mn");
    store.put_game(&game(3, true)).await.unwrap();
    store
        .claim_game_self(&gid, "c-f2", time::OffsetDateTime::now_utc())
        .await
        .unwrap();

    store.fulfill_self_claim("c-f2", &gid, "K1").await.unwrap();
    // Second call: no error, state unchanged.
    store.fulfill_self_claim("c-f2", &gid, "K1").await.unwrap();
    let claim = store
        .get_claim(SELF_LINK_TOKEN, "c-f2")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(claim.revealed_key.as_deref(), Some("K1"));
}

#[tokio::test]
async fn fulfill_self_claim_never_flips_when_claim_lost_to_compensate() {
    // durable-first pin: write-1 (claim key write) must precede write-2 (game flip). Make
    // write-1 fail permanently — claim already Compensated ⇒ pending marker consumed and the
    // recheck sees a non-Fulfilled state ⇒ fulfill errors — and assert the game NEVER flipped.
    let Some(store) = store_or_skip("sc-fulfill-3").await else {
        return;
    };
    let gid = game_id("gk4", "mn");
    store.put_game(&game(4, true)).await.unwrap();
    store
        .claim_game_self(&gid, "c-f3", time::OffsetDateTime::now_utc())
        .await
        .unwrap();
    let mut c = store
        .get_claim(SELF_LINK_TOKEN, "c-f3")
        .await
        .unwrap()
        .unwrap();
    c.state = ClaimState::Compensated; // consumes the gsi2pk pending marker via claim_item
    store.put_claim(&c).await.unwrap();

    let res = store.fulfill_self_claim("c-f3", &gid, "LATE-KEY").await;
    assert!(res.is_err(), "fulfill must lose loudly to compensate");
    let g = store.get_game(&gid).await.unwrap().unwrap();
    assert_ne!(
        g.status,
        GameStatus::BenRedeemed,
        "game must NOT flip when write-1 failed"
    );
}

#[tokio::test]
async fn compensate_self_claim_succeeds_with_no_link_meta_item() {
    let Some(store) = store_or_skip("sc-comp-1").await else {
        return;
    };
    let gid = game_id("gk5", "mn");
    store.put_game(&game(5, true)).await.unwrap();
    store
        .claim_game_self(&gid, "c-c1", time::OffsetDateTime::now_utc())
        .await
        .unwrap();

    // The gift-path compensate MUST fail here (pins WHY the variant exists)…
    let wrong = store.compensate_claim(SELF_LINK_TOKEN, "c-c1", &gid).await;
    assert!(
        wrong.is_err(),
        "gift compensate must cancel on the absent LINK META"
    );

    // …and the SELF variant must succeed: claim compensated, game re-listed.
    store.compensate_self_claim("c-c1", &gid).await.unwrap();
    let claim = store
        .get_claim(SELF_LINK_TOKEN, "c-c1")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(claim.state, ClaimState::Compensated);
    let game = store.get_game(&gid).await.unwrap().unwrap();
    assert_eq!(game.status, GameStatus::Available);
    assert_eq!(game.claim_id, None);
}

// =================================================================================================
// TASK 6: set_game_steam_appid_if_unclaimed — guarded appid writer.
// =================================================================================================

/// Basic: seed a game with no appid, write an appid → Written, re-read confirms the pair.
/// Manual guard: seed a game with Manual source, write → Skipped, pair unchanged.
/// NotFound: unknown id → NotFound.
#[tokio::test]
async fn set_game_steam_appid_if_unclaimed_basic() {
    let Some(store) = store_or_skip("set-steam-appid-basic").await else {
        return;
    };
    let g = game(1, true);
    let gid = g.id.clone();
    store.put_game(&g).await.unwrap();

    // Write appid → Written.
    let result = store
        .set_game_steam_appid_if_unclaimed(&gid, 570, AppidSource::Title)
        .await
        .unwrap();
    assert!(
        matches!(result, AppidWrite::Written),
        "set_game_steam_appid_if_unclaimed on available unclaimed game must be Written"
    );

    // Re-read: appid pair must be set.
    let got = store.get_game(&gid).await.unwrap().unwrap();
    assert_eq!(got.steam_app_id, Some(570));
    assert_eq!(got.appid_source, Some(AppidSource::Title));

    // Unknown id → NotFound.
    let nf = store
        .set_game_steam_appid_if_unclaimed("no-such-id", 1, AppidSource::Title)
        .await
        .unwrap();
    assert!(
        matches!(nf, AppidWrite::NotFound),
        "unknown game id must return NotFound"
    );

    // Manual guard: seed a game with Manual source → Skipped, pair unchanged.
    let mut manual_game = game(2, true);
    manual_game.steam_app_id = Some(400);
    manual_game.appid_source = Some(AppidSource::Manual);
    let manual_gid = manual_game.id.clone();
    store.put_game(&manual_game).await.unwrap();
    let skipped = store
        .set_game_steam_appid_if_unclaimed(&manual_gid, 9999, AppidSource::Title)
        .await
        .unwrap();
    assert!(
        matches!(skipped, AppidWrite::Skipped),
        "Manual-sourced game must return Skipped, got {skipped:?}"
    );
    let after = store.get_game(&manual_gid).await.unwrap().unwrap();
    assert_eq!(
        after.steam_app_id,
        Some(400),
        "Manual appid must be unchanged"
    );
    assert_eq!(after.appid_source, Some(AppidSource::Manual));
}

/// Contested: a Pending game (mid-claim) → early Contested without touching the item.
#[tokio::test]
async fn set_game_steam_appid_if_unclaimed_contested() {
    let Some(store) = store_or_skip("set-steam-appid-contested").await else {
        return;
    };
    let g = game(1, true);
    let gid = g.id.clone();
    store.put_game(&g).await.unwrap();
    let lnk = link("tok-appid-contested");
    store.create_link(&lnk).await.unwrap();
    let now = datetime!(2026-07-06 12:00 UTC);
    store
        .claim_game("tok-appid-contested", &gid, "c-appid", now)
        .await
        .unwrap();

    let result = store
        .set_game_steam_appid_if_unclaimed(&gid, 570, AppidSource::Title)
        .await
        .unwrap();
    assert!(
        matches!(result, AppidWrite::Contested),
        "Pending game must return Contested, got {result:?}"
    );
    // appid must not have been written.
    let after = store.get_game(&gid).await.unwrap().unwrap();
    assert_eq!(
        after.steam_app_id, None,
        "Contested game appid must be unchanged"
    );
}

// =================================================================================================
// TASK 7: CONFIG#STEAM identity, STEAMOWN 7d-ttl cache, guarded owned_by_ben stamp.
// =================================================================================================

/// CONFIG#STEAM: put/get/delete round-trip.
#[tokio::test]
async fn steam_identity_config_roundtrip() {
    let Some(store) = store_or_skip("steam-identity-config").await else {
        return;
    };

    // Initially absent.
    assert_eq!(store.get_steam_identity().await.unwrap(), None);

    // Put → readable back.
    store.put_steam_identity("76561198012345678").await.unwrap();
    assert_eq!(
        store.get_steam_identity().await.unwrap(),
        Some("76561198012345678".into())
    );

    // Delete → gone.
    store.delete_steam_identity().await.unwrap();
    assert_eq!(store.get_steam_identity().await.unwrap(), None);
}

/// STEAMOWN: put/get round-trip asserting (appids, fetched_at) AND the raw DDB `ttl`
/// attribute equals now_epoch + 7 days.
#[tokio::test]
async fn steam_owned_cache_roundtrip() {
    let Some(store) = store_or_skip("steam-owned-cache").await else {
        return;
    };
    let client = raw_client("steam-owned-cache").await;
    let table = "t-steam-owned-cache";

    let steamid = "76561198012345678";
    let appids: Vec<u32> = vec![570, 620, 400];
    let now_epoch: i64 = 1_800_000_000;
    const SEVEN_DAYS_SECS: i64 = 7 * 24 * 3600;
    let expected_ttl = now_epoch + SEVEN_DAYS_SECS;

    // Initially absent.
    assert_eq!(store.get_steam_owned(steamid).await.unwrap(), None);

    // Put the owned cache.
    store
        .put_steam_owned(steamid, &appids, now_epoch)
        .await
        .unwrap();

    // get_steam_owned returns (appids, fetched_at).
    let (got_appids, got_fetched_at) = store.get_steam_owned(steamid).await.unwrap().unwrap();
    assert_eq!(got_appids, appids, "appids must round-trip");
    assert_eq!(got_fetched_at, now_epoch, "fetched_at must equal now_epoch");

    // Raw DDB item must carry a numeric `ttl` attribute = now+7d.
    let pk = format!("STEAMOWN#{steamid}");
    let raw = client
        .get_item()
        .table_name(table)
        .key("pk", AttributeValue::S(pk))
        .key("sk", AttributeValue::S("META".into()))
        .send()
        .await
        .unwrap();
    let item = raw.item.expect("STEAMOWN item must exist after put");
    let ttl_val = item
        .get("ttl")
        .and_then(|v| v.as_n().ok())
        .and_then(|n| n.parse::<i64>().ok())
        .expect("ttl attribute must be a numeric N");
    assert_eq!(ttl_val, expected_ttl, "ttl must be now_epoch + 7d");
}

/// `set_game_owned_by_ben` basic: seed a game, set owned=true → Written, re-read confirms.
/// Unknown id → NotFound.
#[tokio::test]
async fn set_game_owned_by_ben_basic() {
    let Some(store) = store_or_skip("owned-by-ben-basic").await else {
        return;
    };
    let g = game(1, true);
    let gid = g.id.clone();
    store.put_game(&g).await.unwrap();

    // Set owned_by_ben=true → Written.
    let result = store.set_game_owned_by_ben(&gid, true).await.unwrap();
    assert!(
        matches!(result, OwnedWrite::Written),
        "set_game_owned_by_ben on available unclaimed game must be Written"
    );

    // Re-read: owned_by_ben must be true.
    let got = store.get_game(&gid).await.unwrap().unwrap();
    assert!(
        got.owned_by_ben,
        "game must be owned_by_ben after set(true)"
    );

    // Unknown id → NotFound.
    let nf = store
        .set_game_owned_by_ben("no-such-id", true)
        .await
        .unwrap();
    assert!(
        matches!(nf, OwnedWrite::NotFound),
        "unknown game id must return NotFound"
    );
}

/// `set_game_owned_by_ben` contested: a Pending game triggers early Contested without touching
/// the item.
#[tokio::test]
async fn set_game_owned_by_ben_contested() {
    let Some(store) = store_or_skip("owned-by-ben-contested").await else {
        return;
    };
    let g = game(1, true);
    let gid = g.id.clone();
    store.put_game(&g).await.unwrap();
    store
        .create_link(&link("tok-owned-contested"))
        .await
        .unwrap();
    let now = datetime!(2026-07-06 12:00 UTC);
    store
        .claim_game("tok-owned-contested", &gid, "c-owned", now)
        .await
        .unwrap();
    let pending = store.get_game(&gid).await.unwrap().unwrap();
    assert_eq!(pending.status, GameStatus::Pending);

    // set_game_owned_by_ben must detect Pending and return Contested early.
    let result = store.set_game_owned_by_ben(&gid, true).await.unwrap();
    assert!(
        matches!(result, OwnedWrite::Contested),
        "set_game_owned_by_ben on Pending game must return Contested, got {result:?}"
    );

    // game must be unchanged.
    let after = store.get_game(&gid).await.unwrap().unwrap();
    assert_eq!(
        after.status,
        GameStatus::Pending,
        "status must still be Pending"
    );
    assert!(!after.owned_by_ben, "owned_by_ben must be unchanged");
}
