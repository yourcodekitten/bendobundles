# The Attic Whispers Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A scheduled whisper — one forgotten treasure from Ben's catalog surfaced weekly into his Discord with art and a link-cutting deep-link — with a no-repeat log, race-safe idempotence, and five distinct no-send causes that never share a face.

**Architecture:** EventBridge Scheduler (timezone-aware) invokes the existing fulfillment lambda with a static `{"op":"whisper"}` payload → new `FulfillRequest::Whisper` arm → pure selection over `list_listable_games()` minus curated promises minus delivered whispers → conditional-put whisper-log item → send via a second (whisper-register) webhook → mark delivered. Dark/empty/failed paths each announce distinctly; a CloudWatch alarm outside the mechanism covers the run-that-never-happened.

**Tech Stack:** Rust (workspace crates: `domain`, `dynamo`, `fulfillment`), DynamoDB single-table, wiremock + dynamodb-local test harness, Terraform (EventBridge Scheduler, SSM SecureString, CloudWatch alarm).

**Spec:** `docs/spec-attic-whispers.md` (family-reviewed rounds 1–3; the spec's predicate and failure-cause list are contractual).

## Global Constraints

- Workspace toolchain per `rust-toolchain.toml`; `cargo fmt --all` and `cargo clippy --workspace` clean; full `cargo test --workspace` (dynamo-local tests SKIP locally by design — CI is the receipt, never a local pass).
- All commits GPG-signed (`git commit -S`) as `code kitten <yourcodekitten@gmail.com>`, lowercase descriptive messages, branch `feat/attic-whispers`.
- Whisper failures must never touch the sync/gift paths: `Whisper` is its own match arm, its errors its own.
- The five no-send causes (spec §failure-honesty) each get a TESTED arm: ① dark param (zero writes) · ② empty-by-predicate (ops line with per-stage pool sizes) · ③ conditional-put loser (log-only) · ④ send-failed (`delivered=false` receipt) · ⑤ never-ran (CloudWatch alarm, Task 4).
- The whisper register is the friend-surface voice (lowercase, ♡); the ops register carries only dark/empty announcements.

---

### Task 1: WhisperRecord + Store methods (domain + dynamo)

**Files:**
- Modify: `crates/domain/src/lib.rs` (add `WhisperRecord` near `Claim`)
- Modify: `crates/dynamo/src/lib.rs` (three methods on `Store`, near `list_listable_games`)
- Test: `crates/dynamo/tests/store_test.rs` (append; reuse its dynamodb-local skip harness)

**Interfaces:**
- Consumes: existing `Store` internals — follow `create_link`'s `condition_expression("attribute_not_exists(pk)")` idiom (`crates/dynamo/src/lib.rs:665-675`) and `list_listable_games`'s Scan shape (`:983`, filter `begins_with(pk, …) AND sk = META`).
- Produces (later tasks rely on these exact signatures):
  - `domain::WhisperRecord { pub slot: String, pub game_id: String, pub cycle: u32, pub delivered: bool }`
  - `Store::record_whisper(&self, slot: &str, game_id: &str, cycle: u32) -> Result<bool, StoreError>` — `Ok(true)` recorded; `Ok(false)` = this slot already has a whisper (conditional check failed = cause ③). The slot is the ISO week (`2026-W35`), NOT a date — spec §idempotence: the key grain must match the cadence grain, and a UTC date key under an ET schedule lets a midnight-crossing retry double-send.
  - `Store::mark_whisper_delivered(&self, slot: &str) -> Result<(), StoreError>`
  - `Store::list_whispers(&self) -> Result<Vec<WhisperRecord>, StoreError>`

Item shape: `pk = "WHISPER#<slot>"` (slot = ISO week, e.g. `2026-W35`), `sk = "META"`, attrs `game_id (S)`, `cycle (N)`, `delivered (BOOL)`, `created_at (S, rfc3339)`. `WhisperRecord.date` is named `slot: String` throughout. No GSI attributes — whispers are only ever scanned by prefix.

- [ ] **Step 1: Write the failing store tests** (append to `crates/dynamo/tests/store_test.rs`, using its existing `store_or_skip`-style setup — copy the file's local-skip preamble exactly as neighboring tests do):

```rust
#[tokio::test]
async fn whisper_record_is_write_once_per_slot() {
    let Some(store) = store_or_skip("whisper-once").await else { return };
    assert!(store.record_whisper("2026-W35", "g1", 0).await.unwrap()); // first write wins
    assert!(!store.record_whisper("2026-W35", "g2", 0).await.unwrap()); // same slot: loser, Ok(false)
    let all = store.list_whispers().await.unwrap();
    assert_eq!(all.len(), 1);
    assert_eq!(all[0].game_id, "g1");
    assert_eq!(all[0].cycle, 0);
    assert!(!all[0].delivered); // born undelivered — a receipt, not an exclusion
}

#[tokio::test]
async fn whisper_mark_delivered_flips_exactly_that_slot() {
    let Some(store) = store_or_skip("whisper-mark").await else { return };
    store.record_whisper("2026-W35", "g1", 0).await.unwrap();
    store.record_whisper("2026-W36", "g2", 0).await.unwrap();
    store.mark_whisper_delivered("2026-W35").await.unwrap();
    let mut all = store.list_whispers().await.unwrap();
    all.sort_by(|a, b| a.slot.cmp(&b.slot));
    assert!(all[0].delivered);
    assert!(!all[1].delivered);
}
```

- [ ] **Step 2: Run to verify failure** — `cargo test -p dynamo --test store_test whisper` → compile error (methods don't exist). If no dynamodb-local: tests skip at runtime, but the COMPILE failure is the red we need here.
- [ ] **Step 3: Implement** `WhisperRecord` in domain (plain struct, `Serialize, Deserialize, Debug, Clone, PartialEq, Eq`) and the three Store methods: `record_whisper` = PutItem with `condition_expression("attribute_not_exists(pk)")`, mapping `ConditionalCheckFailedException` to `Ok(false)` (follow how the crate already classifies conditional failures — grep `ConditionalCheckFailed` in `crates/dynamo/src/lib.rs` and reuse that exact match arm shape); `mark_whisper_delivered` = UpdateItem `SET delivered = :t` conditioned `attribute_exists(pk)`; `list_whispers` = Scan `begins_with(pk, "WHISPER#") AND sk = :meta`, mapping items manually like the link/game scans do.
- [ ] **Step 4: Run** `cargo test -p dynamo --test store_test whisper` (compiles; skips or passes locally) and `cargo clippy -p dynamo -p domain`.
- [ ] **Step 5: Commit** — `whisper log: write-once-per-slot item + delivered receipt (domain + dynamo)`

---

### Task 2: Pure selection module (`fulfillment::whisper`)

**Files:**
- Create: `crates/fulfillment/src/whisper.rs`
- Modify: `crates/fulfillment/src/lib.rs` (add `pub mod whisper;` — or inline module if the crate keeps single-file style; match the existing layout: `operator_message.rs` is a sibling module, so a sibling file is the house pattern)

**Interfaces:**
- Consumes: `domain::{Game, Link, WhisperRecord}`, `time::OffsetDateTime`.
- Produces (Task 3 relies on these exact signatures):
  - `pub fn active_promises(links: &[Link], now: OffsetDateTime) -> HashSet<String>`
  - `pub fn current_cycle(whispers: &[WhisperRecord]) -> u32`
  - `pub fn delivered_ids(whispers: &[WhisperRecord], cycle: u32) -> HashSet<String>`
  - `pub fn eligible<'a>(games: &'a [Game], promises: &HashSet<String>, excluded: &HashSet<String>) -> Vec<&'a Game>` (is_listable ∧ ∉promises ∧ ∉excluded, **sorted by title then id** — scan order is not stable and the deterministic pick needs a stable order)
  - `pub fn select<'a>(pool: &[&'a Game], julian_day: i64) -> Option<&'a Game>` (artwork-preferred subset first; index = `(julian_day.unsigned_abs().wrapping_mul(2654435761)) as usize % len`)
  - `pub fn whisper_message(game: &Game, site_url: &str) -> String`

- [ ] **Step 1: Write the failing tests** in `whisper.rs`'s `#[cfg(test)] mod tests`, with these exact fixtures at the top of the module:

```rust
fn game(id: &str, title: &str, art: Option<&str>) -> Game {
    Game {
        id: id.into(), title: title.into(), bundle: "Humble Test Bundle".into(),
        gamekey: "gk".into(), machine_name: id.into(), key_type: "steam".into(),
        giftable: true, hidden: false, status: GameStatus::Available,
        claim_id: None, artwork_url: art.map(Into::into), keyindex: 0,
        requires_choice: false, steam_app_id: None, appid_source: None,
        owned_by_ben: false, hidden_source: None,
    }
}

fn game_with_bundle(id: &str, title: &str, bundle: &str, art: Option<&str>) -> Game {
    let mut g = game(id, title, art);
    g.bundle = bundle.into();
    g
}

fn test_link(token: &str) -> Link {
    Link {
        token: token.into(), label: "t".into(), gift_note: None, thank_note: None,
        thanked_at: None, claims_allowed: 3, claims_used: 0, revoked: false,
        expires_at: None, unlock_at: None, curated_game_ids: None,
        created_at: OffsetDateTime::now_utc(),
    }
}

fn wr(slot: &str, game_id: &str, cycle: u32, delivered: bool) -> WhisperRecord {
    WhisperRecord { slot: slot.into(), game_id: game_id.into(), cycle, delivered }
}
```


```rust
#[test]
fn active_promise_excludes_curated_only() {
    let now = OffsetDateTime::now_utc();
    let mut open_shelf = test_link("open");            // curated_game_ids: None
    let mut curated = test_link("cur");
    curated.curated_game_ids = Some(vec!["g1".into()]);
    let mut spent = test_link("spent");                // fully used curated link: promise expired
    spent.curated_game_ids = Some(vec!["g2".into()]);
    spent.claims_allowed = 1; spent.claims_used = 1;
    let mut revoked = test_link("rev");
    revoked.curated_game_ids = Some(vec!["g3".into()]);
    revoked.revoked = true;
    let p = active_promises(&[open_shelf, curated, spent, revoked], now);
    assert_eq!(p, ["g1".to_string()].into_iter().collect());
    // THE VACUOUS-EXCLUSION PIN: an open shelf (18/18 live links today) promises NOTHING —
    // if this assert ever includes the catalog, the whisper has gone permanently silent.
}

#[test]
fn cycle_rolls_over_instead_of_going_quiet() {
    let ws = vec![wr("2026-W31", "g1", 0, true), wr("2026-W32", "g2", 0, true)];
    assert_eq!(current_cycle(&ws), 0);
    let excluded = delivered_ids(&ws, 0);
    let games = [game("g1", "a", None), game("g2", "b", None)];
    let pool = eligible(&games, &HashSet::new(), &excluded);
    assert!(pool.is_empty()); // pool exhausted at cycle 0…
    // …Task 3's handler then re-derives with cycle+1 and an empty exclusion set:
    let pool2 = eligible(&games, &HashSet::new(), &delivered_ids(&ws, 1));
    assert_eq!(pool2.len(), 2); // the attic starts over, never silences
}

#[test]
fn undelivered_whisper_is_a_receipt_not_an_exclusion() {
    let ws = vec![wr("2026-W31", "g1", 0, false)]; // recorded, send FAILED
    assert!(delivered_ids(&ws, 0).is_empty()); // g1 stays eligible — OMBB's ①×two-write arm
}

#[test]
fn select_prefers_dressed_treasures_and_is_deterministic() {
    let g_art = game("g1", "aaa", Some("https://art/1.png"));
    let g_plain = game("g2", "bbb", None);
    let pool = vec![&g_art, &g_plain];
    let picked = select(&pool, 2461281).unwrap();
    assert_eq!(picked.id, "g1"); // artwork subset wins while non-empty
    assert_eq!(select(&pool, 2461281).unwrap().id, picked.id); // same day ⇒ same winner
    let artless = vec![&g_plain];
    assert!(select(&artless, 2461281).is_some()); // delight never gates
    assert!(select(&[], 2461281).is_none());
}

#[test]
fn message_carries_title_bundle_deeplink_and_art() {
    let g = game_with_bundle("g1", "Overgrowth", "Humble Indie Bundle 9", Some("https://art/x.png"));
    let m = whisper_message(&g, "https://bendobundles.com");
    assert!(m.contains("**Overgrowth**"));
    assert!(m.contains("Humble Indie Bundle 9"));
    assert!(m.contains("https://bendobundles.com/admin/catalog?q=Overgrowth"));
    assert!(m.contains("https://art/x.png"));
    assert!(m.starts_with("🕯️")); // the register is the friend voice, not the ops voice
}

#[test]
fn deeplink_urlencodes_the_title() {
    let g = game_with_bundle("g1", "Papers, Please", "HB 12", None);
    let m = whisper_message(&g, "https://bendobundles.com");
    assert!(m.contains("catalog?q=Papers%2C%20Please"));
}
```

- [ ] **Step 2: Run** `cargo test -p fulfillment whisper` → compile failure (module absent).
- [ ] **Step 3: Implement** the six functions. `active_promises`: links where `!revoked && expires_at.map_or(true, |e| e > now) && claims_used < claims_allowed`, flat-map `curated_game_ids.iter().flatten()` — **`None` contributes nothing** (the open-shelf rule, comment it with the 18/18 measurement). `whisper_message`: url-encode via a tiny local percent-encode of the query value only (check `Cargo.toml` first — if `urlencoding` or `percent-encoding` is already a workspace dep, use it; do NOT add a new dependency for one query string, hand-roll the ~10-line RFC3986 unreserved-set encoder instead, with its own test above).
- [ ] **Step 4: Run** `cargo test -p fulfillment whisper` → all green; `cargo clippy -p fulfillment`.
- [ ] **Step 5: Commit** — `whisper selection: curated-promise exclusion, cycle rollover, deterministic dressed-first pick (pure)`

---

### Task 3: The Whisper arm — envelope, Deps, orchestration, handler tests

**Files:**
- Modify: `crates/fulfillment/src/lib.rs` (`FulfillRequest::Whisper` variant; `FulfillResponse::Whispered` fieldless variant beside the measured `FulfillResponse::SyncDone` (lib.rs:181) and shaped like it; `Deps` gains THREE fields — `whisper_notify: Notify`, `whisper_site_url: String`, `whisper_param_name: String` (the SSM param name for the dark one-liner; main.rs passes the `WHISPER_WEBHOOK_PARAM` env value through, or the literal `"(WHISPER_WEBHOOK_PARAM env unset)"` when absent, so the dark message always names something actionable); `handle` gains the arm; new `async fn handle_whisper(deps: &Deps) -> FulfillResponse`)
- Modify: `crates/fulfillment/src/main.rs` (env `WHISPER_WEBHOOK_PARAM` optional + `WHISPER_SITE_URL` defaulting to `https://bendobundles.com`; resolve a second `Notify` at startup exactly like `webhook_read` → `notify`; thread both into `Deps`)
- Test: `crates/fulfillment/tests/handler_test.rs` (extend; every `Deps` literal in the tree gains the two fields — the compiler enumerates the sites)

**Interfaces:**
- Consumes: Task 1's Store methods, Task 2's pure functions, existing `Notify`, `ping_msg`-adjacent `deliver` machinery, `Store::{list_listable_games, list_links}`.
- Produces: wire contract `{"op":"whisper"}` → runs the whisper; `FulfillResponse::Whispered` (fieldless — Scheduler invokes async, payload discarded).

`handle_whisper` control flow (each numbered comment names its spec cause):

```rust
async fn handle_whisper(deps: &Deps) -> FulfillResponse {
    // cause ① — DARK: zero writes, loud, and the ops register carries the light-it one-liner.
    let Notify::Webhook(whisper_url) = &deps.whisper_notify else {
        tracing::warn!(outcome = "whisper_dark", "whisper webhook unconfigured — no-op, zero writes");
        // OperatorMessage has NO runtime-String constructor BY DESIGN (trust boundary,
        // operator_message.rs module docs). Runtime values enter ONLY as Part::Id(&str).
        ping_msg(deps, &OperatorMessage::fmt(
            "whisper is DARK — the attic has a voice and no throat. Light it: aws ssm put-parameter --name {} --type SecureString --overwrite --value <discord webhook url>",
            &[Part::Id(&deps.whisper_param_name)],
        )).await;
        return FulfillResponse::Whispered;
    };
    let games = match deps.store.list_listable_games().await { Ok(g) => g, Err(e) => { tracing::error!(error = ?e, "whisper: cannot list games"); return FulfillResponse::Whispered; } };
    let links = match deps.store.list_links().await { Ok(l) => l, Err(e) => { tracing::error!(error = ?e, "whisper: cannot list links"); return FulfillResponse::Whispered; } };
    let whispers = match deps.store.list_whispers().await { Ok(w) => w, Err(e) => { tracing::error!(error = ?e, "whisper: cannot list whisper log"); return FulfillResponse::Whispered; } };
    let now = OffsetDateTime::now_utc();
    let promises = whisper::active_promises(&links, now);
    let mut cycle = whisper::current_cycle(&whispers);
    let mut pool = whisper::eligible(&games, &promises, &whisper::delivered_ids(&whispers, cycle));
    if pool.is_empty() {
        // exhaustion ⇒ rollover, never silence (spec: corpus size IS the period)
        cycle += 1;
        pool = whisper::eligible(&games, &promises, &whisper::delivered_ids(&whispers, cycle));
    }
    tracing::info!(listable = games.len(), promised = promises.len(), pool = pool.len(), cycle, "whisper pool");
    let Some(pick) = whisper::select(&pool, now.date().to_julian_day() as i64) else {
        // cause ② — EMPTY-BY-PREDICATE, even after rollover: a vacuous predicate must not
        // wear a quiet week's face. Per-stage sizes in the line.
        tracing::warn!(outcome = "whisper_empty", "whisper pool empty after rollover");
        // Counts are runtime values: format them to Strings and pass as Part::Id (the only
        // runtime-&str Part). Bind the Strings BEFORE the call so the borrows live long enough.
        let n_listable = games.len().to_string();
        let n_promised = promises.len().to_string();
        ping_msg(deps, &OperatorMessage::fmt(
            "whisper found NOTHING to say — {} listable, {} promise-excluded, pool 0 after rollover. That is a predicate problem or an empty attic, never a quiet week.",
            &[Part::Id(&n_listable), Part::Id(&n_promised)],
        )).await;
        return FulfillResponse::Whispered;
    };
    let slot = { let (y, w, _) = now.date().to_iso_week_date(); format!("{y}-W{w:02}") }; // ISO week = the tick identity; grain coupled to the weekly cadence BY NAME (spec §idempotence)
    match deps.store.record_whisper(&slot, &pick.id, cycle).await {
        Ok(true) => {}
        Ok(false) => { tracing::info!(outcome = "whisper_already_recorded", slot, "cause ③: this slot already whispered — loser exits"); return FulfillResponse::Whispered; }
        Err(e) => { tracing::error!(error = ?e, "whisper: record failed — NOT sending (record precedes act)"); return FulfillResponse::Whispered; }
    }
    let text = whisper::whisper_message(pick, &deps.whisper_site_url);
    if whisper_send(&deps.http, whisper_url, &text).await {
        if let Err(e) = deps.store.mark_whisper_delivered(&slot).await {
            tracing::error!(error = ?e, slot, "whisper sent but mark failed — item stays a receipt; next tick re-eligible");
        }
    } else {
        // cause ④ — SEND FAILED: the item stays delivered=false, a visible receipt; the GAME
        // stays eligible because exclusions read delivered=true only.
        tracing::error!(outcome = "whisper_send_failed", slot, game = %pick.id, "whisper POST failed");
    }
    FulfillResponse::Whispered
}
```

`whisper_send` — exact contract, because `deliver`'s convention is INVERTED (in `ping_msg`, `deliver(...).await == 1` is the FAILURE branch — verify at `deliver`'s definition before assuming):

```rust
/// Send one whisper. true ⇔ the POST landed. Calls `deliver` directly — NOT `ping_msg` — because
/// the whisper is not an operator message: different webhook, different register, no PING_PREFIX,
/// and its body is runtime text that must not (and cannot) pass through OperatorMessage.
async fn whisper_send(http: &reqwest::Client, url: &str, text: &str) -> bool {
    deliver(http, url, text).await == 0
}
```

(If `deliver`'s parameter/return types differ at the definition site, follow the definition and keep THIS truth table: true = landed, false = failed. The templates above are the CONTENT contract; adjust `OperatorMessage::fmt` arities to the real `Part` API at the call site.)

- [ ] **Step 1: Write the failing handler tests.** First add two fixtures to `handler_test.rs` (the existing `deps(...)` builder stays untouched so its 100+ call sites don't move — `deps_whisper` wraps it):

```rust
/// Whisper-arm Deps: ops webhook and whisper webhook are SEPARATE registers by design.
fn deps_whisper(
    store: Store,
    humble_uri: &str,
    ops_webhook: Option<String>,
    whisper_webhook: Option<String>,
) -> Deps {
    let mut d = deps(store, humble_uri, ops_webhook);
    d.whisper_notify = match whisper_webhook {
        Some(u) => fulfillment::Notify::Webhook(u),
        None => fulfillment::Notify::Disabled,
    };
    d.whisper_site_url = "https://bendobundles.example".into();
    d.whisper_param_name = "/test/whisper-webhook".into();
    d
}

/// A listable game: Available, giftable, visible. Fill every remaining field literally,
/// matching the Game struct — same style as this file's `link()` helper.
fn available_game(id: &str, title: &str) -> Game {
    Game {
        id: id.into(), title: title.into(), bundle: "Humble Test Bundle".into(),
        gamekey: "gk".into(), machine_name: id.into(), key_type: "steam".into(),
        giftable: true, hidden: false, status: GameStatus::Available,
        claim_id: None, artwork_url: None, keyindex: 0, requires_choice: false,
        steam_app_id: None, appid_source: None, owned_by_ben: false, hidden_source: None,
    }
}
```

(The `deps()` builder keeps its current signature; because `Deps` gains three fields, `deps()` itself must initialize them — `whisper_notify: fulfillment::Notify::Disabled`, `whisper_site_url: String::new()`, `whisper_param_name: String::new()` — so every existing test compiles unchanged.) Then the tests:

```rust
#[tokio::test]
async fn whisper_dark_param_writes_nothing_and_pings_ops() {
    let Some(store) = store_or_skip("whisper-dark").await else { return };
    let ops = MockServer::start().await;
    Mock::given(method("POST")).respond_with(ResponseTemplate::new(204)).expect(1).mount(&ops).await; // the light-it one-liner
    store.put_game(&available_game("g1", "Aaa")).await.unwrap();
    let d = deps_whisper(store.clone(), &ops.uri(), Some(ops.uri()), None); // ops webhook on, whisper DARK
    let r = handle(&d, FulfillRequest::Whisper).await;
    assert!(matches!(r, FulfillResponse::Whispered));
    assert!(store.list_whispers().await.unwrap().is_empty()); // 🔴 ZERO WRITES — the ①×two-write arm
}

#[tokio::test]
async fn whisper_happy_path_records_sends_marks() {
    let Some(store) = store_or_skip("whisper-happy").await else { return };
    let wh = MockServer::start().await;
    Mock::given(method("POST")).respond_with(ResponseTemplate::new(204)).expect(1).mount(&wh).await;
    store.put_game(&available_game("g1", "Overgrowth")).await.unwrap();
    let d = deps_whisper(store.clone(), &wh.uri(), None, Some(wh.uri()));
    handle(&d, FulfillRequest::Whisper).await;
    let ws = store.list_whispers().await.unwrap();
    assert_eq!(ws.len(), 1);
    assert_eq!(ws[0].game_id, "g1");
    assert!(ws[0].delivered); // record → send → MARK completed
    let body = String::from_utf8(wh.received_requests().await.unwrap()[0].body.clone()).unwrap();
    assert!(body.contains("Overgrowth"));
}

#[tokio::test]
async fn whisper_send_failure_leaves_a_receipt_not_an_exclusion() {
    let Some(store) = store_or_skip("whisper-sendfail").await else { return };
    let wh = MockServer::start().await;
    Mock::given(method("POST")).respond_with(ResponseTemplate::new(500)).mount(&wh).await;
    store.put_game(&available_game("g1", "Aaa")).await.unwrap();
    let d = deps_whisper(store.clone(), &wh.uri(), None, Some(wh.uri()));
    handle(&d, FulfillRequest::Whisper).await;
    let ws = store.list_whispers().await.unwrap();
    assert_eq!(ws.len(), 1);
    assert!(!ws[0].delivered); // cause ④: visible receipt; the game stays eligible next tick
}

#[tokio::test]
async fn whisper_second_run_same_slot_is_a_quiet_loser() {
    let Some(store) = store_or_skip("whisper-loser").await else { return };
    let wh = MockServer::start().await;
    Mock::given(method("POST")).respond_with(ResponseTemplate::new(204)).expect(1).mount(&wh).await; // exactly ONE send
    store.put_game(&available_game("g1", "Aaa")).await.unwrap();
    let d = deps_whisper(store.clone(), &wh.uri(), None, Some(wh.uri()));
    handle(&d, FulfillRequest::Whisper).await;
    handle(&d, FulfillRequest::Whisper).await; // same ISO-week slot ⇒ conditional loser, cause ③
    assert_eq!(store.list_whispers().await.unwrap().len(), 1);
}

#[tokio::test]
async fn whisper_empty_pool_pings_ops_distinctly() {
    let Some(store) = store_or_skip("whisper-empty").await else { return };
    let ops = MockServer::start().await;
    Mock::given(method("POST")).respond_with(ResponseTemplate::new(204)).expect(1).mount(&ops).await;
    // no games at all — empty attic
    let d = deps_whisper(store.clone(), &ops.uri(), Some(ops.uri()), Some("http://127.0.0.1:1/dead".into()));
    handle(&d, FulfillRequest::Whisper).await;
    assert!(store.list_whispers().await.unwrap().is_empty()); // empty ⇒ no record either
    let body = String::from_utf8(ops.received_requests().await.unwrap()[0].body.clone()).unwrap();
    assert!(body.contains("NOTHING to say")); // cause ② wording, distinct from cause ①'s "DARK"
}
```

Also add the envelope pin (pure, runs everywhere):

```rust
#[test]
fn whisper_envelope_parses() {
    let r: FulfillRequest = serde_json::from_value(serde_json::json!({"op":"whisper"})).unwrap();
    assert!(matches!(r, FulfillRequest::Whisper));
}
```

- [ ] **Step 2: Run** `cargo test -p fulfillment` → compile errors (variant, fields, fns absent).
- [ ] **Step 3: Implement** — variant + fieldless response + Deps fields + `handle` arm + `handle_whisper` + `whisper_send` + main.rs env/Notify wiring. Update every `Deps` literal the compiler names (handler_test's builders, `deps_with_selfheal`, any bin targets under `crates/fulfillment/src/bin/`). Keep the whisper `Notify` resolution startup-once, mirroring `webhook_read`, env `WHISPER_WEBHOOK_PARAM` absent ⇒ `DeliberatelyOff`.
- [ ] **Step 4: Run** `cargo test --workspace` and `cargo clippy --workspace` → green/clean (dynamo-local arms skip locally; CI is their receipt).
- [ ] **Step 5: Commit** — `the whisper arm: {"op":"whisper"} → select, record, send, mark — five no-send causes, none sharing a face`

---

### Task 4: Terraform — scheduler, param, env, IAM, and the outside instrument

**Files:**
- Modify: `terraform/aws-ssm.tf` (whisper webhook param — copy the `discord_webhook` resource shape verbatim: SecureString, `value = "UNSET"`, `lifecycle { ignore_changes = [value] }`, gated on a new var)
- Modify: `terraform/aws-eventbridge.tf` (the `aws_scheduler_schedule` + its invoke role — schedules live with the other time-triggers even though the resource type is new)
- Modify: `terraform/aws-lambda.tf` (env vars + IAM grant locals, mirroring `local.discord_webhook_param_name`/`_arn` exactly — find where those locals are defined and define `whisper_webhook_param_name`/`_arn` beside them)
- Modify: `terraform/aws-cloudwatch-alarms.tf` (cause ⑤), `terraform/tf-variables.tf`, `terraform/production.tfvars`

**Interfaces:**
- Consumes: `module.lambda_fulfillment.lambda_function_arn` / `.lambda_function_name`, `aws_sns_topic.ops_alarms.arn`, the label-module naming pattern (`module "label_whisper"` like `label_sync`).
- Produces: env `WHISPER_WEBHOOK_PARAM` + `WHISPER_SITE_URL` on the fulfillment lambda; a schedule invoking it with `input = jsonencode({ op = "whisper" })`.

- [ ] **Step 1: Variables** (`tf-variables.tf`):

```hcl
variable "whisper_enabled" {
  type        = bool
  default     = false # flipped in production.tfvars; default-off so plan-only environments stay silent
  description = "The attic whispers: weekly forgotten-treasure nudge. Creates the schedule, the whisper webhook SSM container, and the never-ran alarm."
}

variable "whisper_schedule_expression" {
  type        = string
  default     = "cron(0 10 ? * SAT *)"
  description = "Whisper cadence in America/New_York (EventBridge Scheduler is timezone-aware — classic rules are UTC-only and drift across DST, which is why this is a Scheduler schedule)."
}
```

- [ ] **Step 2: SSM param** — copy `aws_ssm_parameter.discord_webhook` as `whisper_webhook`, `count = var.whisper_enabled ? 1 : 0`, same UNSET/ignore_changes comments (a webhook URL IS the credential).
- [ ] **Step 3: Scheduler + role** (in `aws-eventbridge.tf`):

```hcl
module "label_whisper" {
  source  = "bendoerr-terraform-modules/label/null"
  version = "1.0.1"
  context = module.context.shared
  name    = "whisper"
}

resource "aws_iam_role" "whisper_scheduler" {
  count              = var.whisper_enabled ? 1 : 0
  name               = "${module.label_whisper.id}-scheduler"
  assume_role_policy = jsonencode({
    Version = "2012-10-17"
    Statement = [{ Effect = "Allow", Principal = { Service = "scheduler.amazonaws.com" }, Action = "sts:AssumeRole" }]
  })
  tags = module.label_whisper.tags
}

resource "aws_iam_role_policy" "whisper_scheduler_invoke" {
  count  = var.whisper_enabled ? 1 : 0
  name   = "invoke-fulfillment"
  role   = aws_iam_role.whisper_scheduler[0].id
  policy = jsonencode({
    Version = "2012-10-17"
    Statement = [{ Effect = "Allow", Action = "lambda:InvokeFunction", Resource = module.lambda_fulfillment.lambda_function_arn }]
  })
}

# Scheduler, not a classic rule: schedule_expression_timezone makes "Saturday morning" mean
# Saturday morning through DST (family review 2026-08-28, corroborated twice).
resource "aws_scheduler_schedule" "whisper" {
  count                        = var.whisper_enabled ? 1 : 0
  name                         = module.label_whisper.id
  schedule_expression          = var.whisper_schedule_expression
  schedule_expression_timezone = "America/New_York"
  flexible_time_window { mode = "OFF" }
  target {
    arn      = module.lambda_fulfillment.lambda_function_arn
    role_arn = aws_iam_role.whisper_scheduler[0].arn
    input    = jsonencode({ op = "whisper" }) # caught by the typed parse BEFORE the aws.events→Sync fallback
  }
}
```

- [ ] **Step 4: Lambda env + grant** — beside the `discord_webhook` locals: `whisper_webhook_param_name`/`_arn` from the counted resource (`one(aws_ssm_parameter.whisper_webhook[*].name)` shape), merged env `WHISPER_WEBHOOK_PARAM` + `WHISPER_SITE_URL = "https://bendobundles.com"`, and the `ssm:GetParameter` grant on the new ARN in the same conditional-statement list that grants the discord one.
- [ ] **Step 5: The outside instrument (cause ⑤)** in `aws-cloudwatch-alarms.tf`:

```hcl
# Cause ⑤ — the run that never happened. The whisper's own no-send announcements ride the lambda;
# a schedule that stops firing (or a lapsed role) produces silence with no announcer. This alarm's
# trigger is the Scheduler's OWN metrics — a different instrument, so it cannot inherit the failure.
resource "aws_cloudwatch_metric_alarm" "whisper_never_ran" {
  count               = var.whisper_enabled ? 1 : 0
  alarm_name          = "${module.label_whisper.id}-never-ran"
  namespace           = "AWS/Scheduler"
  metric_name         = "InvocationAttemptCount"
  dimensions          = { ScheduleGroup = "default", ScheduleName = module.label_whisper.id }
  statistic           = "Sum"
  period              = 86400
  evaluation_periods  = 8      # 8 daily buckets ⇒ a whole weekly tick plus a day of grace
  threshold           = 1
  comparison_operator = "LessThanThreshold"
  treat_missing_data  = "breaching" # silence from the metric IS the alarm condition
  datapoints_to_alarm = 8
  alarm_actions       = [aws_sns_topic.ops_alarms.arn]
  ok_actions          = [aws_sns_topic.ops_alarms.arn]
  tags                = module.label_whisper.tags
}

resource "aws_cloudwatch_metric_alarm" "whisper_target_errors" {
  count               = var.whisper_enabled ? 1 : 0
  alarm_name          = "${module.label_whisper.id}-target-errors"
  namespace           = "AWS/Scheduler"
  metric_name         = "TargetErrorCount"
  dimensions          = { ScheduleGroup = "default", ScheduleName = module.label_whisper.id }
  statistic           = "Sum"
  period              = 86400
  evaluation_periods  = 1
  threshold           = 0
  comparison_operator = "GreaterThanThreshold"
  treat_missing_data  = "notBreaching"
  alarm_actions       = [aws_sns_topic.ops_alarms.arn]
  tags                = module.label_whisper.tags
}
```

(Verify the exact `AWS/Scheduler` dimension names against current AWS docs at execution time — if they differ, the alarm dimensions follow the docs, not this plan.)

- [ ] **Step 6:** `whisper_enabled = true` in `production.tfvars`; `terraform fmt -check` + `terraform validate` (init with `backend.hcl` per repo README if needed — validate only, NO apply in this task).
- [ ] **Step 7: Commit** — `terraform: the whisper schedule (tz-aware), its dark-until-lit webhook container, and the never-ran alarm`

---

### Task 5: PR

- [ ] Push branch; open PR titled `the attic whispers 💌` with body: the spec link, the five-cause table, the family-review summary, test receipts, and the deploy/live-fire plan (spec §verification). End body with the session link footer per house rules.
- [ ] Watch CI (full suite runs on `pull_request` only — the dynamo-local arms get their receipt here, never locally).

## Self-Review (done at write time)

- Spec coverage: predicate → T2; dark-zero-writes → T3 test 1; distinct causes ①–④ → T3 tests; cause ⑤ → T4 alarms; scheduler-tz → T4; message register → T2 message tests; exhaustion rollover → T2+T3; manual retire → inherited (no task needed — `set_game_hidden` exists).
- Types consistent: `record_whisper(date, game_id, cycle) -> Result<bool>` used identically in T1/T3; `eligible`/`select` signatures match between T2 definition and T3 call sites.
- Known executor freedoms (stated, not placeholders): `OperatorMessage::fmt` exact `Part` arity, the `deps()` builder extension shape, `ConditionalCheckFailed` match idiom, and `AWS/Scheduler` dimension names — each pinned to "read the named neighbor and mirror it".
