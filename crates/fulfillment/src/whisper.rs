//! The attic whispers — pure selection (spec: `docs/spec-attic-whispers.md`).
//!
//! Everything here is I/O-free on purpose: the handler in `lib.rs` does the reads and the
//! two-write orchestration; this module owns the PREDICATE, and the predicate is the part the
//! family review bled on. The load-bearing rules, each pinned by a test below:
//!
//! - **A curated pick is a promise; an open shelf is not.** `Link.curated_game_ids: None` is an
//!   open shelf — the whole listable catalog — and that is 18 of 18 live links (measured
//!   2026-08-28; re-derivation query in the spec). Excluding every game "offered on a link"
//!   would therefore empty the attic forever: the vacuous-exclusion trap, caught at review by
//!   measuring what the naive predicate evaluates to against live data.
//! - **An undelivered whisper is a receipt, never an exclusion** — a send that failed must not
//!   burn the treasure it failed to deliver.
//! - **Exhaustion rolls the cycle over instead of going quiet** — corpus size IS the period, and
//!   silence reads as broken (OMBB). The handler drives the rollover; [`delivered_ids`] just
//!   answers per-cycle.
//! - **Coverage across a cycle is carried by the exclusion log, never by the hash.** The picker's
//!   determinism ("same slot ⇒ same winner") holds only over an unchanged pool — a claim landing
//!   between attempts remaps the index. The conditional put in the store is the guarantee; the
//!   deterministic index is a belt. Do not "improve" the distribution believing it owns coverage.

use domain::{Game, Link, WhisperRecord};
use std::collections::HashSet;
use time::OffsetDateTime;

/// The ids promised on ACTIVE curated links. Active = the exact complement of `can_claim`'s
/// refusal ladder as far as a promise is concerned: not revoked, not expired, slots remaining.
/// (A SEALED link is active — the promise is made before the unwrap.) An open shelf
/// (`curated_game_ids: None`) contributes NOTHING — see the module docs for why that is the
/// whole predicate.
pub fn active_promises(links: &[Link], now: OffsetDateTime) -> HashSet<String> {
    links
        .iter()
        .filter(|l| {
            !l.revoked && l.expires_at.is_none_or(|e| e > now) && l.claims_used < l.claims_allowed
        })
        .flat_map(|l| l.curated_game_ids.iter().flatten().cloned())
        .collect()
}

/// The whisper log's current cycle: the highest cycle any row carries, 0 on an empty log.
pub fn current_cycle(whispers: &[WhisperRecord]) -> u32 {
    whispers.iter().map(|w| w.cycle).max().unwrap_or(0)
}

/// Game ids already whispered-AND-DELIVERED in the given cycle. `delivered == false` rows are
/// failure receipts and deliberately absent here — the treasure stays eligible.
pub fn delivered_ids(whispers: &[WhisperRecord], cycle: u32) -> HashSet<String> {
    whispers
        .iter()
        .filter(|w| w.cycle == cycle && w.delivered)
        .map(|w| w.game_id.clone())
        .collect()
}

/// The candidate pool: listable ∧ unpromised ∧ not-yet-whispered-this-cycle, sorted by
/// (title, id) — Scan/Query order is not stable, and the deterministic pick needs a stable order.
pub fn eligible<'a>(
    games: &'a [Game],
    promises: &HashSet<String>,
    excluded: &HashSet<String>,
) -> Vec<&'a Game> {
    let mut pool: Vec<&Game> = games
        .iter()
        .filter(|g| g.is_listable() && !promises.contains(&g.id) && !excluded.contains(&g.id))
        .collect();
    pool.sort_by(|a, b| a.title.cmp(&b.title).then_with(|| a.id.cmp(&b.id)));
    pool
}

/// Pick one treasure. Candidates with artwork outrank artless (a treasure that can arrive
/// dressed, PRODUCT.md principle 3) — artless stay eligible when they're all that's left
/// (principle 5: delight never gates). Index is Fibonacci-hashed from the slot's julian day so a
/// same-slot retry over an UNCHANGED pool re-derives the same winner; see the module docs for
/// the scope of that determinism.
pub fn select<'a>(pool: &[&'a Game], julian_day: i64) -> Option<&'a Game> {
    let dressed: Vec<&Game> = pool
        .iter()
        .filter(|g| g.artwork_url.is_some())
        .copied()
        .collect();
    let effective: &[&Game] = if dressed.is_empty() { pool } else { &dressed };
    if effective.is_empty() {
        return None;
    }
    let idx = (julian_day.unsigned_abs().wrapping_mul(2_654_435_761)) as usize % effective.len();
    Some(effective[idx])
}

// ── the details card (spec: docs/spec-whisper-details-card.md) ───────────────────────────────
// Discord API limits. The gallery grouping below (≤4 embeds sharing one identical `url` merge
// their images) is CLIENT RENDERING, not API contract — if it ever stops, the card degrades to a
// tall column of single-image embeds, nothing lost; nothing may ASSUME three groups.

pub const EMBED_TITLE_MAX: usize = 256;
pub const EMBED_DESC_MAX: usize = 4096;
pub const EMBED_FIELD_VALUE_MAX: usize = 1024;
pub const EMBED_TOTAL_TEXT_MAX: usize = 6000;
pub const MAX_EMBEDS: usize = 10;
pub const GALLERY_GROUP: usize = 4;

/// Char-boundary-safe truncation with a `…` marker. `max` counts CHARS (Discord counts
/// characters, not bytes).
pub(crate) fn trunc(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

/// Which shape a 🔍 preview is showing — the footer names it, because the two render identically
/// otherwise (family round 1, Lilith ③).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PreviewKind {
    NewestDelivered,
    DryRunPick,
}

impl PreviewKind {
    fn label(self) -> &'static str {
        match self {
            PreviewKind::NewestDelivered => "newest delivered",
            PreviewKind::DryRunPick => "today's dry pick",
        }
    }
}

/// The footer, one constructor: preview marking is a PREFIX (travels with the embeds), the
/// trimmed/trailer-cut markers SUFFIXES. `trailer_cut` names the highest-priority loser when even
/// the link cannot fit — a budget that can't seat its highest-priority element needs a stated
/// loser, not an accident.
fn footer_text(
    cycle: u32,
    slot: &str,
    preview: Option<PreviewKind>,
    trimmed: bool,
    trailer_cut: bool,
) -> String {
    let mut f = String::new();
    if let Some(k) = preview {
        f.push_str(&format!("🔍 preview — {} · ", k.label()));
    }
    f.push_str(&format!("the attic whispers · cycle {cycle} · {slot}"));
    if trimmed {
        f.push_str(" · trimmed to fit");
    }
    if trailer_cut {
        f.push_str(" · trailer link cut");
    }
    f
}

/// The full webhook body: the v1 voice in `content` (minus the bare art URL — the embed carries
/// art now) + the details-card embeds. Pure on purpose: every Discord-limit rule is pinned by a
/// test below and the handler just POSTs the value.
///
/// `allowed_mentions: {"parse": []}` is documented as covering message CONTENT; the steam-wire
/// text in the embeds is protected by a DIFFERENT, observed-but-uncited behavior (embeds do not
/// render mentions). Two layers, two halves — do not claim the field covers the embeds.
pub fn whisper_card(
    game: &Game,
    steam: Option<&dynamo::SteamAppCache>,
    site_url: &str,
    cycle: u32,
    slot: &str,
    preview: Option<PreviewKind>,
) -> serde_json::Value {
    let q = urlencoding::encode(&game.title);
    let mut content = format!(
        "🕯️ *from the attic…*\n**{title}** has been sleeping in *{bundle}*.\ncut a link for someone ♡ → {site}/admin/catalog?q={q}",
        title = game.title,
        bundle = game.bundle,
        site = site_url,
    );
    if let Some(k) = preview {
        content = format!("🔍 *preview — {}, not a new whisper*\n{content}", k.label());
    }
    let embeds = build_embeds(game, steam, cycle, slot, preview);
    serde_json::json!({
        "content": content,
        "embeds": embeds,
        "allowed_mentions": { "parse": [] },
    })
}

/// Fallback embed only for now (the full card lands with the steam half).
fn build_embeds(
    game: &Game,
    _steam: Option<&dynamo::SteamAppCache>,
    cycle: u32,
    slot: &str,
    preview: Option<PreviewKind>,
) -> Vec<serde_json::Value> {
    let title = trunc(&game.title, EMBED_TITLE_MAX);
    let trimmed = title != game.title;
    let mut e = serde_json::json!({
        "title": title,
        "footer": { "text": footer_text(cycle, slot, preview, trimmed, false) },
    });
    if let Some(art) = &game.artwork_url {
        e["image"] = serde_json::json!({ "url": art });
    }
    vec![e]
}

#[cfg(test)]
mod tests {
    use super::*;
    use domain::GameStatus;

    fn game(id: &str, title: &str, art: Option<&str>) -> Game {
        Game {
            id: id.into(),
            title: title.into(),
            bundle: "Humble Test Bundle".into(),
            gamekey: "gk".into(),
            machine_name: id.into(),
            key_type: "steam".into(),
            giftable: true,
            hidden: false,
            status: GameStatus::Available,
            claim_id: None,
            artwork_url: art.map(Into::into),
            keyindex: 0,
            requires_choice: false,
            steam_app_id: None,
            appid_source: None,
            owned_by_ben: false,
            hidden_source: None,
        }
    }

    fn game_with_bundle(id: &str, title: &str, bundle: &str, art: Option<&str>) -> Game {
        let mut g = game(id, title, art);
        g.bundle = bundle.into();
        g
    }

    fn test_link(token: &str) -> Link {
        Link {
            token: token.into(),
            label: "t".into(),
            gift_note: None,
            thank_note: None,
            thanked_at: None,
            claims_allowed: 3,
            claims_used: 0,
            revoked: false,
            expires_at: None,
            unlock_at: None,
            curated_game_ids: None,
            created_at: OffsetDateTime::now_utc(),
        }
    }

    fn wr(slot: &str, game_id: &str, cycle: u32, delivered: bool) -> WhisperRecord {
        WhisperRecord {
            slot: slot.into(),
            game_id: game_id.into(),
            cycle,
            delivered,
        }
    }

    #[test]
    fn active_promise_excludes_curated_only() {
        let now = OffsetDateTime::now_utc();
        let open_shelf = test_link("open"); // curated_game_ids: None
        let mut curated = test_link("cur");
        curated.curated_game_ids = Some(vec!["g1".into()]);
        let mut spent = test_link("spent"); // fully used curated link: promise expired
        spent.curated_game_ids = Some(vec!["g2".into()]);
        spent.claims_allowed = 1;
        spent.claims_used = 1;
        let mut revoked = test_link("rev");
        revoked.curated_game_ids = Some(vec!["g3".into()]);
        revoked.revoked = true;
        let p = active_promises(&[open_shelf, curated, spent, revoked], now);
        assert_eq!(p, ["g1".to_string()].into_iter().collect());
        // THE VACUOUS-EXCLUSION PIN: an open shelf (18/18 live links today) promises NOTHING —
        // if this set ever includes the catalog, the whisper has gone permanently silent.
    }

    #[test]
    fn sealed_curated_link_still_promises() {
        let now = OffsetDateTime::now_utc();
        let mut sealed = test_link("sealed");
        sealed.curated_game_ids = Some(vec!["g9".into()]);
        sealed.unlock_at = Some(now + time::Duration::days(2)); // wrapped, pre-unlock
        let p = active_promises(&[sealed], now);
        assert!(p.contains("g9")); // the promise is made before the unwrap
    }

    #[test]
    fn cycle_rolls_over_instead_of_going_quiet() {
        let ws = vec![wr("2026-W31", "g1", 0, true), wr("2026-W32", "g2", 0, true)];
        assert_eq!(current_cycle(&ws), 0);
        let excluded = delivered_ids(&ws, 0);
        let games = [game("g1", "a", None), game("g2", "b", None)];
        let pool = eligible(&games, &HashSet::new(), &excluded);
        assert!(pool.is_empty()); // pool exhausted at cycle 0…
        // …the handler then re-derives with cycle+1 and an empty exclusion set:
        let pool2 = eligible(&games, &HashSet::new(), &delivered_ids(&ws, 1));
        assert_eq!(pool2.len(), 2); // the attic starts over, never silences
    }

    #[test]
    fn undelivered_whisper_is_a_receipt_not_an_exclusion() {
        let ws = vec![wr("2026-W31", "g1", 0, false)]; // recorded, send FAILED
        assert!(delivered_ids(&ws, 0).is_empty()); // g1 stays eligible — the ①×two-write arm
    }

    #[test]
    fn eligible_respects_listability_and_stable_order() {
        let mut hidden = game("g3", "ccc", None);
        hidden.hidden = true;
        let mut pending = game("g4", "ddd", None);
        pending.status = GameStatus::Pending;
        let games = [
            game("g2", "bbb", None),
            game("g1", "aaa", None),
            hidden,
            pending,
        ];
        let pool = eligible(&games, &HashSet::new(), &HashSet::new());
        let ids: Vec<&str> = pool.iter().map(|g| g.id.as_str()).collect();
        assert_eq!(ids, vec!["g1", "g2"]); // title-sorted; hidden + pending gone via is_listable
    }

    #[test]
    fn select_prefers_dressed_treasures_and_is_deterministic() {
        let g_art = game("g1", "aaa", Some("https://art/1.png"));
        let g_plain = game("g2", "bbb", None);
        let pool = vec![&g_art, &g_plain];
        let picked = select(&pool, 2_461_281).unwrap();
        assert_eq!(picked.id, "g1"); // artwork subset wins while non-empty
        assert_eq!(select(&pool, 2_461_281).unwrap().id, picked.id); // same slot ⇒ same winner
        let artless = vec![&g_plain];
        assert!(select(&artless, 2_461_281).is_some()); // delight never gates
        assert!(select(&[], 2_461_281).is_none());
    }

    #[test]
    fn card_without_steam_carries_v1_information() {
        let g = game_with_bundle(
            "g1",
            "Overgrowth",
            "Humble Indie Bundle 9",
            Some("https://art/x.png"),
        );
        let v = whisper_card(&g, None, "https://bendobundles.com", 0, "2026-W36", None);
        let content = v["content"].as_str().unwrap();
        assert!(content.starts_with("🕯️")); // the register is the friend voice, not the ops voice
        assert!(content.contains("**Overgrowth**"));
        assert!(content.contains("Humble Indie Bundle 9"));
        assert!(content.contains("https://bendobundles.com/admin/catalog?q=Overgrowth"));
        assert!(!content.contains("https://art/x.png")); // art rides the embed now, never the content
        let embeds = v["embeds"].as_array().unwrap();
        assert_eq!(embeds.len(), 1);
        assert_eq!(embeds[0]["title"], "Overgrowth");
        assert_eq!(embeds[0]["image"]["url"], "https://art/x.png");
        assert_eq!(v["allowed_mentions"]["parse"].as_array().unwrap().len(), 0);
    }

    #[test]
    fn card_deeplink_urlencodes_the_title() {
        let g = game_with_bundle("g1", "Papers, Please", "HB 12", None);
        let v = whisper_card(&g, None, "https://bendobundles.com", 0, "2026-W36", None);
        assert!(v["content"].as_str().unwrap().contains("catalog?q=Papers%2C%20Please"));
    }

    #[test]
    fn card_preview_marks_content_and_footer_and_names_its_kind() {
        let g = game("g1", "aaa", None);
        let real = whisper_card(&g, None, "https://s", 0, "2026-W36", None);
        let prev = whisper_card(
            &g,
            None,
            "https://s",
            0,
            "2026-W36",
            Some(PreviewKind::NewestDelivered),
        );
        let dry = whisper_card(
            &g,
            None,
            "https://s",
            0,
            "2026-W36",
            Some(PreviewKind::DryRunPick),
        );
        assert!(prev["content"].as_str().unwrap().starts_with("🔍 *preview"));
        // the FOOTER is the mechanism — it travels with the embeds, the part anyone looks at
        assert!(prev["embeds"][0]["footer"]["text"]
            .as_str()
            .unwrap()
            .starts_with("🔍 preview — newest delivered"));
        assert!(dry["embeds"][0]["footer"]["text"]
            .as_str()
            .unwrap()
            .starts_with("🔍 preview — today's dry pick"));
        let strip = |v: &serde_json::Value| {
            v["embeds"][0]["footer"]["text"]
                .as_str()
                .unwrap()
                .rsplit("the attic whispers")
                .next()
                .unwrap()
                .to_string()
        };
        assert_eq!(strip(&real), strip(&prev)); // same card body under the marking
        assert!(!real["content"].as_str().unwrap().contains("preview"));
        assert!(!real["embeds"][0]["footer"]["text"]
            .as_str()
            .unwrap()
            .contains("preview"));
    }

    #[test]
    fn trunc_is_char_boundary_safe() {
        assert_eq!(trunc("abc", 5), "abc");
        assert_eq!(trunc("abcdef", 5), "abcd…");
        let s = "♡♡♡♡"; // 3-byte chars — a byte-index cut would panic
        assert_eq!(trunc(s, 3), "♡♡…");
    }
}
