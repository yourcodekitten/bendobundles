//! The attic bell 🔔 — spec: docs/spec-attic-bell.md.
//! Pure card builders + best-effort ring. Shares the whisper TRANSPORT (webhook + POST helper),
//! never its SLOT state: nothing in this module may name WHISPER#, record_whisper, or a slot.

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
    Thanks { link_token: String },
}

/// Discord hard cap on `content`; same bound whisper's CONTENT_MAX respects.
const BELL_CONTENT_MAX: usize = 2000;
const BELL_LABEL_MAX: usize = 120;
const BELL_TITLE_MAX: usize = 240;

fn cap(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        s.chars().take(n).collect()
    }
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
        label = cap(label, BELL_LABEL_MAX),
        title = cap(game_title, BELL_TITLE_MAX),
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
        "content": cap(&content, BELL_CONTENT_MAX),
        "embeds": embeds,
        "allowed_mentions": { "parse": [] },
    })
}

pub fn thanks_card(label: &str, note: &str, site_url: &str) -> serde_json::Value {
    // `note` is the STORED value: control/bidi-sanitized at write, ≤500 chars by
    // THANK_NOTE_MAX_CHARS. Mentions are denied structurally, never by scrubbing — all
    // friend-influenced text rides `content`, where allowed_mentions is DOCUMENTED to apply
    // (embed behaviour is observed-not-contract, so this card carries zero embeds and the
    // question does not arise).
    let content = format!(
        "💌 *a note came back…*\n**{label}** says: “{note}”\n{site}/admin/links",
        label = cap(label, BELL_LABEL_MAX),
        note = note,
        site = site_url,
    );
    serde_json::json!({
        "content": cap(&content, BELL_CONTENT_MAX),
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
        assert!(v["content"]
            .as_str()
            .unwrap()
            .contains("a monthly pick, spent with love"));
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
}
