//! fail_stuck_self_claim — one-time operator fail for a SELF claim that can never
//! complete as written (its game_id's machine_name is not, and will never be, among
//! its order's keys). First (and hopefully only) patient: claim
//! 3f46c058 / GAME#HAXSVMZHBvK2E7dW:mylittleuniverse — ben's 2026-07-06 self-claim of
//! a Feb-2025 Humble Choice game whose key was provisioned 29 days later under the
//! choice-tpk name and redeemed by ben out of band (runbook-choice-pair-heal step 4).
//! **DRY-RUN BY DEFAULT; `--execute` writes.**
//!
//! No new write path: this drives the production `Store::fail_self_claim_dead_key`
//! transaction — claim → Failed with the reason durable, game → Expired (retired,
//! never re-listed), claim_id cleared, pending marker consumed, idempotent under the
//! marker race. The bin only adds operator eyes: it prints the current claim + game
//! state and refuses to act unless the claim is SELF-linked, Pending, and pointing at
//! exactly the game_id given.
//!
//! Run by a human, never CI or the lambda:
//!   TABLE_NAME=<table> CLAIM_ID=<id> GAME_ID=<id> REASON="<why>" \
//!     AWS_PROFILE=kitten-maintenance cargo run -p fulfillment --features heal \
//!     --bin fail_stuck_self_claim [-- --execute]
use dynamo::Store;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt().init();
    let execute = std::env::args().any(|a| a == "--execute");
    let table = std::env::var("TABLE_NAME").expect("TABLE_NAME required");
    let claim_id = std::env::var("CLAIM_ID").expect("CLAIM_ID required");
    let game_id = std::env::var("GAME_ID").expect("GAME_ID required");
    let reason = std::env::var("REASON").expect("REASON required (written durably to the claim)");
    let aws_cfg = aws_config::load_from_env().await;
    let store = Store::new(aws_sdk_dynamodb::Client::new(&aws_cfg), table);

    let claim = store
        .get_claim(domain::SELF_LINK_TOKEN, &claim_id)
        .await
        .expect("get_claim")
        .expect("claim not found under LINK#SELF");
    let game = store
        .get_game(&game_id)
        .await
        .expect("get_game")
        .expect("game not found");

    println!(
        "claim {}: state={:?} link={} game_id={} created_at={}",
        claim.id, claim.state, claim.link_token, claim.game_id, claim.created_at
    );
    println!(
        "game  {}: status={:?} claim_id={:?} title={:?}",
        game.id, game.status, game.claim_id, game.title
    );
    println!("reason to write: {reason}");

    // Operator gate — every line a reason NOT to act, checked live.
    assert_eq!(
        claim.link_token,
        domain::SELF_LINK_TOKEN,
        "refusing: not a SELF claim — friend-link claims have budget semantics this bin does not handle"
    );
    assert_eq!(
        claim.state,
        domain::ClaimState::Pending,
        "refusing: claim is not Pending — its fate is already decided"
    );
    assert_eq!(
        claim.game_id, game_id,
        "refusing: claim.game_id does not match GAME_ID — wrong patient"
    );
    assert_eq!(
        game.claim_id.as_deref(),
        Some(claim_id.as_str()),
        "refusing: game.claim_id does not point back at this claim"
    );

    if !execute {
        println!("DRY-RUN: gates pass; rerun with -- --execute to fail the claim.");
        return;
    }

    store
        .fail_self_claim_dead_key(&claim_id, &game_id, &reason)
        .await
        .expect("fail_self_claim_dead_key");

    // Verify by re-read — the write's word is not the receipt.
    let claim = store
        .get_claim(domain::SELF_LINK_TOKEN, &claim_id)
        .await
        .expect("re-read claim")
        .expect("claim vanished");
    let game = store
        .get_game(&game_id)
        .await
        .expect("re-read game")
        .expect("game vanished");
    println!(
        "DONE: claim state={:?} failure_reason={:?}",
        claim.state, claim.failure_reason
    );
    println!(
        "DONE: game status={:?} claim_id={:?}",
        game.status, game.claim_id
    );
    assert_eq!(
        claim.state,
        domain::ClaimState::Failed,
        "post-write claim not Failed"
    );
    assert_eq!(
        game.status,
        domain::GameStatus::Expired,
        "post-write game not Expired"
    );
}
