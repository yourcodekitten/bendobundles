# The Lantern 🏮 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A weekly Sunday-evening Discord message on the whisper register that reports stalled intentions (cold doors, stuck claims, wrapped-past-day gifts, closing doors), with stateless slot-bucketed mentions, a heartbeat that can never send a second lantern, and Markdown-safe interpolation shared with the bell.

**Architecture:** A new pure module `fulfillment::lantern` (bucket function + compose + render) driven by a handler that copies the whisper's record→send→mark skeleton onto a `LANTERN#<sunday-date>` slot row. A Wednesday `lantern_heartbeat` op keeps the never-ran alarm's metric alive and retries an undelivered/un-run Sunday, never a delivered or quiet one. Text safety (`sanitize_line` + `escape_md`) moves to `domain::text` so the bell, the lantern, public-api and admin-api share one copy.

**Tech Stack:** Rust 2024 (workspace), `time 0.3`, `serde_json`, `aws-sdk-dynamodb`, wiremock + dynamodb-local for handler tests, Terraform (EventBridge Scheduler, CloudWatch, SSM), the existing `bendoerr-terraform-modules/*`.

**Spec:** `docs/spec-lantern.md` (v4 at `1a782d7`; decisions section wins over narrative).

## Global Constraints

- **Rust edition 2024**, workspace `time = 0.3` with `serde, formatting, parsing, macros`. No new crates.
- **Never touch `WHISPER#` state, `record_whisper`, or the whisper's slot derivation.** The lantern has its own prefix `LANTERN#`.
- **Register:** the WHISPER webhook (`deps.whisper_notify`). Off-switch: `LANTERN_DISABLED=1`, read ONLY by the lantern. Never `NOTIFY_DISABLED`, never `WHISPER_DISABLED`.
- **Dark-deploy rule:** register unresolved/disabled ⇒ loud no-op, ZERO writes.
- **Slot key = the Sunday DATE (`YYYY-MM-DD`), boundary = Sunday 21:00Z**, buckets `[Sun 21:00Z − 7d, Sun 21:00Z)`. Doors/wrapped mention on BUCKET(k); closing on BUCKET(k+1).
- **Quiet writes exactly one row (`quiet = true`) and zero sends (F1).**
- **Heartbeat: row absent ⇒ full run · delivered OR quiet ⇒ metric only · undelivered ⇒ resend + mark (at-least-once: a landed POST whose MARK fails resends on Wednesday — a duplicate beats a lost bucket).**
- **Text safety order per field: `sanitize_line` → cap → `escape_md`, exactly once; the 2000-char Discord cap applies to the FINAL escaped content.**
- **No bearer capability in any message.** Deep links are `{site}/admin/links` and `{site}/admin/ops` only.
- **Empty lantern is HEALTHY** — it must not page ops (the whisper's empty-pool ping is NOT inherited).
- **Commits:** GPG-signed, authored `code kitten <yourcodekitten@gmail.com>`; message style: lowercase, no conventional-commit prefix (repo convention: see `git log`).
- **Tests:** unit tests run with `cargo test -p <crate> --lib`; handler tests in `crates/fulfillment/tests/handler_test.rs` need dynamodb-local (`store_or_skip` skips locally, CI runs them for real). **CI is the authoritative run** — if the local link OOMs, push and read CI, never "assume green".

---

## File structure

| File | Responsibility |
|---|---|
| `crates/domain/src/lib.rs` | `Link::is_open_door(now)`, `Link::waits(now)` (relocated from admin-api scrapbook); `LanternRecord` |
| `crates/domain/src/text.rs` (new) | `sanitize_line`, `is_spoofing_format_char`, `escape_md` — ONE copy |
| `crates/public-api/src/lib.rs`, `crates/admin-api/src/lib.rs` | adopt `domain::text::sanitize_line` (delete private copies) |
| `crates/admin-api/src/scrapbook.rs` | adopt `Link::is_open_door` / `Link::waits` |
| `crates/fulfillment/src/bell.rs` | cards use sanitize→cap→escape via `domain::text` |
| `crates/dynamo/src/lib.rs` | `record_lantern`, `mark_lantern_delivered`, `get_lantern`, `list_lanterns` |
| `crates/fulfillment/src/lantern.rs` (new) | `Slot`, `tick_slot`, `compose`, `render`, eastern-date formatting — pure |
| `crates/fulfillment/src/lib.rs` | `FulfillRequest::{Lantern, LanternHeartbeat, LanternPreview}`, `FulfillResponse::Lanterned`, `Deps.lantern_disabled`, handlers |
| `crates/fulfillment/src/main.rs` | `lantern_suppressed`, env wiring |
| `crates/fulfillment/tests/handler_test.rs` | lantern handler arms |
| `terraform/aws-eventbridge.tf`, `aws-cloudwatch-alarms.tf`, `aws-lambda.tf`, `tf-variables.tf`, `production.tfvars` | schedules, alarms, env, flag |

---

### Task 1: relocate the door predicates onto `Link`

**Files:**
- Modify: `crates/domain/src/lib.rs` (after `impl Link { … can_claim_if_unsealed … }`, ~line 411)
- Modify: `crates/admin-api/src/scrapbook.rs:101-106` (delete the two free fns, call the methods)
- Test: `crates/domain/src/lib.rs` tests module; existing `crates/admin-api/src/scrapbook.rs` tests pin behaviour across the move

**Interfaces:**
- Produces: `impl Link { pub fn is_open_door(&self, now: OffsetDateTime) -> bool; pub fn waits(&self, now: OffsetDateTime) -> bool }`

- [ ] **Step 1: write the failing domain tests**

Append inside `crates/domain/src/lib.rs`'s existing `#[cfg(test)] mod tests` (find `fn can_claim_sealed_before_unlock`, add beside it):

```rust
    #[test]
    fn is_open_door_is_can_claim_and_waits_ignores_the_seal() {
        let now = OffsetDateTime::UNIX_EPOCH + time::Duration::days(20_000);
        let mut l = Link {
            token: "t".into(), label: "sam".into(), gift_note: None, thank_note: None,
            thanked_at: None, claims_allowed: 1, claims_used: 0, revoked: false,
            expires_at: None, unlock_at: Some(now + time::Duration::days(1)),
            curated_game_ids: None, curated_notes: None, friend_id: None, created_at: now,
        };
        assert!(!l.is_open_door(now), "sealed is not an open door");
        assert!(l.waits(now), "sealed still waits");
        l.unlock_at = None;
        assert!(l.is_open_door(now));
        l.revoked = true;
        assert!(!l.is_open_door(now));
        assert!(!l.waits(now), "revoked waits for nobody");
    }
```

(If the `Link` struct has fields not listed here, copy the initializer from the nearest existing test fixture in the same module — the assertion set is what matters.)

- [ ] **Step 2: run to verify it fails**

Run: `cargo test -p domain --lib is_open_door_is_can_claim_and_waits_ignores_the_seal`
Expected: FAIL — `no method named is_open_door`

- [ ] **Step 3: implement on `Link`**

In `crates/domain/src/lib.rs`, inside `impl Link` after `can_claim_if_unsealed`:

```rust
    /// A live door a friend could walk through RIGHT NOW: `can_claim` succeeds. Relocated from
    /// admin-api's scrapbook (its `link_is_open_door`) when the lantern became the second caller —
    /// two copies of a predicate is where review attention goes to die.
    pub fn is_open_door(&self, now: OffsetDateTime) -> bool {
        self.can_claim(now).is_ok()
    }

    /// "Would be claimable if it weren't wrapped": `can_claim_if_unsealed` succeeds. A sealed
    /// exhausted link is NOT waiting (the tolerance lives here, at the refusal's definition site,
    /// not in a consumer hand-tolerating `Err(Sealed)`). Relocated from the scrapbook's
    /// `link_waits`.
    pub fn waits(&self, now: OffsetDateTime) -> bool {
        self.can_claim_if_unsealed(now).is_ok()
    }
```

- [ ] **Step 4: point the scrapbook at them**

In `crates/admin-api/src/scrapbook.rs` delete:

```rust
fn link_waits(l: &Link, now: OffsetDateTime) -> bool {
    l.can_claim_if_unsealed(now).is_ok()
}
fn link_is_open_door(l: &Link, now: OffsetDateTime) -> bool {
    l.can_claim(now).is_ok()
}
```

and replace every call `link_waits(l, now)` → `l.waits(now)`, `link_is_open_door(l, now)` → `l.is_open_door(now)` (grep the file: `grep -n 'link_waits\|link_is_open_door' crates/admin-api/src/scrapbook.rs`). Keep the doc comment that preceded `link_waits` — move it onto `Link::waits` if it is not already covered by the text in step 3.

- [ ] **Step 5: run both crates' tests**

Run: `cargo test -p domain --lib && cargo test -p admin-api --lib scrapbook`
Expected: PASS, all scrapbook tests unchanged and green (they are the behavioural pin across the move).

- [ ] **Step 6: commit**

```bash
git add crates/domain/src/lib.rs crates/admin-api/src/scrapbook.rs
git commit -S -m "domain: Link::is_open_door + Link::waits — the scrapbook's door predicates, relocated for a second caller"
```

---

### Task 2: `domain::text` — one sanitiser, one Markdown escape

**Files:**
- Create: `crates/domain/src/text.rs`
- Modify: `crates/domain/src/lib.rs` (add `pub mod text;` near the top)
- Modify: `crates/public-api/src/lib.rs:~1090-1122` (delete private `is_spoofing_format_char` + `sanitize_note`; adopt)
- Modify: `crates/admin-api/src/lib.rs:~1085-1118` (delete private `is_spoofing_format_char` + `sanitize_friend_name`; adopt)
- Test: `crates/domain/src/text.rs` tests module

**Interfaces:**
- Produces: `domain::text::sanitize_line(raw: &str) -> String`, `domain::text::is_spoofing_format_char(c: char) -> bool`, `domain::text::escape_md(s: &str) -> String`, `domain::text::MD_META: &[char]`

- [ ] **Step 1: write the failing tests**

Create `crates/domain/src/text.rs` with ONLY the tests first:

```rust
//! Text safety for anything that reaches a Discord `content` field or a stored note/name.
//! ONE copy (public-api's `sanitize_note` and admin-api's `sanitize_friend_name` were byte-identical
//! private fns; the lantern would have been the third). Order at a render site is ALWAYS
//! `sanitize_line` → cap → `escape_md`, exactly once — see `fulfillment::lantern::field`.

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_folds_line_breaks_and_strips_controls() {
        assert_eq!(sanitize_line("a\nb\r\nc\td"), "a b  c d");
        assert_eq!(sanitize_line("x\u{2028}y\u{0085}z"), "x y z");
        assert_eq!(sanitize_line("a\u{0007}b\u{200E}c"), "abc"); // BEL + LRM stripped
        assert_eq!(sanitize_line("plain ♡"), "plain ♡");
    }

    #[test]
    fn escape_md_neutralises_every_metacharacter_exactly_once() {
        let hostile = r"[open your gift](https://evil) `tick` *b* _i_ ~s~ |sp| > q \ back";
        let once = escape_md(hostile);
        assert_eq!(once, r"\[open your gift\]\(https://evil\) \`tick\` \*b\* \_i\_ \~s\~ \|sp\| \> q \\ back");
        // escaping the already-escaped string doubles the backslashes — the helper is NOT
        // idempotent by design; call sites must escape exactly once (pinned at the render tests).
        assert_ne!(escape_md(&once), once);
        for c in MD_META { assert!(once.contains(&format!("\\{c}")), "{c} must be escaped"); }
    }

    #[test]
    fn escape_md_leaves_plain_text_untouched_and_is_char_boundary_safe() {
        assert_eq!(escape_md("celeste ♡ 2018"), "celeste ♡ 2018");
        assert_eq!(escape_md("日本語*"), "日本語\\*");
    }
}
```

- [ ] **Step 2: register the module and run to verify it fails**

Add `pub mod text;` to `crates/domain/src/lib.rs` (top, after the `use` lines).
Run: `cargo test -p domain --lib text::`
Expected: FAIL — `cannot find function sanitize_line`

- [ ] **Step 3: implement**

Prepend to `crates/domain/src/text.rs` (above the tests):

```rust
/// Discord Markdown metacharacters (the set the family settled on 2026-09-16: a code span was
/// rejected because a backtick in the field breaks out of it). Order is irrelevant; membership is.
pub const MD_META: &[char] = &['\\', '*', '_', '~', '`', '|', '>', '[', ']', '(', ')'];

/// Unicode format characters that can spoof text direction/joining. Copied verbatim from the two
/// former private copies (public-api + admin-api) — the range list is the contract, do not trim.
pub fn is_spoofing_format_char(c: char) -> bool {
    matches!(
        c,
        '\u{200B}'..='\u{200F}'   // ZW space/non-joiner/joiner, LRM, RLM
        | '\u{202A}'..='\u{202E}' // LRE/RLE/PDF/LRO/RLO
        | '\u{2060}'..='\u{2064}' // word joiner … invisible plus
        | '\u{2066}'..='\u{2069}' // LRI/RLI/FSI/PDI
        | '\u{FEFF}'              // BOM / ZWNBSP
        | '\u{FE00}'..='\u{FE0F}' // variation selectors
        | '\u{E0100}'..='\u{E01EF}'
    )
}

/// Line/segment separators (newline, CR, tab, VT, FF, NEL, U+2028/U+2029) become one space so a
/// multiline paste keeps its word boundaries; every other control char and every spoofing format
/// char is stripped. Runs BEFORE any emptiness/length check so stripped chars cannot smuggle
/// visible length past a budget. This is the former `sanitize_note` / `sanitize_friend_name`.
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
/// friend's note) renders as the characters typed and never as a masked link, bold, or a heading.
/// NOT idempotent: escaping twice yields literal backslashes in the channel. Call it exactly
/// once, LAST, after sanitise and cap (a cap applied after escaping can cut between `\` and `*`
/// and leave a dangling backslash that escapes the template's own `**`).
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
```

⚠️ **Before committing, diff `is_spoofing_format_char` against the ORIGINAL in `crates/public-api/src/lib.rs`** (the ranges above were transcribed from a partial read — the file is the truth): `grep -n 'fn is_spoofing_format_char' -A 14 crates/public-api/src/lib.rs`. Copy that body verbatim over the one above if they differ in any range.

- [ ] **Step 4: run to verify it passes**

Run: `cargo test -p domain --lib text::`
Expected: PASS (3 tests)

- [ ] **Step 5: adopt in public-api and admin-api**

In `crates/public-api/src/lib.rs`: delete the private `fn is_spoofing_format_char` and `fn sanitize_note`; add `use domain::text::sanitize_line;` and replace every `sanitize_note(` call with `sanitize_line(` (`grep -n 'sanitize_note\|is_spoofing_format_char' crates/public-api/src/lib.rs`). If any test in that crate calls `sanitize_note` directly, redirect it the same way — the test bodies are the behavioural pin.
In `crates/admin-api/src/lib.rs`: same for `sanitize_friend_name` → `sanitize_line`, delete its private `is_spoofing_format_char`.

- [ ] **Step 6: run the three crates**

Run: `cargo test -p domain --lib && cargo test -p public-api --lib && cargo test -p admin-api --lib`
Expected: PASS, and `grep -rn 'fn sanitize_note\|fn sanitize_friend_name\|fn is_spoofing_format_char' crates/` prints exactly ONE hit, in `crates/domain/src/text.rs`.

- [ ] **Step 7: commit**

```bash
git add crates/domain/src/text.rs crates/domain/src/lib.rs crates/public-api/src/lib.rs crates/admin-api/src/lib.rs
git commit -S -m "domain::text — sanitize_line + escape_md, one copy (public-api/admin-api adopt; the lantern is the third caller)"
```

---

### Task 3: the bell adopts sanitise → cap → escape (fixes the thank-note masked-link finding)

**Files:**
- Modify: `crates/fulfillment/src/bell.rs` (`cap`, `unwrap_card`, `thanks_card`, tests)

**Interfaces:**
- Consumes: `domain::text::{sanitize_line, escape_md}`
- Produces: `pub(crate) fn field(raw: &str, max: usize) -> String` in `bell.rs` — sanitise → cap → escape, exactly once (the lantern reuses this exact fn via `crate::bell::field`).

- [ ] **Step 1: write the failing tests** (append to `bell.rs`'s tests module)

```rust
    #[test]
    fn thanks_card_escapes_markdown_exactly_once() {
        let v = thanks_card("sam", "[open your gift](https://evil) and a `tick", "https://s");
        let c = v["content"].as_str().unwrap();
        assert!(c.contains(r"\[open your gift\]\(https://evil\) and a \`tick"), "{c}");
        assert!(!c.contains(r"\\["), "escaped twice: {c}");
        assert!(c.contains("**sam** says"), "template bold must survive: {c}");
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
    fn cards_apply_the_2000_cap_to_the_final_escaped_content() {
        let note = "*".repeat(500);
        let v = thanks_card(&"_".repeat(120), &note, "https://s");
        let c = v["content"].as_str().unwrap();
        assert!(c.chars().count() <= 2000, "{}", c.chars().count());
        assert!(!c.ends_with('\\'), "cap must not leave a dangling backslash");
    }

    #[test]
    fn unwrap_card_title_with_newline_heading_is_flattened() {
        let v = unwrap_card("sam", "Bad\n# Title", None, "https://s", false);
        let c = v["content"].as_str().unwrap();
        assert!(!c.contains("\n# "), "{c}");
        assert!(c.contains("Bad # Title") || c.contains("Bad  # Title"), "{c}");
    }
```

- [ ] **Step 2: run to verify it fails**

Run: `cargo test -p fulfillment --lib bell::`
Expected: FAIL — `cannot find function field`, and `thanks_card_escapes_markdown_exactly_once` fails on the raw note.

- [ ] **Step 3: implement**

In `bell.rs`, below `fn cap`:

```rust
use domain::text::{escape_md, sanitize_line};

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
```

Then:
- `unwrap_card`: `label = field(label, BELL_LABEL_MAX)`, `title = field(game_title, BELL_TITLE_MAX)` in the `content` format; the embed `title` stays `cap(game_title, BELL_TITLE_MAX)` (embed titles are not Markdown — do NOT escape there). Replace `"content": cap(&content, BELL_CONTENT_MAX)` with `"content": cap_content(&content)`.
- `thanks_card`: `label = field(label, BELL_LABEL_MAX)`, `note = field(note, 500)` (500 = `THANK_NOTE_MAX_CHARS`; if that const is reachable from this crate, use it, else define `const BELL_NOTE_MAX: usize = 500;` beside the others with a comment naming the public-api const it mirrors), and `cap_content`. Update the existing comment above `content` to say Markdown is now escaped and why.

- [ ] **Step 4: run**

Run: `cargo test -p fulfillment --lib bell::`
Expected: PASS (existing 4 + new 4). `thanks_card_quotes_the_note_and_denies_mentions` still passes — `@everyone` has no metacharacter.

- [ ] **Step 5: commit**

```bash
git add crates/fulfillment/src/bell.rs
git commit -S -m "bell: sanitise → cap → escape_md on every foreign field — a thank-note can no longer be a masked link"
```

---

### Task 4: `LanternRecord` + the dynamo quartet

**Files:**
- Modify: `crates/domain/src/lib.rs` (beside `WhisperRecord`)
- Modify: `crates/dynamo/src/lib.rs` (after `list_whispers`, ~line 3075)
- Test: `crates/dynamo/tests/store_test.rs`

**Interfaces:**
- Produces:
  - `domain::LanternRecord { pub slot: String, pub delivered: bool, pub quiet: bool, pub doors: u32, pub chimney: u32, pub wrapped: u32, pub closing: u32 }`
  - `Store::record_lantern(&self, slot: &str, quiet: bool, counts: [u32; 4]) -> Result<bool, StoreError>` (Ok(false) = slot taken)
  - `Store::mark_lantern_delivered(&self, slot: &str) -> Result<(), StoreError>`
  - `Store::get_lantern(&self, slot: &str) -> Result<Option<LanternRecord>, StoreError>`
  - `Store::list_lanterns(&self) -> Result<Vec<LanternRecord>, StoreError>`

- [ ] **Step 1: the failing store test** (append to `crates/dynamo/tests/store_test.rs`, copying the file's `store_or_skip`/table-setup idiom from its whisper test — `grep -n 'record_whisper' crates/dynamo/tests/store_test.rs` to find it)

```rust
#[tokio::test]
async fn lantern_record_is_once_per_slot_and_quiet_rows_are_not_delivered() {
    let Some(store) = store_or_skip("lantern_record").await else { return };
    assert!(store.record_lantern("2026-09-20", false, [0, 1, 0, 0]).await.unwrap());
    assert!(!store.record_lantern("2026-09-20", false, [0, 1, 0, 0]).await.unwrap(), "slot taken");
    assert!(store.record_lantern("2026-09-27", true, [0, 0, 0, 0]).await.unwrap());
    let r = store.get_lantern("2026-09-20").await.unwrap().unwrap();
    assert!(!r.delivered && !r.quiet && r.chimney == 1);
    store.mark_lantern_delivered("2026-09-20").await.unwrap();
    assert!(store.get_lantern("2026-09-20").await.unwrap().unwrap().delivered);
    let q = store.get_lantern("2026-09-27").await.unwrap().unwrap();
    assert!(q.quiet && !q.delivered);
    assert!(store.get_lantern("2026-10-04").await.unwrap().is_none());
    let all = store.list_lanterns().await.unwrap();
    assert_eq!(all.len(), 2);
    assert!(matches!(store.mark_lantern_delivered("2026-10-04").await, Err(StoreError::Corrupt(_))));
}
```

- [ ] **Step 2: run to verify it fails**

Run: `cargo test -p dynamo --test store_test lantern_record`
Expected: FAIL — no method `record_lantern` (or SKIP locally without dynamodb-local: then the compile error is the failure signal; CI is the run).

- [ ] **Step 3: the domain record**

In `crates/domain/src/lib.rs` right after `pub struct WhisperRecord { … }`:

```rust
/// One row per lantern slot (`LANTERN#<sunday-date>`). `quiet` = the tick ran and had nothing to
/// say (decision F1: a quiet week must leave a row or the heartbeat re-runs it); `delivered` =
/// the POST landed and MARK succeeded. The four counts are what compose judged — the preview
/// reports them so a predicate bug reads as "0 of N", not as peace.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LanternRecord {
    pub slot: String,
    pub delivered: bool,
    pub quiet: bool,
    pub doors: u32,
    pub chimney: u32,
    pub wrapped: u32,
    pub closing: u32,
}
```

- [ ] **Step 4: the store quartet** — in `crates/dynamo/src/lib.rs` after `list_whispers`, copying its shapes:

```rust
    /// Record the lantern for one SLOT (Sunday date) — the idempotence gate. PutItem conditioned
    /// `attribute_not_exists(pk)`: exactly ONE lantern per slot. `Ok(false)` = slot taken (a
    /// heartbeat or double-fire lost the race) — the designed quiet-loser signal. `quiet = true`
    /// rows are born AND stay `delivered = false`: a quiet week is not a delivered week (the
    /// backlog line keys on delivered).
    pub async fn record_lantern(
        &self,
        slot: &str,
        quiet: bool,
        counts: [u32; 4],
    ) -> Result<bool, StoreError> {
        use aws_sdk_dynamodb::types::AttributeValue as A;
        let mut item: HashMap<String, A> = HashMap::new();
        item.insert("pk".into(), A::S(format!("LANTERN#{slot}")));
        item.insert("sk".into(), A::S("META".into()));
        item.insert("delivered".into(), A::Bool(false));
        item.insert("quiet".into(), A::Bool(quiet));
        for (k, v) in ["doors", "chimney", "wrapped", "closing"].iter().zip(counts) {
            item.insert((*k).into(), A::N(v.to_string()));
        }
        item.insert(
            "created_at".into(),
            A::S(time::OffsetDateTime::now_utc()
                .format(&time::format_description::well_known::Rfc3339)
                .unwrap_or_default()),
        );
        let res = self.client.put_item().table_name(&self.table).set_item(Some(item))
            .condition_expression("attribute_not_exists(pk)").send().await;
        match res {
            Ok(_) => Ok(true),
            Err(sdk_err) => {
                if is_ccf_put(&sdk_err) { Ok(false) }
                else { Err(StoreError::Aws(AwsFault::from_sdk_error("put_item", &sdk_err))) }
            }
        }
    }

    /// Flip one slot's lantern to delivered — the third write of record → send → mark.
    /// Conditioned `attribute_exists(pk)`: marking a never-recorded slot is a caller bug.
    pub async fn mark_lantern_delivered(&self, slot: &str) -> Result<(), StoreError> {
        use aws_sdk_dynamodb::types::AttributeValue as A;
        let res = self.client.update_item().table_name(&self.table)
            .key("pk", A::S(format!("LANTERN#{slot}"))).key("sk", A::S("META".into()))
            .update_expression("SET delivered = :t")
            .expression_attribute_values(":t", A::Bool(true))
            .condition_expression("attribute_exists(pk)").send().await;
        match res {
            Ok(_) => Ok(()),
            Err(sdk_err) => {
                if is_ccf_update(&sdk_err) {
                    Err(StoreError::Corrupt("mark_lantern_delivered on a slot never recorded"))
                } else { Err(StoreError::Aws(AwsFault::from_sdk_error("update_item", &sdk_err))) }
            }
        }
    }

    fn lantern_from_item(
        item: &HashMap<String, aws_sdk_dynamodb::types::AttributeValue>,
    ) -> Result<domain::LanternRecord, StoreError> {
        let slot = item.get("pk").and_then(|v| v.as_s().ok())
            .and_then(|s| s.strip_prefix("LANTERN#"))
            .ok_or(StoreError::Corrupt("lantern row without LANTERN# pk"))?.to_string();
        let b = |k: &str| item.get(k).and_then(|v| v.as_bool().ok()).copied().unwrap_or(false);
        let n = |k: &str| -> u32 {
            item.get(k).and_then(|v| v.as_n().ok()).and_then(|s| s.parse().ok()).unwrap_or(0)
        };
        Ok(domain::LanternRecord {
            slot, delivered: b("delivered"), quiet: b("quiet"),
            doors: n("doors"), chimney: n("chimney"), wrapped: n("wrapped"), closing: n("closing"),
        })
    }

    /// One slot's row, or None — the heartbeat's three-way branch reads this.
    pub async fn get_lantern(&self, slot: &str) -> Result<Option<domain::LanternRecord>, StoreError> {
        use aws_sdk_dynamodb::types::AttributeValue as A;
        let out = self.client.get_item().table_name(&self.table)
            .key("pk", A::S(format!("LANTERN#{slot}"))).key("sk", A::S("META".into()))
            .send().await
            .map_err(|e| StoreError::Aws(AwsFault::from_sdk_error("get_item", &e)))?;
        match out.item() {
            None => Ok(None),
            Some(item) => Ok(Some(Self::lantern_from_item(item)?)),
        }
    }

    /// Every lantern row. Filtered scan like `list_whispers` — one row per week, no GSI spent.
    pub async fn list_lanterns(&self) -> Result<Vec<domain::LanternRecord>, StoreError> {
        use aws_sdk_dynamodb::types::AttributeValue as A;
        let mut rows = Vec::new();
        let mut last_key: Option<HashMap<String, A>> = None;
        loop {
            let out = self.client.scan().table_name(&self.table)
                .filter_expression("begins_with(pk, :pfx) AND sk = :meta")
                .expression_attribute_values(":pfx", A::S("LANTERN#".into()))
                .expression_attribute_values(":meta", A::S("META".into()))
                .set_exclusive_start_key(last_key.take()).send().await
                .map_err(|e| StoreError::Aws(AwsFault::from_sdk_error("scan", &e)))?;
            for item in out.items() { rows.push(Self::lantern_from_item(item)?); }
            match out.last_evaluated_key() {
                Some(k) if !k.is_empty() => last_key = Some(k.clone()),
                _ => break,
            }
        }
        Ok(rows)
    }
```

(Match `is_ccf_put`/`is_ccf_update`/`HashMap` import names to what `record_whisper` uses in the same file — they are already in scope there.)

- [ ] **Step 5: run**

Run: `cargo test -p dynamo --test store_test lantern_record` (CI if no local dynamodb) and `cargo build -p dynamo`
Expected: PASS / builds clean, `cargo clippy -p dynamo -p domain -- -D warnings` clean.

- [ ] **Step 6: commit**

```bash
git add crates/domain/src/lib.rs crates/dynamo/src/lib.rs crates/dynamo/tests/store_test.rs
git commit -S -m "dynamo: LANTERN#<sunday> slot rows — record (once per slot, quiet-aware), mark, get, list"
```

---

### Task 5: `fulfillment::lantern` — slot buckets, compose, render (pure)

**Files:**
- Create: `crates/fulfillment/src/lantern.rs`
- Modify: `crates/fulfillment/src/lib.rs:14-17` (`pub mod lantern;`)

**Interfaces:**
- Consumes: `domain::{Link, Claim, ClaimState, Game, LanternRecord}`, `Link::is_open_door`, `crate::bell::{field, cap_content}`, `crate::RECONCILE_STUCK_ALERT_AGE` (existing const; if it is private, make it `pub(crate)`).
- Produces:
  - `pub struct Slot { pub sunday: time::Date }` with `key() -> String` (`YYYY-MM-DD`), `start()`, `end()`, `next()`, `contains(t)`
  - `pub fn tick_slot(now: OffsetDateTime) -> Slot`
  - `pub struct Input<'a> { pub links: &'a [Link], pub pending: &'a [Claim], pub games: &'a HashMap<String, Game>, pub friends: &'a HashMap<String, String>, pub slot: Slot, pub now: OffsetDateTime, pub any_delivered: bool }`
  - `pub struct Lantern { pub rooms: Vec<Room>, pub counts: [u32; 4] }`, `pub struct Room { pub heading: String, pub lines: Vec<String>, pub more: u32 }`
  - `pub fn compose(input: &Input) -> Option<Lantern>`
  - `pub fn render(l: &Lantern, slot: &Slot, site_url: &str, preview: bool) -> serde_json::Value`
  - `pub const ROOM_CAP: usize = 5;`

- [ ] **Step 1: write the failing tests** — create `crates/fulfillment/src/lantern.rs` with the tests module first:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use domain::{ClaimState, GameStatus};
    use time::macros::datetime;

    fn link(token: &str, created: OffsetDateTime) -> Link {
        Link {
            token: token.into(), label: format!("label-{token}"), gift_note: None, thank_note: None,
            thanked_at: None, claims_allowed: 1, claims_used: 0, revoked: false, expires_at: None,
            unlock_at: None, curated_game_ids: None, curated_notes: None, friend_id: None,
            created_at: created,
        }
    }
    fn game(id: &str, title: &str) -> Game {
        Game {
            id: id.into(), title: title.into(), bundle: "b".into(), gamekey: "gk".into(),
            machine_name: id.into(), key_type: "steam".into(), giftable: true, hidden: false,
            status: GameStatus::Available, claim_id: None, artwork_url: None, keyindex: 0,
            ..Default::default()
        }
    }
    fn claim(id: &str, gid: &str, at: OffsetDateTime) -> Claim {
        Claim {
            id: id.into(), link_token: "SELF".into(), game_id: gid.into(), state: ClaimState::Pending,
            gift_url: None, revealed_key: None, created_at: at, choice_pre_tpks: None,
            ..Default::default()
        }
    }
    fn input<'a>(links: &'a [Link], pending: &'a [Claim], games: &'a HashMap<String, Game>,
                 friends: &'a HashMap<String, String>, now: OffsetDateTime, any_delivered: bool) -> Input<'a> {
        Input { links, pending, games, friends, slot: tick_slot(now), now, any_delivered }
    }
    const SUN_TICK: OffsetDateTime = datetime!(2026-09-20 21:00 UTC);

    #[test]
    fn tick_slot_maps_every_instant_to_the_sunday_whose_2100z_boundary_it_follows() {
        assert_eq!(tick_slot(datetime!(2026-09-20 21:00 UTC)).key(), "2026-09-20");
        assert_eq!(tick_slot(datetime!(2026-09-20 20:59:59 UTC)).key(), "2026-09-13");
        assert_eq!(tick_slot(datetime!(2026-09-23 21:00 UTC)).key(), "2026-09-20", "wednesday → previous sunday");
        assert_eq!(tick_slot(datetime!(2026-09-27 22:00 UTC)).key(), "2026-09-27", "EST-shaped late tick still its own sunday");
        let s = tick_slot(SUN_TICK);
        assert_eq!(s.start(), datetime!(2026-09-13 21:00 UTC));
        assert_eq!(s.end(), datetime!(2026-09-20 21:00 UTC));
        assert!(s.contains(datetime!(2026-09-20 20:59:59 UTC)) && !s.contains(s.end()));
        assert_eq!(s.next().key(), "2026-09-27");
    }

    #[test]
    fn dst_weeks_put_every_instant_in_exactly_one_bucket() {
        // fall-back: 2026-11-01 02:00 ET; spring-forward: 2027-03-14 02:00 ET
        for (a, b) in [
            (datetime!(2026-10-25 21:00 UTC), datetime!(2026-11-01 21:00 UTC)),
            (datetime!(2027-03-07 21:00 UTC), datetime!(2027-03-14 21:00 UTC)),
        ] {
            let mut t = a;
            while t < b + time::Duration::hours(3) {
                let n = [tick_slot(a).next(), tick_slot(b).next(), tick_slot(a), tick_slot(b)]
                    .iter().filter(|s| s.contains(t)).count();
                assert_eq!(n, 1, "{t} in {n} buckets");
                t += time::Duration::minutes(17);
            }
        }
    }

    #[test]
    fn doors_mention_on_the_14d_and_60d_birthdays_and_never_otherwise() {
        let games = HashMap::new(); let friends = HashMap::new(); let none: Vec<Claim> = vec![];
        let slot = tick_slot(SUN_TICK);
        // birthday instant = created + 14d must fall inside [09-13 21:00Z, 09-20 21:00Z)
        let inside = link("in", slot.start() - time::Duration::days(14) + time::Duration::seconds(1));
        let at_end = link("end", slot.end() - time::Duration::days(14));          // +14d == end ⇒ NOT in
        let before = link("before", slot.start() - time::Duration::days(14) - time::Duration::seconds(1));
        let sixty = link("sixty", slot.start() - time::Duration::days(60) + time::Duration::hours(1));
        let links = vec![inside, at_end, before, sixty];
        let l = compose(&input(&links, &none, &games, &friends, SUN_TICK, true)).unwrap();
        let doors = &l.rooms[0];
        assert_eq!(doors.lines.len(), 2, "{:?}", doors.lines);
        assert!(doors.lines[0].contains("label\\-in") || doors.lines[0].contains("label-in"));
        assert!(doors.lines.iter().any(|x| x.contains("shall it stay open")));
        assert_eq!(l.counts, [2, 0, 0, 0]);
    }

    #[test]
    fn shelf_voice_vs_door_voice() {
        let games = HashMap::new(); let friends = HashMap::new(); let none: Vec<Claim> = vec![];
        let slot = tick_slot(SUN_TICK);
        let mut shelf = link("shelf", slot.start() - time::Duration::days(14) + time::Duration::hours(1));
        shelf.claims_allowed = 15;
        let mut door = link("door", slot.start() - time::Duration::days(14) + time::Duration::hours(1));
        door.claims_allowed = 5; door.friend_id = Some("f1".into());
        let links = vec![shelf, door];
        let l = compose(&input(&links, &none, &games, &friends, SUN_TICK, true)).unwrap();
        let lines = &l.rooms[0].lines;
        assert!(lines[0].contains("the shelf for") && lines[0].contains("15 slots"), "{lines:?}");
        assert!(lines[1].contains("the door for"), "{lines:?}");
    }

    #[test]
    fn backlog_line_counts_doors_past_sixty_days_only_until_first_delivery() {
        let games = HashMap::new(); let friends = HashMap::new(); let none: Vec<Claim> = vec![];
        let links = vec![link("old1", SUN_TICK - time::Duration::days(70)), link("old2", SUN_TICK - time::Duration::days(200)),
                         link("young", SUN_TICK - time::Duration::days(40))];
        let l = compose(&input(&links, &none, &games, &friends, SUN_TICK, false)).unwrap();
        assert!(l.rooms[0].lines.iter().any(|x| x.contains("2 doors older than two months")), "{:?}", l.rooms[0].lines);
        assert!(compose(&input(&links, &none, &games, &friends, SUN_TICK, true)).is_none(), "delivered once ⇒ backlog gone ⇒ quiet");
    }

    #[test]
    fn chimney_lists_pending_past_24h_with_week_counter_and_names_the_action() {
        let mut games = HashMap::new(); games.insert("g1".into(), game("g1", "Soulcalibur *VI*"));
        let friends = HashMap::new(); let links: Vec<Link> = vec![];
        let pending = vec![claim("c1", "g1", SUN_TICK - time::Duration::days(72)), claim("c2", "g1", SUN_TICK - time::Duration::hours(23))];
        let l = compose(&input(&links, &pending, &games, &friends, SUN_TICK, true)).unwrap();
        let r = l.rooms.iter().find(|r| r.heading.contains("chimney")).unwrap();
        assert_eq!(r.lines.len(), 1, "{:?}", r.lines);
        assert!(r.lines[0].contains("week 10") && r.lines[0].contains("compensated") && r.lines[0].contains("#234"));
        assert!(r.lines[0].contains(r"Soulcalibur \*VI\*") && !r.lines[0].contains(r"\\*"), "escaped exactly once: {}", r.lines[0]);
    }

    #[test]
    fn wrapped_mentions_seven_days_after_unlock_once() {
        let games = HashMap::new(); let friends = HashMap::new(); let none: Vec<Claim> = vec![];
        let slot = tick_slot(SUN_TICK);
        let mut w = link("w", SUN_TICK - time::Duration::days(100));
        w.unlock_at = Some(slot.start() - time::Duration::days(7) + time::Duration::hours(2));
        let mut early = w.clone(); early.token = "early".into();
        early.unlock_at = Some(slot.start() - time::Duration::days(7) - time::Duration::hours(2));
        let links = vec![w, early];
        let l = compose(&input(&links, &none, &games, &friends, SUN_TICK, true)).unwrap();
        let r = l.rooms.iter().find(|r| r.heading.contains("wrapped")).unwrap();
        assert_eq!(r.lines.len(), 1);
        assert!(r.lines[0].contains("still wrapped a week later"));
    }

    #[test]
    fn closing_looks_forward_into_the_next_bucket_only() {
        let games = HashMap::new(); let friends = HashMap::new(); let none: Vec<Claim> = vec![];
        let slot = tick_slot(SUN_TICK);
        let mut last_thu = link("lastthu", SUN_TICK - time::Duration::days(100));
        last_thu.expires_at = Some(datetime!(2026-09-17 12:00 UTC)); // already closed at the tick
        let mut next_thu = last_thu.clone(); next_thu.token = "nextthu".into();
        next_thu.expires_at = Some(datetime!(2026-09-24 12:00 UTC));
        let mut tonight = last_thu.clone(); tonight.token = "tonight".into();
        tonight.expires_at = Some(slot.end() + time::Duration::seconds(1));
        let mut far = last_thu.clone(); far.token = "far".into();
        far.expires_at = Some(slot.next().end());
        let links = vec![last_thu, next_thu, tonight, far];
        let l = compose(&input(&links, &none, &games, &friends, SUN_TICK, true)).unwrap();
        let r = l.rooms.iter().find(|r| r.heading.contains("closing")).unwrap();
        assert_eq!(r.lines.len(), 2, "{:?}", r.lines);
        assert!(r.lines.iter().all(|x| !x.contains("lastthu") && !x.contains("far")));
    }

    #[test]
    fn empty_input_is_quiet_and_room_cap_announces_more() {
        let games = HashMap::new(); let friends = HashMap::new(); let none: Vec<Claim> = vec![];
        assert!(compose(&input(&[], &none, &games, &friends, SUN_TICK, true)).is_none());
        let slot = tick_slot(SUN_TICK);
        let links: Vec<Link> = (0..8).map(|i| link(&format!("d{i}"), slot.start() - time::Duration::days(14) + time::Duration::minutes(i))).collect();
        let l = compose(&input(&links, &none, &games, &friends, SUN_TICK, true)).unwrap();
        assert_eq!(l.rooms[0].lines.len(), ROOM_CAP);
        assert_eq!(l.rooms[0].more, 3);
    }

    #[test]
    fn render_carries_one_deep_link_per_room_no_mentions_and_the_final_cap() {
        let games = HashMap::new(); let friends = HashMap::new(); let none: Vec<Claim> = vec![];
        let slot = tick_slot(SUN_TICK);
        let links: Vec<Link> = (0..5).map(|i| { let mut l = link(&"*".repeat(120), slot.start() - time::Duration::days(14) + time::Duration::minutes(i)); l.token = format!("t{i}"); l }).collect();
        let l = compose(&input(&links, &none, &games, &friends, SUN_TICK, true)).unwrap();
        let v = render(&l, &slot, "https://s", false);
        let c = v["content"].as_str().unwrap();
        assert!(c.starts_with("🏮 the lantern"));
        assert!(c.contains("https://s/admin/links"));
        assert!(!c.contains("token=") && !c.contains("/l/"), "no bearer capability: {c}");
        assert!(c.chars().count() <= 2000 && !c.ends_with('\\'));
        assert_eq!(v["allowed_mentions"]["parse"].as_array().unwrap().len(), 0);
        assert!(v["embeds"].as_array().unwrap().is_empty());
        let p = render(&l, &slot, "https://s", true);
        assert!(p["content"].as_str().unwrap().contains("(preview"));
    }

    #[test]
    fn eastern_dates_render_local_across_dst() {
        assert_eq!(eastern_date(datetime!(2026-09-21 02:00 UTC)), "sep 20"); // EDT: 22:00 the day before
        assert_eq!(eastern_date(datetime!(2026-12-01 04:30 UTC)), "nov 30"); // EST: 23:30 the day before
        assert_eq!(eastern_date(datetime!(2026-12-01 05:30 UTC)), "dec 1");
    }
}
```

If `Game`/`Claim` do not implement `Default`, replace `..Default::default()` by listing the remaining fields — copy the fixture from `crates/fulfillment/src/whisper.rs` tests (`fn game(...)`) and `crates/admin-api/src/scrapbook.rs` tests (`fn fx_claim(...)`).

- [ ] **Step 2: register + run to verify it fails**

Add `pub mod lantern;` in `crates/fulfillment/src/lib.rs` after `pub mod heal_pairs;`.
Run: `cargo test -p fulfillment --lib lantern::`
Expected: FAIL — unresolved `Slot`, `tick_slot`, `compose`…

- [ ] **Step 3: implement** — prepend to `lantern.rs`:

```rust
//! The lantern 🏮 — spec: docs/spec-lantern.md. PURE: buckets, composition, rendering. The handler
//! in lib.rs owns reads + the record→send→mark writes. Nothing here may name WHISPER#.
//!
//! Load-bearing rules, each pinned below:
//! - Slot key = Sunday DATE; boundary = Sunday 21:00Z; bucket = [end−7d, end). The Sunday tick
//!   (21:00Z EDT / 22:00Z EST) always closes its own bucket, so nothing is skipped or doubled
//!   across DST (OMBB B2b) — under EST the 21–22Z hour rolls forward a week, never lost.
//! - Doors/wrapped mention on BUCKET(k) (past birthdays); closing on BUCKET(k+1) (Lilith).
//! - A Wednesday `now` maps to the previous Sunday (B1): the heartbeat can never win a slot.
//! - Every foreign field goes through `bell::field` (sanitise → cap → escape) EXACTLY once.
//! - Liveness (`is_open_door`, chimney age) is evaluated at `now`, buckets at the slot:
//!   "same bucket, current liveness" (OMBB). Under EST an expiry in the 21–22Z Sunday hour is in
//!   BUCKET(k+1) but already past at the 22:00Z tick, so liveness drops it — harmless, the door
//!   is closed; "never dropped" is a doors/wrapped property, not closing's.

use crate::bell::{cap_content, field};
use domain::{Claim, ClaimState, Game, Link};
use std::collections::HashMap;
use time::{Date, Duration, OffsetDateTime, Time, UtcOffset, Weekday, Month};

pub const ROOM_CAP: usize = 5;
const LABEL_MAX: usize = 120;
const TITLE_MAX: usize = 240;
const BOUNDARY: Time = time::macros::time!(21:00);
/// The sweep's own bar (`pending_age_sweep`): a Pending older than this is a defect, not a gift
/// mid-open. Mirrors `crate::RECONCILE_STUCK_ALERT_AGE`; re-declared as a Duration so this module
/// stays I/O- and lib-free. Keep them equal (pinned by `chimney_bar_matches_the_sweep`).
const CHIMNEY_BAR: Duration = Duration::hours(24);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Slot {
    pub sunday: Date,
}

impl Slot {
    pub fn key(&self) -> String {
        self.sunday.to_string() // `Date` Display = YYYY-MM-DD
    }
    pub fn end(&self) -> OffsetDateTime {
        self.sunday.with_time(BOUNDARY).assume_utc()
    }
    pub fn start(&self) -> OffsetDateTime {
        self.end() - Duration::days(7)
    }
    pub fn next(&self) -> Slot {
        Slot { sunday: self.sunday + Duration::days(7) }
    }
    pub fn contains(&self, t: OffsetDateTime) -> bool {
        self.start() <= t && t < self.end()
    }
}

/// The slot a tick at `now` belongs to: the latest Sunday 21:00Z boundary at or before `now`.
pub fn tick_slot(now: OffsetDateTime) -> Slot {
    let now = now.to_offset(UtcOffset::UTC);
    let back = i64::from(now.weekday().number_days_from_sunday());
    let mut sunday = now.date() - Duration::days(back);
    if back == 0 && now.time() < BOUNDARY {
        sunday -= Duration::days(7);
    }
    Slot { sunday }
}

/// America/New_York offset for `t` — the US rule (2nd Sunday of March 02:00 local → 1st Sunday of
/// November 02:00 local). Hand-rolled because the `time` crate ships no tz database and this is
/// the only place the lantern needs local time (for dates in the message). Pinned across DST.
fn eastern_offset(t: OffsetDateTime) -> UtcOffset {
    let y = t.year();
    let nth_sunday = |m: Month, n: u8| -> Date {
        let first = Date::from_calendar_date(y, m, 1).expect("valid");
        let to_sun = (7 - u8::from(first.weekday().number_days_from_sunday())) % 7;
        first + Duration::days(i64::from(to_sun) + 7 * i64::from(n - 1))
    };
    // transitions at 02:00 local = 07:00Z (EST→EDT) and 06:00Z (EDT→EST)
    let dst_start = nth_sunday(Month::March, 2).with_time(time::macros::time!(07:00)).assume_utc();
    let dst_end = nth_sunday(Month::November, 1).with_time(time::macros::time!(06:00)).assume_utc();
    if t >= dst_start && t < dst_end { UtcOffset::from_hms(-4, 0, 0).expect("edt") }
    else { UtcOffset::from_hms(-5, 0, 0).expect("est") }
}

/// "sep 20" — lowercase month + day, in America/New_York (ben reads local).
pub fn eastern_date(t: OffsetDateTime) -> String {
    let l = t.to_offset(eastern_offset(t));
    let m = match l.month() {
        Month::January => "jan", Month::February => "feb", Month::March => "mar", Month::April => "apr",
        Month::May => "may", Month::June => "jun", Month::July => "jul", Month::August => "aug",
        Month::September => "sep", Month::October => "oct", Month::November => "nov", Month::December => "dec",
    };
    format!("{m} {}", l.day())
}

fn weekday_name(t: OffsetDateTime) -> &'static str {
    match t.to_offset(eastern_offset(t)).weekday() {
        Weekday::Monday => "monday", Weekday::Tuesday => "tuesday", Weekday::Wednesday => "wednesday",
        Weekday::Thursday => "thursday", Weekday::Friday => "friday", Weekday::Saturday => "saturday",
        Weekday::Sunday => "sunday",
    }
}

pub struct Input<'a> {
    pub links: &'a [Link],
    pub pending: &'a [Claim],
    pub games: &'a HashMap<String, Game>,
    /// friend_id → name (admin-written; still goes through `field`).
    pub friends: &'a HashMap<String, String>,
    pub slot: Slot,
    pub now: OffsetDateTime,
    /// Has ANY lantern ever been delivered? (`quiet` rows do not count.) Gates the backlog line.
    pub any_delivered: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Room {
    pub heading: String,
    pub lines: Vec<String>,
    /// Lines beyond ROOM_CAP — rendered as "· and N more".
    pub more: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Lantern {
    pub rooms: Vec<Room>,
    /// doors, chimney, wrapped, closing — what compose JUDGED (logged on quiet too).
    pub counts: [u32; 4],
}

fn is_shelf(l: &Link) -> bool {
    l.claims_allowed > 1 && l.friend_id.is_none() && l.curated_game_ids.is_none()
}

fn recipient(l: &Link, friends: &HashMap<String, String>) -> String {
    let raw = l.friend_id.as_ref().and_then(|id| friends.get(id)).map(String::as_str).unwrap_or(&l.label);
    field(raw, LABEL_MAX)
}

fn title(game_id: &str, games: &HashMap<String, Game>) -> String {
    // an orphan id renders AS the id — counted, never skipped (the scrapbook's rule)
    field(games.get(game_id).map(|g| g.title.as_str()).unwrap_or(game_id), TITLE_MAX)
}

fn room(heading: &str, mut lines: Vec<String>) -> Option<Room> {
    if lines.is_empty() { return None; }
    let more = lines.len().saturating_sub(ROOM_CAP) as u32;
    lines.truncate(ROOM_CAP);
    Some(Room { heading: heading.into(), lines, more })
}

pub fn compose(input: &Input) -> Option<Lantern> {
    let Input { links, pending, games, friends, slot, now, any_delivered } = input;
    let now = *now;
    let next = slot.next();

    // 🚪 doors — open, zero claims, birthday 14d / 60d inside THIS bucket; oldest first
    let mut open: Vec<&Link> = links.iter().filter(|l| l.is_open_door(now) && l.claims_used == 0).collect();
    open.sort_by_key(|l| l.created_at);
    let mut doors = Vec::new();
    let mut backlog = 0u32;
    for l in &open {
        let b14 = l.created_at + Duration::days(14);
        let b60 = l.created_at + Duration::days(60);
        let who = recipient(l, friends);
        let left = l.claims_allowed - l.claims_used;
        if slot.contains(b14) {
            doors.push(if is_shelf(l) {
                format!("· the shelf for {who} — {} slots, nobody's taken one in two weeks", l.claims_allowed)
            } else {
                format!("· the door for {who} — open two weeks, nobody's come through yet")
            });
        } else if slot.contains(b60) {
            doors.push(if is_shelf(l) {
                format!("· the shelf for {who} — {} slots, nobody's taken one in two months. shall it stay open?", l.claims_allowed)
            } else {
                format!("· the door for {who} — open two months, {left} still inside. shall it stay open?")
            });
        } else if !any_delivered && b60 < slot.start() {
            backlog += 1;
        }
    }
    let door_count = doors.len() as u32;
    if backlog > 0 {
        doors.push(format!("· and {backlog} doors older than two months nobody has walked through"));
    }

    // 🕯️ chimney — Pending past the bar, every week, with a week counter and the clearing action
    let mut stuck: Vec<&Claim> = pending.iter()
        .filter(|c| c.state == ClaimState::Pending && now - c.created_at >= CHIMNEY_BAR).collect();
    stuck.sort_by_key(|c| c.created_at);
    let chimney: Vec<String> = stuck.iter().map(|c| {
        let week = (now - c.created_at).whole_days() / 7;
        format!(
            "· {} — a claim started {} never finished (week {week}). it clears when the claim is compensated (slot returned, game re-listed) or fulfilled — no admin button for that yet, see #234",
            title(&c.game_id, games), eastern_date(c.created_at)
        )
    }).collect();

    // 🎁 wrapped — unlocked, unopened, unlock+7d inside THIS bucket
    let mut wrapped = Vec::new();
    for l in links.iter().filter(|l| l.claims_used == 0 && l.is_open_door(now)) {
        if let Some(u) = l.unlock_at && slot.contains(u + Duration::days(7)) {
            wrapped.push(format!("· the gift for {} — openable since {}, still wrapped a week later", recipient(l, friends), eastern_date(u)));
        }
    }

    // ⏳ closing — live with claims left, expiring in the NEXT bucket (looks forward)
    let mut closing = Vec::new();
    let mut soon: Vec<&Link> = links.iter()
        .filter(|l| l.is_open_door(now) && l.expires_at.is_some_and(|e| next.contains(e))).collect();
    soon.sort_by_key(|l| l.expires_at);
    for l in soon {
        let e = l.expires_at.expect("filtered");
        let left = l.claims_allowed - l.claims_used;
        closing.push(format!("· the door for {} closes {} with {left} claim{} left",
            recipient(l, friends), weekday_name(e), if left == 1 { "" } else { "s" }));
    }

    let counts = [door_count, chimney.len() as u32, wrapped.len() as u32, closing.len() as u32];
    let rooms: Vec<Room> = [
        room("🚪 doors nobody has walked through  ↗ {site}/admin/links", doors),
        room("🕯️ stuck in the chimney  ↗ {site}/admin/ops", chimney),
        room("🎁 wrapped, and past its day  ↗ {site}/admin/links", wrapped),
        room("⏳ doors closing soon  ↗ {site}/admin/links", closing),
    ].into_iter().flatten().collect();
    if rooms.is_empty() { None } else { Some(Lantern { rooms, counts }) }
}

/// One `content` message, no embeds, mentions structurally denied. `{site}` in headings is
/// substituted here so compose stays URL-free.
pub fn render(l: &Lantern, slot: &Slot, site_url: &str, preview: bool) -> serde_json::Value {
    let mut content = format!("🏮 the lantern · week of {}{}\n", eastern_date(slot.start()),
        if preview { " (preview — nothing recorded)" } else { "" });
    for r in &l.rooms {
        content.push('\n');
        content.push_str(&r.heading.replace("{site}", site_url));
        content.push('\n');
        for line in &r.lines { content.push_str(line); content.push('\n'); }
        if r.more > 0 { content.push_str(&format!("· and {} more\n", r.more)); }
    }
    serde_json::json!({
        "content": cap_content(content.trim_end()),
        "embeds": [],
        "allowed_mentions": { "parse": [] },
    })
}
```

Notes for the implementer:
- `Link`'s `is_open_door` comes from Task 1. `bell::field`/`cap_content` from Task 3 (make both `pub(crate)`).
- `let … && …` chains are edition-2024 and already used in `domain` — fine.
- Add a tiny test `chimney_bar_matches_the_sweep` asserting `CHIMNEY_BAR == crate::RECONCILE_STUCK_ALERT_AGE` (convert types as needed) so the two bars cannot drift.
- Lines are already escaped by `field`; `render` only joins — this is the "exactly once" discipline the tests pin (single backslash).

- [ ] **Step 4: run**

Run: `cargo test -p fulfillment --lib lantern:: && cargo clippy -p fulfillment -- -D warnings`
Expected: PASS (12 tests), clippy clean.

- [ ] **Step 5: commit**

```bash
git add crates/fulfillment/src/lantern.rs crates/fulfillment/src/lib.rs
git commit -S -m "lantern: slot buckets (sunday-date key, 21:00Z boundary), compose + render — pure, fixtures at every boundary"
```

---

### Task 6: the handlers — lantern, heartbeat, preview — and the env wiring

**Files:**
- Modify: `crates/fulfillment/src/lib.rs` (`FulfillRequest` ~line 161-175, `FulfillResponse` ~line 204, `Deps` ~line 527, dispatch ~line 740, handlers after `handle_whisper`)
- Modify: `crates/fulfillment/src/main.rs` (~line 65 `lantern_suppressed`, ~line 296 Deps build)
- Test: `crates/fulfillment/tests/handler_test.rs` (append; `deps()` builders at lines 135 and 534 gain the new field)

**Interfaces:**
- Consumes: Task 4 store quartet, Task 5 `lantern::{tick_slot, compose, render, Input, Slot}`, `resolve_whisper_url`, `whisper_send_body`, `ping_msg`.
- Produces: `FulfillRequest::{Lantern, LanternHeartbeat, LanternPreview}` (serde `snake_case` ⇒ `{"op":"lantern"}`, `{"op":"lantern_heartbeat"}`, `{"op":"lantern_preview"}`), `FulfillResponse::Lanterned`, `Deps.lantern_disabled: bool`.

- [ ] **Step 1: the failing handler tests** — append to `crates/fulfillment/tests/handler_test.rs`, next to the whisper arms (`deps_whisper` at ~9127):

```rust
fn deps_lantern(store: Store, humble_uri: &str, whisper_webhook: Option<String>) -> Deps {
    deps_whisper(store, humble_uri, None, whisper_webhook)
}

async fn seed_stuck_pending(store: &Store, days: i64) {
    // an Available game + a Pending self-claim `days` old — the #234 shape
    let g = available_game("gk:stuck", "Stuck Game");
    store.put_game(&g).await.unwrap();
    let mut c = domain::Claim {
        id: "c-stuck".into(), link_token: domain::SELF_LINK_TOKEN.into(), game_id: g.id.clone(),
        state: domain::ClaimState::Pending, gift_url: None, revealed_key: None,
        created_at: OffsetDateTime::now_utc() - time::Duration::days(days), choice_pre_tpks: None,
        ..Default::default()
    };
    // use whichever seeding helper the whisper/reconcile tests use for a pending claim
    // (`seed_aged_pending` at ~969 seeds via the store's claim path); copy that idiom here.
    let _ = &mut c;
    seed_aged_pending(store, "gk", "stuck", days * 24).await;
}

#[tokio::test]
async fn lantern_dark_register_writes_nothing() {
    let Some(store) = store_or_skip("lantern_dark").await else { return };
    let humble = MockServer::start().await;
    seed_stuck_pending(&store, 3).await;
    let d = deps_lantern(store.clone(), &humble.uri(), None);
    assert_eq!(handle(&d, FulfillRequest::Lantern).await, FulfillResponse::Lanterned);
    assert!(store.list_lanterns().await.unwrap().is_empty(), "dark ⇒ zero writes");
}

#[tokio::test]
async fn lantern_quiet_writes_exactly_one_row_and_sends_nothing() {
    let Some(store) = store_or_skip("lantern_quiet").await else { return };
    let humble = MockServer::start().await;
    let discord = discord_ok().await;
    let d = deps_lantern(store.clone(), &humble.uri(), Some(discord.uri()));
    assert_eq!(handle(&d, FulfillRequest::Lantern).await, FulfillResponse::Lanterned);
    let rows = store.list_lanterns().await.unwrap();
    assert_eq!(rows.len(), 1);
    assert!(rows[0].quiet && !rows[0].delivered);
    assert_eq!(discord.received_requests().await.unwrap().len(), 0);
}

#[tokio::test]
async fn lantern_sends_records_and_marks_and_slot_is_once() {
    let Some(store) = store_or_skip("lantern_send").await else { return };
    let humble = MockServer::start().await;
    let discord = discord_ok().await;
    seed_stuck_pending(&store, 3).await;
    let d = deps_lantern(store.clone(), &humble.uri(), Some(discord.uri()));
    assert_eq!(handle(&d, FulfillRequest::Lantern).await, FulfillResponse::Lanterned);
    let rows = store.list_lanterns().await.unwrap();
    assert_eq!(rows.len(), 1);
    assert!(rows[0].delivered && !rows[0].quiet && rows[0].chimney == 1);
    let reqs = discord.received_requests().await.unwrap();
    assert_eq!(reqs.len(), 1);
    let body: serde_json::Value = serde_json::from_slice(&reqs[0].body).unwrap();
    let c = body["content"].as_str().unwrap();
    assert!(c.contains("stuck in the chimney") && c.contains("Stuck Game") && c.contains("#234"));
    assert!(!c.contains("Stuck Game\\") , "no stray escapes: {c}");
    // same slot again ⇒ loser, no second send
    assert_eq!(handle(&d, FulfillRequest::Lantern).await, FulfillResponse::Lanterned);
    assert_eq!(discord.received_requests().await.unwrap().len(), 1);
}

#[tokio::test]
async fn lantern_heartbeat_three_way_branch() {
    let Some(store) = store_or_skip("lantern_heartbeat").await else { return };
    let humble = MockServer::start().await;
    let discord = discord_ok().await;
    seed_stuck_pending(&store, 3).await;
    let d = deps_lantern(store.clone(), &humble.uri(), Some(discord.uri()));
    let slot = fulfillment::lantern::tick_slot(OffsetDateTime::now_utc()).key();
    // (a) absent ⇒ full run
    assert_eq!(handle(&d, FulfillRequest::LanternHeartbeat).await, FulfillResponse::Lanterned);
    assert_eq!(discord.received_requests().await.unwrap().len(), 1);
    assert!(store.get_lantern(&slot).await.unwrap().unwrap().delivered);
    // (b) delivered ⇒ nothing
    assert_eq!(handle(&d, FulfillRequest::LanternHeartbeat).await, FulfillResponse::Lanterned);
    assert_eq!(discord.received_requests().await.unwrap().len(), 1);
    // (c) undelivered ⇒ resend + mark: forge an undelivered row for a fresh slot by recording
    //     without marking, then point the heartbeat at it via the store state
    let store2 = store.clone();
    store2.record_lantern("1999-01-03", false, [0, 1, 0, 0]).await.unwrap();
    // the heartbeat only looks at the CURRENT slot, so exercise the resend path through the
    // lib seam: fulfillment::lantern_resend_for_test(&d, "1999-01-03")
    assert!(fulfillment::lantern_resend_for_test(&d, "1999-01-03").await);
    assert_eq!(discord.received_requests().await.unwrap().len(), 2);
    assert!(store.get_lantern("1999-01-03").await.unwrap().unwrap().delivered);
    // (d) quiet ⇒ nothing (F1)
    store.record_lantern("1999-01-10", true, [0, 0, 0, 0]).await.unwrap();
    assert!(!fulfillment::lantern_resend_for_test(&d, "1999-01-10").await, "quiet is not resent");
    assert_eq!(discord.received_requests().await.unwrap().len(), 2);
}

#[tokio::test]
async fn lantern_preview_writes_nothing_and_keeps_the_backlog_line() {
    let Some(store) = store_or_skip("lantern_preview").await else { return };
    let humble = MockServer::start().await;
    let discord = discord_ok().await;
    // an old open door ⇒ backlog line while nothing delivered
    let mut l = link("old-door");
    l.created_at = OffsetDateTime::now_utc() - time::Duration::days(90);
    store.put_link(&l).await.unwrap();
    let d = deps_lantern(store.clone(), &humble.uri(), Some(discord.uri()));
    assert_eq!(handle(&d, FulfillRequest::LanternPreview).await, FulfillResponse::PreviewSent);
    assert!(store.list_lanterns().await.unwrap().is_empty(), "preview ⇒ zero writes");
    let reqs = discord.received_requests().await.unwrap();
    let c = serde_json::from_slice::<serde_json::Value>(&reqs[0].body).unwrap()["content"].as_str().unwrap().to_string();
    assert!(c.contains("(preview") && c.contains("1 doors older than two months"), "{c}");
    // a real tick afterwards STILL carries the backlog line
    assert_eq!(handle(&d, FulfillRequest::Lantern).await, FulfillResponse::Lanterned);
    let reqs = discord.received_requests().await.unwrap();
    let c2 = serde_json::from_slice::<serde_json::Value>(&reqs[1].body).unwrap()["content"].as_str().unwrap().to_string();
    assert!(c2.contains("1 doors older than two months"), "{c2}");
}

#[tokio::test]
async fn lantern_disabled_darkens_only_the_lantern() {
    let Some(store) = store_or_skip("lantern_disabled").await else { return };
    let humble = MockServer::start().await;
    let discord = discord_ok().await;
    seed_stuck_pending(&store, 3).await;
    let mut d = deps_lantern(store.clone(), &humble.uri(), Some(discord.uri()));
    d.lantern_disabled = true;
    assert_eq!(handle(&d, FulfillRequest::Lantern).await, FulfillResponse::Lanterned);
    assert!(store.list_lanterns().await.unwrap().is_empty());
    assert_eq!(discord.received_requests().await.unwrap().len(), 0);
    // the whisper register is untouched by the lantern flag: a bell still rings
    assert!(!d.bell_disabled);
}
```

Use the file's existing helpers (`link`, `available_game`, `seed_aged_pending`, `discord_ok`, `store.put_game`/`put_link` — check their exact names with `grep -n 'pub async fn put_link\|pub async fn put_game' crates/dynamo/src/lib.rs` and the test file's `fn link(`). Where `seed_stuck_pending` above hedges, resolve it to ONE idiom before running.

- [ ] **Step 2: run to verify it fails**

Run: `cargo test -p fulfillment --test handler_test lantern_`
Expected: FAIL to compile — `FulfillRequest::Lantern` does not exist, `Deps` has no `lantern_disabled`.

- [ ] **Step 3: request/response/Deps**

In `crates/fulfillment/src/lib.rs`:
- `FulfillRequest`, after `WhisperPreview`:
```rust
    /// The lantern 🏮 (spec: docs/spec-lantern.md): Sunday's walk through the attic — stalled
    /// intentions, on the whisper register. Scheduler input `{"op":"lantern"}`.
    Lantern,
    /// Wednesday's heartbeat: keeps the never-ran metric alive and retries an un-run or
    /// undelivered Sunday for the SAME slot (`tick_slot` maps Wednesday to the previous Sunday).
    /// Never sends a delivered or quiet slot again. Scheduler input `{"op":"lantern_heartbeat"}`.
    LanternHeartbeat,
    /// Zero-write preview: compose against live data and POST with a preview header. Manual
    /// invoke only. The deploy-verification instrument.
    LanternPreview,
```
- `FulfillResponse`, after `Whispered`:
```rust
    /// The lantern/heartbeat ran (sent, quiet, dark, slot-taken, or a logged failure) — fieldless
    /// like `Whispered`, for the same reason.
    Lanterned,
```
- `Deps`, after `bell_disabled`:
```rust
    /// The lantern's OWN off-switch (`LANTERN_DISABLED`), decoupled like the bell's: shared
    /// credential, split mute.
    pub lantern_disabled: bool,
```
- dispatch (`handle`): add
```rust
        FulfillRequest::Lantern => handle_lantern(deps).await,
        FulfillRequest::LanternHeartbeat => handle_lantern_heartbeat(deps).await,
        FulfillRequest::LanternPreview => handle_lantern_preview(deps).await,
```

- [ ] **Step 4: the handlers** — after `handle_whisper`:

```rust
/// Reads everything compose needs. `Err` ⇒ already logged; caller exits (next tick retries).
async fn lantern_reads(deps: &Deps) -> Option<(Vec<Link>, Vec<Claim>, std::collections::HashMap<String, Game>, std::collections::HashMap<String, String>, Vec<domain::LanternRecord>)> {
    let links = deps.store.list_links().await.map_err(|e| tracing::error!(error = ?e, outcome = "lantern_read_failed", "lantern: cannot list links")).ok()?;
    let pending = deps.store.list_pending_claims().await.map_err(|e| tracing::error!(error = ?e, outcome = "lantern_read_failed", "lantern: cannot list pending")).ok()?;
    let friends = deps.store.list_friends().await.map_err(|e| tracing::error!(error = ?e, outcome = "lantern_read_failed", "lantern: cannot list friends")).ok()?;
    let lanterns = deps.store.list_lanterns().await.map_err(|e| tracing::error!(error = ?e, outcome = "lantern_read_failed", "lantern: cannot list lantern log")).ok()?;
    let mut games = std::collections::HashMap::new();
    for c in &pending {
        if let Ok(Some(g)) = deps.store.get_game(&c.game_id).await { games.insert(c.game_id.clone(), g); }
        // an unreadable/missing game renders as its id — counted, never skipped
    }
    let friends = friends.into_iter().map(|f| (f.id, f.name)).collect();
    Some((links, pending, games, friends, lanterns))
}

/// The lantern gate: the lantern's own flag first, then the shared register (whose dark faces
/// already announce themselves naming the param to light — the same param lights the lantern).
async fn resolve_lantern_url(deps: &Deps) -> Option<String> {
    if deps.lantern_disabled {
        tracing::warn!(outcome = "lantern_dark", "LANTERN_DISABLED set — not lighting, by choice; zero writes");
        return None;
    }
    resolve_whisper_url(deps).await
}

async fn handle_lantern(deps: &Deps) -> FulfillResponse {
    let slot = lantern::tick_slot(OffsetDateTime::now_utc());
    run_lantern(deps, &slot).await;
    FulfillResponse::Lanterned
}

/// RECORD → SEND → MARK for one slot. Returns whether a message was sent.
async fn run_lantern(deps: &Deps, slot: &lantern::Slot) -> bool {
    let Some(url) = resolve_lantern_url(deps).await else { return false };
    let Some((links, pending, games, friends, lanterns)) = lantern_reads(deps).await else { return false };
    let now = OffsetDateTime::now_utc();
    let any_delivered = lanterns.iter().any(|r| r.delivered);
    let input = lantern::Input { links: &links, pending: &pending, games: &games, friends: &friends, slot: slot.clone(), now, any_delivered };
    let Some(card) = lantern::compose(&input) else {
        tracing::info!(outcome = "lantern_quiet", slot = %slot.key(), links = links.len(), pending = pending.len(),
            "lantern: nothing to say this week — quiet row recorded (F1)");
        match deps.store.record_lantern(&slot.key(), true, [0, 0, 0, 0]).await {
            Ok(true) => {}
            Ok(false) => tracing::info!(outcome = "lantern_slot_taken", slot = %slot.key(), "quiet, and the slot was already taken"),
            Err(e) => tracing::error!(error = ?e, slot = %slot.key(), "lantern: quiet record failed"),
        }
        return false;
    };
    let [d, c, w, x] = card.counts;
    match deps.store.record_lantern(&slot.key(), false, card.counts).await {
        Ok(true) => {}
        Ok(false) => { tracing::info!(outcome = "lantern_slot_taken", slot = %slot.key(), "this slot already has a lantern — loser exits"); return false; }
        Err(e) => { tracing::error!(error = ?e, slot = %slot.key(), "lantern: record failed — NOT sending (record precedes act)"); return false; }
    }
    tracing::info!(slot = %slot.key(), doors = d, chimney = c, wrapped = w, closing = x, "lantern composed");
    send_and_mark(deps, &url, slot, &card).await
}

/// SEND then MARK. At-least-once by design: a landed POST whose MARK fails leaves the row
/// undelivered, and Wednesday's heartbeat resends it — a duplicate beats a lost bucket (OMBB).
async fn send_and_mark(deps: &Deps, url: &str, slot: &lantern::Slot, card: &lantern::Lantern) -> bool {
    let body = lantern::render(card, slot, &deps.whisper_site_url, false);
    if whisper_send_body(&deps.http, url, &body).await {
        if let Err(e) = deps.store.mark_lantern_delivered(&slot.key()).await {
            tracing::error!(error = ?e, slot = %slot.key(), "lantern sent but mark failed — heartbeat will resend (at-least-once)");
        }
        true
    } else {
        tracing::error!(outcome = "lantern_send_failed", slot = %slot.key(), "lantern POST failed — row stays undelivered; heartbeat resends");
        ping_msg(deps, &OperatorMessage::fmt(
            "the lantern's SEND FAILED for slot {} — recorded, undelivered; Wednesday's heartbeat will resend. Check the whisper webhook URL / Discord status.",
            &[Part::Id(&slot.key())],
        )).await;
        false
    }
}

/// Wednesday: absent ⇒ full run · delivered|quiet ⇒ exit · undelivered ⇒ resend + mark.
async fn handle_lantern_heartbeat(deps: &Deps) -> FulfillResponse {
    let slot = lantern::tick_slot(OffsetDateTime::now_utc());
    match deps.store.get_lantern(&slot.key()).await {
        Ok(None) => { tracing::warn!(slot = %slot.key(), "heartbeat: Sunday never recorded — running the lantern for its slot"); run_lantern(deps, &slot).await; }
        Ok(Some(r)) if r.delivered || r.quiet => tracing::info!(slot = %slot.key(), quiet = r.quiet, "heartbeat: slot settled — metric touched, nothing to do"),
        Ok(Some(_)) => { resend_undelivered(deps, &slot).await; }
        Err(e) => tracing::error!(error = ?e, outcome = "lantern_read_failed", "heartbeat: cannot read slot row"),
    }
    FulfillResponse::Lanterned
}

/// Recompose the SAME slot (same buckets, current liveness) and send; no RECORD (the row exists).
async fn resend_undelivered(deps: &Deps, slot: &lantern::Slot) -> bool {
    let Some(url) = resolve_lantern_url(deps).await else { return false };
    let Some((links, pending, games, friends, lanterns)) = lantern_reads(deps).await else { return false };
    let any_delivered = lanterns.iter().any(|r| r.delivered);
    let input = lantern::Input { links: &links, pending: &pending, games: &games, friends: &friends, slot: slot.clone(), now: OffsetDateTime::now_utc(), any_delivered };
    match lantern::compose(&input) {
        Some(card) => { tracing::warn!(slot = %slot.key(), "heartbeat: resending an undelivered lantern (at-least-once)"); send_and_mark(deps, &url, slot, &card).await }
        None => { tracing::info!(slot = %slot.key(), "heartbeat: undelivered slot now composes empty — marking delivered to settle it"); let _ = deps.store.mark_lantern_delivered(&slot.key()).await; false }
    }
}

/// Test seam for the heartbeat's undelivered branch on an arbitrary slot key.
#[doc(hidden)]
pub async fn lantern_resend_for_test(deps: &Deps, slot_key: &str) -> bool {
    let sunday = time::Date::parse(slot_key, &time::format_description::well_known::Iso8601::DATE).expect("slot key");
    let slot = lantern::Slot { sunday };
    match deps.store.get_lantern(slot_key).await {
        Ok(Some(r)) if !r.delivered && !r.quiet => resend_undelivered(deps, &slot).await,
        _ => false,
    }
}

/// Zero writes: compose against live data, POST with the preview header.
async fn handle_lantern_preview(deps: &Deps) -> FulfillResponse {
    let Some(url) = resolve_lantern_url(deps).await else { return FulfillResponse::PreviewBlocked };
    let Some((links, pending, games, friends, lanterns)) = lantern_reads(deps).await else { return FulfillResponse::PreviewBlocked };
    let now = OffsetDateTime::now_utc();
    let slot = lantern::tick_slot(now);
    let any_delivered = lanterns.iter().any(|r| r.delivered);
    let undelivered = lanterns.iter().filter(|r| !r.delivered && !r.quiet).count();
    tracing::info!(slot = %slot.key(), rows = lanterns.len(), undelivered, "lantern_preview: log state");
    let input = lantern::Input { links: &links, pending: &pending, games: &games, friends: &friends, slot, now, any_delivered };
    let Some(card) = lantern::compose(&input) else {
        tracing::warn!(outcome = "lantern_preview_quiet", "lantern_preview: this week would be quiet — nothing to show");
        return FulfillResponse::PreviewBlocked;
    };
    let body = lantern::render(&card, &input.slot, &deps.whisper_site_url, true);
    if whisper_send_body(&deps.http, &url, &body).await { FulfillResponse::PreviewSent } else { FulfillResponse::PreviewSendFailed }
}
```

Imports: `Link`, `Claim`, `Game` from `domain` are likely already imported at the top of lib.rs; add what is missing. `time::Date::parse` with `Iso8601::DATE` needs the `parsing` feature (workspace has it).

- [ ] **Step 5: main.rs**

Beside `bell_suppressed`:
```rust
/// Is the LANTERN suppressed? Reads `LANTERN_DISABLED` and ONLY that — same register-decoupling
/// rule as the bell: shared credential, split mute.
fn lantern_suppressed(env: impl Fn(&str) -> Option<String>) -> bool {
    env("LANTERN_DISABLED").as_deref() == Some("1")
}
```
In the `Deps { … }` build: `lantern_disabled: lantern_suppressed(|k| std::env::var(k).ok()),` after `bell_disabled`. If main.rs has a test pinning `whisper_suppressed`/`bell_suppressed` flag names, add the same shape for `lantern_suppressed` (grep `fn bell_suppressed_reads_only` or similar).

Add `lantern_disabled: false,` to BOTH `Deps` builders in `handler_test.rs` (lines ~150 and ~558) and to any other `Deps {` literal in `crates/fulfillment` (`grep -rn 'bell_disabled:' crates/fulfillment/`).

- [ ] **Step 6: run**

Run: `cargo test -p fulfillment --lib && cargo test -p fulfillment --test handler_test lantern_ && cargo clippy --workspace -- -D warnings`
Expected: PASS locally where dynamodb-local exists; otherwise the lib tests pass and the handler tests SKIP — **push and read CI before claiming green.**

- [ ] **Step 7: commit**

```bash
git add crates/fulfillment/src/lib.rs crates/fulfillment/src/main.rs crates/fulfillment/tests/handler_test.rs
git commit -S -m "lantern: handlers — record→send→mark on the sunday slot, wednesday heartbeat (absent/settled/undelivered), zero-write preview, LANTERN_DISABLED"
```

---

### Task 7: terraform — schedules, alarms, flag

**Files:**
- Modify: `terraform/tf-variables.tf` (after `whisper_schedule_expression`)
- Modify: `terraform/aws-eventbridge.tf` (append after the whisper block)
- Modify: `terraform/aws-cloudwatch-alarms.tf` (append after `whisper_target_errors`)
- Modify: `terraform/production.tfvars` (add `lantern_enabled = true`)

**Interfaces:**
- Consumes: `module.label_*` pattern, `module.lambda_fulfillment`, `aws_sns_topic.ops_alarms`, `var.lambda_permissions_boundary_arn`, `var.whisper_enabled` (the register the lantern rides).

- [ ] **Step 1: variables**

```hcl
variable "lantern_enabled" {
  type        = bool
  default     = false # flipped in production.tfvars; default-off so plan-only environments stay silent
  description = "The lantern (spec: docs/spec-lantern.md): Sunday-evening stalled-intentions message on the WHISPER register. Creates the schedule group, the Sunday + Wednesday schedules, and the never-ran/target-error alarms. REQUIRES whisper_enabled — the lantern rides the whisper webhook param; with the whisper off the lantern runs DARK (loud no-op, zero writes)."
  validation {
    condition     = !var.lantern_enabled || var.whisper_enabled
    error_message = "lantern_enabled requires whisper_enabled: the lantern has no register of its own."
  }
}

variable "lantern_schedule_expression" {
  type        = string
  default     = "cron(0 17 ? * SUN *)"
  description = "Lantern tick, America/New_York. 17:00 ET = 21:00Z under EDT / 22:00Z under EST. The slot key is the SUNDAY DATE and the bucket boundary is Sunday 21:00Z (fulfillment::lantern::tick_slot) — the tick must fire AT OR AFTER 21:00Z on its own Sunday, so the margin to the cliff is the distance to MIDNIGHT UTC: ~3h under EDT, ~2h under EST. A tick moved past 19:00 ET would land on MONDAY UTC under EST and map to the NEXT Sunday's slot (a week early, then double). Do not move the tick later without re-deriving; earlier than 17:00 ET is unsafe the other way (before 21:00Z the tick maps to the PREVIOUS Sunday)."
}

variable "lantern_heartbeat_schedule_expression" {
  type        = string
  default     = "cron(0 17 ? * WED *)"
  description = "Wednesday heartbeat: keeps AWS/Scheduler InvocationAttemptCount present at ≤4-day gaps for the never-ran alarm (7-daily-bucket hard cap), and retries an un-run or undelivered Sunday for the SAME slot (tick_slot maps Wednesday to the previous Sunday). It cannot send a delivered or quiet slot again (spec decisions B1 + F1)."
}
```

Note on `validation` referencing another variable: Terraform ≥1.9 allows cross-variable references in validation. Check `terraform version` / `required_version` in the repo; if older, drop the `validation` block and instead add a `precondition` on `aws_scheduler_schedule.lantern` in step 2 with the same condition/message.

- [ ] **Step 2: eventbridge** (append)

```hcl
# ── the lantern (spec: docs/spec-lantern.md) ─────────────────────────────────────────────────
# Its OWN schedule group (AWS/Scheduler metrics carry exactly one dimension, ScheduleGroup — a
# group shared with the whisper would mask either one's silence). Two schedules in the group:
# Sunday = the lantern, Wednesday = the heartbeat; both keep the group's metric present.
module "label_lantern" {
  source  = "bendoerr-terraform-modules/label/null"
  version = "1.0.1"
  context = module.context.shared
  name    = "lantern"
}

resource "aws_iam_role" "lantern_scheduler" {
  count                = var.lantern_enabled ? 1 : 0
  name                 = "${module.label_lantern.id}-scheduler"
  permissions_boundary = var.lambda_permissions_boundary_arn # REQUIRED: IamAppRolesSetBoundary (see whisper_scheduler)
  assume_role_policy = jsonencode({
    Version = "2012-10-17"
    Statement = [{
      Effect    = "Allow"
      Principal = { Service = "scheduler.amazonaws.com" }
      Action    = "sts:AssumeRole"
    }]
  })
  tags = module.label_lantern.tags
}

resource "aws_iam_role_policy" "lantern_scheduler_invoke" {
  count = var.lantern_enabled ? 1 : 0
  name  = "invoke-fulfillment"
  role  = aws_iam_role.lantern_scheduler[0].id
  policy = jsonencode({
    Version = "2012-10-17"
    Statement = [{
      Effect   = "Allow"
      Action   = "lambda:InvokeFunction"
      Resource = module.lambda_fulfillment.lambda_function_arn
    }]
  })
}

resource "aws_scheduler_schedule_group" "lantern" {
  count = var.lantern_enabled ? 1 : 0
  name  = module.label_lantern.id
  tags  = module.label_lantern.tags
}

resource "aws_scheduler_schedule" "lantern" {
  count                        = var.lantern_enabled ? 1 : 0
  name                         = module.label_lantern.id
  group_name                   = aws_scheduler_schedule_group.lantern[0].name
  schedule_expression          = var.lantern_schedule_expression
  schedule_expression_timezone = "America/New_York"
  flexible_time_window { mode = "OFF" }
  target {
    arn      = module.lambda_fulfillment.lambda_function_arn
    role_arn = aws_iam_role.lantern_scheduler[0].arn
    input    = jsonencode({ op = "lantern" })
  }
}

resource "aws_scheduler_schedule" "lantern_heartbeat" {
  count                        = var.lantern_enabled ? 1 : 0
  name                         = "${module.label_lantern.id}-heartbeat"
  group_name                   = aws_scheduler_schedule_group.lantern[0].name
  schedule_expression          = var.lantern_heartbeat_schedule_expression
  schedule_expression_timezone = "America/New_York"
  flexible_time_window { mode = "OFF" }
  target {
    arn      = module.lambda_fulfillment.lambda_function_arn
    role_arn = aws_iam_role.lantern_scheduler[0].arn
    input    = jsonencode({ op = "lantern_heartbeat" })
  }
}
```

- [ ] **Step 3: alarms** (append) — copies of `whisper_never_ran` / `whisper_target_errors` with `lantern` substituted: `count = var.lantern_enabled ? 1 : 0`, `alarm_name = "${module.label_lantern.id}-never-ran"` / `-target-errors`, `dimensions = { ScheduleGroup = aws_scheduler_schedule_group.lantern[0].name }`, same period/evaluation/threshold/treat_missing_data/datapoints_to_alarm values, `tags = module.label_lantern.tags`. Update the never-ran comment: "the Wednesday heartbeat keeps the metric present at ≤4-day gaps".

- [ ] **Step 4: tfvars + fmt + validate**

Add `lantern_enabled = true` to `terraform/production.tfvars` under `whisper_enabled`.
Run: `terraform -chdir=terraform fmt -check && terraform -chdir=terraform validate` (init with `-backend=false` if the backend needs creds: `terraform -chdir=terraform init -backend=false`).
Expected: fmt clean, validate OK.

- [ ] **Step 5: commit**

```bash
git add terraform/
git commit -S -m "terraform: the lantern — sunday + wednesday schedules in their own group, never-ran/target-error alarms, lantern_enabled (requires whisper)"
```

---

### Task 8: docs, follow-up issue, PR

**Files:**
- Modify: `docs/spec-lantern.md` (status line → BUILT, plan path)
- Modify: `README.md` (one line under status if the repo lists features there — check; else skip)

- [ ] **Step 1: spec status**

Change the status line to: `Status: BUILT — plan docs/superpowers/plans/2026-09-16-the-lantern.md; family sign-off at <sha>`.

- [ ] **Step 2: file the follow-up issue** (the button the chimney line names)

```bash
gh issue create -R yourcodekitten/bendobundles \
  --title "admin ops: compensate a stuck Pending claim (the action the lantern's chimney line names)" \
  --body "The lantern (docs/spec-lantern.md) tells ben a Pending claim past the 24h bar 'clears when compensated or fulfilled — no admin button for that yet'. compensate_self_claim exists in fulfillment; nothing in admin-api/web can invoke it (measured 2026-09-16). Shape: an admin-api op that invokes fulfillment with a Compensate request for a claim id, a confirm-step button on /admin/ops beside the stuck row, tests for the self-claim vs friend-claim arms. Disposition of the current #234 specimen is ben's call and is raised at the lantern's reveal."
```

- [ ] **Step 3: push, open the PR, watch CI**

```bash
git push -u origin lantern
gh pr create -R yourcodekitten/bendobundles --base main --head lantern \
  --title "the lantern 🏮 — sunday's walk through the attic: stalled intentions, on the register ben reads" \
  --body-file - <<'EOF'
spec: docs/spec-lantern.md (v4, family crossfire rounds 1–3 integrated) · plan: docs/superpowers/plans/2026-09-16-the-lantern.md

- whispers cover forgotten inventory, bells cover events; the lantern covers the NON-event: cold doors (14d/60d birthdays, stateless, slot-bucketed), claims stuck past the sweep's 24h bar (#234's READ surface, weekly with a week counter, naming the clearing action), wrapped gifts a week past their day, doors closing next week
- slot key = sunday DATE, bucket boundary sunday 21:00Z (B1/B2: an ISO-week key let the wednesday heartbeat win; a now-anchored window leaked across DST); closing looks forward (BUCKET k+1)
- heartbeat = distinct op: absent ⇒ run · delivered|quiet ⇒ nothing · undelivered ⇒ resend (at-least-once). quiet writes a row (F1)
- domain::text — ONE sanitiser + escape_md; public-api/admin-api adopt; the bell's thank-note masked-link finding (Lilith/OMBB) fixed here
- terraform: own schedule group, sunday + wednesday, never-ran + target-error alarms, lantern_enabled (requires whisper_enabled); dark-deploy safe
- deploy verification = `{"op":"lantern_preview"}` (zero writes); the message IS the reveal
EOF
ops/report-pr-status.sh yourcodekitten/bendobundles <pr>   # from ~/code-kitten — run, do not hand-compose
```

---

## Self-review (run after writing; findings fixed inline)

1. **Spec coverage** — four rooms + predicates (T5) · bucket function + DST + closing-forward fixtures (T5) · backlog line keyed on delivered, preview cannot consume (T5 compose flag + T6 preview test) · chimney week N + action + #234 (T5) · shelf voice (T5) · no bearer capability (T5 render test) · escape_md exactly once, cap-then-escape, 2000 on final, title newline flatten (T2/T3/T5) · register + LANTERN_DISABLED decoupled (T6) · idempotence LANTERN#<sunday> (T4/T6) · heartbeat 3-way + F1 (T6) · at-least-once (T6 send_and_mark) · preview zero writes (T6) · never-ran alarm own group + heartbeat cadence (T7) · UTC cliff margin on the variable (T7) · quiet logs population sizes (T6 `run_lantern`) · empty does NOT page ops (T6: no ping on quiet) · Link predicates relocated (T1) · follow-up issue (T8). **Gap found and closed:** eastern-date rendering had no task — added to T5 (`eastern_offset`, `eastern_date`, pinned across DST).
2. **Placeholder scan** — `seed_stuck_pending` in T6 hedges between two idioms; the step says to resolve to one before running (the file's `seed_aged_pending` is the known-good). `..Default::default()` in fixtures is conditional on `Default` impls; T5 step 1 names the fallback fixture sources. No TBD/TODO.
3. **Type consistency** — `record_lantern(slot: &str, quiet: bool, counts: [u32; 4])` used identically in T4/T6; `Lantern.counts: [u32; 4]` in T5 feeds it; `Slot::key()` returns the `YYYY-MM-DD` string used as the store key everywhere; `bell::field`/`cap_content` are `pub(crate)` for `lantern.rs`; `FulfillResponse::Lanterned` used in T6 tests and handlers; `lantern_resend_for_test` is `pub` + `#[doc(hidden)]` for the integration test.
