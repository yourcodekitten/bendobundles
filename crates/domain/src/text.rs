//! Text safety for anything that reaches a Discord `content` field or a stored note/name.
//! ONE copy: public-api's `sanitize_note` and admin-api's `sanitize_friend_name` were
//! byte-identical private fns, and the lantern would have been the third. Order at a render
//! site is ALWAYS `sanitize_line` → cap → `escape_md`, exactly once — see
//! `fulfillment::bell::field`, the one place a foreign string enters `content`.

use crate::is_spoofing_format_char;

/// Discord Markdown metacharacters (the set the family settled on 2026-09-16; a code span was
/// rejected because a backtick in the field breaks out of it). Membership is the contract.
pub const MD_META: &[char] = &['\\', '*', '_', '~', '`', '|', '>', '[', ']', '(', ')'];

/// Line/segment separators (newline, CR, tab, VT, FF, NEL, U+2028/U+2029) become one space so a
/// multiline paste keeps its word boundaries; every other control char and every spoofing
/// format char (`crate::is_spoofing_format_char` — the ONE range list) is stripped. Runs BEFORE
/// any emptiness/length check so stripped chars cannot smuggle visible length past a budget.
/// This is the former `sanitize_note` / `sanitize_friend_name`, verbatim.
pub fn sanitize_line(raw: &str) -> String {
    raw.chars()
        .filter_map(|c| match c {
            '\n' | '\r' | '\t' | '\u{000B}' | '\u{000C}' | '\u{0085}' | '\u{2028}' | '\u{2029}' => {
                Some(' ')
            }
            c if c.is_control() || is_spoofing_format_char(c) => None,
            c => Some(c),
        })
        .collect()
}

/// Backslash-escape every Discord Markdown metacharacter so third-party text (a Steam title, a
/// friend's note) renders as the characters typed — never as a masked link, bold, or a heading.
/// NOT idempotent: escaping twice yields literal backslashes in the channel. Call it exactly
/// once, LAST, after sanitise and cap (a cap applied after escaping can cut between `\` and
/// `*` and leave a dangling backslash that escapes the template's own `**`).
pub fn escape_md(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 8);
    for c in s.chars() {
        if MD_META.contains(&c) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_folds_line_breaks_and_strips_controls() {
        assert_eq!(sanitize_line("a\nb\r\nc\td"), "a b  c d");
        assert_eq!(sanitize_line("x\u{2028}y\u{0085}z"), "x y z");
        assert_eq!(sanitize_line("a\u{0007}b\u{200E}c"), "abc"); // BEL + LRM stripped
        assert_eq!(sanitize_line("plain ♡"), "plain ♡");
        // variation selectors are NOT spoofing chars — ❤️ keeps its FE0F (the plan's first
        // draft re-transcribed the range list narrower; the domain fn is the truth)
        assert_eq!(sanitize_line("\u{2764}\u{FE0F}"), "\u{2764}\u{FE0F}");
    }

    #[test]
    fn escape_md_neutralises_every_metacharacter_exactly_once() {
        let hostile = r"[open your gift](https://evil) `tick` *b* _i_ ~s~ |sp| > q \ back";
        let once = escape_md(hostile);
        assert_eq!(
            once,
            r"\[open your gift\]\(https://evil\) \`tick\` \*b\* \_i\_ \~s\~ \|sp\| \> q \\ back"
        );
        // NOT idempotent by design: escaping the escaped string doubles the backslashes.
        // Call sites escape exactly once (pinned at the render tests).
        assert_ne!(escape_md(&once), once);
        for c in MD_META {
            assert!(once.contains(&format!("\\{c}")), "{c} must be escaped");
        }
    }

    #[test]
    fn escape_md_leaves_plain_text_untouched_and_is_char_boundary_safe() {
        assert_eq!(escape_md("celeste ♡ 2018"), "celeste ♡ 2018");
        assert_eq!(escape_md("日本語*"), "日本語\\*");
        assert_eq!(
            escape_md("label-in"),
            "label-in",
            "`-` is not a metacharacter"
        );
    }
}
