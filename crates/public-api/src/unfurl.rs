//! The wrapping paper: per-token og meta swapped into the deployed index.html.
//! Spec: docs/spec-wrapping-paper.md (r2). The card is the wrapped box, never
//! the contents; personalized ONLY for active|sealed; audience = the room.
use domain::og_text;

pub(crate) const OG_BEGIN: &str = "<!-- og:begin -->";
pub(crate) const OG_END: &str = "<!-- og:end -->";

#[derive(Debug)]
pub(crate) struct MarkerAbsent;

pub(crate) fn swap_og_block(template: &str, meta_html: &str) -> Result<String, MarkerAbsent> {
    let start = template.find(OG_BEGIN).ok_or(MarkerAbsent)?;
    let end_rel = template[start..].find(OG_END).ok_or(MarkerAbsent)?;
    let end = start + end_rel + OG_END.len();
    let mut out = String::with_capacity(template.len() + meta_html.len());
    out.push_str(&template[..start]);
    out.push_str(OG_BEGIN);
    out.push('\n');
    out.push_str(meta_html);
    out.push('\n');
    out.push_str(OG_END);
    out.push_str(&template[end..]);
    Ok(out)
}

const WRAPS: [&str; 8] = ["clay", "rust", "mustard", "moss", "pine", "slate", "heather", "mauve"];

pub(crate) fn wrap_variant(token: &str) -> &'static str {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in token.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    WRAPS[(h % 8) as usize]
}

fn count_words(n: usize) -> String {
    const W: [&str; 12] = ["one","two","three","four","five","six","seven","eight","nine","ten","eleven","twelve"];
    if (1..=12).contains(&n) { W[n - 1].to_string() } else { "a dozen and more".to_string() }
}

fn meta_block(title: &str, desc: &str, image: &str, alt: &str) -> String {
    // INVARIANT (tested, not asserted): every interpolated value has passed
    // og_text — including image URLs (escaping a URL is harmless and correct in
    // attribute context; &→&amp;) — so a future base_url-from-Host refactor
    // cannot bypass the escaper silently (Lilith's MAJOR-1).
    format!(
        "<meta property=\"og:type\" content=\"website\" />\n\
         <meta property=\"og:site_name\" content=\"bendobundles\" />\n\
         <meta property=\"og:title\" content=\"{title}\" />\n\
         <meta property=\"og:description\" content=\"{desc}\" />\n\
         <meta property=\"og:image\" content=\"{image}\" />\n\
         <meta property=\"og:image:width\" content=\"1200\" />\n\
         <meta property=\"og:image:height\" content=\"630\" />\n\
         <meta property=\"og:image:alt\" content=\"{alt}\" />\n\
         <meta name=\"twitter:card\" content=\"summary_large_image\" />\n\
         <meta name=\"twitter:title\" content=\"{title}\" />\n\
         <meta name=\"twitter:description\" content=\"{desc}\" />\n\
         <meta name=\"twitter:image\" content=\"{image}\" />"
    )
}

pub(crate) fn meta_for_link(
    link: &domain::Link,
    now: time::OffsetDateTime,
    base_url: &str,
) -> Option<String> {
    let sealed = match link.can_claim(now) {
        Ok(()) => false,
        Err(domain::ClaimRefusal::Sealed) => true,
        Err(_) => return None, // revoked|expired|exhausted → generic (spec D5)
    };
    let label = og_text(&link.label, 80);
    let title = format!("🎁 ben wrapped something for {label} ♡");
    let desc = if sealed {
        "sealed for now. good things wait.".to_string()
    } else {
        match &link.curated_game_ids {
            Some(ids) if !ids.is_empty() => {
                let n = ids.len();
                let s = if n == 1 { "" } else { "s" };
                format!("{} treasure{s} inside, chosen for you. tap to unwrap.", count_words(n))
            }
            _ => "treasures inside, chosen for you. tap to unwrap.".to_string(),
        }
    };
    let image = og_text(&format!("{base_url}/art/wrap-{}.png", wrap_variant(&link.token)), 200);
    Some(meta_block(&title, &og_text(&desc, 200), &image,
        "a pixel-art wrapped present in ben's pea-green attic"))
}

pub(crate) fn meta_for_shelf(friend: &domain::Friend, base_url: &str) -> String {
    let name = og_text(&friend.name, 80);
    meta_block(
        &format!("📚 the shelf ben keeps for {name}"),
        "every game he's given you, all in one warm place.",
        &og_text(&format!("{base_url}/art/wrap-shelf.png"), 200),
        "a pixel-art shelf of games in ben's attic",
    )
}

#[cfg(test)]
mod tests {
    use time::ext::NumericalDuration;

    fn test_link() -> domain::Link {
        domain::Link {
            token: "abc123".into(),
            label: "dave".into(),
            gift_note: None,
            thank_note: None,
            thanked_at: None,
            claims_allowed: 1,
            claims_used: 0,
            revoked: false,
            expires_at: None,
            unlock_at: None,
            curated_game_ids: None,
            friend_id: None,
            created_at: time::macros::datetime!(2026-07-02 00:00 UTC),
        }
    }

    #[test]
    fn wrap_variant_is_pinned_forever() {
        // GOLDEN: literal pairs — the paper is a promise (spec D1). A determinism-only
        // or spread-only assertion lets a hash-constant refactor slip through green;
        // only independently-computed literals pin the promise.
        // Pins computed INDEPENDENTLY of the implementation (two seats agreeing
        // digit-for-digit) — never print-then-pin, which goldens the first bug.
        // And never REPEATED-UNIT tokens: FNV-1a's step is a bijection on Z/8 and
        // the offset basis &7 = 5, so a unit repeated >=4 times lands on WRAPS[5]
        // "slate" every time (measured; reps=2 is mixed ~60/40) — three such pins
        // are one assertion in a trenchcoat. The exact law (2000/2000 trials): a
        // repeated unit lands on slate iff the ORDER of its per-period permutation
        // divides the repeat count — observed orders {1,2,4}, so any reps
        // divisible by 4 returns to the offset basis; ">=4" coincides only because
        // 64 is a power of two. Rule: COMPUTE the bucket for a candidate pin,
        // never classify the token (Lilith's blocker + law, OMBB's quantifier,
        // both verified on this seat).
        assert_eq!(super::wrap_variant("8af17e0500caf83ec1172baf05a661ba1ee4ab02114f643b4ab5d2efe9ed80ec"), "clay");
        assert_eq!(super::wrap_variant("b8b7c23ab0e0c45567869d4c98c9d489c8000a21da777c54e4cdeba61a513957"), "rust");
        assert_eq!(super::wrap_variant("a0a8bddef089f638f98ca3f13aead6a96aaf25955eaeaa1a614c9a38427e6092"), "mustard");
        let all: std::collections::HashSet<_> =
            (0..64).map(|i| super::wrap_variant(&format!("{i:064x}"))).collect();
        assert!(all.len() >= 6, "64 tokens should hit most of 8 buckets: got {}", all.len());
    }

    #[test]
    fn swap_og_block_replaces_between_markers() {
        let t = "head\n<!-- og:begin -->\nOLD\n<!-- og:end -->\ntail";
        let out = super::swap_og_block(t, "NEW").unwrap();
        assert!(out.contains("NEW") && !out.contains("OLD"));
        assert!(out.starts_with("head\n") && out.ends_with("\ntail"));
    }

    #[test]
    fn swap_og_block_absent_marker_is_typed() {
        assert!(super::swap_og_block("no markers here", "X").is_err());
    }

    #[test]
    fn meta_for_link_active_curated_counts_in_words() {
        let mut link = test_link(); // build with the same helper style store tests use
        link.curated_game_ids = Some(vec!["a".into(), "b".into(), "c".into()]);
        let m = super::meta_for_link(&link, time::OffsetDateTime::now_utc(), "https://x.example").unwrap();
        assert!(m.contains("three treasures inside, chosen for you. tap to unwrap."));
        assert!(m.contains("ben wrapped something for"));
        assert!(m.contains("/art/wrap-")); // one of the 8
    }

    #[test]
    fn meta_for_link_sealed_has_state_but_never_a_clock() {
        let mut link = test_link();
        link.unlock_at = Some(time::OffsetDateTime::now_utc() + 2.days());
        let m = super::meta_for_link(&link, time::OffsetDateTime::now_utc(), "https://x.example").unwrap();
        assert!(m.contains("sealed for now. good things wait."));
        let year = time::OffsetDateTime::now_utc().year().to_string();
        assert!(!m.contains(&year), "no date-ish content in a broadcast card");
    }

    #[test]
    fn meta_for_link_dead_states_are_none() {
        let mut link = test_link();
        link.revoked = true;
        assert!(super::meta_for_link(&link, time::OffsetDateTime::now_utc(), "https://x.example").is_none());
    }

    #[test]
    fn hostile_base_url_cannot_change_structure_either() {
        // base_url is server config today (router()'s 4th param) — this arm exists
        // for the future refactor that derives it from a request.
        let link = test_link();
        let m = super::meta_for_link(&link, time::OffsetDateTime::now_utc(), r#"https://x"><script>"#).unwrap();
        for line in m.lines().filter(|l| !l.trim().is_empty()) {
            assert_eq!(line.trim().matches('<').count(), 1, "injected < in: {line}");
        }
    }

    #[test]
    fn hostile_label_cannot_change_structure_at_the_meta_layer() {
        let mut link = test_link();
        link.label = r#"" onload=x><script>"#.into();
        let m = super::meta_for_link(&link, time::OffsetDateTime::now_utc(), "https://x.example").unwrap();
        // No raw < > outside tag boundaries: every line parses as a <meta …/> element.
        for line in m.lines().filter(|l| !l.trim().is_empty()) {
            let l = line.trim();
            assert!(l.starts_with("<meta ") && l.ends_with("/>"), "unexpected line: {l}");
            assert_eq!(l.matches('<').count(), 1, "injected < in: {l}");
        }
    }
}
