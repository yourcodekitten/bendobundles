use aws_sdk_dynamodb::types::AttributeValue;
use domain::{AppidSource, Claim, ClaimState, Game, GameStatus, Link, SELF_LINK_TOKEN, game_id};
use dynamo::{
    AppidWrite, ClaimTxError, HiddenWrite, OwnedWrite, SYNC_RUN_STALE_SECS, SteamAppCache, Store,
    SyncBegin, SyncState, SyncWrite, sync_run_is_live,
};
use std::collections::HashMap;
use time::macros::datetime;
use uuid::Uuid;

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
        hidden_source: None,
    }
}

fn link(token: &str) -> Link {
    Link {
        token: token.into(),
        label: "dave".into(),
        gift_note: None,
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

/// `set_link_gift_note` is a single-attribute write and the top-level attr is
/// authoritative on read. Two properties pinned here:
///
/// 1. **A stale body writer cannot clobber the note** (the lost-update the old
///    body-only design had): set a note, then land `update_link_meta` with a
///    Link snapshot taken BEFORE the note existed (exactly what a racing revoke
///    carries) — the note must survive, and the revoke must stick.
/// 2. **A note write cannot disturb enforcement**: the fn takes only
///    (token, note), so there is no snapshot to carry stale `revoked` /
///    `claims_allowed` / `expires_at` back in — un-revoking by note edit is
///    unrepresentable. Asserted behaviorally: note ops on a revoked link leave
///    it revoked.
#[tokio::test]
async fn gift_note_scoped_write_survives_stale_body_writers() {
    let Some(store) = store_or_skip("gift-note-scoped").await else {
        return;
    };
    store.create_link(&link("tok-note")).await.unwrap();
    let pre_note_snapshot = store.get_link("tok-note").await.unwrap().unwrap();

    // set the note via the scoped write
    assert!(
        store
            .set_link_gift_note("tok-note", Some("wrote this later ♡"))
            .await
            .unwrap()
    );

    // a concurrent revoke commits from its pre-note snapshot (body has NO note)
    let mut stale_revoke = pre_note_snapshot;
    stale_revoke.revoked = true;
    store.update_link_meta(&stale_revoke).await.unwrap();

    let after = store.get_link("tok-note").await.unwrap().unwrap();
    assert!(after.revoked, "the revoke must stick");
    assert_eq!(
        after.gift_note.as_deref(),
        Some("wrote this later ♡"),
        "a stale body write must not clobber the note (top-level attr is authoritative)"
    );

    // editing/clearing the note on the revoked link must leave it revoked
    assert!(
        store
            .set_link_gift_note("tok-note", Some("edited ♡"))
            .await
            .unwrap()
    );
    let edited = store.get_link("tok-note").await.unwrap().unwrap();
    assert!(edited.revoked, "a note edit must never un-revoke");
    assert_eq!(edited.gift_note.as_deref(), Some("edited ♡"));

    assert!(store.set_link_gift_note("tok-note", None).await.unwrap());
    let cleared = store.get_link("tok-note").await.unwrap().unwrap();
    assert!(cleared.revoked);
    assert_eq!(cleared.gift_note, None, "REMOVE path clears the note");

    // unknown token → Ok(false), and nothing is created
    assert!(
        !store
            .set_link_gift_note("no-such-tok", Some("x"))
            .await
            .unwrap()
    );
    assert_eq!(store.get_link("no-such-tok").await.unwrap(), None);
}

/// The note must survive `claim_game`'s `SET body` — the OTHER stale body writer
/// named in `domain::Link`'s doc. Guards against a refactor that turns the
/// claim's link write into a full-item replace (which would drop the top-level
/// `gift_note` attr) or otherwise rewrites attrs it doesn't own.
#[tokio::test]
async fn gift_note_survives_claim_body_rewrite() {
    let Some(store) = store_or_skip("gift-note-claim").await else {
        return;
    };
    store.put_game(&game(1, true)).await.unwrap();
    let mut noted = link("tok-noted");
    noted.gift_note = Some("picked for you ♡".into());
    store.create_link(&noted).await.unwrap();

    let now = datetime!(2026-07-02 12:00 UTC);
    store
        .claim_game("tok-noted", &game_id("gk1", "mn"), "c1", now)
        .await
        .unwrap();

    let after = store.get_link("tok-noted").await.unwrap().unwrap();
    assert_eq!(after.claims_used, 1);
    assert_eq!(
        after.gift_note.as_deref(),
        Some("picked for you ♡"),
        "claim's body rewrite must not disturb the note"
    );
}

/// The note lives ONLY in the top-level attribute — the `body` blob must never
/// carry it (schema::link_body strips it at every writer). Otherwise a body
/// written while a note was set retains the text verbatim at rest after a
/// "clear", indefinitely on links that see no further body write — a delete
/// that doesn't delete (OMBB, #69 review). Asserted RAW, against the stored
/// item, across all three body writers: create, update_link_meta, claim_game.
#[tokio::test]
async fn gift_note_never_persisted_in_body_blob() {
    let Some(store) = store_or_skip("gift-note-purge").await else {
        return;
    };
    let client = raw_client("gift-note-purge").await;
    let table = "t-gift-note-purge";
    const SECRET: &str = "between us only ♡";

    let raw_body = |token: &'static str| {
        let client = client.clone();
        async move {
            let out = client
                .get_item()
                .table_name(table)
                .key("pk", AttributeValue::S(format!("LINK#{token}")))
                .key("sk", AttributeValue::S("META".into()))
                .send()
                .await
                .unwrap();
            let item = out.item.unwrap();
            item.get("body").unwrap().as_s().unwrap().to_string()
        }
    };

    // create (writer 1): note readable via the attr, absent from body
    store.put_game(&game(1, true)).await.unwrap();
    let mut noted = link("tok-purge");
    noted.gift_note = Some(SECRET.into());
    store.create_link(&noted).await.unwrap();
    assert_eq!(
        store
            .get_link("tok-purge")
            .await
            .unwrap()
            .unwrap()
            .gift_note
            .as_deref(),
        Some(SECRET)
    );
    assert!(
        !raw_body("tok-purge").await.contains("between us"),
        "create must not serialize the note into body"
    );

    // claim_game (writer 2) rewrites body — still noteless
    store
        .claim_game(
            "tok-purge",
            &game_id("gk1", "mn"),
            "c1",
            datetime!(2026-07-02 12:00 UTC),
        )
        .await
        .unwrap();
    assert!(
        !raw_body("tok-purge").await.contains("between us"),
        "claim's body rewrite must not serialize the note"
    );

    // update_link_meta (writer 3, the revoke shape) — still noteless
    let mut l = store.get_link("tok-purge").await.unwrap().unwrap();
    l.revoked = true;
    store.update_link_meta(&l).await.unwrap();
    assert!(
        !raw_body("tok-purge").await.contains("between us"),
        "update_link_meta must not serialize the note"
    );

    // and after a clear, NOTHING anywhere holds the text
    assert!(store.set_link_gift_note("tok-purge", None).await.unwrap());
    let out = client
        .get_item()
        .table_name(table)
        .key("pk", AttributeValue::S("LINK#tok-purge".into()))
        .key("sk", AttributeValue::S("META".into()))
        .send()
        .await
        .unwrap();
    let item = out.item.unwrap();
    assert!(!item.contains_key("gift_note"), "attr removed on clear");
    assert!(
        !item
            .get("body")
            .unwrap()
            .as_s()
            .unwrap()
            .contains("between us"),
        "cleared note must leave no copy at rest"
    );
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

/// batch_get_steam_apps: found appids come back keyed by app_id (including
/// negative-cache stubs); missing appids are simply absent (no error); empty
/// input short-circuits to an empty map with no I/O.
#[tokio::test]
async fn batch_get_steam_apps_found_and_missing() {
    let Some(store) = store_or_skip("batch-get-steam-apps").await else {
        return;
    };
    let full = steam_app_cache_full(570);
    let stub = steam_app_cache_stub(571);
    store.put_steam_app(&full).await.unwrap();
    store.put_steam_app(&stub).await.unwrap();

    let map = store
        .batch_get_steam_apps(&[570, 571, 99999])
        .await
        .unwrap();

    assert_eq!(map.len(), 2, "two found, one missing");
    assert_eq!(
        map.get(&570).unwrap().detail.as_ref().unwrap().genres,
        vec!["Action".to_string(), "Indie".to_string()]
    );
    assert!(
        map.get(&571).unwrap().detail.is_none(),
        "negative-cache stub round-trips through the batch read"
    );
    assert!(!map.contains_key(&99999));

    let empty = store.batch_get_steam_apps(&[]).await.unwrap();
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
        gift_note: None,
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

/// I1: DynamoDB-level Manual guard — mapper cannot clobber a Manual override even
/// under a concurrent read→write race (the condition expression rejects the write).
///
/// This is the DynamoDB-condition path of the guard. The in-memory check at the top
/// of `set_game_steam_appid_if_unclaimed` catches the non-race case; this test seeds
/// the item with `appid_source = Manual` directly so the DDB condition is what stands
/// between the mapper and a lost admin write.
///
/// RED: before the `appid_source <> :manual` condition was added to the PutItem call,
/// the in-memory guard still caught this via the pre-read, but the DDB condition did
/// NOT independently enforce it — a true concurrent race would have clobbered Manual.
#[tokio::test]
async fn set_game_steam_appid_if_unclaimed_manual_guard_ddb_condition() {
    let Some(store) = store_or_skip("appid-manual-guard-ddb").await else {
        return;
    };

    // Seed a game with appid_source = Manual (admin override already set).
    let mut manual_game = game(1, true);
    manual_game.steam_app_id = Some(999);
    manual_game.appid_source = Some(AppidSource::Manual);
    let gid = manual_game.id.clone();
    store.put_game(&manual_game).await.unwrap();

    // Mapper attempt must be Skipped — Manual is untouchable.
    let result = store
        .set_game_steam_appid_if_unclaimed(&gid, 12345, AppidSource::Title)
        .await
        .unwrap();
    assert!(
        matches!(result, AppidWrite::Skipped),
        "Manual-sourced game must return Skipped, got {result:?}"
    );

    // Stored values must be unchanged.
    let after = store.get_game(&gid).await.unwrap().unwrap();
    assert_eq!(
        after.steam_app_id,
        Some(999),
        "Manual steam_app_id must not be clobbered"
    );
    assert_eq!(
        after.appid_source,
        Some(AppidSource::Manual),
        "appid_source must remain Manual"
    );
}

/// I1-regression: Title-sourced game → mapper write still succeeds (guard must not block non-Manual).
#[tokio::test]
async fn set_game_steam_appid_if_unclaimed_title_source_still_written() {
    let Some(store) = store_or_skip("appid-title-regression").await else {
        return;
    };

    // Seed a game with appid_source = Title (the normal mapper case).
    let mut title_game = game(1, true);
    title_game.steam_app_id = Some(100);
    title_game.appid_source = Some(AppidSource::Title);
    let gid = title_game.id.clone();
    store.put_game(&title_game).await.unwrap();

    // Mapper update must succeed.
    let result = store
        .set_game_steam_appid_if_unclaimed(&gid, 200, AppidSource::Title)
        .await
        .unwrap();
    assert!(
        matches!(result, AppidWrite::Written),
        "Title-sourced game must be overwritable by mapper, got {result:?}"
    );

    let after = store.get_game(&gid).await.unwrap().unwrap();
    assert_eq!(after.steam_app_id, Some(200), "appid must be updated");
    assert_eq!(after.appid_source, Some(AppidSource::Title));
}

// -------------------------------------------------------------------------------------------------
// FIX 1 new tests: DDB-condition race guard + top-level appid_source attribute.
// -------------------------------------------------------------------------------------------------

/// FIX 1 — race guard via admin path: after `set_game_steam_appid_admin(..Some(appid))` stamps
/// appid_source=Manual at the DDB level, a subsequent `set_game_steam_appid_if_unclaimed` MUST
/// return Skipped and the stored appid must be unchanged. Before FIX 1 the DDB condition used
/// "Manual" (PascalCase) which never matched the snake_case serialized "manual", so the condition
/// was dead; only the in-memory guard in `set_game_steam_appid_if_unclaimed` caught it.
/// After FIX 1, both the in-memory guard AND the DDB condition use "manual".
#[tokio::test]
async fn set_game_steam_appid_if_unclaimed_after_admin_set_returns_skipped() {
    let Some(store) = store_or_skip("appid-admin-then-unclaimed").await else {
        return;
    };

    // Start with no appid — mapper can write.
    let mut g = game(1, true);
    let gid = g.id.clone();
    store.put_game(&g).await.unwrap();

    // Admin sets appid + Manual source.
    let result = store
        .set_game_steam_appid_admin(&gid, Some(77777))
        .await
        .unwrap();
    assert!(matches!(result, AppidWrite::Written));

    // Verify admin stamp is in DDB.
    let after_admin = store.get_game(&gid).await.unwrap().unwrap();
    assert_eq!(after_admin.steam_app_id, Some(77777));
    assert_eq!(after_admin.appid_source, Some(AppidSource::Manual));

    // Mapper attempt — in-memory guard fires on the read, OR DDB condition fires on the write.
    // Either way, must return Skipped and not clobber the admin appid.
    let skipped = store
        .set_game_steam_appid_if_unclaimed(&gid, 99999, AppidSource::Title)
        .await
        .unwrap();
    assert!(
        matches!(skipped, AppidWrite::Skipped),
        "after admin set Manual, if_unclaimed must return Skipped, got {skipped:?}"
    );

    // Stored appid must be unchanged.
    let final_game = store.get_game(&gid).await.unwrap().unwrap();
    assert_eq!(
        final_game.steam_app_id,
        Some(77777),
        "admin appid must not be clobbered by mapper"
    );
    assert_eq!(
        final_game.appid_source,
        Some(AppidSource::Manual),
        "appid_source must remain Manual"
    );

    // ── Manual-set game with stale in-memory view ──────────────────────────────
    // Re-use the game: craft a stale copy where appid_source reads as None
    // (simulates the mapper reading before the admin write landed). Then call
    // set_game_steam_appid_if_unclaimed *on the same game id* — the function
    // will re-read from DDB internally and the in-memory guard will also see Manual.
    // This confirms the guard path is exercised regardless of the caller's stale
    // in-memory view. The DDB condition is the safety net for the true race.
    g.appid_source = None; // simulate stale read
    g.steam_app_id = None;
    // We do NOT write g back to DDB — the DDB item still has Manual.
    let skipped2 = store
        .set_game_steam_appid_if_unclaimed(&gid, 55555, AppidSource::Humble)
        .await
        .unwrap();
    assert!(
        matches!(skipped2, AppidWrite::Skipped),
        "stale-view attempt must still be Skipped, got {skipped2:?}"
    );
    let still_77777 = store.get_game(&gid).await.unwrap().unwrap();
    assert_eq!(still_77777.steam_app_id, Some(77777));
}

/// FIX 1 — top-level attribute: after `put_game` with `appid_source = Some(Manual)`,
/// the raw DDB item MUST contain a top-level `appid_source` attribute with value "manual".
/// RED before the schema change (appid_source was only inside the `body` JSON blob).
#[tokio::test]
async fn appid_source_is_top_level_attribute() {
    let Some(store) = store_or_skip("appid-toplevel-attr").await else {
        return;
    };
    let client = raw_client("appid-toplevel-attr").await;
    let table = "t-appid-toplevel-attr";

    // Put a game with Manual appid_source.
    let mut g = game(1, true);
    g.steam_app_id = Some(12345);
    g.appid_source = Some(AppidSource::Manual);
    let gid = g.id.clone();
    store.put_game(&g).await.unwrap();

    // Fetch the raw DDB item — NOT via the store API (which only reads body).
    let pk = format!("GAME#{gid}");
    let raw = client
        .get_item()
        .table_name(table)
        .key("pk", aws_sdk_dynamodb::types::AttributeValue::S(pk))
        .key(
            "sk",
            aws_sdk_dynamodb::types::AttributeValue::S("META".into()),
        )
        .send()
        .await
        .unwrap()
        .item
        .expect("item must exist");

    // Top-level `appid_source` must be present with value "manual" (snake_case).
    let top_level_src = raw
        .get("appid_source")
        .expect("appid_source must be a top-level DDB attribute after FIX 1")
        .as_s()
        .expect("appid_source must be a String AttributeValue");
    assert_eq!(
        top_level_src, "manual",
        "top-level appid_source must be \"manual\" (snake_case), got \"{top_level_src}\""
    );

    // Top-level `appid_source` must also be present for Title.
    let mut g2 = game(2, true);
    g2.steam_app_id = Some(9001);
    g2.appid_source = Some(AppidSource::Title);
    let gid2 = g2.id.clone();
    store.put_game(&g2).await.unwrap();

    let pk2 = format!("GAME#{gid2}");
    let raw2 = client
        .get_item()
        .table_name(table)
        .key("pk", aws_sdk_dynamodb::types::AttributeValue::S(pk2))
        .key(
            "sk",
            aws_sdk_dynamodb::types::AttributeValue::S("META".into()),
        )
        .send()
        .await
        .unwrap()
        .item
        .expect("item must exist");

    let top_level_src2 = raw2
        .get("appid_source")
        .expect("appid_source must be top-level for Title source too")
        .as_s()
        .expect("appid_source must be a String AttributeValue");
    assert_eq!(top_level_src2, "title");

    // When appid_source is None, the attribute must be ABSENT (so attribute_not_exists fires).
    let mut g3 = game(3, true);
    g3.appid_source = None;
    let gid3 = g3.id.clone();
    store.put_game(&g3).await.unwrap();

    let pk3 = format!("GAME#{gid3}");
    let raw3 = client
        .get_item()
        .table_name(table)
        .key("pk", aws_sdk_dynamodb::types::AttributeValue::S(pk3))
        .key(
            "sk",
            aws_sdk_dynamodb::types::AttributeValue::S("META".into()),
        )
        .send()
        .await
        .unwrap()
        .item
        .expect("item must exist");

    assert!(
        !raw3.contains_key("appid_source"),
        "appid_source must NOT be present at top level when game.appid_source is None"
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

// ── set_game_steam_appid_admin tests ─────────────────────────────────────────

/// `set_game_steam_appid_admin` with Some(appid) sets steam_app_id and appid_source=Manual.
#[tokio::test]
async fn set_game_steam_appid_admin_sets_manual() {
    let Some(store) = store_or_skip("appid-admin-set").await else {
        return;
    };
    let g = game(1, true);
    let gid = g.id.clone();
    store.put_game(&g).await.unwrap();

    let result = store
        .set_game_steam_appid_admin(&gid, Some(12345))
        .await
        .unwrap();
    assert!(
        matches!(result, AppidWrite::Written),
        "setting appid on available game must be Written"
    );

    let got = store.get_game(&gid).await.unwrap().unwrap();
    assert_eq!(got.steam_app_id, Some(12345), "steam_app_id must be set");
    assert_eq!(
        got.appid_source,
        Some(AppidSource::Manual),
        "appid_source must be Manual"
    );
}

/// `set_game_steam_appid_admin` with None clears both steam_app_id and appid_source.
#[tokio::test]
async fn set_game_steam_appid_admin_clears_to_none() {
    let Some(store) = store_or_skip("appid-admin-clear").await else {
        return;
    };
    let mut g = game(1, true);
    g.steam_app_id = Some(99999);
    g.appid_source = Some(AppidSource::Manual);
    let gid = g.id.clone();
    store.put_game(&g).await.unwrap();

    let result = store.set_game_steam_appid_admin(&gid, None).await.unwrap();
    assert!(
        matches!(result, AppidWrite::Written),
        "clearing appid on available game must be Written"
    );

    let got = store.get_game(&gid).await.unwrap().unwrap();
    assert!(got.steam_app_id.is_none(), "steam_app_id must be cleared");
    assert!(got.appid_source.is_none(), "appid_source must be cleared");
}

/// `set_game_steam_appid_admin` on a non-existent game → NotFound.
#[tokio::test]
async fn set_game_steam_appid_admin_notfound() {
    let Some(store) = store_or_skip("appid-admin-notfound").await else {
        return;
    };
    let result = store
        .set_game_steam_appid_admin("no-such-id", Some(1))
        .await
        .unwrap();
    assert!(
        matches!(result, AppidWrite::NotFound),
        "unknown game id must return NotFound"
    );
}

/// `set_game_steam_appid_admin` on a Pending game → Contested.
#[tokio::test]
async fn set_game_steam_appid_admin_contested() {
    // Use a UUID-based table name to avoid pollution across reruns on the same moto server.
    let uid = Uuid::new_v4().simple().to_string();
    let Some(store) = store_or_skip(&format!("appid-admin-ct-{}", &uid[..8])).await else {
        return;
    };
    let g = game(1, true);
    let gid = g.id.clone();
    store.put_game(&g).await.unwrap();
    let link_token = format!("tok-appid-admin-{}", &uid[..8]);
    store.create_link(&link(&link_token)).await.unwrap();
    let now = datetime!(2026-07-06 12:00 UTC);
    store
        .claim_game(
            &link_token,
            &gid,
            &format!("c-appid-admin-{}", &uid[..8]),
            now,
        )
        .await
        .unwrap();

    let result = store
        .set_game_steam_appid_admin(&gid, Some(1))
        .await
        .unwrap();
    assert!(
        matches!(result, AppidWrite::Contested),
        "Pending game must return Contested"
    );
}

/// `set_game_steam_appid_admin` BYPASSES the Manual guard — unlike
/// `set_game_steam_appid_if_unclaimed` which returns Skipped on Manual source.
#[tokio::test]
async fn set_game_steam_appid_admin_bypasses_manual_guard() {
    let Some(store) = store_or_skip("appid-admin-bypass").await else {
        return;
    };
    let mut g = game(1, true);
    g.steam_app_id = Some(111);
    g.appid_source = Some(AppidSource::Manual);
    let gid = g.id.clone();
    store.put_game(&g).await.unwrap();

    // set_game_steam_appid_if_unclaimed returns Skipped on Manual source.
    let skipped = store
        .set_game_steam_appid_if_unclaimed(&gid, 222, AppidSource::Humble)
        .await
        .unwrap();
    assert!(
        matches!(skipped, AppidWrite::Skipped),
        "if_unclaimed must return Skipped on Manual source"
    );

    // set_game_steam_appid_admin overrides it.
    let written = store
        .set_game_steam_appid_admin(&gid, Some(222))
        .await
        .unwrap();
    assert!(
        matches!(written, AppidWrite::Written),
        "admin override must bypass Manual guard → Written"
    );

    let got = store.get_game(&gid).await.unwrap().unwrap();
    assert_eq!(got.steam_app_id, Some(222), "appid must be updated to 222");
    assert_eq!(
        got.appid_source,
        Some(AppidSource::Manual),
        "source must remain Manual after override"
    );
}

// =================================================================================================
// TASK 2 (game-detail-modal plan): STEAMAPP enrichment cache — put/get/list round-trips.
// =================================================================================================

fn steam_app_cache_full(app_id: u32) -> SteamAppCache {
    SteamAppCache {
        app_id,
        detail: Some(steam_client::SteamAppDetail {
            app_id,
            name: format!("Test Game {app_id}"),
            developers: vec!["Dev Studio".into()],
            publishers: vec!["Pub Corp".into()],
            genres: vec!["Action".into(), "Indie".into()],
            release_date: Some("1 Jan, 2020".into()),
            short_description: "A test game.".into(),
            header_image: Some("https://cdn.steam/header.jpg".into()),
            video_hls_url: None,
            video_thumbnail: None,
            screenshots: vec![],
            tags: vec![],
            content_descriptor_ids: vec![],
            content_notes: None,
        }),
        overall: Some(steam_client::ReviewSummary {
            desc: "Mostly Positive".into(),
            total_positive: 900,
            total_negative: 100,
            total_reviews: 1000,
        }),
        recent: Some(steam_client::RecentReviews {
            percent_positive: 88,
            count: 50,
        }),
        fetched_at: 1_800_000_000,
        reviews_fetched_at: 1_800_001_000,
    }
}

fn steam_app_cache_stub(app_id: u32) -> SteamAppCache {
    SteamAppCache {
        app_id,
        detail: None, // negative-cache: app delisted or never existed
        overall: None,
        recent: None,
        fetched_at: 1_800_000_000,
        reviews_fetched_at: 1_800_000_000,
    }
}

/// Full STEAMAPP cache item round-trips through put/get: with detail populated AND
/// the negative-cache stub (detail=None). get on a missing id returns Ok(None).
#[tokio::test]
async fn steam_app_cache_roundtrip() {
    let Some(store) = store_or_skip("steam-app-roundtrip").await else {
        return;
    };

    let full = steam_app_cache_full(570);
    let stub = steam_app_cache_stub(620);

    // Initially absent.
    assert_eq!(store.get_steam_app(570).await.unwrap(), None);
    assert_eq!(store.get_steam_app(620).await.unwrap(), None);

    // Put both.
    store.put_steam_app(&full).await.unwrap();
    store.put_steam_app(&stub).await.unwrap();

    // Full item round-trips exactly.
    let got_full = store.get_steam_app(570).await.unwrap().unwrap();
    assert_eq!(got_full, full, "full cache item must round-trip exactly");

    // Negative-cache stub round-trips (detail=None preserved).
    let got_stub = store.get_steam_app(620).await.unwrap().unwrap();
    assert_eq!(
        got_stub, stub,
        "negative-cache stub must round-trip with detail=None"
    );

    // Missing id → Ok(None).
    assert_eq!(store.get_steam_app(999999).await.unwrap(), None);
}

/// list_steam_app_ids returns exactly the written app_ids and does NOT include
/// non-STEAMAPP items (games) — the begins_with("STEAMAPP#") filter is the gate.
#[tokio::test]
async fn list_steam_app_ids_excludes_non_steamapp_items() {
    let Some(store) = store_or_skip("steam-app-list").await else {
        return;
    };

    // Write two STEAMAPP items (one full, one stub).
    store
        .put_steam_app(&steam_app_cache_full(100))
        .await
        .unwrap();
    store
        .put_steam_app(&steam_app_cache_stub(200))
        .await
        .unwrap();

    // Also write a GAME item to confirm it is excluded from the list.
    store.put_game(&game(1, true)).await.unwrap();

    let ids = store.list_steam_app_ids().await.unwrap();
    let mut sorted_ids = ids.clone();
    sorted_ids.sort();
    assert_eq!(
        sorted_ids,
        vec![100u32, 200u32],
        "list must return exactly the two STEAMAPP ids"
    );

    // Idempotent overwrite: re-put one item, list must not grow.
    store
        .put_steam_app(&steam_app_cache_full(100))
        .await
        .unwrap();
    let ids2 = store.list_steam_app_ids().await.unwrap();
    assert_eq!(
        ids2.len(),
        2,
        "idempotent overwrite must not duplicate entries"
    );
}

/// Pre-#61 blobs have no `screenshots` key in the detail JSON. `#[serde(default)]` must
/// fill `[]` — if this ever fails, every cached app in prod stops deserializing.
#[test]
fn steam_app_cache_pre_screenshots_blob_deserializes() {
    let body = r#"{"app_id":413150,"detail":{"app_id":413150,"name":"Stardew Valley","developers":["ConcernedApe"],"publishers":["ConcernedApe"],"genres":["Indie"],"release_date":"Feb 26, 2016","short_description":"farm.","header_image":null,"video_hls_url":null,"video_thumbnail":null},"overall":null,"recent":null,"fetched_at":100,"reviews_fetched_at":100}"#;
    let cache: dynamo::SteamAppCache =
        serde_json::from_str(body).expect("pre-screenshots blob must still deserialize");
    assert_eq!(cache.detail.expect("detail present").screenshots, vec![]);
}
