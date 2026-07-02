use domain::{Claim, ClaimState, Game, GameStatus, Link, game_id};
use dynamo::Store;
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
