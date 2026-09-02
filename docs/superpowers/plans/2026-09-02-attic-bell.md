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
- **Q④ resolved: at-most-once, and misses reach the MONITORED channel.** The Bell handler NEVER
  returns a function error (an Event retry re-runs the whole handler and can double-send after a
  *partial success* — a webhook POST has no idempotency key). A send failure is `tracing::error!`
  **plus an ops `ping_msg`** (whisper cause-④'s own pattern): a WARN nobody reads is
  at-never-once (OMBB's 8-sent-0-received specimen, his box).
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
- Modify: `crates/fulfillment/src/whisper.rs` (make `trunc`, `CONTENT_MAX`, `EMBED_TITLE_MAX`
  `pub(crate)` if not already — reuse, never re-implement caps)

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
    }

    #[test]
    fn unwrap_card_choice_says_so_and_artless_has_no_thumbnail() {
        let v = unwrap_card("sam", "Celeste", None, "https://s", true);
        assert!(v["content"].as_str().unwrap().contains("a monthly pick, spent with love"));
        assert!(v["embeds"][0].get("thumbnail").is_none());
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
    let mut embed = serde_json::json!({});
    if let Some(url) = artwork_url {
        embed["thumbnail"] = serde_json::json!({ "url": url });
    }
    serde_json::json!({
        "content": cap(&content, BELL_CONTENT_MAX),
        "embeds": [embed],
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
git add crates/fulfillment/src/bell.rs crates/fulfillment/src/lib.rs crates/fulfillment/src/whisper.rs
git commit -S -m "🔔 bell cards: pure builders for the unwrap and thanks moments"
```

---

### Task 2: Wire shapes — `BellEvent`, `FulfillRequest::Bell`, `FulfillResponse::Belled`

**Files:**
- Modify: `crates/fulfillment/src/lib.rs` (`FulfillRequest` enum at :116, `FulfillResponse` enum
  near :193)
- Modify: `crates/fulfillment/src/bell.rs` (the `BellEvent` type lives with its cards)

**Interfaces:**
- Produces: `#[derive(Debug, Clone, Serialize, Deserialize)] pub enum BellEvent` with variants
  `Unwrap { link_token: String, game_id: String, choice: bool }` and
  `Thanks { link_token: String }` (snake_case tags to match the existing wire style — copy the
  serde attributes `FulfillRequest` itself uses; do not invent a different casing).
- Produces: `FulfillRequest::Bell { event: BellEvent }` and fieldless `FulfillResponse::Belled`.

- [ ] **Step 1: Write the failing wire tests** (beside the existing FulfillRequest serde tests —
  find them with `grep -n '"op"' crates/fulfillment/src/lib.rs | head` and match their style)

```rust
#[test]
fn bell_request_wire_shape_round_trips() {
    let req: FulfillRequest = serde_json::from_value(serde_json::json!({
        "op": "bell",
        "event": { "kind": "unwrap", "link_token": "t", "game_id": "g", "choice": true }
    })).unwrap();
    match req {
        FulfillRequest::Bell { event: bell::BellEvent::Unwrap { link_token, game_id, choice } } => {
            assert_eq!((link_token.as_str(), game_id.as_str(), choice), ("t", "g", true));
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
    Unwrap { link_token: String, game_id: String, #[serde(default)] choice: bool },
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
    /// The bell ran (sent, dark no-op, or swallowed failure — see handle_bell: every outcome is
    /// this variant BY DESIGN, because an Event-invoke retries on function error and a double
    /// ring is worse than a missed one).
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
        BellEvent::Unwrap { link_token, game_id, choice } => {
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
```

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
- Modify: `crates/dynamo/src/lib.rs` (two new Store methods; follow the `ADD` update-expression
  style at :1143)
- Modify: `crates/dynamo/tests/store_test.rs` (counter round-trip test, existing
  dynamodb-local harness — tests SKIP locally without `DYNAMODB_LOCAL_URL`; CI runs them)
- Modify: `crates/fulfillment/src/bell.rs` (`ring` increments after a successful send)
- Modify: `crates/fulfillment/src/lib.rs` (`handle_whisper` reads the count; near the
  `whisper_card` call at ~:4940)
- Modify: `crates/fulfillment/src/whisper.rs` (`whisper_card` gains a `bell_count: Option<u32>`
  parameter and one content line)

**Interfaces:**
- Produces: `Store::increment_bell_count(&self, week: &str) -> Result<(), StoreError>` — atomic
  `ADD rings :one` on item `pk = "BELL#<week>"` (its OWN namespace; `WHISPER#` is untouchable).
- Produces: `Store::get_bell_count(&self, week: &str) -> Result<u32, StoreError>` — 0 when the
  item is absent.
- Changes: `whisper_card(game, steam, site_url, cycle, slot, preview, bell_count: Option<u32>)` —
  `None` = counter unreadable (say nothing rather than lie a zero); `Some(n)` renders one line:
  `n == 0` → `the bell was quiet this week` · `n > 0` → `the bell rang {n} time(s) this week ♡`.

- [ ] **Step 1: Write the failing store test** (in `store_test.rs`, matching its harness style)

```rust
#[tokio::test]
async fn bell_count_increments_and_reads_zero_when_absent() {
    let Some(store) = local_store().await else { return }; // the harness's existing skip shape
    assert_eq!(store.get_bell_count("2026-W36").await.unwrap(), 0);
    store.increment_bell_count("2026-W36").await.unwrap();
    store.increment_bell_count("2026-W36").await.unwrap();
    assert_eq!(store.get_bell_count("2026-W36").await.unwrap(), 2);
    assert_eq!(store.get_bell_count("2026-W37").await.unwrap(), 0); // week-scoped
}
```

(`local_store()` stands for whatever constructor/skip helper the file's other tests use — copy it
verbatim from a neighboring test; do NOT invent a second harness path.)

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p dynamo bell_count 2>&1 | tail -5`
Expected: compile FAIL (methods not defined). (If dynamodb-local is absent the RUN would skip —
the compile failure still proves the red.)

- [ ] **Step 3: Implement the two Store methods**

```rust
    /// The bell's ring ledger: `ADD rings :one` on `pk = BELL#<week>`, own namespace, never
    /// WHISPER#. Purpose is falsifiability, not accounting — the weekly whisper prints this so
    /// a dead bell stops reading as a quiet week (spec-attic-bell, Lilith's ④).
    pub async fn increment_bell_count(&self, week: &str) -> Result<(), StoreError> {
        self.client
            .update_item()
            .table_name(&self.table)
            .key("pk", AttributeValue::S(format!("BELL#{week}")))
            .key("sk", AttributeValue::S("COUNT".into()))
            .update_expression("ADD rings :one")
            .expression_attribute_values(":one", AttributeValue::N("1".into()))
            .send()
            .await
            .map_err(box_sdk_err)?;   // match the file's real error-mapping helper
        Ok(())
    }

    pub async fn get_bell_count(&self, week: &str) -> Result<u32, StoreError> {
        let out = self
            .client
            .get_item()
            .table_name(&self.table)
            .key("pk", AttributeValue::S(format!("BELL#{week}")))
            .key("sk", AttributeValue::S("COUNT".into()))
            .send()
            .await
            .map_err(box_sdk_err)?;
        Ok(out
            .item()
            .and_then(|i| i.get("rings"))
            .and_then(|v| v.as_n().ok())
            .and_then(|n| n.parse::<u32>().ok())
            .unwrap_or(0))
    }
```

(Key names/types and the error helper must match the file's conventions — read a neighboring
method first; if the table uses different key attribute names, copy THOSE.)

- [ ] **Step 4: Wire `ring` to increment after a successful send**

At the end of `ring`'s success path (send returned `true`):

```rust
    // ledger of rings, best-effort like everything here: the count exists so the weekly
    // whisper can contradict a silent bell; a failed increment is a WARN, never a failed ring.
    let week = {
        let (y, w, _) = time::OffsetDateTime::now_utc().date().to_iso_week_date();
        format!("{y}-W{w:02}")
    };
    if let Err(e) = deps.store.increment_bell_count(&week).await {
        tracing::warn!(error = ?e, week, "bell rang but the ring ledger write failed");
    }
```

- [ ] **Step 5: Wire `handle_whisper` + `whisper_card`**

In `handle_whisper`, right before the `whisper_card` call (the `slot` string is already the ISO
week): `let bell_count = deps.store.get_bell_count(&slot).await.ok();` and pass it as the new
final argument. In `whisper_card`, add the parameter and append to `content` before the cap:

```rust
    if let Some(n) = bell_count {
        content.push_str(&match n {
            0 => "\n*(the bell was quiet this week)*".to_string(),
            1 => "\n🔔 *(the bell rang once this week ♡)*".to_string(),
            n => format!("\n🔔 *(the bell rang {n} times this week ♡)*"),
        });
    }
```

Update every existing `whisper_card(` call site (including `handle_whisper_preview` and every
test) — pass `None` in the preview path (a preview must not imply a live counter read) and in old
tests; add ONE new card test asserting the three renderings (None → absent, 0 → quiet line,
2 → "rang 2 times").

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
- Test: the existing public-api test module's mock invoker (find it:
  `grep -n 'impl Invoker for' crates/public-api/src -r`)

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

(The bodies must be REAL: copy the fixture setup from the nearest existing `handle_post_claim`
test — `grep -n 'async fn.*claim' crates/public-api/src/lib.rs | grep test` — and extend the mock
invoker with a `Vec<FulfillRequest>` behind a Mutex. Write all four with full setup; the pattern
exists in-file.)

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
    // 2s hard budget on the invoke call itself, so even a hung control plane costs the friend
    // at most 2s — typical is ~20ms.
    match tokio::time::timeout(std::time::Duration::from_secs(2),
        s.invoker.bell(FulfillRequest::Bell {
            event: fulfillment::bell::BellEvent::Unwrap {
                link_token: token.clone(),
                game_id: body.game_id.clone(),
                choice: requires_choice,
            },
        })).await {
        Ok(Ok(())) => {}
        Ok(Err(e)) => tracing::warn!(error = %e, "unwrap bell invoke failed — gift unaffected"),
        Err(_) => tracing::warn!("unwrap bell invoke timed out (2s) — gift unaffected"),
    }
```

Thanks call site (inside the `Ok(dynamo::SetThanksOutcome::Set)` arm, after the info line):

```rust
    match tokio::time::timeout(std::time::Duration::from_secs(2),
        s.invoker.bell(FulfillRequest::Bell {
            event: fulfillment::bell::BellEvent::Thanks { link_token: token.clone() },
        })).await {
        Ok(Ok(())) => {}
        Ok(Err(e)) => tracing::warn!(error = %e, "thanks bell invoke failed — thanks unaffected"),
        Err(_) => tracing::warn!("thanks bell invoke timed out (2s) — thanks unaffected"),
    }
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
toggle · uniform Event invoke with a 2s budget · choice clause without a pick count ·
at-most-once + ops ping on miss + the whisper-carried ring count), plus these two fences:
- "**The bell is not the pick ledger.** The admin is truth for the monthly-pick budget; the bell
  may miss and must never be counted with." (Lilith's ③)
- "Mentions-deny provenance: `allowed_mentions` covers `content` by contract; embed behaviour is
  observed-not-contract, which is why all friend-influenced text rides `content`." (OMBB's ⑤)
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
