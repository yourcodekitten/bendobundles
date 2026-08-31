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
// gate-9 ③ (the survivor of a killed finding): MAX_EMBEDS and steam-client's SCREENSHOT_CAP were
// equal BY COINCIDENCE, in two crates, with nothing asserting the relation — and the footer is
// sealed before the gallery loop, so a cap above the budget would drop screenshots silently AND
// unannounceably. Top rung of the enforcement ladder: this REFUSES at compile time (a runtime
// test for it could never go red under this assert, so there deliberately isn't one).
const _: () = assert!(
    steam_client::SCREENSHOT_CAP <= MAX_EMBEDS,
    "steam-client SCREENSHOT_CAP exceeds the whisper card's embed budget — screenshots would drop silently"
);
pub const GALLERY_GROUP: usize = 4;
/// Discord's cap on message CONTENT — separate from every embed budget, audited separately
/// (pass-1 review: the embed budgets were airtight while content had no guard at all).
pub const CONTENT_MAX: usize = 2000;
/// Display caps inside the content voice line. Losers to the deep-link, the stated winner.
pub const CONTENT_TITLE_MAX: usize = 180;
pub const CONTENT_BUNDLE_MAX: usize = 180;
/// Raw-prefix cap for the catalog query token — a FILTER, not a display: cut without the `…`
/// marker (an encoded ellipsis would break `Catalog.tsx`'s substring match).
pub const CONTENT_QUERY_MAX: usize = 120;

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
    // Content-side loser order (pass-1 review): the deep-link WINS — it is the feature's one-tap
    // core; display title and bundle are the losers; the query token is a raw prefix. These caps
    // keep the NORMAL line short; the LIVE truncation at the seam below (gate-9 ②) is what makes
    // the CONTENT_MAX bound bind — config-fed site_url included, no prose claims, no trust.
    let q_prefix: String = game.title.chars().take(CONTENT_QUERY_MAX).collect();
    let q = urlencoding::encode(&q_prefix);
    let mut content = format!(
        "🕯️ *from the attic…*\n**{title}** has been sleeping in *{bundle}*.\ncut a link for someone ♡ → {site}/admin/catalog?q={q}",
        title = trunc(&game.title, CONTENT_TITLE_MAX),
        bundle = trunc(&game.bundle, CONTENT_BUNDLE_MAX),
        site = site_url,
    );
    if let Some(k) = preview {
        content = format!("🔍 *preview — {}, not a new whisper*\n{content}", k.label());
    }
    // gate-9 ②: the cap is ENFORCED here, not asserted in prose — site_url is config and a
    // pathological value must produce a short message, never a Discord 400. The cut announces
    // itself through the embed footer (content has no footer; the embeds travel with it).
    let content_full = content;
    let content = trunc(&content_full, CONTENT_MAX);
    let content_trimmed = content != content_full;
    let embeds = build_embeds(game, steam, cycle, slot, preview, content_trimmed);
    serde_json::json!({
        "content": content,
        "embeds": embeds,
        "allowed_mentions": { "parse": [] },
    })
}

/// The details card as embeds. Every media element has a place in a STATED loser order (spec
/// §class): screenshots + trailer-link outrank header art (header rides the thumbnail chain),
/// the trailer link outranks the description tail, and if even the link cannot fit it is cut AND
/// the footer names it — silence is never an outcome.
fn build_embeds(
    game: &Game,
    steam: Option<&dynamo::SteamAppCache>,
    cycle: u32,
    slot: &str,
    preview: Option<PreviewKind>,
    content_trimmed: bool,
) -> Vec<serde_json::Value> {
    let detail = steam.and_then(|c| c.detail.as_ref());
    let store_url = game
        .steam_app_id
        .map(|id| format!("https://store.steampowered.com/app/{id}"));
    // any truncation flips this; the footer announces it. Seeded with the CONTENT cut — the one
    // truncation that happens outside this fn (content has no footer of its own).
    let mut trimmed = content_trimmed;

    let title = trunc(&game.title, EMBED_TITLE_MAX);
    trimmed |= title != game.title;
    let mut main = serde_json::json!({ "title": title });
    if let Some(u) = &store_url {
        main["url"] = serde_json::json!(u);
    }
    // footer + fields text participate in the 6000 budget; measure the fixed parts against the
    // WORST-CASE footer (both markers on) so flipping a flag late can never overflow the budget.
    let footer_max = footer_text(cycle, slot, preview, true, true);

    let mut fields: Vec<(String, String)> = Vec::new();
    if let Some(d) = detail {
        let devs = d.developers.join(", ");
        let pubs = d.publishers.join(", ");
        let by = if pubs.is_empty() || pubs == devs {
            devs.clone()
        } else {
            format!("{devs} · {pubs}")
        };
        if !by.is_empty() {
            fields.push(("by".into(), by));
        }
        if let Some(r) = &d.release_date {
            fields.push(("released".into(), r.clone()));
        }
        let tags: &[String] = if d.tags.is_empty() {
            &d.genres
        } else {
            &d.tags
        };
        if !tags.is_empty() {
            fields.push(("tags".into(), tags.join(" · ")));
        }
    }
    if let Some(c) = steam {
        let line = match (&c.overall, &c.recent) {
            (Some(o), r) => {
                let pct = ((o.total_positive as f64 / (o.total_reviews.max(1) as f64)) * 100.0)
                    .round() as u64;
                let total = fmt_thousands(o.total_reviews);
                let recent = r
                    .as_ref()
                    .map(|r| {
                        format!(
                            " ({}% of {} recent)",
                            r.percent_positive,
                            fmt_thousands(r.count)
                        )
                    })
                    .unwrap_or_default();
                Some(format!("{} — {pct}% of {total}{recent}", o.desc))
            }
            (None, Some(r)) => Some(format!(
                "{}% of {} recent",
                r.percent_positive,
                fmt_thousands(r.count)
            )),
            (None, None) => None,
        };
        if let Some(l) = line {
            fields.push(("reviews".into(), l));
        }
    }
    fields.push((
        "bundle".into(),
        format!("{} ({})", game.bundle, game.key_type),
    ));

    trimmed |= fields
        .iter()
        .any(|(_, v)| v.chars().count() > EMBED_FIELD_VALUE_MAX);
    let fields_chars: usize = fields
        .iter()
        .map(|(n, v)| n.chars().count() + v.chars().count().min(EMBED_FIELD_VALUE_MAX))
        .sum();
    let fixed = title_len_after_trunc(&game.title) + footer_max.chars().count() + fields_chars;

    // TRAILER LINK SURVIVES TRUNCATION BY CONSTRUCTION (gate-5 blocker 2): trunc cuts the TAIL
    // and the link was the tail. Loser order stated, not accidental: ① description tail loses to
    // the link (reserve the link's chars, truncate only the body); ② if the link ITSELF cannot
    // fit, it is cut and the footer NAMES it. No unsigned subtraction near the boundary (debug
    // panic / release wrap → 6000 blown → Discord 400 → whisper not sent). Arm ② is unreachable
    // through this fn's real inputs today (worst-case fixed ≈5.5k of 6000, link ≈70) — guarded
    // anyway because the build is promised TOTAL; defensive, not fixture-tested.
    let mut description = String::new();
    let mut trailer_cut = false;
    if let Some(d) = detail {
        let link = match (&d.video_hls_url, &store_url) {
            // copy promises nothing: age-gated titles show a gate, never write "autoplays"
            (Some(_), Some(u)) => format!("\n\n[🎬 watch the trailer]({u})"),
            _ => String::new(),
        };
        let total_budget = EMBED_DESC_MAX.min(EMBED_TOTAL_TEXT_MAX.saturating_sub(fixed));
        let link_len = link.chars().count();
        if link_len <= total_budget {
            let body = trunc(&d.short_description, total_budget - link_len); // guarded: link_len <= total_budget
            trimmed |= body != d.short_description;
            description = format!("{body}{link}");
        } else {
            trailer_cut = !link.is_empty();
            trimmed |= trailer_cut;
            description = trunc(&d.short_description, total_budget);
            trimmed |= description != d.short_description;
        }
    }
    if !description.is_empty() {
        main["description"] = serde_json::json!(description);
    }

    main["fields"] = serde_json::json!(
        fields
            .iter()
            .map(|(n, v)| serde_json::json!({
                "name": trunc(n, EMBED_TITLE_MAX),
                "value": trunc(v, EMBED_FIELD_VALUE_MAX),
                "inline": true,
            }))
            .collect::<Vec<_>>()
    );

    // MEDIA COMPLETENESS (family round 1, the blocking finding): with screenshots present,
    // embed[0].image is screenshots[0] so the header consumes NO image slot — 10 shots fit in 10
    // embeds. Header art rides the thumbnail chain; with no screenshots it stays the image.
    let shots: &[steam_client::Screenshot] =
        detail.map(|d| d.screenshots.as_slice()).unwrap_or(&[]);
    let image = shots
        .first()
        .map(|s| s.full.clone())
        .or_else(|| detail.and_then(|d| d.header_image.clone()))
        .or_else(|| game.artwork_url.clone());
    if let Some(img) = &image {
        main["image"] = serde_json::json!({ "url": img });
    }
    let thumb = detail
        .and_then(|d| d.video_thumbnail.clone())
        .or_else(|| detail.and_then(|d| d.header_image.clone()))
        .or_else(|| game.artwork_url.clone());
    if let (Some(t), true) = (&thumb, thumb != image) {
        main["thumbnail"] = serde_json::json!({ "url": t });
    }

    main["footer"] =
        serde_json::json!({ "text": footer_text(cycle, slot, preview, trimmed, trailer_cut) });

    let mut embeds = vec![main];
    // galleries carry screenshots[1..] (screenshots[0] is embed[0]'s image). Grouping keyed on
    // `url` — client rendering, not API contract; degrades to a tall column, nothing lost.
    if let Some(base) = &store_url {
        for (i, shot) in shots.iter().enumerate().skip(1) {
            if embeds.len() >= MAX_EMBEDS {
                break;
            }
            let group = i / GALLERY_GROUP; // i 1-3 ride group A with the main embed; 4-7 B; 8-9 C
            let url = match group {
                0 => base.clone(),
                1 => format!("{base}#more"),
                _ => format!("{base}#more2"),
            };
            embeds.push(serde_json::json!({ "url": url, "image": { "url": shot.full } }));
        }
    }
    embeds
}

/// Title length after the trunc pass — so `fixed` and the emitted title agree.
fn title_len_after_trunc(t: &str) -> usize {
    trunc(t, EMBED_TITLE_MAX).chars().count()
}

/// 1,234-style thousands formatting (the card uses toLocaleString; Discord gets the same shape).
fn fmt_thousands(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, ch) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(ch);
    }
    out
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
        assert!(
            v["content"]
                .as_str()
                .unwrap()
                .contains("catalog?q=Papers%2C%20Please")
        );
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
        assert!(
            prev["embeds"][0]["footer"]["text"]
                .as_str()
                .unwrap()
                .starts_with("🔍 preview — newest delivered")
        );
        assert!(
            dry["embeds"][0]["footer"]["text"]
                .as_str()
                .unwrap()
                .starts_with("🔍 preview — today's dry pick")
        );
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
        assert!(
            !real["embeds"][0]["footer"]["text"]
                .as_str()
                .unwrap()
                .contains("preview")
        );
    }

    #[test]
    fn trunc_is_char_boundary_safe() {
        assert_eq!(trunc("abc", 5), "abc");
        assert_eq!(trunc("abcdef", 5), "abcd…");
        let s = "♡♡♡♡"; // 3-byte chars — a byte-index cut would panic
        assert_eq!(trunc(s, 3), "♡♡…");
    }

    // ── the full details card (spec: docs/spec-whisper-details-card.md) ──────────────────────

    /// A full cache blob with n screenshots.
    fn steam_cache(n_shots: usize, hls: bool) -> dynamo::SteamAppCache {
        dynamo::SteamAppCache {
            app_id: 570,
            detail: Some(steam_client::SteamAppDetail {
                app_id: 570,
                name: "Overgrowth".into(),
                developers: vec!["Wolfire".into()],
                publishers: vec!["Wolfire".into()], // == devs → suppressed
                genres: vec!["Action".into()],
                release_date: Some("Oct 16, 2017".into()),
                short_description: "a rabbit does kung fu.".into(),
                header_image: Some("https://cdn/header.jpg".into()),
                video_hls_url: hls.then(|| "https://cdn/movie.m3u8".into()),
                video_thumbnail: Some("https://cdn/vthumb.jpg".into()),
                screenshots: (0..n_shots)
                    .map(|i| steam_client::Screenshot {
                        thumbnail: format!("https://cdn/s{i}t.jpg"),
                        full: format!("https://cdn/s{i}.jpg"),
                    })
                    .collect(),
                tags: vec!["Ninja".into(), "Rabbits".into()],
                content_descriptor_ids: vec![2, 5],
                content_notes: Some("cartoon rabbit violence".into()),
            }),
            overall: Some(steam_client::ReviewSummary {
                desc: "Very Positive".into(),
                total_positive: 900,
                total_negative: 100,
                total_reviews: 1000,
            }),
            recent: Some(steam_client::RecentReviews {
                percent_positive: 88,
                count: 42,
            }),
            fetched_at: 0,
            reviews_fetched_at: 0,
        }
    }

    #[test]
    fn card_full_blob_renders_every_card_element() {
        let mut g = game("g1", "Overgrowth", Some("https://art/x.png"));
        g.steam_app_id = Some(570);
        let v = whisper_card(
            &g,
            Some(&steam_cache(2, true)),
            "https://s",
            3,
            "2026-W36",
            None,
        );
        let e0 = &v["embeds"][0];
        assert_eq!(e0["url"], "https://store.steampowered.com/app/570");
        assert!(
            e0["description"]
                .as_str()
                .unwrap()
                .contains("a rabbit does kung fu.")
        );
        assert!(
            e0["description"]
                .as_str()
                .unwrap()
                .contains("[🎬 watch the trailer](https://store.steampowered.com/app/570)")
        );
        let fields = e0["fields"].as_array().unwrap();
        let get = |n: &str| {
            fields.iter().find(|f| f["name"] == n).unwrap()["value"]
                .as_str()
                .unwrap()
                .to_string()
        };
        assert_eq!(get("by"), "Wolfire"); // pubs suppressed when == devs
        assert_eq!(get("released"), "Oct 16, 2017");
        assert_eq!(get("tags"), "Ninja · Rabbits"); // tags outrank genres, card rule
        assert_eq!(
            get("reviews"),
            "Very Positive — 90% of 1,000 (88% of 42 recent)"
        );
        assert_eq!(get("bundle"), "Humble Test Bundle (steam)");
        // media completeness (family round 1): with screenshots present, embed[0].image is
        // screenshots[0] — the header consumes NO image slot; it rides the thumbnail chain
        assert_eq!(e0["image"]["url"], "https://cdn/s0.jpg");
        assert_eq!(e0["thumbnail"]["url"], "https://cdn/vthumb.jpg"); // video_thumbnail ?? header ?? artwork
        assert!(e0["footer"]["text"].as_str().unwrap().contains("cycle 3"));
        assert!(e0["footer"]["text"].as_str().unwrap().contains("2026-W36"));
        assert!(!e0["footer"]["text"].as_str().unwrap().contains("trimmed")); // nothing trimmed here
    }

    #[test]
    fn card_without_screenshots_keeps_header_as_image() {
        let mut g = game("g1", "aaa", None);
        g.steam_app_id = Some(570);
        let v = whisper_card(
            &g,
            Some(&steam_cache(0, false)),
            "https://s",
            0,
            "2026-W36",
            None,
        );
        assert_eq!(v["embeds"][0]["image"]["url"], "https://cdn/header.jpg"); // nothing displaced it
    }

    #[test]
    fn card_never_leaks_admin_only_descriptors() {
        let mut g = game("g1", "aaa", None);
        g.steam_app_id = Some(570);
        let v = whisper_card(
            &g,
            Some(&steam_cache(1, false)),
            "https://s",
            0,
            "2026-W36",
            None,
        );
        let s = serde_json::to_string(&v).unwrap();
        assert!(!s.contains("cartoon rabbit violence")); // #71: admin-only, spec's own exclusion
    }

    #[test]
    fn card_ten_screenshots_all_ship_zero_silent_drops() {
        // family round 1, the blocking finding: the first layout promised header + 10 shots = 11
        // images in 10 slots and PINNED the silent drop of s9. This test is the anti-pin: ALL TEN.
        let mut g = game("g1", "aaa", None);
        g.steam_app_id = Some(570);
        let v = whisper_card(
            &g,
            Some(&steam_cache(10, false)),
            "https://s",
            0,
            "2026-W36",
            None,
        );
        let embeds = v["embeds"].as_array().unwrap();
        assert_eq!(embeds.len(), MAX_EMBEDS); // main(s0) + 3 gallery-A(s1-3) + 4 B(s4-7) + 2 C(s8-9)
        let base = "https://store.steampowered.com/app/570";
        let urls: Vec<&str> = embeds.iter().map(|e| e["url"].as_str().unwrap()).collect();
        assert_eq!(urls.iter().filter(|u| **u == base).count(), 4); // group A
        assert_eq!(
            urls.iter()
                .filter(|u| **u == format!("{base}#more"))
                .count(),
            4
        ); // group B
        assert_eq!(
            urls.iter()
                .filter(|u| **u == format!("{base}#more2"))
                .count(),
            2
        ); // group C
        let images: Vec<&str> = embeds
            .iter()
            .filter_map(|e| e["image"]["url"].as_str())
            .collect();
        for i in 0..10 {
            assert!(
                images.contains(&format!("https://cdn/s{i}.jpg").as_str()),
                "screenshot {i} missing"
            );
        }
    }

    #[test]
    fn card_two_screenshots_make_one_gallery_and_no_empty_groups() {
        let mut g = game("g1", "aaa", None);
        g.steam_app_id = Some(570);
        let v = whisper_card(
            &g,
            Some(&steam_cache(2, false)),
            "https://s",
            0,
            "2026-W36",
            None,
        );
        let embeds = v["embeds"].as_array().unwrap();
        assert_eq!(embeds.len(), 2); // main carries s0; one gallery member carries s1; no #more groups
        assert!(
            embeds
                .iter()
                .all(|e| !e["url"].as_str().unwrap().contains("#more"))
        );
    }

    #[test]
    fn card_truncation_announces_itself_in_the_footer() {
        let mut cache = steam_cache(0, false);
        if let Some(d) = cache.detail.as_mut() {
            d.short_description = "x".repeat(9000);
        }
        let mut g = game("g1", "aaa", None);
        g.steam_app_id = Some(570);
        let v = whisper_card(&g, Some(&cache), "https://s", 0, "2026-W36", None);
        assert!(
            v["embeds"][0]["footer"]["text"]
                .as_str()
                .unwrap()
                .ends_with("· trimmed to fit")
        );
    }

    #[test]
    fn card_text_budget_holds_under_hostile_description() {
        let mut cache = steam_cache(10, true);
        if let Some(d) = cache.detail.as_mut() {
            d.short_description = "x".repeat(9000);
        }
        let mut g = game("g1", &"t".repeat(300), None);
        g.steam_app_id = Some(570);
        let v = whisper_card(&g, Some(&cache), "https://s", 0, "2026-W36", None);
        let embeds = v["embeds"].as_array().unwrap();
        assert_eq!(
            embeds[0]["title"].as_str().unwrap().chars().count(),
            EMBED_TITLE_MAX
        );
        // gate-5 blocker 2: the trailer link must SURVIVE the hostile description — it is
        // reserved before truncation, never the tail that gets cut
        assert!(
            embeds[0]["description"]
                .as_str()
                .unwrap()
                .contains("[🎬 watch the trailer]")
        );
        assert!(
            embeds[0]["footer"]["text"]
                .as_str()
                .unwrap()
                .contains("trimmed to fit")
        );
        let total: usize = embeds
            .iter()
            .map(|e| {
                e["title"].as_str().unwrap_or("").chars().count()
                    + e["description"].as_str().unwrap_or("").chars().count()
                    + e["footer"]["text"].as_str().unwrap_or("").chars().count()
                    + e["fields"]
                        .as_array()
                        .map(|fs| {
                            fs.iter()
                                .map(|f| {
                                    f["name"].as_str().unwrap_or("").chars().count()
                                        + f["value"].as_str().unwrap_or("").chars().count()
                                })
                                .sum::<usize>()
                        })
                        .unwrap_or(0)
            })
            .sum();
        assert!(
            total <= EMBED_TOTAL_TEXT_MAX,
            "combined embed text {total} > {EMBED_TOTAL_TEXT_MAX}"
        );
    }

    #[test]
    fn card_degrades_per_half_when_reviews_or_detail_missing() {
        let mut g = game("g1", "aaa", Some("https://art/x.png"));
        g.steam_app_id = Some(570);
        let mut only_reviews = steam_cache(0, false);
        only_reviews.detail = None; // negative-cache stub
        let v = whisper_card(&g, Some(&only_reviews), "https://s", 0, "2026-W36", None);
        let fields = v["embeds"][0]["fields"].as_array().unwrap();
        assert!(fields.iter().any(|f| f["name"] == "reviews"));
        assert!(!fields.iter().any(|f| f["name"] == "by"));
        assert_eq!(v["embeds"][0]["image"]["url"], "https://art/x.png"); // artwork fallback
        let mut only_detail = steam_cache(0, false);
        only_detail.overall = None;
        only_detail.recent = None;
        let v2 = whisper_card(&g, Some(&only_detail), "https://s", 0, "2026-W36", None);
        assert!(
            !v2["embeds"][0]["fields"]
                .as_array()
                .unwrap()
                .iter()
                .any(|f| f["name"] == "reviews")
        );
    }

    #[test]
    fn content_holds_under_hostile_title_and_bundle() {
        // pass-1 review finding: content has its OWN Discord cap (2000) and carried the title
        // twice (display + urlencoded query) plus unbounded wire bundle. The deep-link WINS
        // (feature core); display title/bundle lose; the query is a RAW PREFIX (no … marker —
        // it is a filter token, and an encoded ellipsis would break the catalog match).
        let g = game_with_bundle("g1", &"t♡".repeat(300), &"b".repeat(900), None);
        let v = whisper_card(
            &g,
            None,
            "https://bendobundles.example",
            0,
            "2026-W36",
            Some(PreviewKind::DryRunPick),
        );
        let content = v["content"].as_str().unwrap();
        assert!(
            content.chars().count() <= CONTENT_MAX,
            "content {} > {}",
            content.chars().count(),
            CONTENT_MAX
        );
        assert!(content.contains("/admin/catalog?q=")); // the winner survived
        assert!(!content.contains("%E2%80%A6")); // no encoded … in the filter token
    }

    #[test]
    fn content_cap_is_enforced_not_asserted() {
        // gate-9 ②: CONTENT_MAX was declared and never enforced — site_url is config, and a
        // pathological value walked straight past every "structurally bounded" comment. The cap
        // is a live guard now, and the cut announces itself in the embed footer (content has no
        // footer of its own — the embeds travel with it).
        let g = game("g1", "aaa", None);
        let huge_site = format!("https://x/{}", "p".repeat(3000));
        let v = whisper_card(&g, None, &huge_site, 0, "2026-W36", None);
        assert!(v["content"].as_str().unwrap().chars().count() <= CONTENT_MAX);
        assert!(
            v["embeds"][0]["footer"]["text"]
                .as_str()
                .unwrap()
                .contains("trimmed to fit")
        );
    }

    #[test]
    fn card_thumbnail_dropped_when_it_would_duplicate_image() {
        let mut g = game("g1", "aaa", None);
        g.steam_app_id = Some(570);
        let mut cache = steam_cache(0, false);
        if let Some(d) = cache.detail.as_mut() {
            d.video_thumbnail = None;
            d.header_image = None;
        }
        // image falls back to artwork; thumbnail would fall back to the same artwork → dropped
        let mut g2 = g.clone();
        g2.artwork_url = Some("https://art/same.png".into());
        let v = whisper_card(&g2, Some(&cache), "https://s", 0, "2026-W36", None);
        assert_eq!(v["embeds"][0]["image"]["url"], "https://art/same.png");
        assert!(v["embeds"][0].get("thumbnail").is_none());
    }
}
