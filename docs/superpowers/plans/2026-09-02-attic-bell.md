# The Attic Bell Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Real-time warm Discord notifications to ben for the two moments that close the giving
loop — a friend unwrapping a gift, and a friend's thank-you landing.

**Architecture:** A new `FulfillRequest::Bell { event }` op in the fulfillment lambda reuses the
whisper transport (`resolve_whisper_url` dark gate + `whisper_send_body` POST) and NEVER touches
`WHISPER#` slot state. public-api fires the op **fire-and-forget** (`InvocationType::Event`, a new
`Invoker::bell` method) after each durable success — uniform for both events, zero friend-visible
latency. The Bell handler swallows its own failures (at-most-once; async-invoke retries only fire
on function error). Card builders are pure functions beside `whisper_card`, pinned by tests.

**Tech Stack:** Rust (axum public-api, lambda fulfillment), reqwest webhook POST, serde wire
shapes, existing dynamo `Store`.

**Spec:** `docs/spec-attic-bell.md` (this plan implements it; the spec's open questions ①–④ are
resolved below in Global Constraints — carried to OMBB sign-off with reasons).

## Global Constraints

- **Q① resolved (OMBB, 2026-09-02): reuse `whisper_webhook`** — his own doctrine's axis is
  *credential lifecycle*, and bell + whisper share one room, one URL, one rotation event: two
  params would store one secret twice and the un-updated copy fails silently at rotation.
  **BUT share the secret, split the OFF-SWITCH**: a `BELL_DISABLED` env toggle (loud no-op when
  set), so ben can mute per-event bells without darking the weekly whisper. No new SSM param.
- **Q② resolved: uniform public-api-side `Event` invoke for BOTH events** — an inline POST inside
  fulfillment's sync Gift path puts a webhook connect-hang in the claim's latency path (OMBB's
  bite, and the reason the spec lean flipped before it landed); lambda-freeze rules out
  post-response sends, so the separate Event invocation is the only shape where the webhook can
  be slow without the friend feeling it. ring() re-reads authoritative state, so the "invoker
  knows less" cost is nil.
- **Q③ resolved: choice claims ring the same bell** with one extra clause: `(a monthly pick,
  spent with love)`. NO remaining-pick count — it is not in hand at ring time and the rule is
  "don't add a read for it" (OMBB).
- **Q④ resolved, worded honestly (Lilith): at-most-once AGAINST HANDLER FAILURE; delivery is
  at-least-once by Lambda's async contract**, so a rare double ring stays possible with no
  function error and no idempotency key to dedupe on — priced as cheap-but-sloppy, accepted.
  The Bell handler NEVER returns a function error (a retry re-runs the whole handler and can
  double-send after a *partial success*). A send failure is `tracing::error!` **plus an ops
  `ping_msg`** (whisper cause-④'s pattern; `ping_msg` verified at `lib.rs:4632` to ride
  `deps.notify` — the OPS credential, so the report survives a dead whisper webhook). ⚠️ The ops
  register's READER is not demonstrated from this seat (OMBB's 8-sent-0-received specimen, his
  box) — the spec credits it PROVISIONALLY and deploy verification carries a Ben-confirm.
- **Mentions-deny provenance (OMBB's ⑤):** `allowed_mentions: {"parse": []}` is DOCUMENTED for
  `content`; embed behaviour is observed-not-contract. Both bell cards therefore put ALL
  friend-influenced text in `content` (the thanks card carries zero embeds), so the documented
  layer covers everything that needs covering — record this in bell.rs's comments, claim no more.
- The bell must not change ANY existing response shape, status code, or latency class on
  `/api/l/{token}/claim` or `/api/l/{token}/thanks`.
- `WHISPER#` slot state is untouchable: no `record_whisper`, no `mark_whisper_delivered`, no slot
  derivation anywhere in bell code. (Saturday 2026-W36's tick must arrive virgin.)
- The thanks bell sends the **STORED** `thank_note` (read back via `get_link` inside the Bell
  handler), never a value carried in the invoke payload.
- `allowed_mentions: {"parse": []}` on every bell body.
- No bell on admin self-claim (`Reveal`), no bell on compensated/parked/refused outcomes, no
  retroactive bells. Only public-api calls `Invoker::bell`.
- All commits GPG-signed (`-S`), authored `code kitten <yourcodekitten@gmail.com>`.
- Workspace checks used throughout: `cargo test -p fulfillment -p public-api` and
  `cargo clippy -p fulfillment -p public-api -- -D warnings` (repo toolchain; local `cargo test`
  is fine here — full suite runs on the PR).

## File Structure

- `crates/fulfillment/src/bell.rs` — NEW. Pure card builders + the `ring` orchestration. One
  responsibility: turn a BellEvent into one webhook body and send it, best-effort.
- `crates/fulfillment/src/lib.rs` — `FulfillRequest::Bell` + `FulfillResponse::Belled` variants,
  dispatch arm, `mod bell;`, and pub(crate) visibility for the two reused helpers.
- `crates/public-api/src/lib.rs` — `Invoker::bell` trait method + `LambdaInvoker` impl +
  call sites in `handle_post_claim` / `handle_post_thanks` + mock-invoker tests.
- `docs/spec-attic-bell.md` — decisions recorded (Q①–Q④ answers replace the open-questions
  section after OMBB sign-off).

---

### Task 1: Pure bell cards in `crates/fulfillment/src/bell.rs`

**Files:**
- Create: `crates/fulfillment/src/bell.rs`
- Modify: `crates/fulfillment/src/lib.rs` (add `pub mod bell;` next to `pub mod whisper;` at :16)

(The bell carries its OWN `cap` + caps rather than borrowing whisper's `trunc`: whisper's caps are
card-layout numbers tuned to ITS card and drift independently; the shared invariant is only
"content ≤ 2000", pinned by the pathological-inputs test.)

**Interfaces:**
- Produces: `pub fn unwrap_card(label: &str, game_title: &str, artwork_url: Option<&str>, site_url: &str, choice: bool) -> serde_json::Value`
- Produces: `pub fn thanks_card(label: &str, note: &str, site_url: &str) -> serde_json::Value`

- [ ] **Step 1: Write the failing tests** (in `bell.rs`'s `#[cfg(test)] mod tests`)

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unwrap_card_carries_voice_art_and_deny() {
        let v = unwrap_card("sam ♡", "Celeste", Some("https://art/1.png"),
                            "https://bendobundles.com", false);
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
        assert!(v["content"].as_str().unwrap().contains("a monthly pick, spent with love"));
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
        for v in [unwrap_card(&long, &long, None, &long, false),
                  thanks_card(&long, &long, &long)] {
            assert!(v["content"].as_str().unwrap().chars().count() <= 2000);
        }
    }
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p fulfillment bell:: 2>&1 | tail -5`
Expected: compile FAIL (`unwrap_card` not defined).

- [ ] **Step 3: Implement the cards**

```rust
//! The attic bell 🔔 — spec: docs/spec-attic-bell.md.
//! Pure card builders + best-effort ring. Shares the whisper TRANSPORT (webhook + POST helper),
//! never its SLOT state: nothing in this module may name WHISPER#, record_whisper, or a slot.

use domain::Game;

/// Discord hard cap on `content`; same bound whisper's CONTENT_MAX respects.
const BELL_CONTENT_MAX: usize = 2000;
const BELL_LABEL_MAX: usize = 120;
const BELL_TITLE_MAX: usize = 240;

fn cap(s: &str, n: usize) -> String {
    if s.chars().count() <= n { s.to_string() } else { s.chars().take(n).collect() }
}

pub fn unwrap_card(
    label: &str,
    game_title: &str,
    artwork_url: Option<&str>,
    site_url: &str,
    choice: bool,
) -> serde_json::Value {
    let spent = if choice { " (a monthly pick, spent with love)" } else { "" };
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
    // THANK_NOTE_MAX_CHARS. Mentions are denied structurally, never by scrubbing.
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
```

(If `use domain::Game;` is unused at this stage, omit it until Task 3.)

- [ ] **Step 4: Run tests to verify pass**

Run: `cargo test -p fulfillment bell:: 2>&1 | tail -5`
Expected: 4 passed.

- [ ] **Step 5: Commit**

```bash
git add crates/fulfillment/src/bell.rs crates/fulfillment/src/lib.rs
git commit -S -m "🔔 bell cards: pure builders for the unwrap and thanks moments"
```

---

### Task 2: Wire shapes — `BellEvent`, `FulfillRequest::Bell`, `FulfillResponse::Belled`

**Files:**
- Modify: `crates/fulfillment/src/lib.rs` (`FulfillRequest` enum at :116, `FulfillResponse` enum
  near :193)
- Modify: `crates/fulfillment/src/bell.rs` (the `BellEvent` type lives with its cards)

**Interfaces:**
- Produces: `#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)] pub enum BellEvent` with variants
  `Unwrap { link_token: String, game_id: String, week: String, choice: bool }` and
  `Thanks { link_token: String }` (snake_case tags to match the existing wire style — copy the
  serde attributes `FulfillRequest` itself uses; do not invent a different casing). `week` is the
  ring's ledger week, stamped by the sender (Task 5) so the unwrap/ring pair shares one bucket.
- Produces: `FulfillRequest::Bell { event: BellEvent }` and fieldless `FulfillResponse::Belled`.

- [ ] **Step 1: Write the failing wire tests** (beside the existing FulfillRequest serde tests —
  find them with `grep -n '"op"' crates/fulfillment/src/lib.rs | head` and match their style)

```rust
#[test]
fn bell_request_wire_shape_round_trips() {
    let req: FulfillRequest = serde_json::from_value(serde_json::json!({
        "op": "bell",
        "event": { "kind": "unwrap", "link_token": "t", "game_id": "g", "week": "2026-W36", "choice": true }
    })).unwrap();
    match req {
        FulfillRequest::Bell { event: bell::BellEvent::Unwrap { link_token, game_id, week, choice } } => {
            assert_eq!((link_token.as_str(), game_id.as_str(), week.as_str(), choice), ("t", "g", "2026-W36", true));
        }
        other => panic!("wrong parse: {other:?}"),
    }
    let thanks: FulfillRequest = serde_json::from_value(serde_json::json!({
        "op": "bell", "event": { "kind": "thanks", "link_token": "t" }
    })).unwrap();
    assert!(matches!(thanks,
        FulfillRequest::Bell { event: bell::BellEvent::Thanks { .. } }));
}

#[test]
fn bell_wire_cannot_contaminate_existing_ops() {
    // the whisper spec's own rule: a new op must not widen or shadow existing parses.
    let sync: FulfillRequest = serde_json::from_value(serde_json::json!({"op": "whisper"})).unwrap();
    assert!(matches!(sync, FulfillRequest::Whisper));
}
```

NOTE: adjust `"op"`/`"kind"` tag names to whatever `FulfillRequest`'s existing serde attributes
actually produce — read the enum's `#[serde(...)]` first; the test asserts the REAL wire, and the
existing `Whisper` test style is the authority.

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p fulfillment bell_request_wire 2>&1 | tail -5`
Expected: compile FAIL (`Bell` variant not defined).

- [ ] **Step 3: Add the variants**

In `bell.rs`:

```rust
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum BellEvent {
    /// `week`: the ledger week this unwrap's RING must be counted in — computed by the SENDER
    /// beside the gift response (bell::current_week()), so the unwrap/ring pair cannot straddle
    /// a week boundary across the async invoke (Lilith: at a handful of claims a week, a ±1
    /// straddle gap is indistinguishable from a real miss).
    Unwrap { link_token: String, game_id: String, week: String, #[serde(default)] choice: bool },
    Thanks { link_token: String },
}
```

In `lib.rs`, inside `FulfillRequest` (matching its existing tag style):

```rust
    /// The attic bell (spec: docs/spec-attic-bell.md): one warm webhook message for a durable
    /// friend-side moment. Fired by public-api as InvocationType::Event — NEVER RequestResponse,
    /// NEVER from a schedule. Shares the whisper transport, never WHISPER# slot state.
    Bell { event: bell::BellEvent },
```

and inside `FulfillResponse`:

```rust
    /// The bell ran (sent, dark no-op, or swallowed failure): every outcome is this variant BY
    /// DESIGN — a function error would trigger the async-invoke retry and double-send after a
    /// partial success. This buys at-most-once AGAINST HANDLER FAILURE only; Lambda async
    /// delivery is itself at-least-once, so a rare double ring remains possible and accepted.
    Belled,
```

- [ ] **Step 4: Run tests to verify pass**

Run: `cargo test -p fulfillment bell 2>&1 | tail -5`
Expected: all bell tests pass; existing wire tests untouched.

- [ ] **Step 5: Commit**

```bash
git add crates/fulfillment/src/bell.rs crates/fulfillment/src/lib.rs
git commit -S -m "🔔 bell wire: BellEvent + FulfillRequest::Bell + Belled response"
```

---

### Task 3: `bell::ring` — the best-effort orchestration + dispatch arm

**Files:**
- Modify: `crates/fulfillment/src/bell.rs` (add `ring`)
- Modify: `crates/fulfillment/src/lib.rs` (dispatch arm in the op `match`; `Deps` gains
  `pub bell_disabled: bool`; make `resolve_whisper_url` and `whisper_send_body` `pub(crate)` —
  they sit near :4690–:4706)
- Modify: `crates/fulfillment/src/main.rs` (read `BELL_DISABLED` env → `Deps.bell_disabled`,
  beside the existing `WHISPER_DISABLED` handling — copy its style; also grep for every other
  place a `Deps { .. }` literal is constructed — tests included — and add the field)

**Interfaces:**
- Consumes: `resolve_whisper_url(deps: &Deps) -> Option<String>` (the dark gate),
  `whisper_send_body(&deps.http, &url, &body) -> bool`, `ping_msg(deps, &OperatorMessage)` (the
  ops line, whisper cause-④'s pattern), `deps.store.get_link(token)`, `deps.store.get_game(id)`,
  `deps.whisper_site_url`.
- Produces: `pub async fn ring(deps: &Deps, event: &BellEvent)` — returns `()`, NEVER errors.
- Produces: `pub fn current_week() -> String` (in bell.rs) — the ISO-week string (`2026-W36`
  shape), ONE implementation for every bell-ledger write AND for public-api's event stamping
  (Task 5 calls `fulfillment::bell::current_week()`); the whisper's own slot derivation stays
  its own (it is the tick IDENTITY, coupled to the schedule by name — different meaning).
- Produces: `Deps.bell_disabled: bool` — the bell's OWN off-switch (Q①: shared secret, split
  disable flag; muting bells must not dark the weekly whisper).

- [ ] **Step 1: Write the failing dispatch test** (style-match the existing handler tests; the
  key property testable without a store: the dispatch arm exists and returns `Belled`)

```rust
#[tokio::test]
async fn bell_op_always_answers_belled_even_on_a_dark_deploy() {
    // Deps with whisper_notify dark (however the existing whisper dark-deploy test builds it —
    // find it: grep -n 'dark' crates/fulfillment/src/lib.rs | head — and reuse that fixture
    // helper VERBATIM; do not build a second Deps-fixture path).
    let deps = dark_deps_fixture().await;
    let resp = handle(&deps, FulfillRequest::Bell {
        event: bell::BellEvent::Thanks { link_token: "nope".into() },
    }).await;
    assert!(matches!(resp, FulfillResponse::Belled));
}
```

NOTE: `handle`/fixture names must be copied from the real test module — the property is
"Bell op ⇒ Belled, no panic, no error, zero writes", asserted with whatever harness the whisper
dark-deploy test already uses. If no reusable Deps fixture exists, test `ring`'s pure preconditions
instead and let the dispatch arm be covered by Task 4's end-to-end mock — but LOOK first.

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p fulfillment bell_op 2>&1 | tail -5`
Expected: compile FAIL (no dispatch arm / no `ring`).

- [ ] **Step 3: Implement `ring` + dispatch**

In `bell.rs`:

```rust
use crate::Deps;

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
        tracing::info!(outcome = "bell_disabled", "bell: BELL_DISABLED set — not ringing, by choice");
        return;
    }
    let Some(url) = crate::resolve_whisper_url(deps).await else {
        // dark deploy: same loud no-op face as the whisper — the resolve fn already logged it.
        return;
    };
    let body = match event {
        BellEvent::Unwrap { link_token, game_id, choice, .. } => {
            let label = match deps.store.get_link(link_token).await {
                Ok(Some(l)) => l.label,
                Ok(None) => { tracing::warn!(link_token, "bell: unwrap for unknown link — not ringing"); return; }
                Err(e) => { tracing::warn!(error = ?e, "bell: link read failed — not ringing"); return; }
            };
            let (title, art) = match deps.store.get_game(game_id).await {
                Ok(Some(g)) => (g.title, g.artwork_url),
                Ok(None) => { tracing::warn!(game_id, "bell: unwrap for unknown game — not ringing"); return; }
                Err(e) => { tracing::warn!(error = ?e, "bell: game read failed — not ringing"); return; }
            };
            unwrap_card(&label, &title, art.as_deref(), &deps.whisper_site_url, *choice)
        }
        BellEvent::Thanks { link_token } => {
            let link = match deps.store.get_link(link_token).await {
                Ok(Some(l)) => l,
                Ok(None) => { tracing::warn!(link_token, "bell: thanks for unknown link — not ringing"); return; }
                Err(e) => { tracing::warn!(error = ?e, "bell: link read failed — not ringing"); return; }
            };
            // the STORED note, never a payload-carried copy (spec: content & security).
            let Some(note) = link.thank_note.as_deref() else {
                tracing::warn!(link_token, "bell: thanks event but no stored note — not ringing");
                return;
            };
            thanks_card(&link.label, note, &deps.whisper_site_url)
        }
    };
    if !crate::whisper_send_body(&deps.http, &url, &body).await {
        // A WARN nobody reads is at-never-once (OMBB, ④): the miss goes to the MONITORED ops
        // channel via the same pattern as whisper cause-④. Frequency is bounded by construction
        // (claims ≤ claims_allowed; thanks write-once), so this cannot storm.
        tracing::error!(outcome = "bell_send_failed", "bell POST failed — accepted loss, no retry");
        crate::ping_msg(deps, &crate::OperatorMessage::fmt(
            "the attic bell failed to ring ({}): webhook POST failed — the moment passed unheard; no retry by design",
            &[crate::operator_message::Part::Id(match event {
                BellEvent::Unwrap { .. } => "unwrap",
                BellEvent::Thanks { .. } => "thanks",
            })],
        )).await;
    }
}
```

(`OperatorMessage`/`Part` paths must match the real module layout — `grep -n 'use.*OperatorMessage\|ping_msg(' crates/fulfillment/src/lib.rs | head` and copy an existing call site's imports.)

In `lib.rs`'s op dispatch (beside the `Whisper` arm):

```rust
FulfillRequest::Bell { event } => {
    bell::ring(deps, &event).await;
    FulfillResponse::Belled
}
```

- [ ] **Step 4: Run the fulfillment suite**

Run: `cargo test -p fulfillment 2>&1 | tail -5`
Expected: all pass (new + existing).

- [ ] **Step 5: Clippy**

Run: `cargo clippy -p fulfillment -- -D warnings 2>&1 | tail -3`
Expected: clean.

- [ ] **Step 6: Commit**

```bash
git add crates/fulfillment/src/bell.rs crates/fulfillment/src/lib.rs
git commit -S -m "🔔 bell::ring: dark-gated, best-effort, slot-state-free dispatch"
```

---

### Task 4: The bell ledger-of-rings — `BELL#` week counter + the whisper carries the count

*(Lilith's ④: a bell failing 100% is silent, and silent is what "nobody claimed anything" looks
like. The weekly whisper — already end-to-end monitored — carries "rang N times this week", which
turns an unfalsifiable silence into a number that can contradict itself. No new channel, no new
failure domain.)*

**Files:**
- Modify: `crates/dynamo/src/lib.rs` (`pub enum BellCounter { Unwraps, Rings }` + two new Store
  methods; follow the `ADD` update-expression style at :1143)
- Modify: `crates/dynamo/tests/store_test.rs` (counter round-trip test, existing
  dynamodb-local harness — tests SKIP locally without `DYNAMODB_LOCAL_URL`; CI runs them)
- Modify: `crates/fulfillment/src/bell.rs` (`ring` increments after a successful send)
- Modify: `crates/fulfillment/src/lib.rs` (both `fulfill_claim` Ok-arms `:894`/`:1451`, the
  reconcile heal-success arm in `claimed_tpk_terminal` ~`:2086`, and `handle_whisper` reads the
  count near the `whisper_card` call at ~:4940)
- Modify: `crates/fulfillment/tests/handler_test.rs` (population-enumeration asserts, Step 5)
- Modify: `crates/fulfillment/src/whisper.rs` (`whisper_card` gains a `bell_count: Option<u32>`
  parameter and one content line)

**Interfaces:**
- Produces: `Store::increment_bell_counter(&self, week: &str, field: BellCounter) -> Result<(), StoreError>`
  — atomic `ADD <field> :one` on item `pk = "BELL#<week>", sk = "COUNT"` (its OWN namespace;
  `WHISPER#` is untouchable). `pub enum BellCounter { Unwraps, Rings }` maps to attribute names
  `"unwraps"` / `"rings"` — an enum, never a caller-supplied string (no injectable attr names).
- Produces: `Store::get_bell_counts(&self, week: &str) -> Result<(u32, u32), StoreError>` —
  `(unwraps, rings)`, `(0, 0)` when the item is absent.
- Changes: `whisper_card(game, steam, site_url, cycle, slot, preview, bell_counts: Option<(u32, u32)>)`
  — `None` = counters unreadable (say nothing rather than lie a zero). TWO numbers from
  INDEPENDENT sources (OMBB's sharpening: one number stays ambiguous — "rang 0" reads exactly
  like a quiet week; "4 unwrapped, 0 rings" contradicts itself):
  - `unwraps` is incremented at BOTH `fulfill_claim` Ok-arms (`:894`/`:1451`) — every friend
    gift made durable, whatever route minted it; the reconcile heal RINGS INLINE at its success
    site so the pair balances by construction (sign-off ruling: a healed claim is a real unwrap
    and ben gets told; excluding it would delete the disagreeing population). Bell-independent:
    not gated by `BELL_DISABLED`, not touched by webhook health.
  - `rings` is incremented by `ring` after a successful send.
  Render: `(0, 0)` → `*(the attic was quiet this week)*` · otherwise →
  `🔔 *({u} unwrapped · the bell rang {r})*` — the reader sees the contradiction, the card never
  computes a verdict.

- [ ] **Step 1: Write the failing store test** (in `store_test.rs`, matching its harness style)

```rust
#[tokio::test]
async fn bell_counters_increment_independently_and_read_zero_when_absent() {
    let Some(store) = store_or_skip("bell_counters").await else { return }; // store_test.rs:30, the real skip helper (scouted)
    assert_eq!(store.get_bell_counts("2026-W36").await.unwrap(), (0, 0));
    store.increment_bell_counter("2026-W36", BellCounter::Unwraps).await.unwrap();
    store.increment_bell_counter("2026-W36", BellCounter::Unwraps).await.unwrap();
    store.increment_bell_counter("2026-W36", BellCounter::Rings).await.unwrap();
    assert_eq!(store.get_bell_counts("2026-W36").await.unwrap(), (2, 1));
    assert_eq!(store.get_bell_counts("2026-W37").await.unwrap(), (0, 0)); // week-scoped
}
```

(`store_or_skip(<test-name>)` is the file's real constructor/skip helper at `store_test.rs:30` —
verified 2026-09-02; do NOT invent a second harness path.)

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p dynamo bell_count 2>&1 | tail -5`
Expected: compile FAIL (methods not defined). (If dynamodb-local is absent the RUN would skip —
the compile failure still proves the red.)

- [ ] **Step 3: Implement the two Store methods**

```rust
    /// The bell's week ledger: two counters, INDEPENDENT sources, one item — `pk = BELL#<week>`,
    /// own namespace, never WHISPER#. Purpose is falsifiability, not accounting: the weekly
    /// whisper prints both so a dead bell contradicts the durable unwrap count instead of
    /// reading as a quiet week (spec-attic-bell, Lilith's ④ + OMBB's two-numbers sharpening).
    pub async fn increment_bell_counter(&self, week: &str, field: BellCounter) -> Result<(), StoreError> {
        let attr = match field { BellCounter::Unwraps => "unwraps", BellCounter::Rings => "rings" };
        self.client
            .update_item()
            .table_name(&self.table)
            .key("pk", AttributeValue::S(format!("BELL#{week}")))
            .key("sk", AttributeValue::S("COUNT".into()))
            .update_expression(format!("ADD {attr} :one"))   // owned: the builder takes Into<String>
            .expression_attribute_values(":one", AttributeValue::N("1".into()))
            .send()
            .await
            .map_err(box_sdk_err)?;   // match the file's real error-mapping helper
        Ok(())
    }

    pub async fn get_bell_counts(&self, week: &str) -> Result<(u32, u32), StoreError> {
        let out = self
            .client
            .get_item()
            .table_name(&self.table)
            .key("pk", AttributeValue::S(format!("BELL#{week}")))
            .key("sk", AttributeValue::S("COUNT".into()))
            .send()
            .await
            .map_err(box_sdk_err)?;
        let read = |name: &str| -> u32 {
            out.item()
                .and_then(|i| i.get(name))
                .and_then(|v| v.as_n().ok())
                .and_then(|n| n.parse::<u32>().ok())
                .unwrap_or(0)
        };
        Ok((read("unwraps"), read("rings")))
    }
```

(Key names/types and the error helper must match the file's conventions — read a neighboring
method first; if the table uses different key attribute names, copy THOSE.)

- [ ] **Step 4: Wire `ring` to increment after a successful send**

REPLACE ring's final `if !crate::whisper_send_body(...) { ... }` block WHOLE with this if/else —
the increment lives in the SUCCESS branch only (an increment pasted after the if-block would
count rings on failures, which un-falsifies the whole ledger):

```rust
    if crate::whisper_send_body(&deps.http, &url, &body).await {
        // ledger of rings, best-effort like everything here: the count exists so the weekly
        // whisper can contradict a silent bell; a failed increment is a WARN, never a failed
        // ring. UNWRAP RINGS ONLY — `rings` must be a true pair with `unwraps` (same population,
        // same week: the event CARRIES its ledger week, computed beside the gift response, so
        // the pair cannot straddle a weekly boundary across the async hop). Thanks bells are
        // deliberately uncounted: adding them makes rings ≥ unwraps normal and the suspect
        // direction unreadable.
        if let BellEvent::Unwrap { week, .. } = event {
            if let Err(e) = deps.store.increment_bell_counter(week, dynamo::BellCounter::Rings).await {
                tracing::warn!(error = ?e, week, "bell rang but the ring ledger write failed");
            }
        }
    } else {
        // A WARN nobody reads is at-never-once (OMBB, ④): the miss goes to the MONITORED ops
        // channel via the same pattern as whisper cause-④ — and the ops webhook is a DIFFERENT
        // credential, so this report survives a dead whisper webhook. Frequency is bounded by
        // construction (claims ≤ claims_allowed; thanks write-once), so this cannot storm.
        tracing::error!(outcome = "bell_send_failed", "bell POST failed — accepted loss, no retry");
        crate::ping_msg(deps, &crate::OperatorMessage::fmt(
            "the attic bell failed to ring ({}): webhook POST failed — the moment passed unheard; no retry by design",
            &[crate::operator_message::Part::Id(match event {
                BellEvent::Unwrap { .. } => "unwrap",
                BellEvent::Thanks { .. } => "thanks",
            })],
        )).await;
    }
```

- [ ] **Step 5: Wire the unwrap counter + the RECONCILE ring, `handle_whisper` and `whisper_card`**

🔴 DESIGN, settled at sign-off (OMBB's blocker + Lilith's product ruling): `unwraps` counts the
honest noun — EVERY friend gift made durable — at BOTH `fulfill_claim` `Ok(())` arms
(`lib.rs:894` and `:1451`; friend-only by construction — SelfClaim flows through
`reveal_claimed_tpk`, never `fulfill_claim`). The reconcile-healed claim is a friend who got a
gift while ben was never told: excluding it from the denominator would balance the pair by
deleting the population that disagreed (retuning a test to its own output). Instead THE HEAL
RINGS TOO — the pair balances by construction because every counted event has a ring route:
- HTTP route: public-api's Event invoke (Task 5).
- Reconcile route: an INLINE `bell::ring` at the heal's success site — a background invocation,
  so webhook latency is harmless there.
Any FUTURE gift-minting route that forgets to ring shows up as sustained `rings < unwraps` —
the counter detecting a missed-notification class, which is its job.

At both `fulfill_claim` `Ok(())` arms, immediately after the fulfill succeeds:

```rust
                        // durable unwrap ledger — the honest noun: every friend gift made
                        // durable, whatever route minted it (HTTP or reconcile heal). Bell-BLIND:
                        // not gated by BELL_DISABLED, blind to webhook health. Best-effort: a
                        // ledger miss is a WARN, never a failed gift.
                        let week = bell::current_week();
                        if let Err(e) = deps.store.increment_bell_counter(&week, dynamo::BellCounter::Unwraps).await {
                            tracing::warn!(error = ?e, week, "unwrap ledger write failed — gift unaffected");
                        }
```

At the reconcile heal's success site, REPLACE `claimed_tpk_terminal`'s combined success arm
(`lib.rs:2086`, today `FulfillResponse::GiftUrl { .. } | FulfillResponse::RevealedKey { .. } =>`)
with this split — GUARDED ARM FIRST, the split in the CODE not the prose (OMBB's blocker: pasted
unguarded, a reconciled SELF-claim would ring ben's own action AND drift `rings > unwraps` with
no unwrap, since reveal never touches `fulfill_claim`):

```rust
                match resp {
                    // the heal rings too (product ruling, sign-off 2026-09-02): a healed claim
                    // is a friend who got their gift on the bumpy path — the one case ben most
                    // wants to hear about. Guarded arm FIRST: a reconciled SELF-claim must not
                    // ring (decided non-goal). Inline is fine HERE — reconcile is a background
                    // invocation, nobody waits on the webhook. choice: true — B2 heals choice.
                    FulfillResponse::GiftUrl { .. }
                        if claim.link_token != domain::SELF_LINK_TOKEN =>
                    {
                        tracing::info!(claim_id = %claim.id, "reconcile(choice): completed a crash-between-writes claim");
                        bell::ring(deps, &bell::BellEvent::Unwrap {
                            link_token: claim.link_token.clone(),
                            game_id: claim.game_id.clone(),
                            week: bell::current_week(),
                            choice: true,
                        }).await;
                    }
                    FulfillResponse::GiftUrl { .. } | FulfillResponse::RevealedKey { .. } => {
                        tracing::info!(claim_id = %claim.id, "reconcile(choice): completed a crash-between-writes claim");
                    }
                    // ... keep the existing KeyDead and catch-all arms below unchanged
```

**POPULATION ENUMERATION, asserted not eyeballed (the sign-off flip condition):** in
`crates/fulfillment/tests/handler_test.rs`, extend one existing Gift-success test with
`assert_eq!(store.get_bell_counts(&bell::current_week()).await.unwrap().0, 1)` and one existing
reconcile-heal test with BOTH asserts REQUIRED, no fallback: `unwraps` incremented by the heal
AND the ring attempted — the harness DOES capture logs: `capture_logs()` at
`handler_test.rs:3275`, which carries its own vacuity guard (verified at sign-off; the earlier
"if the harness captures" hatch is closed — a subagent that cannot find the helper STOPS and
reports rather than shipping the ring unasserted). With the test webhook dark, assert the ring's
dark no-op face in the captured logs. The rings side of the HTTP route is pinned by Task 5's
api_test asserts (bell called iff GiftUrl). One event set, every route ringing, both directions
tested.

⚠️ NAMING (Lilith): the card says "N unwrapped" to BEN — with this design the column means
exactly that, every friend unwrap, so the noun is honest. Do not re-scope the column without
renaming it.

(Admin self-claims get NO count and NO ring — `reveal_claimed_tpk`'s path never touches
`fulfill_claim`, and the reconcile ring site explicitly skips `SELF_LINK_TOKEN`. `rings` counts
UNWRAP bells only — thanks bells would break the pairing — so `rings < unwraps` stays readable
as the suspect direction.)

In `handle_whisper`, right before the `whisper_card` call (the `slot` string is already the ISO
week): `let bell_counts = deps.store.get_bell_counts(&slot).await.ok();` and pass it as the new
final argument. In `whisper_card`, add the parameter and append to `content` before the cap:

```rust
    if let Some((u, r)) = bell_counts {
        content.push_str(&if (u, r) == (0, 0) {
            "\n*(the attic was quiet this week)*".to_string()
        } else {
            format!("\n🔔 *({u} unwrapped · the bell rang {r})*")
        });
    }
```

Update every existing `whisper_card(` call site (including `handle_whisper_preview` and every
test) — pass `None` in the preview path (a preview must not imply a live counter read) and in old
tests; add ONE new card test asserting the three renderings (None → no line, (0,0) → quiet line,
(4,0) → "4 unwrapped · the bell rang 0" — the contradiction the reader must be able to see).

- [ ] **Step 6: Run the suites**

Run: `cargo test -p dynamo -p fulfillment 2>&1 | tail -5`
Expected: pass (dynamo counter test may SKIP locally; CI is authoritative).

- [ ] **Step 7: Commit**

```bash
git add crates/dynamo/src/lib.rs crates/dynamo/tests/store_test.rs crates/fulfillment/src/bell.rs crates/fulfillment/src/lib.rs crates/fulfillment/src/whisper.rs
git commit -S -m "🔔 the ring ledger: BELL# week counter, and the whisper carries the count"
```

---

### Task 5: `Invoker::bell` — fire-and-forget from public-api

**Files:**
- Modify: `crates/public-api/src/lib.rs` (trait at :28, `LambdaInvoker` at :34–:58,
  `handle_post_claim` success arms near :899, `handle_post_thanks` `Set` arm near :1136)
- Test: `crates/public-api/tests/api_test.rs` — the real harness: `MockInvoker` at `:129`,
  57 `#[tokio::test]`s (scouted 2026-09-02; the in-src test module is only a wire-contract test —
  do NOT hunt fixtures in src/lib.rs)

**Interfaces:**
- Consumes: `FulfillRequest::Bell` / `bell::BellEvent` from Task 2.
- Produces: trait method `async fn bell(&self, req: FulfillRequest) -> Result<(), String>` —
  Event invoke, no response parse. Mock invokers gain a recording impl.

- [ ] **Step 1: Write the failing tests** (in the existing public-api test style, mock invoker)

```rust
#[tokio::test]
async fn claim_success_rings_the_unwrap_bell_and_response_is_unchanged() {
    // existing claim-success fixture + a recording mock invoker. Assert:
    // 1. response body/status identical to the pre-bell shape (byte-for-byte of the JSON keys)
    // 2. exactly one bell() call, FulfillRequest::Bell{ Unwrap{ link_token, game_id, choice }}
    //    matching the claim, choice mirroring requires_choice
}

#[tokio::test]
async fn thanks_success_rings_the_thanks_bell() {
    // existing thanks-success fixture. Assert one bell() call with Thanks{ link_token }.
}

#[tokio::test]
async fn refused_claim_and_refused_thanks_ring_nothing() {
    // revoked link claim + AlreadyThanked second post: zero bell() calls.
}

#[tokio::test]
async fn bell_invoke_failure_never_touches_the_response() {
    // mock bell() returns Err("boom"): claim response still 200 with gift_url; thanks still 200.
}
```

(The bodies must be REAL: FIRST run `grep -n 'claim\|thanks' crates/public-api/tests/api_test.rs | head -30`
and read the nearest claim-success and thanks-success tests whole; copy their fixture setup and
extend `MockInvoker` (`api_test.rs:129`) with a `Vec<FulfillRequest>` behind a Mutex — its `bell`
impl records and returns `Ok(())` by default, `Err` when the test says so. Write all four with
full setup — the pattern exists in that file. **If no claim-success fixture exists to copy, STOP
and report rather than inventing a harness.** NB: `adapter_test.rs:235` has a second impl
(`NoInvoker`) — every `impl Invoker` in tests/ must gain the new `bell` method or the crate stops
compiling; that compile error is the roster, follow it.)

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p public-api bell 2>&1 | tail -5`
Expected: compile FAIL (`bell` not on the trait).

- [ ] **Step 3: Implement**

Trait + production impl:

```rust
#[async_trait]
pub trait Invoker: Send + Sync {
    async fn gift(&self, req: FulfillRequest) -> Result<FulfillResponse, String>;
    /// Fire-and-forget bell: InvocationType::Event, 202-and-done. Callers log-and-continue on
    /// Err — a bell failure may never fail, slow, or color a friend response.
    async fn bell(&self, req: FulfillRequest) -> Result<(), String>;
}

// in impl Invoker for LambdaInvoker:
    async fn bell(&self, req: FulfillRequest) -> Result<(), String> {
        let payload = serde_json::to_vec(&req).map_err(|e| e.to_string())?;
        self.client
            .invoke()
            .function_name(&self.fn_name)
            .invocation_type(aws_sdk_lambda::types::InvocationType::Event)
            .payload(aws_sdk_lambda::primitives::Blob::new(payload))
            .send()
            .await
            .map_err(|e| format!("{e:?}"))?;
        Ok(())
    }
```

Call sites — claim success (the arm that returns the gift to the friend, near :899's
`gift_result` match; ring on the outcomes that mean "the friend got their game NOW":
`FulfillResponse::GiftUrl` and the self-serve reveal equivalent if the PUBLIC claim path has one —
read the match first; do NOT ring on AlreadyRedeemed/KeyDead/Parked/Error):

```rust
    // 🔔 after durable success, before the response: Event invoke is milliseconds and a
    // failure is a WARN — never touches the friend's moment. (Fired pre-response because a
    // frozen lambda never finishes a post-response task.) The NUMBER is picked (Lilith's ②):
    // 2s hard budget on the invoke call itself, CHOSEN AGAINST THE COLD PATH (Lilith): this app
    // is low-traffic, so the first claim after a quiet spell pays DNS+TCP+TLS to the lambda
    // endpoint with no pooled connection — a warm-measured 1s would concentrate misses on
    // exactly the claim that matters. Affordable to bound at all BECAUSE misses are counted
    // (the ring ledger) and pinged (ops) — OMBB's coupling. Warm typical ~20ms.
    let t0 = std::time::Instant::now();
    let mut outcome = "ok";
    match tokio::time::timeout(std::time::Duration::from_secs(2),
        s.invoker.bell(FulfillRequest::Bell {
            event: fulfillment::bell::BellEvent::Unwrap {
                link_token: token.clone(),
                game_id: body.game_id.clone(),
                week: fulfillment::bell::current_week(),   // stamped HERE so the unwrap/ring pair shares a week across the async hop
                choice: requires_choice,
            },
        })).await {
        Ok(Ok(())) => {}
        Ok(Err(e)) => {
            outcome = "err";
            tracing::warn!(error = %e, "unwrap bell invoke failed — gift unaffected");
        }
        Err(_) => {
            outcome = "timeout";
            tracing::warn!("unwrap bell invoke timed out (2s) — gift unaffected");
        }
    }
    // the 2s prior becomes a distribution (OMBB) — with the outcome BESIDE the duration (Lilith):
    // a killed request and a 1998ms success write the same-shaped number, so an unlabelled p99
    // reads the CAP as data — the distribution could only ever argue the budget down, never up.
    tracing::info!(bell_invoke_ms = t0.elapsed().as_millis() as u64, outcome, "bell invoke duration");
```

Thanks call site (inside the `Ok(dynamo::SetThanksOutcome::Set)` arm, after the info line):

```rust
    let t0 = std::time::Instant::now();
    let mut outcome = "ok";
    match tokio::time::timeout(std::time::Duration::from_secs(2),
        s.invoker.bell(FulfillRequest::Bell {
            event: fulfillment::bell::BellEvent::Thanks { link_token: token.clone() },
        })).await {
        Ok(Ok(())) => {}
        Ok(Err(e)) => {
            outcome = "err";
            tracing::warn!(error = %e, "thanks bell invoke failed — thanks unaffected");
        }
        Err(_) => {
            outcome = "timeout";
            tracing::warn!("thanks bell invoke timed out (2s) — thanks unaffected");
        }
    }
    tracing::info!(bell_invoke_ms = t0.elapsed().as_millis() as u64, outcome, "bell invoke duration");
```

(Exact variable names — `token`, `body.game_id`, `requires_choice` — must be read off the real
handler; the claim handler builds `FulfillRequest::Gift` from the same values, so lift them from
that construction.)

- [ ] **Step 4: Run both suites**

Run: `cargo test -p public-api -p fulfillment 2>&1 | tail -5`
Expected: all pass.

- [ ] **Step 5: Clippy**

Run: `cargo clippy -p public-api -p fulfillment -- -D warnings 2>&1 | tail -3`
Expected: clean.

- [ ] **Step 6: Commit**

```bash
git add crates/public-api/src/lib.rs
git commit -S -m "🔔 public-api rings: Event-invoke bell on claim + thanks durable success"
```

---

### Task 6: Spec close-out + IAM verification note

**Files:**
- Modify: `docs/spec-attic-bell.md` (replace "open questions" with "decisions", carrying OMBB's
  sign-off outcomes; add the deploy note below)
- Verify (read-only): `terraform/` — public-api's `lambda:InvokeFunction` grant on the
  fulfillment fn covers `InvocationType::Event` (same IAM action). Record the file:line in the
  spec's deploy note. If the grant is somehow resource-scoped in a way Event breaks (it is not —
  same action, same resource), STOP and surface before deploying.

- [ ] **Step 1: Verify the IAM grant**

Run: `grep -rn 'InvokeFunction' terraform/ terraform-iam/ | head`
Expected: the public-api role's grant naming the fulfillment function. Copy file:line into the
spec deploy note.

- [ ] **Step 2: Update the spec**

Replace the "open questions" section with a "decisions (2026-09-02, family review + OMBB
sign-off)" section recording Q①–Q④ answers as implemented (reuse webhook + split BELL_DISABLED
toggle · uniform Event invoke with a 2s budget (cold-path-chosen) · choice clause without a pick count ·
at-most-once + ops ping on miss + the whisper-carried ring count), plus these two fences:
- "**The bell is not the pick ledger.** The admin is truth for the monthly-pick budget; the bell
  may miss and must never be counted with." (Lilith's ③)
- "**Coverage split, stated:** the whisper's `unwraps · rings` line covers *bell broken, channel
  healthy* — it structurally CANNOT report a dead whisper webhook (the report dies with the
  channel). Dead-channel detection is the per-miss ops `ping_msg`, which rides the OPS webhook —
  a different credential — plus the weekly whisper's own cause-④ ping. Out-of-band at bell
  frequency; a metric+alarm remains the deferred second step, with this line as its reason."
  (Lilith's ①-walkback + OMBB's report-path rule, closed by the two-credential split.)
- "Mentions-deny provenance: `allowed_mentions` covers `content` by contract; embed behaviour is
  observed-not-contract, which is why all friend-influenced text rides `content`." (OMBB's ⑤)
- "**Reading the count line, direction stated (OMBB):** delivery is at-least-once, so `rings`
  may legally EXCEED `unwraps` — `rings > unwraps` is a benign duplicate; **`rings < unwraps` is
  the suspect direction.** A counter that cries wolf on its first legal duplicate gets ignored
  forever." `rings` counts unwrap bells ONLY (one population with `unwraps`), and each ring is
  counted in the week its EVENT carries — stamped beside the gift response — so a week-boundary
  straddle is bounded to a milliseconds window (Lilith). Residual ±1 remains possible; **only a
  sustained or multi-unit gap is signal.**
- "**The counter covers unwrap bells only:** a thanks-bell outage is invisible to the
  `unwraps · rings` line by design (thanks would break the pairing) and is visible only via the
  per-miss ops ping." (OMBB's minor, placed where the fences live.)
- "**The count line clips ~a day a week, both columns equally:** the Saturday whisper reads
  `BELL#<current week>`, so unwraps/rings landing after that read (Sat eve, Sunday) are printed
  by no card — next week's card reads its own key. The direction rule survives (both columns
  drop together); totals under-report and that is said here rather than discovered." (OMBB.)
- "**The ops register's reader is PROVISIONAL:** every detection path here is a sender; nobody
  has demonstrated a human reads the ops room from this seat. Deploy verification includes ben
  confirming one ops-register message reached eyes; until then 'detection in hours' is a
  capability claim, not a property." (OMBB's third-time shape.)
Plus: "Deploy note: no new infra. The bell rides the existing whisper webhook and
the existing public-api→fulfillment invoke grant (`<file:line>`). Dark deploy behaviour inherited:
webhook UNSET ⇒ loud no-op; `BELL_DISABLED` is the bell's own mute."

- [ ] **Step 3: Run the full workspace check once**

Run: `cargo test --workspace 2>&1 | tail -5` (local best-effort; the PR's CI is authoritative)
Expected: pass.

- [ ] **Step 4: Commit**

```bash
git add docs/spec-attic-bell.md
git commit -S -m "🔔 spec: decisions recorded, deploy note added"
```
