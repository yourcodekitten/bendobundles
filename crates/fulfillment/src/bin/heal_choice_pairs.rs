//! heal_choice_pairs — one-time sweep for the legacy offered/tpk duplicate GAME pairs
//! (15 in the 2026-07-31 prod scan). Spec Q5 / A6b, family-signed 2026-07-31.
//! **DRY-RUN BY DEFAULT; `--execute` deletes.**
//!
//! This is NOT delete-on-absence (that contract stands, undiluted): every delete rests on
//! positive dual evidence, re-derived LIVE at execution time —
//!   1. the sibling's id derives to the offered id through `domain::choice_tpk_bases`, AND
//!   2. the offered row exists and carries the key fields post-flip (`requires_choice == false`), AND
//!   3. the month's LIVE order (fetched now, not from the scheduling scan) carries a tpk matching
//!      the offered name — the order authorizes, the scan only schedules.
//!
//! State-gate (skip + print any pair failing it): sibling `status == Available`,
//! `claim_id.is_none()`, `!hidden`, `appid_source != Some(Manual)`. (mylittleuniverse's sibling
//! fails the gate today — expected; heal by hand post-claim.)
//!
//! Run by a human with AWS credentials + a humble cookie, never by CI or the lambda:
//!   TABLE_NAME=<table> HUMBLE_COOKIE=<sess> \
//!     AWS_PROFILE=kitten-maintenance cargo run -p fulfillment --features heal \
//!     --bin heal_choice_pairs [-- --execute]
use domain::Game;
use dynamo::Store;
use fulfillment::heal_pairs::{Verdict, find_pairs, pair_verdict};
use humble_client::{HumbleClient, KeyEntry, SessionCookie};
use std::collections::HashMap;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt().init();
    let execute = std::env::args().any(|a| a == "--execute");
    let table = std::env::var("TABLE_NAME").expect("TABLE_NAME required");
    let cookie =
        std::env::var("HUMBLE_COOKIE").expect("HUMBLE_COOKIE required (live-order re-verify)");
    let aws_cfg = aws_config::load_from_env().await;
    let store = Store::new(aws_sdk_dynamodb::Client::new(&aws_cfg), table);
    let humble = HumbleClient::new("https://www.humblebundle.com", SessionCookie::new(cookie))
        .expect("HumbleClient construction");

    let games = store.list_all_games().await.expect("list_all_games");
    let by_id: HashMap<String, Game> = games.iter().map(|g| (g.id.clone(), g.clone())).collect();
    let pairs = find_pairs(&games);
    println!(
        "mode={} — found {} candidate pair(s)",
        if execute { "EXECUTE" } else { "DRY-RUN" },
        pairs.len()
    );

    // Fetch each involved gamekey's live order once (the order AUTHORIZES; the scan only scheduled).
    // Pass the entries themselves — the widened gate (#158) reads `.redeemed`/`.expired` off them,
    // not just the machine_name.
    let mut order_tpks: HashMap<String, Vec<KeyEntry>> = HashMap::new();
    for gk in pairs
        .iter()
        .map(|p| &p.gamekey)
        .collect::<std::collections::HashSet<_>>()
    {
        match humble.order(gk).await {
            Ok(o) => {
                order_tpks.insert(gk.clone(), o.keys);
            }
            Err(e) => {
                eprintln!(
                    "WARN: live order fetch for {gk} failed ({e}) — its pairs will not corroborate"
                );
            }
        }
    }

    let (mut healed, mut skipped) = (0u32, 0u32);
    for p in &pairs {
        let offered = by_id.get(&p.offered_id);
        let live = order_tpks.get(&p.gamekey).cloned().unwrap_or_default();
        let verdict = pair_verdict(&p.sibling, offered, &live);
        match verdict {
            Verdict::Heal => {
                println!("HEAL  {} -> flips into {}", p.sibling.id, p.offered_id);
                if execute {
                    store.delete_game(&p.sibling.id).await.expect("delete_game");
                    println!("      deleted sibling {}", p.sibling.id);
                }
                healed += 1;
            }
            Verdict::Skip(reason) => {
                println!("SKIP  {} ({reason})", p.sibling.id);
                skipped += 1;
            }
        }
    }
    println!("healed={healed} skipped={skipped}");
}
