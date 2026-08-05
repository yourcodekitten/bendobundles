//! Pure decision logic for the `heal_choice_pairs` operator sweep (spec Q5 / A6b).
//!
//! Extracted OUT of the `heal`-gated `heal_choice_pairs` bin so the normal `cargo test --workspace`
//! runs these tests: the bin is `required-features = ["heal"]`, so anything living there is only
//! *compiled* (by clippy `--all-features`), never *run*, by the default suite — a Skip-logic
//! regression would ship green. Only the `dynamo::Store::delete_game` CALL stays gated in the bin;
//! the gate decision itself is pure and always testable here.
use domain::{AppidSource, Game, GameStatus};

/// The gate decision for one candidate pair.
#[derive(Debug, PartialEq, Eq)]
pub enum Verdict {
    Heal,
    Skip(String),
}

/// The pure gate decision for one candidate (sibling, its offered row, the live order's tpks).
/// Heal only on positive dual evidence AND zero app-owned state on the sibling — except a
/// BenRedeemed/Expired sibling may still heal when the LIVE order's own tpk for the sibling
/// confirms that exact status (out-of-band redemption/expiry, #158): the audit already flipped
/// the sibling off Available, so gating on Available alone would wedge it forever.
pub fn pair_verdict(
    sibling: &Game,
    offered: Option<&Game>,
    live_order_tpks: &[humble_client::KeyEntry],
) -> Verdict {
    // State-gate the SIBLING: only rows with zero app-owned state auto-heal, widened for
    // BenRedeemed/Expired iff the live order confirms via the sibling's OWN tpk (exact
    // machine_name match — the sibling IS the tpk row).
    match sibling.status {
        GameStatus::Available => {}
        GameStatus::BenRedeemed => {
            let confirmed = live_order_tpks
                .iter()
                .any(|e| e.machine_name == sibling.machine_name && e.redeemed);
            if !confirmed {
                return Verdict::Skip(
                    "status BenRedeemed but live order does not confirm redemption".into(),
                );
            }
        }
        GameStatus::Expired => {
            let confirmed = live_order_tpks
                .iter()
                .any(|e| e.machine_name == sibling.machine_name && e.expired);
            if !confirmed {
                return Verdict::Skip(
                    "status Expired but live order does not confirm expiration".into(),
                );
            }
        }
        _ => {
            return Verdict::Skip(format!(
                "sibling status is {:?}, not Available",
                sibling.status
            ));
        }
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
        .any(|t| domain::choice_tpk_matches(&t.machine_name, &offered.machine_name))
    {
        return Verdict::Skip("order does not corroborate (no matching tpk live)".into());
    }
    Verdict::Heal
}

/// Candidate pair found in the scan: a choice-shaped `sibling` row and the `offered_id` its
/// grammar derives to, both under `gamekey`.
pub struct Pair {
    pub gamekey: String,
    pub sibling: Game,
    pub offered_id: String,
}

/// Scan the catalog for offered/tpk duplicate pairs: a choice-shaped GAME row whose derived
/// offered id also exists as a row under the same gamekey. Pure over the game list.
pub fn find_pairs(games: &[Game]) -> Vec<Pair> {
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

    fn key_entry(mn: &str) -> humble_client::KeyEntry {
        humble_client::KeyEntry {
            machine_name: mn.into(),
            human_name: "H".into(),
            key_type: "steam".into(),
            redeemed: false,
            expired: false,
            giftable: true,
            keyindex: 0,
            redeemed_key_val: None,
            steam_app_id: None,
            is_gift: None,
        }
    }

    #[test]
    fn gate_pass_yields_heal() {
        let sibling = game("GK:omega_row_choice_steam", "omega_row_choice_steam");
        let offered = game("GK:omega", "omega"); // requires_choice=false (flipped)
        let live = vec![key_entry("omega_row_choice_steam")];
        assert_eq!(pair_verdict(&sibling, Some(&offered), &live), Verdict::Heal);
    }

    #[test]
    fn status_not_available_skips() {
        // App-owned state: a sibling already Gifted/redeemed must never be auto-deleted.
        let mut sibling = game("GK:omega_row_choice_steam", "omega_row_choice_steam");
        sibling.status = GameStatus::Gifted;
        let offered = game("GK:omega", "omega");
        let live = vec![key_entry("omega_row_choice_steam")];
        assert!(
            matches!(pair_verdict(&sibling, Some(&offered), &live), Verdict::Skip(r) if r.contains("not Available"))
        );
    }

    #[test]
    fn hidden_sibling_skips() {
        // `hidden` is app-owned state (an admin/auto-hide decision) — never auto-heal over it.
        let mut sibling = game("GK:omega_row_choice_steam", "omega_row_choice_steam");
        sibling.hidden = true;
        let offered = game("GK:omega", "omega");
        let live = vec![key_entry("omega_row_choice_steam")];
        assert!(
            matches!(pair_verdict(&sibling, Some(&offered), &live), Verdict::Skip(r) if r.contains("hidden"))
        );
    }

    #[test]
    fn claim_entangled_skips() {
        let mut sibling = game("GK:omega_row_choice_steam", "omega_row_choice_steam");
        sibling.claim_id = Some("c1".into());
        let offered = game("GK:omega", "omega");
        let live = vec![key_entry("omega_row_choice_steam")];
        assert!(
            matches!(pair_verdict(&sibling, Some(&offered), &live), Verdict::Skip(r) if r.contains("claim-entangled"))
        );
    }

    #[test]
    fn offered_still_requires_choice_skips() {
        let sibling = game("GK:omega_row_choice_steam", "omega_row_choice_steam");
        let mut offered = game("GK:omega", "omega");
        offered.requires_choice = true; // not flipped yet
        let live = vec![key_entry("omega_row_choice_steam")];
        assert!(
            matches!(pair_verdict(&sibling, Some(&offered), &live), Verdict::Skip(r) if r.contains("not flipped"))
        );
    }

    #[test]
    fn order_not_corroborating_skips() {
        let sibling = game("GK:omega_row_choice_steam", "omega_row_choice_steam");
        let offered = game("GK:omega", "omega");
        let live: Vec<humble_client::KeyEntry> = vec![]; // live order carries no matching tpk
        assert!(
            matches!(pair_verdict(&sibling, Some(&offered), &live), Verdict::Skip(r) if r.contains("corroborate"))
        );
    }

    #[test]
    fn manual_appid_sibling_skips() {
        let mut sibling = game("GK:omega_row_choice_steam", "omega_row_choice_steam");
        sibling.appid_source = Some(AppidSource::Manual);
        let offered = game("GK:omega", "omega");
        let live = vec![key_entry("omega_row_choice_steam")];
        assert!(
            matches!(pair_verdict(&sibling, Some(&offered), &live), Verdict::Skip(r) if r.contains("Manual"))
        );
    }

    #[test]
    fn benredeemed_sibling_heals_when_order_confirms() {
        // #158: the audit already flipped this sibling off Available — the live order's own
        // tpk for it confirms the redemption, so the gate must still let it through.
        let mut sibling = game("GK:omega_row_choice_steam", "omega_row_choice_steam");
        sibling.status = GameStatus::BenRedeemed;
        let offered = game("GK:omega", "omega");
        let mut entry = key_entry("omega_row_choice_steam");
        entry.redeemed = true;
        let live = vec![entry];
        assert_eq!(pair_verdict(&sibling, Some(&offered), &live), Verdict::Heal);
    }

    #[test]
    fn benredeemed_sibling_skips_when_order_does_not_confirm() {
        let mut sibling = game("GK:omega_row_choice_steam", "omega_row_choice_steam");
        sibling.status = GameStatus::BenRedeemed;
        let offered = game("GK:omega", "omega");
        // live tpk exists but is not redeemed — does not confirm the sibling's status.
        let live = vec![key_entry("omega_row_choice_steam")];
        assert!(
            matches!(pair_verdict(&sibling, Some(&offered), &live), Verdict::Skip(r) if r.contains("does not confirm redemption"))
        );
    }

    #[test]
    fn expired_sibling_heals_when_order_confirms() {
        let mut sibling = game("GK:omega_row_choice_steam", "omega_row_choice_steam");
        sibling.status = GameStatus::Expired;
        let offered = game("GK:omega", "omega");
        let mut entry = key_entry("omega_row_choice_steam");
        entry.expired = true;
        let live = vec![entry];
        assert_eq!(pair_verdict(&sibling, Some(&offered), &live), Verdict::Heal);
    }

    #[test]
    fn gifted_sibling_still_skips() {
        // Behavior pin: the widening covers BenRedeemed/Expired only. Gifted must still hit
        // the plain "not Available" fallback — do not alter this to make it fail.
        let mut sibling = game("GK:omega_row_choice_steam", "omega_row_choice_steam");
        sibling.status = GameStatus::Gifted;
        let offered = game("GK:omega", "omega");
        let mut entry = key_entry("omega_row_choice_steam");
        entry.redeemed = true;
        entry.expired = true;
        let live = vec![entry];
        assert!(
            matches!(pair_verdict(&sibling, Some(&offered), &live), Verdict::Skip(r) if r.contains("not Available"))
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
