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
use domain::{AppidSource, Game, GameStatus};
use dynamo::Store;
use humble_client::{HumbleClient, SessionCookie};
use std::collections::HashMap;

#[derive(Debug, PartialEq, Eq)]
enum Verdict {
    Heal,
    Skip(String),
}

/// The pure gate decision for one candidate (sibling, its offered row, the live order's tpks).
/// Heal only on positive dual evidence AND zero app-owned state on the sibling.
fn pair_verdict(sibling: &Game, offered: Option<&Game>, live_order_tpks: &[String]) -> Verdict {
    // State-gate the SIBLING: only rows with zero app-owned state auto-heal.
    if sibling.status != GameStatus::Available {
        return Verdict::Skip(format!(
            "sibling status is {:?}, not Available",
            sibling.status
        ));
    }
    if sibling.claim_id.is_some() {
        return Verdict::Skip("claim-entangled (sibling has a claim_id)".into());
    }
    if sibling.hidden {
        return Verdict::Skip("sibling is hidden (app-owned state)".into());
    }
    if sibling.appid_source == Some(AppidSource::Manual) {
        return Verdict::Skip("sibling carries a Manual appid (app-owned state)".into());
    }
    // Positive dual evidence.
    let Some(offered) = offered else {
        return Verdict::Skip("offered row missing (no flip target)".into());
    };
    if offered.requires_choice {
        return Verdict::Skip("not flipped yet — run after a post-D7 sync".into());
    }
    if !live_order_tpks
        .iter()
        .any(|t| domain::choice_tpk_matches(t, &offered.machine_name))
    {
        return Verdict::Skip("order does not corroborate (no matching tpk live)".into());
    }
    Verdict::Heal
}

/// Candidate pair found in the scan: a choice-shaped `sibling` row and the `offered_id` its
/// grammar derives to, both under `gamekey`.
struct Pair {
    gamekey: String,
    sibling: Game,
    offered_id: String,
}

/// Scan the catalog for offered/tpk duplicate pairs: a choice-shaped GAME row whose derived
/// offered id also exists as a row under the same gamekey. Pure over the game list.
fn find_pairs(games: &[Game]) -> Vec<Pair> {
    let ids: std::collections::HashSet<&str> = games.iter().map(|g| g.id.as_str()).collect();
    let mut pairs = Vec::new();
    for g in games {
        let Some((exact, stripped)) = domain::choice_tpk_bases(&g.machine_name) else {
            continue;
        };
        // Ladder, not set: exact base first (an offered name may itself end _row), then region-
        // stripped; first hit wins.
        for candidate in std::iter::once(exact).chain(stripped) {
            let candidate_id = domain::game_id(&g.gamekey, &candidate);
            if candidate_id != g.id && ids.contains(candidate_id.as_str()) {
                pairs.push(Pair {
                    gamekey: g.gamekey.clone(),
                    sibling: g.clone(),
                    offered_id: candidate_id,
                });
                break;
            }
        }
    }
    pairs
}

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
    let mut order_tpks: HashMap<String, Vec<String>> = HashMap::new();
    for gk in pairs
        .iter()
        .map(|p| &p.gamekey)
        .collect::<std::collections::HashSet<_>>()
    {
        match humble.order(gk).await {
            Ok(o) => {
                order_tpks.insert(
                    gk.clone(),
                    o.keys.iter().map(|k| k.machine_name.clone()).collect(),
                );
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

#[cfg(test)]
mod tests {
    use super::*;

    fn game(id: &str, mn: &str) -> Game {
        Game {
            id: id.into(),
            title: "T".into(),
            bundle: "B".into(),
            gamekey: "GK".into(),
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
            hidden_source: None,
        }
    }

    #[test]
    fn gate_pass_yields_heal() {
        let sibling = game("GK:omega_row_choice_steam", "omega_row_choice_steam");
        let offered = game("GK:omega", "omega"); // requires_choice=false (flipped)
        let live = vec!["omega_row_choice_steam".to_string()];
        assert_eq!(pair_verdict(&sibling, Some(&offered), &live), Verdict::Heal);
    }

    #[test]
    fn claim_entangled_skips() {
        let mut sibling = game("GK:omega_row_choice_steam", "omega_row_choice_steam");
        sibling.claim_id = Some("c1".into());
        let offered = game("GK:omega", "omega");
        let live = vec!["omega_row_choice_steam".to_string()];
        assert!(
            matches!(pair_verdict(&sibling, Some(&offered), &live), Verdict::Skip(r) if r.contains("claim-entangled"))
        );
    }

    #[test]
    fn offered_still_requires_choice_skips() {
        let sibling = game("GK:omega_row_choice_steam", "omega_row_choice_steam");
        let mut offered = game("GK:omega", "omega");
        offered.requires_choice = true; // not flipped yet
        let live = vec!["omega_row_choice_steam".to_string()];
        assert!(
            matches!(pair_verdict(&sibling, Some(&offered), &live), Verdict::Skip(r) if r.contains("not flipped"))
        );
    }

    #[test]
    fn order_not_corroborating_skips() {
        let sibling = game("GK:omega_row_choice_steam", "omega_row_choice_steam");
        let offered = game("GK:omega", "omega");
        let live: Vec<String> = vec![]; // live order carries no matching tpk
        assert!(
            matches!(pair_verdict(&sibling, Some(&offered), &live), Verdict::Skip(r) if r.contains("corroborate"))
        );
    }

    #[test]
    fn manual_appid_sibling_skips() {
        let mut sibling = game("GK:omega_row_choice_steam", "omega_row_choice_steam");
        sibling.appid_source = Some(AppidSource::Manual);
        let offered = game("GK:omega", "omega");
        let live = vec!["omega_row_choice_steam".to_string()];
        assert!(
            matches!(pair_verdict(&sibling, Some(&offered), &live), Verdict::Skip(r) if r.contains("Manual"))
        );
    }

    #[test]
    fn find_pairs_matches_the_row_sibling() {
        let games = vec![
            game("GK:omega", "omega"),
            game("GK:omega_row_choice_steam", "omega_row_choice_steam"),
            game("GK:solo_choice_steam", "solo_choice_steam"), // no offered sibling → not a pair
        ];
        let pairs = find_pairs(&games);
        assert_eq!(pairs.len(), 1);
        assert_eq!(pairs[0].sibling.id, "GK:omega_row_choice_steam");
        assert_eq!(pairs[0].offered_id, "GK:omega");
    }
}
