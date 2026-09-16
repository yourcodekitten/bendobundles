//! The attic bell 🔔 — spec: docs/spec-attic-bell.md.
//! Pure card builders + best-effort ring. Shares the whisper TRANSPORT (webhook + POST helper),
//! never its SLOT state: nothing in this module may name WHISPER#, record_whisper, or a slot.

use crate::Deps;
use crate::operator_message::{OperatorMessage, Part};
use domain::text::{escape_md, sanitize_line};

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum BellEvent {
    /// `week`: the ledger week this unwrap's RING must be counted in — computed by the SENDER
    /// beside the gift response (`current_week()`), so the unwrap/ring pair cannot straddle a
    /// week boundary across the async invoke (at a handful of claims a week, a ±1 straddle gap
    /// is indistinguishable from a real miss).
    Unwrap {
        link_token: String,
        game_id: String,
        week: String,
        #[serde(default)]
        choice: bool,
    },
    Thanks {
        link_token: String,
    },
}

/// Discord hard cap on `content`; same bound whisper's CONTENT_MAX respects.
const BELL_CONTENT_MAX: usize = 2000;
const BELL_LABEL_MAX: usize = 120;
const BELL_TITLE_MAX: usize = 240;
/// Mirrors public-api's private `THANK_NOTE_MAX_CHARS` (the stored note's write-time budget).
const BELL_NOTE_MAX: usize = 500;

fn cap(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        s.chars().take(n).collect()
    }
}

/// The ONE way a foreign string enters `content`: sanitise (fold line breaks, strip controls —
/// a Steam title with `\n# ` must not become a heading), cap by CHARS, THEN escape exactly once
/// (escaping first lets the cap cut between `\` and `*`). Shared with the lantern via
/// `crate::bell::field`; do not re-implement.
pub(crate) fn field(raw: &str, max: usize) -> String {
    escape_md(&cap(&sanitize_line(raw), max))
}

/// Final-content cap that never strands a trailing backslash (a `\` at the cut would escape the
/// character after it in the template). Applied to the FINISHED string, after every field's own
/// escape — so the 2000 bound is on what Discord receives.
pub(crate) fn cap_content(s: &str) -> String {
    let mut out = cap(s, BELL_CONTENT_MAX);
    while out.ends_with('\\') {
        out.pop();
    }
    out
}

pub fn unwrap_card(
    label: &str,
    game_title: &str,
    artwork_url: Option<&str>,
    site_url: &str,
    choice: bool,
) -> serde_json::Value {
    let spent = if choice {
        " (a monthly pick, spent with love)"
    } else {
        ""
    };
    let content = format!(
        "🔔 *the attic rings…*\n**{label}** just unwrapped **{title}**{spent} ♡\n{site}/admin/links",
        label = field(label, BELL_LABEL_MAX),
        title = field(game_title, BELL_TITLE_MAX),
        site = site_url,
    );
    // artless ⇒ NO embed at all: an embed object with no renderable field is a Discord 400,
    // and the bell must not die precisely for games without artwork. With art, the embed
    // carries the (catalog-owned) title so it is never thumbnail-only.
    let embeds = match artwork_url {
        Some(url) => serde_json::json!([{
            "title": cap(game_title, BELL_TITLE_MAX),
            "thumbnail": { "url": url },
        }]),
        None => serde_json::json!([]),
    };
    serde_json::json!({
        "content": cap_content(&content),
        "embeds": embeds,
        "allowed_mentions": { "parse": [] },
    })
}

/// The bell ledger's week key — ONE implementation (the whisper's slot derivation is a different
/// meaning: tick identity, schedule-coupled; this is just "which week does this count in").
pub fn current_week() -> String {
    let (y, w, _) = time::OffsetDateTime::now_utc().date().to_iso_week_date();
    format!("{y}-W{w:02}")
}

/// Ring the bell for one event. Best-effort BY CONTRACT: every failure is a log line and a clean
/// return — an Event-invoked lambda retries on function error, and a double ring is worse than a
/// missed one. The gift may never miss; the bell may.
pub async fn ring(deps: &Deps, event: &BellEvent) {
    if deps.bell_disabled {
        // the bell's OWN off-switch (shared secret, split disable flag): muting bells must not
        // dark the weekly whisper, and vice versa. Loud, so a muted bell never reads as broken.
        tracing::info!(
            outcome = "bell_disabled",
            "bell: BELL_DISABLED set — not ringing, by choice"
        );
        return;
    }
    let Some(url) = crate::resolve_whisper_url(deps).await else {
        // dark deploy: same loud no-op face as the whisper — the resolve fn already logged it.
        return;
    };
    let body = match event {
        BellEvent::Unwrap {
            link_token,
            game_id,
            choice,
            ..
        } => {
            let label = match deps.store.get_link(link_token).await {
                Ok(Some(l)) => l.label,
                Ok(None) => {
                    tracing::warn!(link_token, "bell: unwrap for unknown link — not ringing");
                    return;
                }
                Err(e) => {
                    tracing::warn!(error = ?e, "bell: link read failed — not ringing");
                    return;
                }
            };
            let (title, art) = match deps.store.get_game(game_id).await {
                Ok(Some(g)) => (g.title, g.artwork_url),
                Ok(None) => {
                    tracing::warn!(game_id, "bell: unwrap for unknown game — not ringing");
                    return;
                }
                Err(e) => {
                    tracing::warn!(error = ?e, "bell: game read failed — not ringing");
                    return;
                }
            };
            unwrap_card(
                &label,
                &title,
                art.as_deref(),
                &deps.whisper_site_url,
                *choice,
            )
        }
        BellEvent::Thanks { link_token } => {
            let link = match deps.store.get_link(link_token).await {
                Ok(Some(l)) => l,
                Ok(None) => {
                    tracing::warn!(link_token, "bell: thanks for unknown link — not ringing");
                    return;
                }
                Err(e) => {
                    tracing::warn!(error = ?e, "bell: link read failed — not ringing");
                    return;
                }
            };
            // the STORED note, never a payload-carried copy (spec: content & security).
            let Some(note) = link.thank_note.as_deref() else {
                tracing::warn!(
                    link_token,
                    "bell: thanks event but no stored note — not ringing"
                );
                return;
            };
            thanks_card(&link.label, note, &deps.whisper_site_url)
        }
    };
    if crate::whisper_send_body(&deps.http, &url, &body).await {
        // ledger of rings, best-effort like everything here: the count exists so the weekly
        // whisper can contradict a silent bell; a failed increment is a WARN, never a failed
        // ring. UNWRAP RINGS ONLY — `rings` must be a true pair with `unwraps` (same population,
        // same week: the event CARRIES its ledger week, computed beside the gift response, so
        // the pair cannot straddle a weekly boundary across the async hop). Thanks bells are
        // deliberately uncounted: adding them makes rings ≥ unwraps normal and the suspect
        // direction unreadable.
        if let BellEvent::Unwrap { week, .. } = event
            && let Err(e) = deps
                .store
                .increment_bell_counter(week, dynamo::BellCounter::Rings)
                .await
        {
            tracing::warn!(error = ?e, week, "bell rang but the ring ledger write failed");
        }
    } else {
        // A WARN nobody reads is at-never-once: the miss goes to the MONITORED ops channel via
        // the same pattern as whisper cause-④ — and the ops webhook is a DIFFERENT credential,
        // so this report survives a dead whisper webhook. Frequency is bounded by construction
        // (claims ≤ claims_allowed; thanks write-once), so this cannot storm.
        tracing::error!(
            outcome = "bell_send_failed",
            "bell POST failed — accepted loss, no retry"
        );
        crate::ping_msg(
            deps,
            &OperatorMessage::fmt(
                "the attic bell failed to ring ({}): webhook POST failed — the moment passed unheard; no retry by design",
                &[Part::Id(match event {
                    BellEvent::Unwrap { .. } => "unwrap",
                    BellEvent::Thanks { .. } => "thanks",
                })],
            ),
        )
        .await;
    }
}

pub fn thanks_card(label: &str, note: &str, site_url: &str) -> serde_json::Value {
    // `note` is the STORED value: control/bidi-sanitized at write, ≤500 chars by
    // THANK_NOTE_MAX_CHARS. Mentions are denied structurally, never by scrubbing — all
    // friend-influenced text rides `content`, where allowed_mentions is DOCUMENTED to apply
    // (embed behaviour is observed-not-contract, so this card carries zero embeds and the
    // question does not arise). Markdown, however, is NOT denied by `allowed_mentions`: Discord
    // renders masked links in webhook content, so a note of `[open your gift](https://…)` became
    // a clickable link with made-up text (Lilith, lantern review 2026-09-16). `field` escapes it.
    let content = format!(
        "💌 *a note came back…*\n**{label}** says: “{note}”\n{site}/admin/links",
        label = field(label, BELL_LABEL_MAX),
        note = field(note, BELL_NOTE_MAX),
        site = site_url,
    );
    serde_json::json!({
        "content": cap_content(&content),
        "embeds": [],
        "allowed_mentions": { "parse": [] },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unwrap_card_carries_voice_art_and_deny() {
        let v = unwrap_card(
            "sam ♡",
            "Celeste",
            Some("https://art/1.png"),
            "https://bendobundles.com",
            false,
        );
        let content = v["content"].as_str().unwrap();
        assert!(content.contains("🔔"));
        assert!(content.contains("sam ♡"));
        assert!(content.contains("Celeste"));
        assert!(content.contains("https://bendobundles.com/admin/links"));
        assert!(!content.contains("monthly pick"));
        assert_eq!(v["allowed_mentions"]["parse"].as_array().unwrap().len(), 0);
        assert_eq!(v["embeds"][0]["thumbnail"]["url"], "https://art/1.png");
        assert_eq!(v["embeds"][0]["title"], "Celeste"); // never thumbnail-only
    }

    #[test]
    fn unwrap_card_choice_says_so_and_artless_sends_zero_embeds() {
        let v = unwrap_card("sam", "Celeste", None, "https://s", true);
        assert!(
            v["content"]
                .as_str()
                .unwrap()
                .contains("a monthly pick, spent with love")
        );
        // no art ⇒ NO embed: an empty embed object is a Discord 400, not a blank space
        assert!(v["embeds"].as_array().unwrap().is_empty());
    }

    #[test]
    fn thanks_card_quotes_the_note_and_denies_mentions() {
        let v = thanks_card("sam", "omg i loved this @everyone", "https://s");
        let content = v["content"].as_str().unwrap();
        assert!(content.contains("💌"));
        assert!(content.contains("omg i loved this @everyone")); // deny is structural, not scrubbing
        assert_eq!(v["allowed_mentions"]["parse"].as_array().unwrap().len(), 0);
    }

    #[test]
    fn cards_cap_pathological_inputs() {
        // config/store-fed strings must produce short messages, never a Discord 400 —
        // whisper's gate-9 ② rule, inherited.
        let long = "x".repeat(6000);
        for v in [
            unwrap_card(&long, &long, None, &long, false),
            thanks_card(&long, &long, &long),
        ] {
            assert!(v["content"].as_str().unwrap().chars().count() <= 2000);
        }
    }

    #[test]
    fn thanks_card_escapes_markdown_exactly_once() {
        let v = thanks_card(
            "sam",
            "[open your gift](https://evil) and a `tick",
            "https://s",
        );
        let c = v["content"].as_str().unwrap();
        assert!(
            c.contains(r"\[open your gift\]\(https://evil\) and a \`tick"),
            "{c}"
        );
        assert!(!c.contains(r"\\["), "escaped twice: {c}");
        assert!(
            c.contains("**sam** says"),
            "template bold must survive: {c}"
        );
    }

    #[test]
    fn field_caps_before_escaping_so_no_dangling_backslash() {
        // cap of 3 lands right before the `*`: "ab*" → "ab\*" (3 chars kept, THEN escaped)
        assert_eq!(field("ab*cd", 3), "ab\\*");
        // a max-length note of nothing but metacharacters doubles in length after escaping
        let note = "*".repeat(500);
        assert_eq!(field(&note, 500).chars().count(), 1000);
    }

    #[test]
    fn cap_content_cuts_at_2000_and_never_strands_a_backslash() {
        // thanks_card cannot reach 2000 (≈240+1000+50), so test the cap DIRECTLY where the cut
        // lands on an escape: 1999 'y' + "\x" is 2001 chars → cap keeps 1999 'y' + '\' → popped.
        let s = format!("{}\\x", "y".repeat(1999));
        assert_eq!(cap_content(&s), "y".repeat(1999));
        assert_eq!(cap_content("short").as_str(), "short");
        // a card path that DOES reach the cap: a 6000-char site_url on unwrap_card
        let v = unwrap_card("sam", "Celeste", None, &"u".repeat(6000), false);
        let c = v["content"].as_str().unwrap();
        assert!(c.chars().count() <= 2000 && !c.ends_with('\\'));
    }

    #[test]
    fn unwrap_card_title_with_newline_heading_is_flattened() {
        let v = unwrap_card("sam", "Bad\n# Title", None, "https://s", false);
        let c = v["content"].as_str().unwrap();
        assert!(!c.contains("\n# "), "{c}");
        assert!(c.contains("Bad # Title"), "{c}");
    }
}
