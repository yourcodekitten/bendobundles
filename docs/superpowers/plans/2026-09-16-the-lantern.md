# The Lantern 🏮 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A weekly Sunday-evening Discord message on the whisper register that reports stalled intentions (cold doors, stuck claims, wrapped-past-day gifts, closing doors), with stateless slot-bucketed mentions, a heartbeat that can never send a second lantern, and Markdown-safe interpolation shared with the bell.

**Architecture:** A new pure module `fulfillment::lantern` (bucket function + compose + render) driven by a handler that copies the whisper's record→send→mark skeleton onto a `LANTERN#<sunday-date>` slot row. A Wednesday `lantern_heartbeat` op keeps the never-ran alarm's metric alive and retries an undelivered/un-run Sunday, never a delivered or quiet one. Text safety (`sanitize_line` + `escape_md`) moves to `domain::text` so the bell, the lantern, public-api and admin-api share one copy.

**Tech Stack:** Rust 2024 (workspace), `time 0.3`, `serde_json`, `aws-sdk-dynamodb`, wiremock + dynamodb-local for handler tests, Terraform (EventBridge Scheduler, CloudWatch, SSM), the existing `bendoerr-terraform-modules/*`.

**Spec:** `docs/spec-lantern.md` (v5 at `290f41b`; decisions section wins over narrative).

**Branch:** `lantern` already exists on origin (spec + this plan live on it). Every task commits to it; no task creates a branch.

**Review:** cold implementation-plan-review 2026-09-16T07:2x (3 blockers, 8 majors) integrated — see the tail of this file.

## Global Constraints

- **Rust edition 2024**, workspace `time = 0.3` with `serde, formatting, parsing, macros`. No new crates.
- **Never touch `WHISPER#` state, `record_whisper`, or the whisper's slot derivation.** The lantern has its own prefix `LANTERN#`.
- **Register:** the WHISPER webhook (`deps.whisper_notify`). Off-switch: `LANTERN_DISABLED=1`, read ONLY by the lantern. Never `NOTIFY_DISABLED`, never `WHISPER_DISABLED`.
- **Dark-deploy rule:** register unresolved/disabled ⇒ loud no-op, ZERO writes.
- **Slot key = the Sunday DATE (`YYYY-MM-DD`), boundary = Sunday 21:00Z**, buckets `[Sun 21:00Z − 7d, Sun 21:00Z)`. Doors/wrapped mention on BUCKET(k); closing on BUCKET(k+1).
- **Quiet writes exactly one row (`quiet = true`) and zero sends (F1).**
- **Heartbeat: row absent AND at least one lantern row exists ⇒ full run · absent AND ZERO rows ⇒ metric only + `outcome=lantern_heartbeat_no_history` warn (enablement on Mon–Wed must not send a lantern for the Sunday before it existed) · delivered OR quiet ⇒ metric only · undelivered ⇒ resend + mark (at-least-once: a landed POST whose MARK fails resends on Wednesday — a duplicate beats a lost bucket) · undelivered that now composes EMPTY ⇒ mark quiet, never delivered.**
- **Text safety order per field: `sanitize_line` → cap → `escape_md`, exactly once; the 2000-char Discord cap applies to the FINAL escaped content.**
- **No bearer capability in any message.** Deep links are `{site}/admin/links` and `{site}/admin/ops` only.
- **Empty lantern is HEALTHY** — it must not page ops (the whisper's empty-pool ping is NOT inherited).
- **Commits:** GPG-signed, authored `code kitten <yourcodekitten@gmail.com>`; message style: lowercase, no conventional-commit prefix (repo convention: see `git log`).
- **`LANTERN_DISABLED` is NOT terraform-plumbed** — parity with `BELL_DISABLED` (measured: no `*_DISABLED` is in `aws-lambda.tf`); it is a manual env edit on the lambda for an operator mute. The spec's "plumbed" wording is corrected here.
- **Lint, CI's exact two commands, run in EVERY task's verify step:** `cargo fmt --check && cargo clippy --workspace --all-targets --all-features -- -D warnings` (`.github/workflows/ci.yml:43-44`). Run `cargo fmt` before them; never put several statements on one line in shipped code (the plan's own snippets are compressed for reading — `cargo fmt` expands them).
- **Store-backed tests (handler_test, store_test, iam_capture) run LOCALLY against moto:** `uvx --from 'moto[server]' moto_server -p 8000 &` then `DYNAMODB_LOCAL_URL=http://localhost:8000 cargo test …` (`store_or_skip` panics rather than skips when the URL is set — a forged green is impossible). CI runs the same against `amazon/dynamodb-local:2.5.2`; **CI is the authoritative run** — if the local link OOMs, push and read CI, never "assume green".
- **`terraform/production.tfvars` is gitignored** (`.gitignore:18`) — the `lantern_enabled = true` flip is a DEPLOY-checklist step, never a commit.
- **Mutes are split for real:** `lantern_notify` is resolved from the whisper's `SecretRead` with `LANTERN_DISABLED` only — `WHISPER_DISABLED` must not dark the lantern (review found the first draft coupled them through `resolve_whisper_url`). (The bell has the same coupling today — `bell::ring` calls `resolve_whisper_url` — noted in the follow-up issue, not fixed here.)
- **Ticks are 17:05 ET** (`cron(5 17 ? * SUN *)`, heartbeat `cron(5 17 ? * WED *)`): the bucket boundary is 21:00Z and a tick exactly ON the boundary has zero margin against clock skew (a 20:59:59.9 reading maps to LAST week's slot and exits `slot_taken`, which reads healthy). Margin to the UTC-midnight cliff: **2h55 EDT / 1h55 EST**.

---

## File structure

| File | Responsibility |
|---|---|
| `crates/domain/src/lib.rs` | `Link::is_open_door(now)`, `Link::waits(now)` (relocated from admin-api scrapbook); `LanternRecord` |
| `crates/domain/src/text.rs` (new) | `sanitize_line`, `escape_md` — ONE copy (`is_spoofing_format_char` ALREADY lives in `domain/src/lib.rs:654`, public-api imports it; admin-api's private copy is deleted) |
| `crates/public-api/src/lib.rs`, `crates/admin-api/src/lib.rs` | adopt `domain::text::sanitize_line` (delete private copies) |
| `crates/admin-api/src/scrapbook.rs` | adopt `Link::is_open_door` / `Link::waits` |
| `crates/fulfillment/src/bell.rs` | cards use sanitize→cap→escape via `domain::text` |
| `crates/dynamo/src/lib.rs` | `record_lantern`, `mark_lantern_delivered`, `mark_lantern_quiet`, `get_lantern`, `list_lanterns` |
| `crates/dynamo/tests/iam_capture.rs` | captures the five lantern calls; corpus + policy templates regenerated (the harness DERIVES the IAM policies from traffic) |
| `crates/fulfillment/src/lantern.rs` (new) | `Slot`, `tick_slot`, `compose`, `render`, eastern-date formatting — pure |
| `crates/fulfillment/src/lib.rs` | `FulfillRequest::{Lantern, LanternHeartbeat, LanternPreview}`, `FulfillResponse::Lanterned`, `Deps.lantern_notify`, `LanternReads`, handlers |
| `crates/fulfillment/src/main.rs` | `lantern_suppressed`, env wiring |
| `crates/fulfillment/tests/handler_test.rs` | lantern handler arms |
| `terraform/aws-eventbridge.tf`, `aws-cloudwatch-alarms.tf`, `tf-variables.tf`, `production.tfvars` | schedules, alarms, flag (no lambda env change — see Global Constraints) |

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

Run: `cargo fmt && cargo test -p domain --lib && cargo test -p admin-api --lib scrapbook && cargo fmt --check && cargo clippy --workspace --all-targets --all-features -- -D warnings`
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
- Modify: `crates/public-api/src/lib.rs:1103-1122` (delete private `sanitize_note`; KEEP `use domain::is_spoofing_format_char;` at :21)
- Modify: `crates/admin-api/src/lib.rs:1073-1118` (delete private `is_spoofing_format_char` AND `sanitize_friend_name`; adopt)
- Test: `crates/domain/src/text.rs` tests module

**Interfaces:**
- Consumes: `domain::is_spoofing_format_char(c: char) -> bool` — EXISTING at `crates/domain/src/lib.rs:654` ("MOVED VERBATIM from public-api"); do NOT write another.
- Produces: `domain::text::sanitize_line(raw: &str) -> String`, `domain::text::escape_md(s: &str) -> String`, `domain::text::MD_META: &[char]`

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

use crate::is_spoofing_format_char;

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

`is_spoofing_format_char` is `domain`'s existing `pub fn` (`lib.rs:654`, 20 ranges incl. `00AD`, `2060–206F`, `FFF9–FFFB`, `E0000–E007F`; it deliberately does NOT strip variation selectors, so `❤️` keeps its `FE0F`). The plan's first draft re-transcribed a narrower list — the review caught it. Use the existing fn; never re-declare it.

- [ ] **Step 4: run to verify it passes**

Run: `cargo test -p domain --lib text::`
Expected: PASS (3 tests)

- [ ] **Step 5: adopt in public-api and admin-api**

In `crates/public-api/src/lib.rs`: delete ONLY the private `fn sanitize_note` (:1112-1122) and its doc comment; KEEP `use domain::is_spoofing_format_char;` (:21) if anything else in the file still uses it, else drop the now-unused import (clippy `-D warnings` fails on unused imports). Add `use domain::text::sanitize_line;` and replace every `sanitize_note(` call with `sanitize_line(` (`grep -n 'sanitize_note' crates/public-api/src/lib.rs`). If any test calls `sanitize_note` directly, redirect it — the test bodies are the behavioural pin.
In `crates/admin-api/src/lib.rs`: delete BOTH the private `fn is_spoofing_format_char` (:1073, the "deliberate second copy") and `fn sanitize_friend_name` (:1105); add `use domain::text::sanitize_line;`; replace every `sanitize_friend_name(` call with `sanitize_line(`. Update `domain/src/lib.rs:652-653`'s comment ("keeps the count at two") to say the count is now ONE.

- [ ] **Step 6: run the three crates**

Run: `cargo fmt && cargo test -p domain --lib && cargo test -p public-api --lib && cargo test -p admin-api --lib && cargo fmt --check && cargo clippy --workspace --all-targets --all-features -- -D warnings`
Expected: PASS; `grep -rn 'fn is_spoofing_format_char' crates/` prints exactly ONE hit (`crates/domain/src/lib.rs`), and `grep -rn 'fn sanitize_note\|fn sanitize_friend_name' crates/` prints ZERO.

- [ ] **Step 7: commit**

```bash
git add crates/domain/src/text.rs crates/domain/src/lib.rs crates/public-api/src/lib.rs crates/admin-api/src/lib.rs
git commit -S -m "domain::text — sanitize_line + escape_md, one copy (public-api/admin-api adopt; admin-api's second is_spoofing_format_char deleted)"
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
    fn cap_content_cuts_at_2000_and_never_strands_a_backslash() {
        // thanks_card cannot reach 2000 (≈240+1000+50), so test the cap DIRECTLY where the cut
        // lands on an escape: 1999 'y' + "\\x" is 2001 chars → cap keeps 1999 'y' + '\\' → popped.
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
        assert!(c.contains("Bad # Title") || c.contains("Bad  # Title"), "{c}");
    }
```

- [ ] **Step 2: run to verify it fails**

Run: `cargo test -p fulfillment --lib bell::`
Expected: FAIL — `cannot find function field`, and `thanks_card_escapes_markdown_exactly_once` fails on the raw note.

- [ ] **Step 3: implement**

In `bell.rs`: add `use domain::text::{escape_md, sanitize_line};` at the TOP of the file (beside the existing `use crate::Deps;` block — move that block up too if it sits mid-file), then below `fn cap`:

```rust
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

Run: `cargo fmt && cargo test -p fulfillment --lib bell:: && cargo fmt --check && cargo clippy --workspace --all-targets --all-features -- -D warnings`
Expected: PASS (existing 4 + new 4). `thanks_card_quotes_the_note_and_denies_mentions` still passes — `@everyone` has no metacharacter. `field_caps_before_escaping_so_no_dangling_backslash` is a real red first: `field` does not exist.

- [ ] **Step 5: commit**

```bash
git add crates/fulfillment/src/bell.rs
git commit -S -m "bell: sanitise → cap → escape_md on every foreign field — a thank-note can no longer be a masked link"
```

---

### Task 4: `LanternRecord` + the dynamo quintet + the IAM capture corpus

**Files:**
- Modify: `crates/domain/src/lib.rs` (beside `WhisperRecord`)
- Modify: `crates/dynamo/src/lib.rs` (after `list_whispers`, ~line 3075)
- Modify: `crates/dynamo/tests/iam_capture.rs` (~line 999-1025, the whisper's fulfillment captures — add the lantern's five beside them)
- Regenerate: `terraform/iam-request-corpus.json`, `terraform/policies/dynamo-rw-fulfillment.json.tpl` (write mode, see step 6)
- Test: `crates/dynamo/tests/store_test.rs`

**Interfaces:**
- Produces:
  - `domain::LanternRecord { pub slot: String, pub delivered: bool, pub quiet: bool, pub doors: u32, pub chimney: u32, pub wrapped: u32, pub closing: u32 }`
  - `Store::record_lantern(&self, slot: &str, quiet: bool, counts: [u32; 4]) -> Result<bool, StoreError>` (Ok(false) = slot taken)
  - `Store::mark_lantern_delivered(&self, slot: &str) -> Result<(), StoreError>`
  - `Store::mark_lantern_quiet(&self, slot: &str) -> Result<(), StoreError>` (an undelivered row whose resend composes empty is settled as quiet — `delivered` must keep meaning delivered, it keys the backlog line)
  - `Store::get_lantern(&self, slot: &str) -> Result<Option<LanternRecord>, StoreError>`
  - `Store::list_lanterns(&self) -> Result<Vec<LanternRecord>, StoreError>`

- [ ] **Step 1: the failing store test** (append to `crates/dynamo/tests/store_test.rs`, copying the file's `store_or_skip`/table-setup idiom from its whisper test — `grep -n 'record_whisper' crates/dynamo/tests/store_test.rs` to find it). **Add `StoreError` to the file's `use dynamo::{…}` list at :6-10** — it is not imported today and the last assertion needs it.

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
    assert!(store.record_lantern("2026-10-11", false, [1, 0, 0, 0]).await.unwrap());
    store.mark_lantern_quiet("2026-10-11").await.unwrap();
    let settled = store.get_lantern("2026-10-11").await.unwrap().unwrap();
    assert!(settled.quiet && !settled.delivered, "settled-as-quiet is not delivered");
    assert!(store.get_lantern("2026-10-04").await.unwrap().is_none());
    let all = store.list_lanterns().await.unwrap();
    assert_eq!(all.len(), 3);
    assert!(matches!(store.mark_lantern_delivered("2026-10-04").await, Err(StoreError::Corrupt(_))));
    assert!(matches!(store.mark_lantern_quiet("2026-10-04").await, Err(StoreError::Corrupt(_))));
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

    /// Settle an undelivered row as QUIET (the heartbeat's resend composed empty — nothing was
    /// ever sent, so `delivered` stays false). Same condition as the delivered mark.
    pub async fn mark_lantern_quiet(&self, slot: &str) -> Result<(), StoreError> {
        use aws_sdk_dynamodb::types::AttributeValue as A;
        let res = self.client.update_item().table_name(&self.table)
            .key("pk", A::S(format!("LANTERN#{slot}"))).key("sk", A::S("META".into()))
            .update_expression("SET quiet = :t")
            .expression_attribute_values(":t", A::Bool(true))
            .condition_expression("attribute_exists(pk)").send().await;
        match res {
            Ok(_) => Ok(()),
            Err(sdk_err) => {
                if is_ccf_update(&sdk_err) {
                    Err(StoreError::Corrupt("mark_lantern_quiet on a slot never recorded"))
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
        // `delivered`/`quiet` are the row's MEANING — absent is Corrupt, like list_whispers' fields.
        // The four counts are diagnostics — absent/garbage reads 0 (a row written before a count
        // existed must still load). Asymmetry is deliberate and this comment is why.
        let b = |k: &str| -> Result<bool, StoreError> {
            item.get(k).and_then(|v| v.as_bool().ok()).copied()
                .ok_or(StoreError::Corrupt("lantern row missing a bool field"))
        };
        let n = |k: &str| -> u32 {
            item.get(k).and_then(|v| v.as_n().ok()).and_then(|s| s.parse().ok()).unwrap_or(0)
        };
        Ok(domain::LanternRecord {
            slot, delivered: b("delivered")?, quiet: b("quiet")?,
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

Run: `DYNAMODB_LOCAL_URL=http://localhost:8000 cargo test -p dynamo --test store_test lantern_record` (moto up per Global Constraints) then `cargo fmt --check && cargo clippy --workspace --all-targets --all-features -- -D warnings`
Expected: PASS, lint clean.

- [ ] **Step 6: capture the five calls in the IAM harness and regenerate the corpus**

`crates/dynamo/tests/iam_capture.rs` is a hand-written driver whose OUTPUT is the generated IAM policy; a store call it does not drive silently stops being described (the whisper shipped with exactly this gap, #210 review pass 1). In the fulfillment section, right after the `mark_whisper_delivered` capture (~:1023), add:

```rust
    // the lantern (handle_lantern / heartbeat / preview, fulfillment/src/lib.rs): FIVE calls on
    // LANTERN# rows — record (conditional put), mark delivered / mark quiet (conditional
    // updates), get (point read), list (filtered scan). spec: docs/spec-lantern.md
    capture(cap, &mut m, "list_lanterns", async {
        s.list_lanterns().await.unwrap();
    })
    .await;
    capture(cap, &mut m, "record_lantern", async {
        s.record_lantern("2026-09-20", false, [0, 1, 0, 0]).await.unwrap();
    })
    .await;
    capture(cap, &mut m, "get_lantern", async {
        s.get_lantern("2026-09-20").await.unwrap();
    })
    .await;
    capture(cap, &mut m, "mark_lantern_delivered", async {
        s.mark_lantern_delivered("2026-09-20").await.unwrap();
    })
    .await;
    capture(cap, &mut m, "mark_lantern_quiet", async {
        s.mark_lantern_quiet("2026-09-20").await.unwrap();
    })
    .await;
```

(Match the exact `capture(cap, &mut m, "<name>", async { … }).await;` shape of the whisper block above it — copy one and edit.) Then, with moto up:

```bash
DYNAMODB_LOCAL_URL=http://localhost:8000 IAM_CORPUS_WRITE=1 cargo test -p dynamo --test iam_capture
git diff --stat terraform/iam-request-corpus.json terraform/policies/
```

Expected: the corpus gains five `fulfillment` entries whose `leading_keys` are `["LANTERN#"]` (scan: `[]`) and whose `attributes` are `pk, sk, delivered, quiet, doors, chimney, wrapped, closing, created_at` (put) / `delivered` or `quiet` (updates); the fulfillment policy template diff is EMPTY or attribute-only — **it must not change the deny prefixes** (`SESSION#*`, `OIDCSTATE#*`). Read the diff like the IAM change it is. Then the drift gate (default mode) must be green: `DYNAMODB_LOCAL_URL=http://localhost:8000 cargo test -p dynamo --test iam_capture`.

- [ ] **Step 7: commit**

```bash
git add crates/domain/src/lib.rs crates/dynamo/src/lib.rs crates/dynamo/tests/store_test.rs crates/dynamo/tests/iam_capture.rs terraform/iam-request-corpus.json terraform/policies/
git commit -S -m "dynamo: LANTERN#<sunday> slot rows — record (once per slot, quiet-aware), mark delivered/quiet, get, list; IAM corpus captures the five calls"
```

---

### Task 5: `fulfillment::lantern` — slot buckets, compose, render (pure)

**Files:**
- Create: `crates/fulfillment/src/lantern.rs`
- Modify: `crates/fulfillment/src/lib.rs:14-17` (`pub mod lantern;`)

**Interfaces:**
- Consumes: `domain::{Link, Claim, ClaimState, Game}`, `Link::is_open_door` (Task 1), `crate::bell::{field, cap_content}` (Task 3, `pub(crate)`), `crate::RECONCILE_STUCK_ALERT_AGE: time::Duration` (existing private const at `lib.rs:113` — a child module reads it via `crate::`, no visibility change).
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
    // `Game` and `Claim` do NOT impl Default (measured) — every field is listed.
    fn game(id: &str, title: &str) -> Game {
        Game {
            id: id.into(), title: title.into(), bundle: "b".into(), gamekey: "gk".into(),
            machine_name: id.into(), key_type: "steam".into(), giftable: true, hidden: false,
            status: GameStatus::Available, claim_id: None, artwork_url: None, keyindex: 0,
            requires_choice: false, steam_app_id: None, appid_source: None, owned_by_ben: false,
            hidden_source: None, acquired_at: None,
        }
    }
    fn claim(id: &str, gid: &str, at: OffsetDateTime) -> Claim {
        Claim {
            id: id.into(), link_token: "SELF".into(), game_id: gid.into(), state: ClaimState::Pending,
            gift_url: None, revealed_key: None, created_at: at, choice_pre_tpks: None,
            failure_reason: None,
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
        assert_eq!(tick_slot(datetime!(2026-09-20 21:05 UTC)).key(), "2026-09-20", "the real EDT tick");
        assert_eq!(tick_slot(datetime!(2026-09-20 20:59:59 UTC)).key(), "2026-09-13", "before the boundary is LAST week — why the tick is 17:05 not 17:00");
        assert_eq!(tick_slot(datetime!(2026-09-23 21:05 UTC)).key(), "2026-09-20", "wednesday → previous sunday");
        assert_eq!(tick_slot(datetime!(2026-11-01 22:05 UTC)).key(), "2026-11-01", "the real EST tick, first Sunday after fall-back");
        assert_eq!(tick_slot(datetime!(2026-10-25 21:05 UTC)).next().key(), "2026-11-01", "EDT tick's next IS the EST tick's slot");
        assert_eq!(tick_slot(datetime!(2027-03-14 21:05 UTC)).key(), "2027-03-14", "first EDT tick after spring-forward");
        let s = tick_slot(SUN_TICK);
        assert_eq!(s.start(), datetime!(2026-09-13 21:00 UTC));
        assert_eq!(s.end(), datetime!(2026-09-20 21:00 UTC));
        assert!(s.contains(datetime!(2026-09-20 20:59:59 UTC)) && !s.contains(s.end()));
        assert_eq!(s.next().key(), "2026-09-27");
    }

    #[test]
    fn weekly_buckets_partition_time_exactly_including_the_dst_change_weeks() {
        // Buckets are fixed 21:00Z boundaries, so nothing here is DST-dependent BY CONSTRUCTION —
        // that is the point (B2b): the 169h/167h tick-to-tick weeks around the fall-back
        // (2026-11-01) and spring-forward (2027-03-14) still map every instant to ONE bucket.
        // Three consecutive slots cover [a−7d, b+7d); the walk stays inside [a, b+3h).
        for (a, b) in [
            (datetime!(2026-10-25 21:00 UTC), datetime!(2026-11-01 21:00 UTC)),
            (datetime!(2027-03-07 21:00 UTC), datetime!(2027-03-14 21:00 UTC)),
        ] {
            let slots = [tick_slot(a), tick_slot(b), tick_slot(b).next()];
            assert_eq!(slots[0].next(), slots[1]);
            let mut t = a;
            while t < b + time::Duration::hours(3) {
                let n = slots.iter().filter(|s| s.contains(t)).count();
                assert_eq!(n, 1, "{t} in {n} buckets");
                t += time::Duration::minutes(17);
            }
        }
    }

    #[test]
    fn chimney_bar_matches_the_sweep() {
        assert_eq!(CHIMNEY_BAR, crate::RECONCILE_STUCK_ALERT_AGE);
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
        // oldest first: `sixty` (created ~60d ago) precedes `in` (~14d ago)
        // `-` is not a Markdown metacharacter, so labels render unescaped
        assert!(doors.lines[0].contains("label-sixty") && doors.lines[0].contains("shall it stay open"), "{:?}", doors.lines);
        assert!(doors.lines[1].contains("label-in") && doors.lines[1].contains("two weeks"), "{:?}", doors.lines);
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
        assert!(l.rooms[0].lines[0].contains("2 doors older than two months"), "backlog line is FIRST: {:?}", l.rooms[0].lines);
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
        // each via link() so LABELS differ too (a clone keeps `label-lastthu`; the review caught it)
        let old = SUN_TICK - time::Duration::days(100);
        let mut last_thu = link("lastthu", old);
        last_thu.expires_at = Some(datetime!(2026-09-17 12:00 UTC)); // already closed at the tick
        let mut next_thu = link("nextthu", old);
        next_thu.expires_at = Some(datetime!(2026-09-24 12:00 UTC));
        let mut tonight = link("tonight", old);
        tonight.expires_at = Some(slot.end() + time::Duration::seconds(1));
        let mut far = link("far", old);
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
        // five chimney titles of 240 '*' each → 480 escaped chars per line → ~2,600 raw: the
        // 2000 cap is REACHED here (the review found the first draft of this test topped out ~1,550)
        let mut games = HashMap::new();
        for i in 0..5 { games.insert(format!("g{i}"), game(&format!("g{i}"), &"*".repeat(240))); }
        let friends = HashMap::new(); let links: Vec<Link> = vec![];
        let pending: Vec<Claim> = (0..5).map(|i| claim(&format!("c{i}"), &format!("g{i}"), SUN_TICK - time::Duration::days(3 + i))).collect();
        let slot = tick_slot(SUN_TICK);
        let l = compose(&input(&links, &pending, &games, &friends, SUN_TICK, true)).unwrap();
        let raw: usize = l.rooms.iter().map(|r| r.lines.iter().map(|x| x.chars().count()).sum::<usize>()).sum();
        assert!(raw > 2000, "fixture must overflow the cap to test it: {raw}");
        let v = render(&l, &slot, "https://s", false);
        let c = v["content"].as_str().unwrap();
        assert!(c.starts_with("🏮 the lantern · week of sep 13"));
        assert!(c.contains("https://s/admin/ops"));
        assert!(!c.contains("token=") && !c.contains("/l/"), "no bearer capability: {c}");
        assert_eq!(c.chars().count(), 2000);
        assert!(!c.ends_with('\\'));
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

The fixtures list every field because neither struct implements `Default` (verified 2026-09-16 at `290f41b`); if a field has been added since, the compiler names it — add it with the neutral value.

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
        // FIRST, so the room cap can never push it into "and N more" on the one tick that has it
        doors.insert(0, format!("· and {backlog} doors older than two months nobody has walked through"));
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
- `chimney_bar_matches_the_sweep` (in step 1) pins `CHIMNEY_BAR == crate::RECONCILE_STUCK_ALERT_AGE` — both `time::Duration`, no conversion.
- Deep links: ONE per room heading, not per line as the spec's mock-up shows — a deliberate deviation (five identical URLs per room is noise); the spec's mock-up is illustrative, the decisions section does not fix per-line links.
- Header reads "week of ⟨eastern date of slot.start()⟩" — the SUNDAY the bucket opened (sep 13 for the 09-20 tick), not the Monday; the spec mock-up's "sep 14" is corrected to match.
- Lines are already escaped by `field`; `render` only joins — this is the "exactly once" discipline the tests pin (single backslash).

- [ ] **Step 4: run**

Run: `cargo fmt && cargo test -p fulfillment --lib lantern:: && cargo fmt --check && cargo clippy --workspace --all-targets --all-features -- -D warnings`
Expected: PASS (14 tests), lint clean.

- [ ] **Step 5: commit**

```bash
git add crates/fulfillment/src/lantern.rs crates/fulfillment/src/lib.rs
git commit -S -m "lantern: slot buckets (sunday-date key, 21:00Z boundary), compose + render — pure, fixtures at every boundary"
```

---

### Task 6: the handlers — lantern, heartbeat, preview — and the env wiring

**Files:**
- Modify: `crates/fulfillment/src/lib.rs` (`FulfillRequest` :117-172, `FulfillResponse` :177-222, `Deps` :508-555 with `bell_disabled` at :527, dispatch ~:740, handlers after `handle_whisper` ~:4922)
- Modify: `crates/fulfillment/src/main.rs` (~line 65 `lantern_suppressed`, ~line 296 Deps build)
- Test: `crates/fulfillment/tests/handler_test.rs` (append; `deps()` builders at lines 135 and 534 gain the new field)

**Interfaces:**
- Consumes: Task 4 store quartet, Task 5 `lantern::{tick_slot, compose, render, Input, Slot}`, `resolve_whisper_url`, `whisper_send_body`, `ping_msg`.
- Produces: `FulfillRequest::{Lantern, LanternHeartbeat, LanternPreview}` (serde `snake_case` ⇒ `{"op":"lantern"}`, `{"op":"lantern_heartbeat"}`, `{"op":"lantern_preview"}`), `FulfillResponse::Lanterned`, `Deps.lantern_notify: Notify` (resolved in main.rs from the whisper's `SecretRead` + `LANTERN_DISABLED` only), `struct LanternReads`. No test seam is exported — every heartbeat arm is reachable through `handle` by forging the current slot's row.

- [ ] **Step 1: the failing handler tests** — append to `crates/fulfillment/tests/handler_test.rs`, next to the whisper arms (`deps_whisper` at ~9127):

```rust
fn deps_lantern(store: Store, humble_uri: &str, webhook: Option<String>) -> Deps {
    let mut d = deps_whisper(store, humble_uri, None, webhook.clone());
    d.lantern_notify = match webhook {
        Some(u) => fulfillment::Notify::Webhook(u),
        None => fulfillment::Notify::Disabled,
    };
    d
}

/// A Pending claim `days` old on game "gk:stuck" (title "Stardew Valley" — the helper's own
/// fixture), via the file's existing `seed_aged_pending(store, gid, token, claim_id, created)`
/// at ~line 969: it puts the game, creates link `token` (claims_used becomes 1 — NOT a door),
/// and claims it Pending at `created`. Friend-claim or self-claim is irrelevant to the chimney.
async fn seed_stuck_pending(store: &Store, days: i64) {
    seed_aged_pending(store, "gk:stuck", "stuck-link", "c-stuck",
        OffsetDateTime::now_utc() - time::Duration::days(days)).await;
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
    assert!(c.contains("stuck in the chimney") && c.contains("Stardew Valley") && c.contains("#234"));
    assert!(!c.contains("Stardew Valley\\"), "no stray escapes: {c}");
    // same slot again ⇒ loser, no second send
    assert_eq!(handle(&d, FulfillRequest::Lantern).await, FulfillResponse::Lanterned);
    assert_eq!(discord.received_requests().await.unwrap().len(), 1);
}

// The heartbeat reads the CURRENT slot (tick_slot(now)), so every arm is driven through `handle`
// by forging that slot's row first — no test seam in the lib.
#[tokio::test]
async fn lantern_heartbeat_absent_runs_then_delivered_does_nothing() {
    let Some(store) = store_or_skip("lantern_hb_absent").await else { return };
    let humble = MockServer::start().await;
    let discord = discord_ok().await;
    seed_stuck_pending(&store, 3).await;
    let d = deps_lantern(store.clone(), &humble.uri(), Some(discord.uri()));
    let slot = fulfillment::lantern::tick_slot(OffsetDateTime::now_utc()).key();
    // history exists ⇒ "absent" means Sunday FAILED, not "didn't exist yet"
    store.record_lantern("2000-01-02", true, [0, 0, 0, 0]).await.unwrap();
    assert_eq!(handle(&d, FulfillRequest::LanternHeartbeat).await, FulfillResponse::Lanterned);
    assert_eq!(discord.received_requests().await.unwrap().len(), 1, "absent (with history) ⇒ full run");
    assert!(store.get_lantern(&slot).await.unwrap().unwrap().delivered);
    assert_eq!(handle(&d, FulfillRequest::LanternHeartbeat).await, FulfillResponse::Lanterned);
    assert_eq!(discord.received_requests().await.unwrap().len(), 1, "delivered ⇒ nothing");
}

#[tokio::test]
async fn lantern_heartbeat_resends_an_undelivered_slot_and_marks_it() {
    let Some(store) = store_or_skip("lantern_hb_undelivered").await else { return };
    let humble = MockServer::start().await;
    let discord = discord_ok().await;
    seed_stuck_pending(&store, 3).await;
    let d = deps_lantern(store.clone(), &humble.uri(), Some(discord.uri()));
    let slot = fulfillment::lantern::tick_slot(OffsetDateTime::now_utc()).key();
    // forge Sunday's "sent-but-mark-failed" (or POST-failed) state: recorded, undelivered
    assert!(store.record_lantern(&slot, false, [0, 1, 0, 0]).await.unwrap());
    assert_eq!(handle(&d, FulfillRequest::LanternHeartbeat).await, FulfillResponse::Lanterned);
    assert_eq!(discord.received_requests().await.unwrap().len(), 1, "undelivered ⇒ resend");
    assert!(store.get_lantern(&slot).await.unwrap().unwrap().delivered, "…and mark");
    assert_eq!(handle(&d, FulfillRequest::LanternHeartbeat).await, FulfillResponse::Lanterned);
    assert_eq!(discord.received_requests().await.unwrap().len(), 1, "now settled");
}

#[tokio::test]
async fn lantern_heartbeat_with_no_history_sends_nothing() {
    // enabled on a Monday–Wednesday: the first heartbeat finds no row for the Sunday BEFORE the
    // lantern existed. "Absent" means "didn't exist yet", not "failed" — metric only.
    let Some(store) = store_or_skip("lantern_hb_nohistory").await else { return };
    let humble = MockServer::start().await;
    let discord = discord_ok().await;
    seed_stuck_pending(&store, 3).await; // there IS something it could say
    let d = deps_lantern(store.clone(), &humble.uri(), Some(discord.uri()));
    assert_eq!(handle(&d, FulfillRequest::LanternHeartbeat).await, FulfillResponse::Lanterned);
    assert_eq!(discord.received_requests().await.unwrap().len(), 0);
    assert!(store.list_lanterns().await.unwrap().is_empty(), "no history ⇒ zero writes");
}

#[tokio::test]
async fn lantern_heartbeat_settles_an_undelivered_slot_that_composes_empty_as_quiet() {
    let Some(store) = store_or_skip("lantern_hb_empty_resend").await else { return };
    let humble = MockServer::start().await;
    let discord = discord_ok().await;
    // history exists (a delivered old slot) so the no-history arm is not the one firing
    store.record_lantern("2000-01-02", false, [0, 1, 0, 0]).await.unwrap();
    store.mark_lantern_delivered("2000-01-02").await.unwrap();
    let d = deps_lantern(store.clone(), &humble.uri(), Some(discord.uri()));
    let slot = fulfillment::lantern::tick_slot(OffsetDateTime::now_utc()).key();
    store.record_lantern(&slot, false, [0, 1, 0, 0]).await.unwrap(); // undelivered, but nothing stuck now
    assert_eq!(handle(&d, FulfillRequest::LanternHeartbeat).await, FulfillResponse::Lanterned);
    assert_eq!(discord.received_requests().await.unwrap().len(), 0);
    let r = store.get_lantern(&slot).await.unwrap().unwrap();
    assert!(r.quiet && !r.delivered, "settled as quiet, never as delivered");
}

#[tokio::test]
async fn lantern_heartbeat_leaves_a_quiet_slot_alone() {
    let Some(store) = store_or_skip("lantern_hb_quiet").await else { return };
    let humble = MockServer::start().await;
    let discord = discord_ok().await;
    seed_stuck_pending(&store, 3).await; // there IS something to say — the quiet row must still win
    let d = deps_lantern(store.clone(), &humble.uri(), Some(discord.uri()));
    let slot = fulfillment::lantern::tick_slot(OffsetDateTime::now_utc()).key();
    assert!(store.record_lantern(&slot, true, [0, 0, 0, 0]).await.unwrap());
    assert_eq!(handle(&d, FulfillRequest::LanternHeartbeat).await, FulfillResponse::Lanterned);
    assert_eq!(discord.received_requests().await.unwrap().len(), 0, "quiet ⇒ nothing (F1)");
    assert!(!store.get_lantern(&slot).await.unwrap().unwrap().delivered);
}

#[tokio::test]
async fn lantern_preview_writes_nothing_and_keeps_the_backlog_line() {
    let Some(store) = store_or_skip("lantern_preview").await else { return };
    let humble = MockServer::start().await;
    let discord = discord_ok().await;
    // an old open door ⇒ backlog line while nothing delivered
    let mut l = link("old-door");
    l.created_at = OffsetDateTime::now_utc() - time::Duration::days(90);
    store.create_link(&l).await.unwrap();
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
async fn lantern_mute_is_its_own_and_the_whisper_mute_does_not_reach_it() {
    let Some(store) = store_or_skip("lantern_disabled").await else { return };
    let humble = MockServer::start().await;
    let discord = discord_ok().await;
    seed_stuck_pending(&store, 3).await;
    let mut d = deps_lantern(store.clone(), &humble.uri(), Some(discord.uri()));
    // lantern muted ⇒ dark: zero writes, zero sends
    d.lantern_notify = fulfillment::Notify::Disabled;
    assert_eq!(handle(&d, FulfillRequest::Lantern).await, FulfillResponse::Lanterned);
    assert!(store.list_lanterns().await.unwrap().is_empty());
    assert_eq!(discord.received_requests().await.unwrap().len(), 0);
    // WHISPER muted, lantern not ⇒ the lantern still lights (the review found the first draft
    // routed the lantern through resolve_whisper_url, which made WHISPER_DISABLED dark it too)
    d.lantern_notify = fulfillment::Notify::Webhook(discord.uri());
    d.whisper_notify = fulfillment::Notify::Disabled;
    d.bell_disabled = true;
    assert_eq!(handle(&d, FulfillRequest::Lantern).await, FulfillResponse::Lanterned);
    assert_eq!(discord.received_requests().await.unwrap().len(), 1);
    assert_eq!(store.list_lanterns().await.unwrap().len(), 1);
}
```

Helpers used are the file's own, verified at `290f41b`: `link(token)` (~116, label "dave", 1 slot, created now), `seed_aged_pending(store, gid, token, claim_id, created)` (~969), `discord_ok()` (~1009), `deps_whisper(...)` (~9127); store methods `create_link(&Link)` (dynamo ~831), `put_game(&Game)` (~679), `list_lanterns`/`get_lantern`/`record_lantern` from Task 4.

- [ ] **Step 2: run to verify it fails**

Run: `cargo test -p fulfillment --test handler_test lantern_`
Expected: FAIL to compile — `FulfillRequest::Lantern` does not exist, `Deps` has no `lantern_notify`.

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
    /// The LANTERN register: the whisper's CREDENTIAL (same SecretRead, one rotation event)
    /// resolved with the lantern's OWN flag, `LANTERN_DISABLED` — so `WHISPER_DISABLED` cannot
    /// dark the lantern and vice versa. A split `bool` beside `whisper_notify` (the bell's shape)
    /// is NOT enough: `resolve_whisper_url` reads a Notify that was resolved with the whisper's
    /// flag, so routing through it re-couples the mutes (plan review 2026-09-16).
    pub lantern_notify: Notify,
```
- dispatch (`handle`): add
```rust
        FulfillRequest::Lantern => handle_lantern(deps).await,
        FulfillRequest::LanternHeartbeat => handle_lantern_heartbeat(deps).await,
        FulfillRequest::LanternPreview => handle_lantern_preview(deps).await,
```

- [ ] **Step 4: the handlers** — after `handle_whisper`:

```rust
/// Everything compose needs, read in one place. A named struct, not a tuple — clippy's
/// `type_complexity` and the next reader both prefer it.
struct LanternReads {
    links: Vec<Link>,
    pending: Vec<Claim>,
    games: std::collections::HashMap<String, Game>,
    friends: std::collections::HashMap<String, String>,
    lanterns: Vec<domain::LanternRecord>,
}

/// `None` ⇒ already logged; caller exits (next tick retries).
async fn lantern_reads(deps: &Deps) -> Option<LanternReads> {
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
    Some(LanternReads { links, pending, games, friends, lanterns })
}

/// The lantern gate on ITS OWN Notify (never `resolve_whisper_url` — that one carries the
/// whisper's mute). Same three faces as the whisper's gate; the dark advice names the whisper
/// param because that IS the credential the lantern rides. Match all three; never let-else.
async fn resolve_lantern_url(deps: &Deps) -> Option<String> {
    match &deps.lantern_notify {
        Notify::Webhook(u) => Some(u.clone()),
        Notify::Disabled => {
            tracing::warn!(outcome = "lantern_dark", "lantern register unconfigured or LANTERN_DISABLED — no-op, zero writes");
            ping_msg(deps, &OperatorMessage::fmt(
                "the lantern is DARK — it rides the whisper webhook ({}); light that param, or unset LANTERN_DISABLED",
                &[Part::Id(&deps.whisper_param_name)],
            )).await;
            None
        }
        Notify::Unresolved => {
            tracing::error!(outcome = "lantern_unresolved", "lantern register configured but UNREADABLE — no-op, zero writes");
            ping_msg(deps, &OperatorMessage::fmt(
                "the lantern's webhook {} is configured but UNREADABLE — check ssm:GetParameter and the KMS grant. Do NOT overwrite the value.",
                &[Part::Id(&deps.whisper_param_name)],
            )).await;
            None
        }
    }
}

async fn handle_lantern(deps: &Deps) -> FulfillResponse {
    let slot = lantern::tick_slot(OffsetDateTime::now_utc());
    run_lantern(deps, &slot).await;
    FulfillResponse::Lanterned
}

/// RECORD → SEND → MARK for one slot. Returns whether a message was sent.
async fn run_lantern(deps: &Deps, slot: &lantern::Slot) -> bool {
    let Some(url) = resolve_lantern_url(deps).await else { return false };
    let Some(r) = lantern_reads(deps).await else { return false };
    let now = OffsetDateTime::now_utc();
    let any_delivered = r.lanterns.iter().any(|x| x.delivered);
    let input = lantern::Input { links: &r.links, pending: &r.pending, games: &r.games, friends: &r.friends, slot: slot.clone(), now, any_delivered };
    let Some(card) = lantern::compose(&input) else {
        tracing::info!(outcome = "lantern_quiet", slot = %slot.key(), links = r.links.len(), pending = r.pending.len(),
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

/// Wednesday: absent+history ⇒ full run · absent+NO history ⇒ metric only (the lantern did not
/// exist last Sunday — enabling on Mon–Wed must not send for a Sunday before enablement) ·
/// delivered|quiet ⇒ exit · undelivered ⇒ resend + mark.
/// ⚠️ Residual, stated: with ZERO history a Sunday schedule that never fires is masked by its
/// own heartbeat (the metric stays present). The warn outcome below is the only tell, plus the
/// deploy checklist's "watch the first real Sunday tick".
async fn handle_lantern_heartbeat(deps: &Deps) -> FulfillResponse {
    let slot = lantern::tick_slot(OffsetDateTime::now_utc());
    match deps.store.get_lantern(&slot.key()).await {
        Ok(None) => {
            let history = match deps.store.list_lanterns().await {
                Ok(v) => !v.is_empty(),
                Err(e) => { tracing::error!(error = ?e, outcome = "lantern_read_failed", "heartbeat: cannot list lantern log"); return FulfillResponse::Lanterned; }
            };
            if history {
                tracing::warn!(slot = %slot.key(), "heartbeat: Sunday never recorded — running the lantern for its slot");
                run_lantern(deps, &slot).await;
            } else {
                tracing::warn!(outcome = "lantern_heartbeat_no_history", slot = %slot.key(), "heartbeat: no lantern has ever run — not running one for a Sunday before enablement; metric touched only");
            }
        }
        Ok(Some(r)) if r.delivered || r.quiet => tracing::info!(slot = %slot.key(), quiet = r.quiet, "heartbeat: slot settled — metric touched, nothing to do"),
        Ok(Some(_)) => { resend_undelivered(deps, &slot).await; }
        Err(e) => tracing::error!(error = ?e, outcome = "lantern_read_failed", "heartbeat: cannot read slot row"),
    }
    FulfillResponse::Lanterned
}

/// Recompose the SAME slot (same buckets, current liveness) and send; no RECORD (the row exists).
async fn resend_undelivered(deps: &Deps, slot: &lantern::Slot) -> bool {
    let Some(url) = resolve_lantern_url(deps).await else { return false };
    let Some(r) = lantern_reads(deps).await else { return false };
    let any_delivered = r.lanterns.iter().any(|x| x.delivered);
    let input = lantern::Input { links: &r.links, pending: &r.pending, games: &r.games, friends: &r.friends, slot: slot.clone(), now: OffsetDateTime::now_utc(), any_delivered };
    match lantern::compose(&input) {
        Some(card) => { tracing::warn!(slot = %slot.key(), "heartbeat: resending an undelivered lantern (at-least-once)"); send_and_mark(deps, &url, slot, &card).await }
        None => {
            // nothing was ever sent for this slot ⇒ settle it as QUIET; `delivered` keeps meaning delivered
            tracing::info!(outcome = "lantern_quiet", slot = %slot.key(), "heartbeat: undelivered slot now composes empty — settled as quiet");
            if let Err(e) = deps.store.mark_lantern_quiet(&slot.key()).await { tracing::error!(error = ?e, slot = %slot.key(), "heartbeat: mark quiet failed"); }
            false
        }
    }
}

/// Zero writes: compose against live data, POST with the preview header.
async fn handle_lantern_preview(deps: &Deps) -> FulfillResponse {
    let Some(url) = resolve_lantern_url(deps).await else { return FulfillResponse::PreviewBlocked };
    let Some(r) = lantern_reads(deps).await else { return FulfillResponse::PreviewBlocked };
    let now = OffsetDateTime::now_utc();
    let slot = lantern::tick_slot(now);
    let any_delivered = r.lanterns.iter().any(|x| x.delivered);
    let undelivered = r.lanterns.iter().filter(|x| !x.delivered && !x.quiet).count();
    tracing::info!(slot = %slot.key(), rows = r.lanterns.len(), undelivered, "lantern_preview: log state");
    let input = lantern::Input { links: &r.links, pending: &r.pending, games: &r.games, friends: &r.friends, slot, now, any_delivered };
    let Some(card) = lantern::compose(&input) else {
        tracing::warn!(outcome = "lantern_preview_quiet", "lantern_preview: this week would be quiet — nothing to show");
        return FulfillResponse::PreviewBlocked;
    };
    let body = lantern::render(&card, &input.slot, &deps.whisper_site_url, true);
    if whisper_send_body(&deps.http, &url, &body).await { FulfillResponse::PreviewSent } else { FulfillResponse::PreviewSendFailed }
}
```

Imports: `Link`, `Claim`, `Game` from `domain` are likely already imported at the top of lib.rs; add what is missing.

- [ ] **Step 5: main.rs**

Beside `bell_suppressed`:
```rust
/// Is the LANTERN suppressed? Reads `LANTERN_DISABLED` and ONLY that — same register-decoupling
/// rule as the bell: shared credential, split mute.
fn lantern_suppressed(env: impl Fn(&str) -> Option<String>) -> bool {
    env("LANTERN_DISABLED").as_deref() == Some("1")
}
```
After `let whisper_notify = Notify::resolve(whisper_read, whisper_disabled);` (:148) — `SecretRead` is `Clone` (`lib.rs:429`), so clone the read BEFORE it is moved: change that line to `Notify::resolve(whisper_read.clone(), whisper_disabled)` and add
```rust
    // The LANTERN register: the SAME credential read, its OWN flag. Resolving a second Notify
    // (rather than a bool beside whisper_notify) is what keeps WHISPER_DISABLED from reaching it.
    let lantern_disabled = lantern_suppressed(|k| std::env::var(k).ok());
    let lantern_notify = Notify::resolve(whisper_read, lantern_disabled);
```
then clone it into the closure like `whisper_notify` (:178) and put `lantern_notify,` in the `Deps { … }` build (:289-304) after `bell_disabled`. main.rs pins the flag NAMES by test for `whisper_suppressed`/`bell_suppressed` (`grep -n 'suppressed' crates/fulfillment/src/main.rs` for the test names) — add the same two-arm shape for `lantern_suppressed`: `LANTERN_DISABLED=1` ⇒ true; `BELL_DISABLED=1`/`WHISPER_DISABLED=1`/`NOTIFY_DISABLED=1` ⇒ false.

Add `lantern_notify: fulfillment::Notify::Disabled,` to BOTH `Deps` builders in `handler_test.rs` (lines ~150 and ~558) and to any other `Deps {` literal in `crates/fulfillment` (`grep -rn 'bell_disabled:' crates/fulfillment/`).

- [ ] **Step 6: run**

Run: `cargo fmt && cargo test -p fulfillment --lib && DYNAMODB_LOCAL_URL=http://localhost:8000 cargo test -p fulfillment --test handler_test lantern_ && cargo fmt --check && cargo clippy --workspace --all-targets --all-features -- -D warnings`
Expected: PASS against moto; **push and read CI before claiming green** (the full workspace link may OOM locally).

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
  default     = "cron(5 17 ? * SUN *)"
  description = "Lantern tick, America/New_York. 17:05 ET = 21:05Z under EDT / 22:05Z under EST. The slot key is the SUNDAY DATE and the bucket boundary is Sunday 21:00Z (fulfillment::lantern::tick_slot): the tick must fire AFTER 21:00Z on its own Sunday, and 17:00 sharp would sit ON the boundary with zero margin against clock skew (a 20:59:59.9 reading maps to LAST week's slot and exits slot_taken, which reads healthy) — hence :05. Margin to the UTC-midnight cliff: 2h55 under EDT, 1h55 under EST. A tick moved past 19:00 ET lands on MONDAY UTC under EST and maps to the NEXT Sunday's slot (a week early, then double). Do not move the tick without re-deriving both edges."
}

variable "lantern_heartbeat_schedule_expression" {
  type        = string
  default     = "cron(5 17 ? * WED *)"
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

- [ ] **Step 4: fmt + validate (NO tfvars commit)**

`terraform/production.tfvars` is gitignored (`.gitignore:18`) — a `lantern_enabled = true` line there is a DEPLOY step (Task 8's checklist), not a commit; adding it now and `git add`-ing would silently drop it.
Run: `terraform -chdir=terraform fmt -check && terraform -chdir=terraform validate` (init with `-backend=false` if the backend needs creds: `terraform -chdir=terraform init -backend=false`).
Expected: fmt clean, validate OK.

- [ ] **Step 5: commit**

```bash
git add terraform/tf-variables.tf terraform/aws-eventbridge.tf terraform/aws-cloudwatch-alarms.tf
git commit -S -m "terraform: the lantern — sunday 17:05 + wednesday 17:05 schedules in their own group, never-ran/target-error alarms, lantern_enabled (requires whisper)"
```

---

### Task 8: docs, follow-up issue, PR

**Files:**
- Modify: `docs/spec-lantern.md` (status line → BUILT, plan path)
- (README has no feature list — measured; nothing to do there.)

- [ ] **Step 1: spec status**

Change the status line to: `Status: BUILT — plan docs/superpowers/plans/2026-09-16-the-lantern.md; OMBB plan sign-off at the sha his sign-off message names` (fill the sha from the room message; it is recorded in the PR body too). Also correct the spec's mechanism bullet that says `LANTERN_DISABLED` is "plumbed" — it is a manual env edit, parity with `BELL_DISABLED`.

- [ ] **Step 2: write the deploy checklist into the spec** (a section `## deploy checklist`): ① `lantern_enabled = true` added to the LOCAL, gitignored `terraform/production.tfvars` ② `terraform plan` shows exactly: 1 schedule group, 2 schedules, 1 role + 1 policy, 2 alarms — nothing else ③ apply ④ `aws lambda invoke --payload '{"op":"lantern_preview"}'` ⇒ `preview_sent`, message in the channel with the `(preview` header, `list_lanterns` still empty (read via the preview's own log line `rows=0`) ⑤ OWED on the checkpoint: watch the first real tick Sun 2026-09-20 17:05 ET — expect one message + one delivered row; the heartbeat's no-history arm makes a dead Sunday schedule invisible until then.

- [ ] **Step 3: file the follow-up issue** (the button the chimney line names — and the bell's shared mute)

```bash
gh issue create -R yourcodekitten/bendobundles \
  --title "admin ops: compensate a stuck Pending claim (the action the lantern's chimney line names)" \
  --body "The lantern (docs/spec-lantern.md) tells ben a Pending claim past the 24h bar 'clears when compensated or fulfilled — no admin button for that yet'. compensate_self_claim exists in fulfillment; nothing in admin-api/web can invoke it (measured 2026-09-16). Shape: an admin-api op that invokes fulfillment with a Compensate request for a claim id, a confirm-step button on /admin/ops beside the stuck row, tests for the self-claim vs friend-claim arms. Disposition of the current #234 specimen is ben's call and is raised at the lantern's reveal.

Also from the lantern's plan review, same register, different bug: bell::ring gates on bell_disabled and then calls resolve_whisper_url, whose Notify was resolved with WHISPER_DISABLED — so WHISPER_DISABLED darks the bell too, contrary to spec-attic-bell Q①. The lantern resolves its own Notify from the same SecretRead with its own flag; the bell should adopt that shape (bell_notify) in its own PR."
```

- [ ] **Step 4: push, open the PR, watch CI**

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
2. **Placeholder scan** — fixtures list every struct field (no `Default` impls exist); `seed_stuck_pending` delegates to the file's `seed_aged_pending` with its real 5-arg signature. No TBD/TODO.
3. **Type consistency** — `record_lantern(slot: &str, quiet: bool, counts: [u32; 4])` used identically in T4/T6; `Lantern.counts: [u32; 4]` in T5 feeds it; `Slot::key()` returns the `YYYY-MM-DD` string used as the store key everywhere; `bell::field`/`cap_content` are `pub(crate)` for `lantern.rs`; `FulfillResponse::Lanterned` used in T6 tests and handlers; no test seam exported.

## Cold review 2026-09-16T07:2x — integrated (verdict was "not ready"; all blockers + majors fixed here)

- **B1** T2 re-declared `is_spoofing_format_char` with a NARROWER transcribed range list; the real one is `domain::is_spoofing_format_char` (`lib.rs:654`, 20 ranges, keeps variation selectors) and public-api already imports it. Fixed: text.rs consumes it; admin-api's deliberate second copy is deleted (count 2 → 1).
- **B2** the DST bucket test listed `Slot(11-01)` twice (`tick_slot(a).next() == tick_slot(b)`) so every instant counted 2. Fixed: three distinct consecutive slots; renamed to say the partition is UTC-fixed by construction.
- **B3** `seed_stuck_pending` called `seed_aged_pending` with a 4-arg signature that does not exist and asserted a title the helper never writes. Fixed: real 5-arg call, "Stardew Valley", `create_link`.
- **M1** doors test asserted `lines[0]` = the 14d door; compose sorts oldest-first so it is the 60d one. Fixed: order asserted explicitly. **M2** `chimney_bar_matches_the_sweep` had no code. Fixed. **M3** `..Default::default()` on structs with no `Default`. Fixed: full field lists. **M4** the 2000-cap card test could not reach 2000 (fake). Fixed: `cap_content` tested directly at the cut-on-backslash case + a card path that does reach it. **M5** the heartbeat's undelivered arm ran only through a `pub` test seam on a 1999 slot. Fixed: three tests through `handle` by forging the CURRENT slot's row; seam deleted. **M6** the disabled test asserted a struct literal. Fixed: vice-versa arm through `handle`. **M7** no task created the branch — it already exists (stated in the header). **M8** `StoreError` not imported in store_test — stated.
- **OMBB plan gate round 1 (07:42, 4 parts) — integrated:** P1 `-` is not escaped (asserts were wrong) · P2 closing clones kept `label-lastthu` (each link built via `link()`) · P3 the cap test string was exactly 2000 (1999 now) · CI's exact fmt/clippy lines in every verify step · `LanternReads` struct (type_complexity) · tfvars gitignored → deploy checklist · render cap test now overflows (five 240-`*` chimney titles) · heartbeat no-history arm (+test, residual stated) · empty resend settles QUIET via new `mark_lantern_quiet` (+test) · ticks 17:05 (margins 2h55/1h55) · real EST 22:05Z tick asserted · **split mute made real: `lantern_notify` resolved from the shared SecretRead with `LANTERN_DISABLED` only** (bell's identical coupling → follow-up issue) · backlog line FIRST · header = the Sunday the bucket opened · iam_capture captures the five calls + corpus regenerated against moto.
- **Open questions answered:** ① `LANTERN_DISABLED` is NOT tf-plumbed — bell parity, now in Global Constraints and the spec. ② EST 21–22Z closing drop is ratified by OMBB round 2 ("harmless — the door is already closed"); the tick stays 17:00 ET. ③ `lantern_from_item`: bools Corrupt-on-absent (meaning), counts 0-on-absent (diagnostics) — asymmetry made deliberate and commented.
