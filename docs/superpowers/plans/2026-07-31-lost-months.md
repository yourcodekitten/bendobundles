# The Lost Months Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Choice discovery walks the full Choice era (era-aware stop, no truncation), survives every observed vintage/active wire shape, derives claimed-sets order-authoritatively, and stops minting duplicate GAME rows — surfacing ben's 50+ unclaimed vintage picks and the dark active month.

**Architecture:** Two stacked layers (family stacked-PR pilot). Layer 1 (`kitten/client-timeouts`, its own PR, lands first): HTTP timeouts on the humble client + webhook ping client — a precondition of the lambda-budget acceptance. Layer 2 (`kitten/lost-months`, rebased on layer 1): matcher fn in `domain`; wire hardening + `Option<String>` gamekey + era-aware walk in `humble-client`; order-index plumbing, claimed-set derivation, gamekey ladder, D7 flip-routing, observability in `fulfillment`; heal script + runbook for the 15 legacy pairs.

**Tech Stack:** Rust, serde, wreq (Chrome-emulating HTTP), tokio, wiremock (client tests), cargo workspaces.

**Spec:** `docs/superpowers/specs/2026-07-31-lost-months-design.md` (family-signed 2026-07-31). Read it before executing any task — Global Constraints below compress it, they don't replace it.

## Global Constraints

- All commits GPG-signed (`git commit -S`) as `code kitten <yourcodekitten@gmail.com>`.
- TDD every task: failing test observed before implementation, suite green after.
- NEVER log raw membership-blob content, key values (`redeemed_key_val`), or session cookies. Shape metadata only (field presence, sources, counts).
- The sync path NEVER deletes a row. The only delete in this plan lives in the operator heal script (Task 10), dry-run by default.
- Discovery writes `requires_choice: true` only with an order-derived claimed set (spec D3). The blob NEVER alone marks a game claimed.
- A `_choice_*` tpk the grammar can't parse still mints a row normally, loudly (spec D7 knot) — a lost key is worse than a loud pair.
- Branch structure: Tasks 1–2 on `kitten/client-timeouts` (branched from `main`); Tasks 3–10 on `kitten/lost-months` rebased onto `kitten/client-timeouts`. Do not interleave. **NB the repo is already on `kitten/lost-months` with the spec+plan commits.** Execution order: (a) `git checkout main && git checkout -b kitten/client-timeouts`, do Tasks 1–2, push, PR; (b) `git checkout kitten/lost-months && git rebase kitten/client-timeouts` (the spec/plan commits ride on top), do Tasks 3–10.
- **Fulfillment test precondition (Tasks 6–9):** these tests need dynamodb-local. Start it (`docker run -p 8000:8000 amazon/dynamodb-local` or the repo's existing script) AND export `DYNAMODB_LOCAL_URL=http://localhost:8000` before running them — the harness `store_or_skip` (handler_test.rs:81) *silently SKIPS* when no store is reachable and the URL is unset, which would forge a green "verify" step. With the URL set, an unreachable store PANICS instead. A skipped test is not a passing test: if you see `SKIP`, the store isn't up — fix that before trusting any RED or GREEN in Tasks 6–9.
- Run `cargo test --workspace` (not just the touched crate) before every commit; `cargo clippy --workspace -- -D warnings` must stay clean.

---

### Task 1: humble-client HTTP timeouts (Layer 1)

**Files:**
- Modify: `crates/humble-client/src/lib.rs:537-540` (the `wreq::Client::builder()` call in `HumbleClient::new`)
- Test: `crates/humble-client/tests/client_test.rs`

**Interfaces:**
- Produces: no signature changes — `HumbleClient::new` behavior gains a 30s total / 10s connect timeout on every request. Task 5's walk-deadline arithmetic (spec A5) assumes exactly these values.

- [ ] **Step 1: Branch**

```bash
git checkout main && git pull && git checkout -b kitten/client-timeouts
```

- [ ] **Step 2: Write the failing test**

In `client_test.rs`, next to the other wiremock tests (the file's helper builds a client against a `MockServer` — follow the existing `client(&server)`-style setup used by the neighboring tests):

```rust
#[tokio::test]
async fn requests_time_out_instead_of_hanging_forever() {
    let server = wiremock::MockServer::start().await;
    // A response slower than the client timeout — the request must fail with
    // HumbleError::Network, not hang. 35s delay > 30s timeout; the test completes
    // in ~30s worst case, acceptable for one guard test.
    wiremock::Mock::given(wiremock::matchers::method("GET"))
        .respond_with(
            wiremock::ResponseTemplate::new(200)
                .set_delay(std::time::Duration::from_secs(35))
                .set_body_string("{}"),
        )
        .mount(&server)
        .await;
    let c = client(&server); // the file's existing constructor helper
    let err = c.gamekeys().await.unwrap_err();
    assert!(matches!(err, HumbleError::Network(_)), "got {err:?}");
}
```

- [ ] **Step 3: Run test to verify it fails**

Run: `cargo test -p humble-client requests_time_out -- --nocapture`
Expected: FAIL — the request hangs until wiremock's delay elapses and returns Ok, or the test times out; either way no `Network` error is produced (if the test *passes* immediately, wreq already has a default timeout — stop, investigate, report).

- [ ] **Step 4: Implement**

```rust
let http = wreq::Client::builder()
    .emulation(Emulation::Chrome137)
    .redirect(wreq::redirect::Policy::none()) // a 302-to-login must surface, not follow
    // No timeout ⇒ one hung socket eats the whole lambda budget (backlog #5; lost-months
    // A5 precondition). 30s total / 10s connect: generous for humble's slowest real
    // responses (the membership blob), tight enough that even a worst-case serial walk
    // stays bounded by the Task-5 walk deadline, not by 40 × hang.
    .timeout(std::time::Duration::from_secs(30))
    .connect_timeout(std::time::Duration::from_secs(10))
    .build()?;
```

- [ ] **Step 5: Verify green, commit**

Run: `cargo test --workspace` — all pass, new test included.

```bash
git add crates/humble-client && git commit -S -m "fix(humble-client): 30s request / 10s connect timeout — a hung socket must not eat the lambda budget"
```

### Task 2: fulfillment webhook/ping client timeout (Layer 1) + open the Layer-1 PR

**Files:**
- Modify: `crates/fulfillment/src/main.rs` (the `reqwest::Client` construction passed into `Deps.http`)
- Test: none (construction wiring — the builder call is config, covered by compilation; the behavioral guard lives in Task 1's pattern)

**Interfaces:**
- Produces: `Deps.http` (the discord-webhook ping client) carries a 5s timeout.

- [ ] **Step 1: Locate and modify the construction**

Find the `reqwest::Client` built in `main.rs` (it feeds `Deps { http, .. }`). Change to:

```rust
let http = reqwest::Client::builder()
    .timeout(std::time::Duration::from_secs(5)) // ping is fire-and-forget; a hung webhook must not stall the run
    .build()
    .expect("reqwest client");
```

(If it's currently `reqwest::Client::new()`, this replaces it; keep any existing builder options.)

- [ ] **Step 2: Verify, commit, PR**

Run: `cargo test --workspace && cargo clippy --workspace -- -D warnings`

```bash
git add crates/fulfillment && git commit -S -m "fix(fulfillment): 5s timeout on the webhook ping client"
git push -u origin kitten/client-timeouts
gh pr create --title "fix: HTTP timeouts on humble client (30s/10s) and webhook pings (5s)" \
  --body "Layer 1 of the stacked lost-months work (family Q4 decision 2026-07-31): the client has no timeout today and layer 2 raises walk exposure 26→40 GETs — one hung socket would eat the lambda budget A5 promises to stay inside. Backlog #5. Layer 2 (lost-months) rebases on this."
```

Then switch to the EXISTING Layer-2 branch and rebase it onto Layer 1 (M12 — the repo is already on `kitten/lost-months` with the spec+plan commits; do NOT re-create or cherry-pick): `git checkout kitten/lost-months && git rebase kitten/client-timeouts`. The spec/plan doc commits ride on top of the timeout layer; resolve any trivial doc conflict keeping both. Tasks 3–10 commit onto this rebased branch.

### Task 3: the matcher — `domain::choice_tpk_bases` / `choice_tpk_matches`

**Files:**
- Modify: `crates/domain/src/lib.rs` (new fns next to `game_id` at :287)
- Test: `crates/domain/src/lib.rs` tests module

**Interfaces:**
- Produces:
  - `pub fn choice_tpk_bases(tpk_machine_name: &str) -> Option<(String, Option<String>)>` — `Some((exact_base, region_stripped_base))` for a `_choice_*`-shaped name, `None` otherwise. `region_stripped_base` is `Some` only when a `_row`/`_ww` token was present (ladder rung 2).
  - `pub fn choice_tpk_matches(tpk_machine_name: &str, offered_machine_name: &str) -> bool` — the spec-D3 grammar rung.
- Consumed by: Task 6 (claimed-set), Task 8 (D7 routing), Task 10 (heal script).

- [ ] **Step 1: Write the failing tests** (grammar cases enumerated from the 2026-07-31 prod scan)

```rust
#[test]
fn choice_tpk_bases_grammar() {
    // plain platform suffix
    assert_eq!(
        choice_tpk_bases("wingspan_choice_steam"),
        Some(("wingspan".into(), None))
    );
    // region token before _choice
    assert_eq!(
        choice_tpk_bases("mylittleuniverse_row_choice_steam"),
        Some(("mylittleuniverse_row".into(), Some("mylittleuniverse".into())))
    );
    assert_eq!(
        choice_tpk_bases("beholder2_ww_choice_steam"),
        Some(("beholder2_ww".into(), Some("beholder2".into())))
    );
    // platform is open: gog / origin / battlenet / future
    assert_eq!(
        choice_tpk_bases("diabloiv_choice_battlenet"),
        Some(("diabloiv".into(), None))
    );
    assert_eq!(
        choice_tpk_bases("somegame_choice_gog"),
        Some(("somegame".into(), None))
    );
    // multi-word machine names keep their own underscores
    assert_eq!(
        choice_tpk_bases("citizensleeper2_starwardvector_choice_steam"),
        Some(("citizensleeper2_starwardvector".into(), None))
    );
    // NOT choice-shaped: monthly-era, bundle keys, bare names, empty platform
    assert_eq!(choice_tpk_bases("holypotatoeswereinspace_monthly_steam"), None);
    assert_eq!(choice_tpk_bases("wingspan"), None);
    assert_eq!(choice_tpk_bases("wingspan_choice_"), None);
    // fires-anyway for the platform CHARSET rung (minor: without this every negative reds via
    // a different branch, so deleting the lowercase-check leaves all tests green). Uppercase
    // platform must NOT parse — the grammar is strictly lowercase+digit.
    assert_eq!(choice_tpk_bases("wingspan_choice_Steam"), None);
}

#[test]
fn choice_tpk_matches_is_the_grammar_rung() {
    // strip-grammar equality (the _row pair that killed starts_with)
    assert!(choice_tpk_matches("mylittleuniverse_row_choice_steam", "mylittleuniverse"));
    // exact-base match: an offered name that itself ends _row
    assert!(choice_tpk_matches("mylittleuniverse_row_choice_steam", "mylittleuniverse_row"));
    assert!(choice_tpk_matches("wingspan_choice_steam", "wingspan"));
    // bare equality (defensive: claim-all mints may drop the suffix)
    assert!(choice_tpk_matches("wingspan", "wingspan"));
    // non-matches: different game, prefix-hazard neighbor, monthly key
    assert!(!choice_tpk_matches("wingspan_choice_steam", "wing"));
    assert!(!choice_tpk_matches("atomicheart_row_choice_steam", "atomic"));
    assert!(!choice_tpk_matches("holypotatoeswereinspace_monthly_steam", "holypotatoeswereinspace"));
}
```

- [ ] **Step 2: Run to verify they fail** — `cargo test -p domain choice_tpk` → FAIL (fns not defined).

- [ ] **Step 3: Implement** (no regex dep — manual suffix parsing; `domain` has no regex today and doesn't get one for this)

```rust
/// Choice-tpk machine-name grammar, enumerated from prod 2026-07-31 (175 rows; spec D3):
/// `<offered>[_row|_ww]_choice_<platform>` where platform is one-or-more `[a-z0-9]`.
/// Returns `Some((exact_base, region_stripped))` for a choice-shaped name — `exact_base`
/// keeps a `_row`/`_ww` region token (an offered name may itself end that way; D7's
/// candidate ladder tries exact first), `region_stripped` is `Some` only when a region
/// token existed. `None` = not choice-shaped (bundle/monthly keys; never guessed at).
pub fn choice_tpk_bases(tpk_machine_name: &str) -> Option<(String, Option<String>)> {
    let (base, platform) = tpk_machine_name.rsplit_once("_choice_")?;
    if base.is_empty()
        || platform.is_empty()
        || !platform.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
    {
        return None;
    }
    let region_stripped = ["_row", "_ww"]
        .iter()
        .find_map(|r| base.strip_suffix(r))
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    Some((base.to_string(), region_stripped))
}

/// Spec D3's grammar rung: does this tpk record the claim of this offered game?
/// Bare equality is defensive cover for a claim-all mint that drops the suffix.
pub fn choice_tpk_matches(tpk_machine_name: &str, offered_machine_name: &str) -> bool {
    if tpk_machine_name == offered_machine_name {
        return true;
    }
    match choice_tpk_bases(tpk_machine_name) {
        Some((exact, stripped)) => {
            exact == offered_machine_name || stripped.as_deref() == Some(offered_machine_name)
        }
        None => false,
    }
}
```

- [ ] **Step 4: Verify green** — `cargo test -p domain` all pass. NB `rsplit_once` splits at the LAST `_choice_` — a hypothetical `x_choice_y_choice_steam` bases to `x_choice_y`, which is the correct reading.

- [ ] **Step 5: Commit** — `git add crates/domain && git commit -S -m "feat(domain): choice-tpk machine-name grammar (bases + matcher), enumerated from prod"`

### Task 4: wire hardening + `Option<String>` gamekey end-to-end (spec D1 + Q3)

**Files:**
- Modify: `crates/humble-client/src/model.rs:56-84` (`ContentChoiceOptions`), `crates/humble-client/src/lib.rs` (`ChoiceMonth.gamekey` at :321, `choice_month` at :666, `choice_months` at :753)
- Modify: `crates/fulfillment/src/lib.rs:3273-3287` (the two `detail.gamekey` uses — becomes the Task-7 ladder's input; for THIS task, compile-fix with the transitional form below)
- Test: `crates/humble-client/tests/client_test.rs` + new fixtures in `crates/humble-client/tests/fixtures/`

**Interfaces:**
- Produces: `ChoiceMonth.gamekey: Option<String>` (was `String` with `""` sentinel from the list walk). `choice_month` populates it `None` when the blob omits it; `choice_months` `None` where it used `unwrap_or_default()`. NO caller may ever see `""`.
- Consumes: nothing from earlier tasks.

**Test idiom (READ FIRST — the real one, verified against `client_test.rs`):** there are NO `mount_membership_page` / `mount_subscription_pages` helpers. The membership blob is loaded with `fixture_str(name)` (`:11` region — NOT `fixture()`, which does `serde_json::from_str().unwrap()` and PANICS on HTML) and mounted inline. The month read hits `GET /membership/<slug>`; the existing membership tests (~`:490`) and `choice_months_walks_the_cursor_pagination` (~`:865`) are the patterns to copy. The client constructor is `async`: `client(&server).await`. Add `HumbleError` to the file's imports (`:1` currently imports only `{HumbleClient, SessionCookie}`).

- [ ] **Step 1: Build one synthetic fixture** (spec D5 forbids captured blobs): copy the existing membership fixture `choice_month` tests load to `crates/humble-client/tests/fixtures/membership_no_gamekey.html`, and in the copied `webpack-monthly-product-data` JSON delete the `"gamekey"` key from `contentChoiceOptions` (the may-2020 / july-2026 prod shape: `missing field \`gamekey\``, requestIds `549e7e95`, `012814b8`). Everything else stays valid.

- [ ] **Step 2: Write the failing tests** (inline mounts, real idiom)

```rust
#[tokio::test]
async fn month_without_blob_gamekey_parses_with_none() {
    let server = wiremock::MockServer::start().await;
    wiremock::Mock::given(wiremock::matchers::method("GET"))
        .and(wiremock::matchers::path("/membership/may-2020"))
        .respond_with(
            wiremock::ResponseTemplate::new(200)
                .set_body_string(fixture_str("membership_no_gamekey.html")),
        )
        .mount(&server)
        .await;
    let c = client(&server).await;
    let m = c.choice_month("may-2020").await.expect("must parse, not die on a dropped field");
    assert_eq!(m.gamekey, None);
    assert!(!m.offered_games.is_empty(), "offered games still extracted");
    assert!(m.claimed_machine_names.is_some(), "claim-state still extracted");
}

#[tokio::test]
async fn list_walk_gamekey_is_never_empty_string() {
    // The gamekey-less newest months must surface as None, never "". Reuse the exact
    // page mounts of the existing choice_months walk test (copy its inline Mock setup).
    let server = wiremock::MockServer::start().await;
    mount_the_existing_walk_pages(&server).await; // inline copy from choice_months_walks_the_cursor_pagination (:865)
    let c = client(&server).await;
    let walk = c.choice_months(26).await.unwrap(); // 1-arg signature as it exists NOW; Task 5 rewrites it
    for m in &walk.months {
        assert_ne!(m.gamekey.as_deref(), Some(""), "empty-string gamekey is banned: {}", m.product_url_path);
    }
}
```

(Write the second test against the CURRENT 1-arg `choice_months`; Task 5 updates the signature and this assertion rides along. `mount_the_existing_walk_pages` is a rename for "paste the inline `Mock::given` page mounts from the existing walk test" — there is no helper, inline them.)

- [ ] **Step 3: Verify the first test FAILS** with the strict-parse error (`missing field gamekey`) — this reproduces prod. `cargo test -p humble-client month_without_blob_gamekey`

- [ ] **Step 4a: Fix the `Default` bound FIRST (BLOCKER — the struct below won't compile otherwise).** `#[serde(default)]` on `content_choice_data` requires `ContentChoiceData` to impl `Default`. At `model.rs:100` change `#[derive(Deserialize)]` to `#[derive(Deserialize, Default)]` on `ContentChoiceData` (its fields — `initial: ContentChoiceInitial`, `game_data: HashMap` — are both `Default`-able, so the derive succeeds).

- [ ] **Step 4: Implement**

`model.rs` — one comment per field, following the `SubProductWire` precedent at :144-150:

```rust
#[derive(Deserialize)]
pub(crate) struct ContentChoiceOptions {
    // Optional: the membership blob drops `gamekey` on BOTH ends of history — the active
    // month (july-2026, requestId 549e7e95) and vintage (may-2020, requestId 012814b8).
    // Absence is resolved by discovery's ladder (list gamekey → order-side), never defaulted
    // to "": a fabricated gamekey would poison `game_id()` and both claim writes.
    #[serde(default)]
    pub gamekey: Option<String>,
    #[serde(default)]
    pub title: String,
    // Defaulted: `choice_month` falls back to the request's own slug when absent — the
    // caller knows which month it asked for.
    #[serde(default, rename = "productUrlPath")]
    pub product_url_path: String,
    #[serde(default, rename = "productMachineName")]
    pub product_machine_name: String,
    #[serde(default, rename = "usesChoices")]
    pub uses_choices: bool,
    #[serde(default, rename = "isActiveContent")]
    pub is_active_content: bool,
    #[serde(default, rename = "canRedeemGames")]
    pub can_redeem_games: bool,
    // Defaulted: a blob with no choice data yields zero offered games — discovery logs the
    // shape (months_skipped) instead of killing the month on parse.
    #[serde(default, rename = "contentChoiceData")]
    pub content_choice_data: ContentChoiceData,
    #[serde(default, rename = "contentChoicesMade")]
    pub content_choices_made: ContentChoicesMade,
}
```

`lib.rs` `ChoiceMonth`:

```rust
    /// `Some` when any source supplied it; `None` when the wire dropped it (both the
    /// gamekey-less newest months in the LIST and the blob-drop months in the DETAIL read).
    /// Never `""` — resolution/skip is the caller's job (fulfillment's D2 ladder).
    pub gamekey: Option<String>,
```

`choice_month` (:666): `gamekey: cco.gamekey` (plus `product_url_path`: if `cco.product_url_path.is_empty()` use the `month_url` argument). `choice_months` (:753): replace `let gamekey = p.gamekey.unwrap_or_default();` with `let gamekey = p.gamekey.filter(|g| !g.is_empty());` and store `gamekey` directly.

`fulfillment` compile-fix (transitional until Task 7 installs the ladder): where `detail.gamekey` is used (:3275, :3283, :3286), destructure first:

```rust
        let Some(month_gamekey) = detail.gamekey.clone() else {
            tracing::warn!(month = %detail.product_url_path, "choice discovery: month has no gamekey from any source yet — skipping (ladder lands in a later task)");
            continue;
        };
```

and use `month_gamekey` in the three sites.

- [ ] **Step 4b: General D1 audit (M5 — spec D1 is "harden GENERALLY", not just gamekey).** The spec's thesis is that humble drops fields week to week (the 2026-06-30 `missing field \`initial\`` cohort proves shapes drift), so hardening only `gamekey` leaves the next dropped sub-field to re-trigger the same month-death. Grep every membership-blob wire struct — `ContentChoiceOptions`, `ContentChoiceData`, `ContentChoicesMade`, `ContentChoiceInitial`, and their nested types in `model.rs` — for any field WITHOUT `#[serde(default)]` (and no `Option`). For each identity-non-critical one, add `#[serde(default)]` (deriving `Default` on its type as needed, per Step 4a) with a one-line justification comment. A field that is genuinely load-bearing for the claimed-set/redeemability gate stays strict (its absence SHOULD skip the month loudly, not parse to a wrong default) — note which and why. Deliverable: a grep proof in the commit body that no non-defaulted strict field remains except the deliberately-strict ones.

- [ ] **Step 5: Verify green** — `cargo test --workspace`. The existing `choice_month`/`choice_months` tests must still pass (real gamekeys now arrive as `Some`); one existing walk test asserts `june.gamekey == ""` — it becomes `== None` under the Option change (enumerate it in the tests-to-update).

- [ ] **Step 6: Commit** — `git add crates/humble-client crates/fulfillment && git commit -S -m "fix(humble-client): membership blob survives dropped fields (general D1 audit); gamekey is Option end-to-end, empty-string sentinel abolished"`

### Task 5: era-aware walk-to-completion (spec D4) — pace, deadline, stop-reason, cap 40

**Files:**
- Modify: `crates/humble-client/src/lib.rs` (`choice_months` :726-798, `ChoiceMonthsWalk` :369)
- Modify: `crates/fulfillment/src/lib.rs:80` (`CHOICE_DISCOVERY_MAX_PAGES`), :3188 (call site), :3209-3214 (truncation warn)
- Test: `crates/humble-client/tests/client_test.rs` (existing walk tests + new era-stop tests)

**Interfaces:**
- Produces:
  ```rust
  pub enum WalkStop { CursorEnd, EraStop, Cap, Deadline }
  pub struct ChoiceMonthsWalk { pub months: Vec<ChoiceMonth>, pub stop: WalkStop }
  impl ChoiceMonthsWalk { pub fn complete_for_choice(&self) -> bool /* CursorEnd | EraStop */ }
  pub async fn choice_months(&self, max_pages: usize, pace: Duration, deadline: Duration) -> Result<ChoiceMonthsWalk, HumbleError>
  ```
- Consumes: Task 4's `Option<String>` gamekey.

**Test idiom:** same as Task 4 — inline `Mock::given(method("GET")).and(path(BASE_or_cursor)).respond_with(ResponseTemplate::new(200).set_body_json(json!({...})))`, one mount per page path (page 1 = `.../subscription_products_with_gamekeys/`, page 2 = `.../<cursor>`), built from the JSON shape in the existing `choice_months_walks_the_cursor_pagination` test. Define page JSON inline with `serde_json::json!`; a monthly-era product is `{ "productMachineName": "may_2019_monthly", "productUrlPath": "may-2019", "gamekey": "...", "contentChoiceData": {"game_data": {}} }`. `client(&server).await`. `ResponseTemplate::set_delay(...)` for the deadline test.

- [ ] **Step 1: Write the failing tests** (inline page mounts; the five existing call sites to migrate are listed in Step 3)

```rust
// NB fixture years: monthly-era products MUST carry pre-2019 url paths ("december-2018"),
// NOT 2019 — the Choice era began Dec 2019, and the chronology guard warns+keeps-walking on a
// non-choice slug dated >= that boundary. A "*_2019_monthly" fixture would red these tests on
// the CHRONOLOGY branch, not the era logic (M11).
#[tokio::test]
async fn walk_era_stops_on_a_full_page_of_pre_choice_slugs() {
    // page 1: 3 choice months (productMachineName ends "_choice") + cursor "C2".
    // page 2: 3 monthly-era products (pre-2019 slugs, non-empty machine_name, no _choice) + cursor "C3".
    let server = wiremock::MockServer::start().await;
    mount_page(&server, "/api/v1/subscriptions/humble_monthly/subscription_products_with_gamekeys/", choice_page(&["dec_2019_choice","nov_2019_choice","oct_2019_choice"], Some("C2"))).await;
    mount_page(&server, ".../C2", monthly_page(&["december-2018","november-2018","october-2018"], Some("C3"))).await; // slugs pre-2019
    let c = client(&server).await;
    let walk = c.choice_months(40, std::time::Duration::ZERO, std::time::Duration::from_secs(60)).await.unwrap();
    assert!(matches!(walk.stop, WalkStop::EraStop));
    assert!(walk.complete_for_choice());
    assert_eq!(walk.months.len(), 3, "monthly-era products are not months");
}

#[tokio::test]
async fn empty_slug_disqualifies_page_from_era_stop() {
    // page 2: 2 monthly (pre-2019) + 1 EMPTY productMachineName (dropped field) → NOT era-stop;
    // walk continues to page 3 which is empty (cursor end).
    let server = wiremock::MockServer::start().await;
    mount_page(&server, ".../", choice_page(&["dec_2019_choice"], Some("C2"))).await;
    mount_page(&server, ".../C2", page_with_empty_slug(Some("C3"))).await; // 2 monthly + 1 ""-slug
    mount_page(&server, ".../C3", empty_products_page()).await;
    let c = client(&server).await;
    let walk = c.choice_months(40, std::time::Duration::ZERO, std::time::Duration::from_secs(60)).await.unwrap();
    assert!(matches!(walk.stop, WalkStop::CursorEnd));
}

#[tokio::test]
async fn empty_slug_live_choice_product_is_kept_not_dropped() {
    // M3 fires-anyway: a MIXED boundary page — 1 real choice + 1 EMPTY-machine_name product
    // (a live-Choice month whose slug dropped, the EXACT bug this PR kills) + 1 pre-2019 monthly.
    // The empty-slug product must be KEPT in walk.months (Task 7's ladder may still resolve it),
    // and the walk must NOT era-stop. A subagent that "filters to _choice only" fails HERE.
    let server = wiremock::MockServer::start().await;
    mount_page(&server, ".../", mixed_page(
        &["nov_2019_choice"],   // real choice
        1,                       // one empty-machine_name product (offered game "orphan")
        &["october-2018"],       // one pre-2019 monthly
        Some("C2"),
    )).await;
    mount_page(&server, ".../C2", empty_products_page()).await;
    let c = client(&server).await;
    let walk = c.choice_months(40, std::time::Duration::ZERO, std::time::Duration::from_secs(60)).await.unwrap();
    assert!(matches!(walk.stop, WalkStop::CursorEnd), "mixed page must not era-stop");
    assert!(walk.months.iter().any(|m| m.offered_games.iter().any(|g| g.machine_name == "orphan")),
        "the empty-slug live-Choice product must be KEPT, not silently dropped");
}

#[tokio::test]
async fn walk_deadline_bounds_a_slow_server() {
    // Every page delays 300ms and always hands back a cursor; a 500ms deadline stops
    // the walk as Deadline after ~1-2 pages, not at the cap.
    let server = wiremock::MockServer::start().await;
    mount_slow_always_cursor(&server, std::time::Duration::from_millis(300)).await; // path-regex catch-all, 200 + delay + cursor
    let c = client(&server).await;
    let walk = c.choice_months(40, std::time::Duration::ZERO, std::time::Duration::from_millis(500)).await.unwrap();
    assert!(matches!(walk.stop, WalkStop::Deadline));
    assert!(!walk.complete_for_choice());
}
```

(`mount_page`, `choice_page`, `monthly_page`, `page_with_empty_slug`, `mixed_page`, `empty_products_page`, `mount_slow_always_cursor` are small inline test helpers YOU define at the top of this test module — each wraps `serde_json::json!` + a `Mock::given` mount. `choice_page(&machine_names, cursor)` builds products whose `productMachineName` ends `_choice` and whose `productUrlPath` derives from the name; `monthly_page(&slugs, cursor)` builds non-choice products with those url paths + a `*_monthly` machine_name; `mixed_page(&choice_mns, n_empty, &monthly_slugs, cursor)` builds choice + `n_empty` empty-machine_name products (each offering a game "orphan") + monthly; `page_with_empty_slug` = 2 monthly + 1 empty. They are NOT existing helpers.)

- [ ] **Step 1b: Migrate every existing walk-test call site (M7 — miss one and T5 won't compile).** SIX callers of `choice_months` must move to the 3-arg signature `choice_months(N, Duration::ZERO, Duration::from_secs(60))`: the five pre-existing (`client_test.rs:908, :959, :988, :1028, :1062`) PLUS the NEW 1-arg test Task 4 added (`list_walk_gamekey_is_never_empty_string`). Replace `assert!(walk.complete)`/`assert!(!walk.complete)` with `assert!(walk.complete_for_choice())` / `matches!(walk.stop, WalkStop::Cap)`; `choice_months_max_pages_bounds_a_nonstop_cursor` (:1041) asserts `matches!(walk.stop, WalkStop::Cap)`. Also the `!walk.complete` reader at `fulfillment/src/lib.rs:3209` → `!walk.complete_for_choice()` (Step 3). Grep `choice_months(` across the workspace after this and confirm ZERO 1-arg callers remain.

- [ ] **Step 2: Verify they fail** — `cargo test -p humble-client walk_` → FAIL (no `WalkStop`, wrong signature).

- [ ] **Step 3: Implement.** In `choice_months`:

```rust
pub async fn choice_months(
    &self,
    max_pages: usize,
    pace: std::time::Duration,
    deadline: std::time::Duration,
) -> Result<ChoiceMonthsWalk, HumbleError> {
    const BASE: &str = "/api/v1/subscriptions/humble_monthly/subscription_products_with_gamekeys";
    let started = tokio::time::Instant::now();
    let mut months = Vec::new();
    let mut cursor: Option<String> = None;
    let mut stop = WalkStop::Cap;
    for page_no in 0..max_pages {
        if page_no > 0 {
            if started.elapsed() >= deadline {
                stop = WalkStop::Deadline;
                break;
            }
            tokio::time::sleep(pace).await; // 26+ rapid GETs from a lambda IP is the bot profile SYNC_PACE exists to avoid
        }
        let path = match &cursor {
            None => format!("{BASE}/"),
            Some(c) => format!("{BASE}/{c}"),
        };
        let page: SubProductsPage = self.get_json(&path).await?;
        if page.products.is_empty() {
            stop = WalkStop::CursorEnd; // an empty page is cursor-end, never era-stop (family review)
            break;
        }
        // Era discriminant (family review, final form): the SLUG decides — a product is
        // pre-Choice iff product_machine_name is NON-EMPTY and does not end "_choice".
        // An empty slug (droppable field) is an anomaly-warn and DISQUALIFIES the page
        // from era-stopping: a dropped field must never read as "pre-Choice era".
        let mut page_all_pre_choice = true;
        let mut page_had_anomaly = false;
        for p in &page.products {
            if p.product_machine_name.is_empty() {
                tracing::warn!(title = %p.title, "choice walk: product with empty machine_name — anomaly, page cannot era-stop");
                page_had_anomaly = true;
            } else if p.product_machine_name.ends_with("_choice") {
                page_all_pre_choice = false;
            } else {
                // Chronology sanity: a "pre-choice" slug dated inside the Choice era
                // (url path "<month>-<year>", year >= 2019) is drift, not history.
                if let Some(year) = p.product_url_path.rsplit('-').next().and_then(|y| y.parse::<i32>().ok()) {
                    if year >= 2019 {
                        tracing::warn!(machine_name = %p.product_machine_name, year, "choice walk: non-choice slug dated inside the Choice era — anomaly, page cannot era-stop");
                        page_had_anomaly = true;
                    }
                }
            }
        }
        if page_all_pre_choice && !page_had_anomaly {
            tracing::info!(
                boundary = %page.products[0].product_machine_name,
                pages = page_no + 1,
                "choice walk: reached the pre-Choice era — complete for Choice"
            );
            stop = WalkStop::EraStop;
            break;
        }
        // On a mixed boundary page (some _choice, some not) append ONLY choice products;
        // an empty-slug product is an anomaly kept (Task 7's ladder may resolve it — dropping
        // it silently is the exact bug class this PR kills). Monthly-era products are not months.
        for p in page
            .products
            .into_iter()
            .filter(|p| p.product_machine_name.is_empty() || p.product_machine_name.ends_with("_choice"))
        {
            // gamekey per Task 4: None when the list dropped it, never "".
            let gamekey = p.gamekey.filter(|g| !g.is_empty());
            let mut offered_games: Vec<OfferedGame> = p
                .content_choice_data
                .game_data
                .into_iter()
                .map(|(machine_name, g)| OfferedGame { machine_name, title: g.title })
                .collect();
            offered_games.sort_by(|a, b| a.machine_name.cmp(&b.machine_name));
            months.push(ChoiceMonth {
                gamekey,
                title: p.title,
                product_url_path: p.product_url_path,
                product_machine_name: p.product_machine_name,
                uses_choices: p.uses_choices,
                is_active_content: p.is_active_content,
                can_redeem_games: p.can_redeem_games,
                total_choices: None, // list endpoint carries no pick budget
                offered_games,
                claimed_machine_names: None, // list can't see picks — None (unknown), not Some(empty)
            });
        }
        match page.cursor {
            Some(c) if !c.trim().is_empty() => cursor = Some(c),
            _ => {
                stop = WalkStop::CursorEnd;
                break;
            }
        }
    }
    if matches!(stop, WalkStop::Cap | WalkStop::Deadline) {
        tracing::warn!(max_pages, months = months.len(), stop = ?stop,
            "choice_months stopped before the era boundary — the month list is TRUNCATED, not the full history");
    }
    Ok(ChoiceMonthsWalk { months, stop })
}
```

NOTE the era-check runs BEFORE appending the page's products (monthly-era products are not months) but only skips them when the WHOLE page era-stops; a mixed page (some `_choice`, some not — the boundary page) appends ONLY the `_choice` products and keeps walking. Implement that: when `!page_all_pre_choice`, filter the append to products whose machine_name ends `_choice` OR is empty (empty = anomaly, keep — Task 7's ladder may still resolve it; dropping it silently is the exact bug class this PR kills).

`fulfillment`: `CHOICE_DISCOVERY_MAX_PAGES: usize = 40;` + new `const CHOICE_WALK_DEADLINE: Duration = Duration::from_secs(120);` and the call becomes `choice_months(CHOICE_DISCOVERY_MAX_PAGES, SYNC_PACE, CHOICE_WALK_DEADLINE)`; the `!walk.complete` warn becomes `!walk.complete_for_choice()`.

- [ ] **Step 4: Verify green** — `cargo test --workspace`.
- [ ] **Step 5: Commit** — `git commit -S -m "feat(humble-client): era-aware walk-to-completion — slug-discriminant era-stop, pace, walk deadline, stop-reason; cap 26→40"`

### Task 6: order index + order-authoritative claimed-set (spec D3)

**Files:**
- Modify: `crates/humble-client/src/lib.rs` (`Order` :269 — add `product_machine_name`), `crates/humble-client/src/model.rs` (flow it from the order wire's `product.machine_name` — `ProductWire` at :20 already deserializes the product block)
- Modify: `crates/fulfillment/src/lib.rs` (`run_sync` :2960-3084 collects the index; `discover_choice_games` :3183 takes it and derives claimed-sets)
- Test: `crates/fulfillment/tests/handler_test.rs` + `crates/humble-client/tests/client_test.rs` (Order field)

**Interfaces:**
- Produces:
  ```rust
  /// Everything discovery needs from the order walk. Built by run_sync, passed by ref.
  pub(crate) struct OrderIndex {
      /// gamekey → tpk machine_names (the authoritative claimed record, spec D3)
      tpks_by_gamekey: std::collections::HashMap<String, Vec<String>>,
      /// order product machine_name ("july_2026_choice") → gamekey (D2 ladder rung 3)
      gamekey_by_product: std::collections::HashMap<String, String>,
  }
  ```
  `discover_choice_games(deps, healed, cookie_ok, orders: &OrderIndex) -> u32`
  - The sync test harness helpers (defined in Step 0, consumed by Tasks 6–9).
- Consumes: Task 3's `choice_tpk_matches`.

- [ ] **Step 0: Build the sync test harness (BLOCKER fix — these helpers do NOT exist yet; Tasks 6–9 all depend on them).** `handler_test.rs` has NO fluent harness — only `store_or_skip(test) -> Option<Store>` (:81), `humble_at(uri)` (:895), `order_json(gamekey, machine, redeemed)` (:899, single-tpk, product carries only `human_name`), and `membership_html(...)` (:2610). The sync driver is `handle(&deps, FulfillRequest::Sync).await`, and reads are `deps.store.get_game(&domain::game_id(gk, mn))`. Add these free helpers to the test module (each wraps the real idiom — no new abstraction, just named setup the four tasks share):

```rust
// --- lost-months sync-test helpers (Tasks 6–9) ---

/// Full order JSON with N tpks AND a product machine_name (order_json only does one tpk,
/// no machine_name — insufficient for choice claimed-set tests).
fn choice_order_json(gamekey: &str, product_machine: &str, tpk_machines: &[&str]) -> serde_json::Value {
    let tpks: Vec<_> = tpk_machines.iter().map(|m| serde_json::json!({
        "machine_name": m, "human_name": "Test Game", "key_type": "steam",
        "is_expired": false, "keyindex": 0,
    })).collect();
    serde_json::json!({
        "gamekey": gamekey,
        "product": { "human_name": "Test Choice Month", "machine_name": product_machine },
        "tpkd_dict": { "all_tpks": tpks },
        "subproducts": [],
    })
}

/// Mount the humble endpoints a sync touches and return a wired `Deps`. The order walk is
/// `GET /api/v1/user/order` (returns [{gamekey}]) → `GET /api/v1/order/<gk>?all_tpkds=true`
/// per key; the choice walk is `GET /api/v1/subscriptions/.../` then `/membership/<slug>`.
/// `orders`: (gamekey, product_machine, &[tpk_machine]) — mounted as BOTH a gamekeys-listing
/// entry AND its order body, so the OrderIndex is populated exactly as prod builds it.
/// `sub_list`: (slug, Option<gamekey>) newest-first, mounted as one page then empty.
/// `months`: (slug, membership-blob-html). Returns `deps(store, uri, webhook)`.
async fn sync_deps(
    store: Store, humble: &wiremock::MockServer,
    orders: &[(&str, &str, &[&str])],
    sub_list: &[(&str, Option<&str>)],
    months: &[(&str, String)],
    webhook: Option<String>,
) -> Deps {
    // order walk: the listing carries every order gamekey; each order returns its tpks.
    let listing: Vec<_> = orders.iter().map(|(gk, ..)| serde_json::json!({ "gamekey": gk })).collect();
    Mock::given(method("GET")).and(path("/api/v1/user/order"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!(listing)))
        .mount(humble).await;
    for (gk, product, tpks) in orders {
        Mock::given(method("GET")).and(path(format!("/api/v1/order/{gk}")))
            .respond_with(ResponseTemplate::new(200).set_body_json(choice_order_json(gk, product, tpks)))
            .mount(humble).await;
    }
    // choice walk: one subscription page (products newest-first, no cursor = complete).
    let products: Vec<_> = sub_list.iter().map(|(slug, gk)| {
        let mn = format!("{}_choice", slug.replace('-', "_"));
        serde_json::json!({
            "gamekey": gk, "title": slug, "productUrlPath": slug, "productMachineName": mn,
            "usesChoices": false, "isActiveContent": false, "canRedeemGames": true,
            "contentChoiceData": { "game_data": {} }
        })
    }).collect();
    Mock::given(method("GET")).and(path(format!("{CHOICE_LIST_BASE}/")))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({ "products": products })))
        .mount(humble).await;
    for (slug, blob) in months {
        Mock::given(method("GET")).and(path(format!("/membership/{slug}")))
            .respond_with(ResponseTemplate::new(200).set_body_string(blob.clone()).append_header("content-type", "text/html"))
            .mount(humble).await;
    }
    deps(store, &humble.uri(), webhook)
}

/// All GAME rows whose id starts with "<gamekey>:" — via list_all_games (dynamo:1985);
/// there is no prefix-scan in the store, so a full list + filter is the mechanism.
async fn games_for_gamekey(store: &Store, gamekey: &str) -> Vec<domain::Game> {
    let prefix = format!("{gamekey}:");
    store.list_all_games().await.unwrap().into_iter().filter(|g| g.id.starts_with(&prefix)).collect()
}
```

The `months` arg takes the `membership_html(slug, gamekey, title, &[(mn,title)], &[chosen])` output (handler_test.rs:2610). In later Task 6–9 snippets, `mount_choice_month(slug, offered, chosen)` is shorthand for a `(slug, membership_html(slug, gk, title, offered, chosen))` entry in `months`; `mount_order_with_tpks(gk, product, tpks)` is an `orders` entry; `mount_subscription_list_with(&[...])` is `sub_list`; `mount_order_failure(gk)` is: instead of mounting that gk's order 200, mount it as `ResponseTemplate::new(500)` (list it in `orders` for the listing entry but override its order mount to 500 — or add a `failed_orders: &[&str]` param). `h.run_sync()` = `handle(&deps, FulfillRequest::Sync).await`. `games_with_prefix(p)`/`all_games()` = `games_for_gamekey`/`list_all_games`.

Commit this alone: `git commit -S -m "test(fulfillment): sync-test harness for choice discovery (multi-tpk order json, sync_deps, games_for_gamekey)"`. Every Task 6–9 test below is written against THESE names; `h.mount_choice_month(...)` etc. in later snippets are shorthand for "mount via `sync_deps`'s `months`/`orders`/`sub_list` args" — translate each to a `sync_deps(...)` call + `handle(&deps, FulfillRequest::Sync).await` + `games_for_gamekey(...)` assertions. Guard every test with `let Some(store) = store_or_skip("name").await else { return };`.

- [ ] **Step 1: Order wire test.** In `client_test.rs`, extend the existing `order()` test to assert `order.product_machine_name == "<the fixture's product.machine_name>"` (add a `"machine_name"` to the fixture's `product` block if absent — today `ProductWire` at model.rs:20 deserializes ONLY `human_name`, so `machine_name` is a NEW field on both the wire struct and `Order`). Run → FAIL (field missing). Implement: add `#[serde(default)] pub machine_name: String` to `ProductWire` (model.rs:20) and `pub product_machine_name: String` to `Order`, flow it through (an order without it yields `""`, which never matches a ladder lookup). Green. Commit `-S -m "feat(humble-client): Order carries product_machine_name"`.

- [ ] **Step 2: Failing fulfillment test — claimed-set is order-authoritative.** In `handler_test.rs`, following the file's existing wiremock sync-test idiom (mock humble endpoints + moto/local store, run `run_sync`, assert on written rows):

```rust
#[tokio::test]
async fn discovery_claimed_set_comes_from_the_order_not_the_blob() {
    // Month fixture: 3 offered games (alpha, beta, gamma). Blob claims NOTHING
    // (contentChoicesMade absent — the vintage no-picks shape). The month's ORDER
    // carries tpk "beta_row_choice_steam" → beta IS claimed, whatever the blob says.
    let h = sync_harness().await; // the file's existing setup idiom
    h.mount_choice_month("december-2020", offered(&["alpha", "beta", "gamma"]), claims(&[])).await;
    h.mount_order_with_tpks("GKDEC2020", "december_2020_choice", &["beta_row_choice_steam"]).await;
    h.run_sync().await;
    let games = h.games_with_prefix("GAME#GKDEC2020").await;
    let names: Vec<_> = games.iter().map(|g| g.machine_name.as_str()).collect();
    assert!(names.contains(&"alpha") && names.contains(&"gamma"));
    assert!(!names.contains(&"beta"), "order-claimed game must not be written claimable");
}

#[tokio::test]
async fn discovery_skips_month_loudly_when_order_is_silent() {
    // Month enumerated + parseable, but its order read FAILED this pass (mock 500) →
    // no requires_choice rows for that month this pass; a warn fires; next sync retries.
    let h = sync_harness().await;
    h.mount_choice_month("november-2020", offered(&["delta"]), claims(&[])).await;
    h.mount_order_failure("GKNOV2020").await;
    h.run_sync().await;
    assert!(h.games_with_prefix("GAME#GKNOV2020").await.is_empty(),
        "no claimed-set source ⇒ no claimable writes ⇒ no ghost rows");
}

#[tokio::test]
async fn discovery_canary_counts_unmatched_choice_tpks() {
    // minor (T6 canary was implemented in T6 but only exercised via T8's path). A 1:N grant:
    // the order carries a base tpk that matches an offered game PLUS a DLC tpk that matches
    // nothing offered. offered "kappa" is claimed (base tpk present) → not written; the DLC
    // tpk is the canary — the month still processes, kappa is removed, nothing errors.
    let h = sync_harness().await;
    h.mount_subscription_list_with(&[("may-2021b", Some("GKMAY"))]).await;
    h.mount_choice_month("may-2021b", offered(&["kappa"]), claims(&[])).await;
    h.mount_order_with_tpks("GKMAY", "may_2021_choice", &["kappa_choice_steam", "kappa_dlc_choice_steam"]).await;
    h.run_sync().await;
    // kappa is order-claimed (base tpk) → not written claimable; the unmatched DLC tpk is
    // counted (canary), never fatal — the month processed without error.
    assert!(h.games_with_prefix("GAME#GKMAY").await.iter().all(|g| g.machine_name != "kappa"),
        "order-claimed kappa not re-listed; unmatched DLC tpk is canary-counted, not fatal");
}
```

- [ ] **Step 3: Verify both fail** (current code trusts the blob and knows nothing of orders).

- [ ] **Step 4: Implement.** In `run_sync`'s order loop (:3050 region), build the index as orders succeed:

```rust
    let mut order_index = OrderIndex::default();
    // … inside the per-gamekey loop, after a successful order read:
    order_index.tpks_by_gamekey.insert(
        order.gamekey.clone(),
        order.keys.iter().map(|k| k.machine_name.clone()).collect(),
    );
    if !order.product_machine_name.is_empty() {
        order_index.gamekey_by_product.insert(order.product_machine_name.clone(), order.gamekey.clone());
    }
```

(A failed order read inserts nothing — absence from `tpks_by_gamekey` IS the "order silent" signal.) **Thread the signature (MAJOR fix):** `discover_choice_games` gains a fourth param `orders: &OrderIndex`; declare `order_index` BEFORE the order loop (:3050 region) so it outlives the loop, populate it inside, and update the call at `lib.rs:3100` from `discover_choice_games(deps, &mut healed_this_run, &mut cookie_ok)` to `discover_choice_games(deps, &mut healed_this_run, &mut cookie_ok, &order_index)`. Then in `discover_choice_games`, replace the `claimable_games()` block (:3262-3269) with:

```rust
        // Spec D3: claimed is ORDER-authoritative; the blob never alone marks a game
        // claimed. Order silent (read failed this pass — the ladder guarantees an order
        // exists) ⇒ skip the month LOUDLY and let the next sync retry: writing claimable
        // rows on missing evidence would mint ghosts that additive-never-delete keeps forever.
        let Some(order_tpks) = orders.tpks_by_gamekey.get(&month_gamekey) else {
            tracing::warn!(month = %detail.product_url_path, gamekey = %month_gamekey,
                "choice discovery: order silent for month — skipping this pass (claimed-set unknowable, retried next sync)");
            continue;
        };
        let claimable: Vec<&OfferedGame> = detail
            .offered_games
            .iter()
            .filter(|o| !order_tpks.iter().any(|t| domain::choice_tpk_matches(t, &o.machine_name)))
            .collect();
        // Canary (spec D3 rung 2): a _choice_* tpk matching NO offered name — expected
        // for 1:N grants (base+DLC), logged never month-fatal.
        let unmatched = order_tpks.iter()
            .filter(|t| domain::choice_tpk_bases(t).is_some())
            .filter(|t| !detail.offered_games.iter().any(|o| domain::choice_tpk_matches(t, &o.machine_name)))
            .count();
        if unmatched > 0 {
            tracing::warn!(month = %detail.product_url_path, unmatched, "choice discovery: choice tpks matching no offered name (1:N grants or new grammar) — counted, not fatal");
        }
```

The blob's `claimed_machine_names` no longer participates in claimed-set derivation (offered-side only, per spec). Delete the `claimable_games()` call; keep `claimed = detail.claimed_machine_names` OUT of the subtraction.

- [ ] **Step 5: Verify green; update the three existing discovery tests (M9 — named, not "investigate").** These read claimed-set from the blob's `chosen` arg today and MUST be migrated to mount an order (D3 is now order-authoritative), else they either fail or pass vacuously:
  - `sync_discovers_offered_choice_games_as_requires_choice_true` (handler_test.rs:2647) — today passes `&["already_picked"]` as blob-chosen; now mount an order for `gkJun26` whose tpks include `already_picked_choice_steam`, and keep the assertion that `already_picked` is NOT written claimable (proof it's the ORDER, not the blob, that removes it).
  - `sync_choice_discovery_skips_page_non_redeemable_month` (:2736) — mount its month's order (empty tpks fine); the `canRedeemGames=false` skip is unchanged, but it now needs an order present so the skip isn't masked by an order-silent skip.
  - `sync_discovers_claim_all_tier_offers` (:2798) — claim-all tier (`usesChoices=false`); mount its order with the already-claimed subset as `_choice_steam` tpks; assert offered−claimed writes match.
  Run the full `--workspace` suite (with `DYNAMODB_LOCAL_URL` set — a `SKIP` here is a failed verify, not a pass).
- [ ] **Step 6: Commit** — `git commit -S -m "feat(fulfillment): claimed-set is order-authoritative via the tpk grammar; order-silent months skip loudly (spec D3)"`

### Task 7: the gamekey ladder + shape observability (spec D2 + D5)

**Files:**
- Modify: `crates/fulfillment/src/lib.rs` (`discover_choice_games` — replace Task 4's transitional skip)
- Test: `crates/fulfillment/tests/handler_test.rs`

**Interfaces:**
- Consumes: Task 4's `detail.gamekey: Option<String>`, Task 5's walk months (list gamekeys), Task 6's `OrderIndex.gamekey_by_product`.
- Produces: every processed month has a real gamekey or a structured skip; log field `gamekey_source` ∈ `blob|list|order|none`.

- [ ] **Step 1: Failing tests**

```rust
#[tokio::test]
async fn gamekey_ladder_blob_absent_list_hit() {
    // Detail blob drops gamekey (the may-2020 shape); the LIST carries it for the slug.
    let h = sync_harness().await;
    h.mount_subscription_list_with(&[("may-2020", Some("GKMAY2020"))]).await;
    h.mount_choice_month_without_gamekey("may-2020", offered(&["epsilon"])).await;
    h.mount_order_with_tpks("GKMAY2020", "may_2020_choice", &[]).await;
    h.run_sync().await;
    let games = h.games_with_prefix("GAME#GKMAY2020").await;
    assert_eq!(games.len(), 1, "ladder resolved via list; month processed");
}

#[tokio::test]
async fn gamekey_ladder_probe_month_resolves_via_order_product() {
    // The active month: NOT in the list (probe), blob drops gamekey — rung 3 matches the
    // order whose product_machine_name is the slug's underscore form + "_choice".
    let h = sync_harness().await;
    h.mount_subscription_list_with(&[]).await;
    h.mount_choice_month_without_gamekey("july-2026", offered(&["zeta"])).await;
    h.mount_order_with_tpks("GKJUL2026", "july_2026_choice", &[]).await;
    h.run_sync().await;
    assert_eq!(h.games_with_prefix("GAME#GKJUL2026").await.len(), 1, "active month resolved via order side");
}

#[tokio::test]
async fn gamekey_ladder_all_absent_skips_only_that_month() {
    // minor (T7 test#3 was a fake red — Task 4's transitional skip also wrote nothing, so it
    // passed at Step 2 without the ladder). Distinguish: TWO months — "march-2020" has no
    // gamekey anywhere (skipped), "april-2020" resolves via list (written). Proves the skip is
    // per-month, not a blanket no-op.
    let h = sync_harness().await;
    h.mount_subscription_list_with(&[("march-2020", None), ("april-2020", Some("GKAPR"))]).await;
    h.mount_choice_month_without_gamekey("march-2020", offered(&["eta"])).await;
    h.mount_choice_month_without_gamekey("april-2020", offered(&["theta"])).await;
    h.mount_order_with_tpks("GKAPR", "april_2020_choice", &[]).await;
    h.run_sync().await;
    assert!(h.all_games().await.iter().all(|g| g.machine_name != "eta"), "no gamekey ⇒ march skipped");
    assert!(h.games_with_prefix("GAME#GKAPR").await.iter().any(|g| g.machine_name == "theta"), "april resolved via list ⇒ written");
}
```

- [ ] **Step 2: Verify they fail** (Task 4's transitional skip means the first two write nothing).

- [ ] **Step 3: Implement.** Replace the transitional `let Some(month_gamekey) = …` with the ladder. The walk's per-slug gamekeys are collected before the target loop:

```rust
    let list_gamekey: std::collections::HashMap<&str, &str> = walk
        .months
        .iter()
        .filter_map(|m| Some((m.product_url_path.as_str(), m.gamekey.as_deref()?)))
        .collect();
```

and inside the per-month loop, after the detail read. **`slug` binding (M8):** the loop this
inserts into is the existing `for (slug, is_probe) in &targets` (fulfillment lib.rs:3232), so
`slug: &String` is already bound to the month's url-path form ("july-2026"). The load-bearing
invariant, stated so it isn't guessed: **`slug` == the list's `product_url_path` key == the
input to the order-product construction** (`slug.replace('-','_') + "_choice"`). All three are
the same month identity; the ladder relies on it:

```rust
        // Spec D2: the gamekey ladder — blob → list → order-side → loud skip. Never "".
        // Rung 3 derives the order-product machine_name from the slug ("july-2026" →
        // "july_2026_choice"). CONFIRMED against prod (2026-07-05 findings: the order
        // endpoint's product.machine_name IS "<month>_<year>_choice", e.g.
        // "april_2026_choice") — the same form the membership blob's productMachineName
        // carries, so the two endpoints agree. This is the SOLE resolution for the active
        // month (A1). A rung-3 MISS (slug_product not in the index) on a probe month is
        // logged by the "gamekey_source = none" skip below — a shape drift surfaces as a
        // visible skip, never a silently-dark month.
        let slug_product = format!("{}_choice", slug.replace('-', "_"));
        let (month_gamekey, gamekey_source) = match (
            detail.gamekey.as_deref(),
            list_gamekey.get(slug.as_str()).copied(),
            orders.gamekey_by_product.get(&slug_product).map(String::as_str),
        ) {
            (Some(g), _, _) => (g.to_string(), "blob"),
            (None, Some(g), _) => (g.to_string(), "list"),
            (None, None, Some(g)) => (g.to_string(), "order"),
            (None, None, None) => {
                tracing::warn!(month = %slug, gamekey_source = "none",
                    choices_made_absent = detail.claimed_machine_names.is_none(),
                    "choice discovery: no gamekey from any rung — skipping (shape logged)");
                continue;
            }
        };
        if gamekey_source != "blob" {
            tracing::warn!(month = %slug, gamekey_source, "choice discovery: blob dropped gamekey — resolved via ladder");
        }
```

- [ ] **Step 3b: Bound the detail-read fan-out (M6 — OMBB's spec-gate arithmetic rider, done).** The Task-5 list-walk deadline (120s) bounds only the LIST walk; the ~77 per-month DETAIL reads are bounded only by the Task-1 per-request 30s timeout, which does NOT bound them in aggregate (77 × 30s = 38min worst case if every read hung). Add a whole-pass deadline: `const CHOICE_DISCOVERY_DEADLINE: Duration = Duration::from_secs(180);`, capture `let started = tokio::time::Instant::now();` at the top of `discover_choice_games`, and at the top of the per-month `for (slug, is_probe) in &targets` loop:

```rust
        if started.elapsed() >= CHOICE_DISCOVERY_DEADLINE {
            tracing::warn!(processed = months_processed, "choice discovery: pass deadline — partial pass (additive, retried next sync)");
            break;
        }
```

Safe because discovery is additive (a month not reached this pass surfaces next sync). Arithmetic against A5: list 12s + detail-fan-out bounded 180s + D7 reads 2s ≈ 194s worst-bounded, inside the 393s headroom (900−507 baseline); nominal added ≈ +40s. NO new order reads (OrderIndex is built by the existing order walk, not re-fetched here).

- [ ] **Step 4: Verify green** — the Task-4 transitional-skip warn text is gone from the codebase (grep for `"ladder lands in a later task"` → zero hits).
- [ ] **Step 5: Commit** — `git commit -S -m "feat(fulfillment): gamekey resolution ladder blob→list→order + whole-pass deadline + structured shape logging (spec D2/D5, A5)"`

### Task 8: D7 — choice tpks flip the offered row instead of minting siblings

**Files:**
- Modify: `crates/fulfillment/src/lib.rs:3050-3080` (the order-walk key loop)
- Test: `crates/fulfillment/tests/handler_test.rs`

**Interfaces:**
- Consumes: Task 3's `choice_tpk_bases`, dynamo's `get_game(&self, id: &str) -> Result<Option<Game>, StoreError>` (:434).
- Produces: A6a — one GAME row per game across the claim lifecycle.

- [ ] **Step 1: Failing test (the A6a fixture — the spec's one deliberate carve-out from verified-in-prod)**

```rust
#[tokio::test]
async fn choice_tpk_flips_the_offered_row_and_stays_stable_across_a_second_sync() {
    // (harness shorthand → sync_deps per Task 6 Step 0.) Sync 1: discovery writes offered
    // row GK1:omega (requires_choice=true). Between syncs the pick is spent — the order now
    // carries "omega_row_choice_steam". Sync 2 must FLIP GK1:omega and NOT mint a sibling.
    // Sync 3 (M10): the KEY invariant — id stability rests on the routing re-deriving the
    // candidate id EVERY sync, NOT on merge_sync keeping machine_name (post-flip the row is
    // Available → merge_sync's ..fresh branch makes machine_name the tpk name). A third sync
    // with the same tpk must STILL target GK1:omega, not re-split into a sibling.
    let store = store_or_skip("d7-flip-stable").await; let Some(store) = store else { return };
    let humble = MockServer::start().await;
    let mut deps = sync_deps(store, &humble,
        &[("GK1", "june_2021_choice", &[])],
        &[("june-2021", Some("GK1"))],
        &[("june-2021", membership_html("june-2021","GK1","June 2021",&[("omega","Omega")],&[]))],
        None).await;
    handle(&deps, FulfillRequest::Sync).await;
    assert_eq!(games_for_gamekey(&deps.store, "GK1").await.len(), 1);

    // remount GK1's order with the spent-pick tpk, then sync twice more.
    remount_order(&humble, "GK1", "june_2021_choice", &["omega_row_choice_steam"]).await;
    for pass in 2..=3 {
        handle(&deps, FulfillRequest::Sync).await;
        let games = games_for_gamekey(&deps.store, "GK1").await;
        assert_eq!(games.len(), 1, "pass {pass}: no sibling minted: {:?}", games.iter().map(|g| &g.id).collect::<Vec<_>>());
        assert_eq!(games[0].id, "GK1:omega", "pass {pass}: routing re-derived the stable id");
        assert!(!games[0].requires_choice, "pass {pass}: flipped");
    }
}

#[tokio::test]
async fn flip_preserves_app_owned_state() {
    // minor→major: the merge_sync guarantee that makes routing SAFE — hidden / claim_id /
    // Manual-appid survive the flip. Seed an offered row that's been hidden + Manual-appid'd,
    // then flip it; those app-owned fields must persist (merge_sync's Pending/Gifted-or-hidden
    // preservation), only requires_choice + key fields refresh.
    let store = store_or_skip("d7-preserve").await; let Some(store) = store else { return };
    // seed GK3:sigma as an offered row with hidden=true, appid_source=Manual, a set steam_app_id
    seed_offered_game(&store, "GK3", "sigma", |g| { g.hidden = true; g.appid_source = Some(AppidSource::Manual); g.steam_app_id = Some(12345); }).await;
    let humble = MockServer::start().await;
    let deps = sync_deps(store, &humble, &[("GK3","x_choice",&["sigma_choice_steam"])], &[], &[], None).await;
    handle(&deps, FulfillRequest::Sync).await;
    let g = deps.store.get_game(&game_id("GK3","sigma")).await.unwrap().unwrap();
    assert!(!g.requires_choice, "flipped");
    assert!(g.hidden, "hidden preserved across flip");
    assert_eq!(g.appid_source, Some(AppidSource::Manual), "Manual appid wins");
    assert_eq!(g.steam_app_id, Some(12345), "Manual appid value preserved");
}

#[tokio::test]
async fn unparseable_choice_tpk_still_mints_normally_and_loudly() {
    // D7 knot: grammar can't parse it → still a row, under its own id, WITH a warn (minor:
    // the "loud" half was unasserted). "weird_choice_" fails choice_tpk_bases (empty platform).
    let store = store_or_skip("d7-unparseable").await; let Some(store) = store else { return };
    let humble = MockServer::start().await;
    let deps = sync_deps(store, &humble, &[("GK2","some_bundle",&["weird_choice_"])], &[], &[], None).await;
    handle(&deps, FulfillRequest::Sync).await;
    // mints under the tpk id (choice_tpk_bases returned None → no routing)
    assert!(deps.store.get_game(&game_id("GK2","weird_choice_")).await.unwrap().is_some());
    // "loud": if the file has a log-capture idiom, assert the mint/warn; if not, this test at
    // least proves the mint. (The warn lives at the D7 read-error/fallthrough site.)
}

#[tokio::test]
async fn d7_candidate_read_error_fails_safe_to_tpk_id() {
    // minor: the get_game Err → mint-under-tpk-id fail-safe branch. If dynamodb-local can be
    // made to error the candidate read this is exercised; otherwise this documents the branch
    // the code MUST have (a read error must never DROP the key — mint under the tpk id, warn).
    // If the harness can't inject a store-read error cheaply, assert the branch exists by
    // code-review and SKIP with an explicit eprintln rather than omit it.
}
```

`remount_order`, `seed_offered_game(store, gk, mn, mutator)` are two more small helpers alongside Task 6 Step 0's set (`remount_order` re-mounts a gk's `/api/v1/order/<gk>` with fresh tpks; `seed_offered_game` writes a `requires_choice=true` Game with a closure to tweak fields, using the store's game-write path).

- [ ] **Step 2: Verify the first fails** (two rows today — the prod pair-factory reproduced in a fixture).

- [ ] **Step 3: Implement.** In the order-walk key loop (:3051), before constructing `Game`:

```rust
        for key in &order.keys {
            // Spec D7: a choice-suffixed tpk may be the post-claim record of a game
            // discovery already surfaced under the OFFERED name — route onto that row so
            // merge_sync flips it, instead of minting the sibling (15 live pairs in prod,
            // 2026-07-31 scan). Ladder, not set: exact base first (an offered name may
            // itself end _row), region-stripped second, FIRST HIT wins.
            let mut id = domain::game_id(&order.gamekey, &key.machine_name);
            if let Some((exact, stripped)) = domain::choice_tpk_bases(&key.machine_name) {
                for candidate in std::iter::once(exact).chain(stripped) {
                    let candidate_id = domain::game_id(&order.gamekey, &candidate);
                    match deps.store.get_game(&candidate_id).await {
                        Ok(Some(_)) => {
                            id = candidate_id; // offered row exists (whatever its current state — a
                            break; // previously-flipped row keeps receiving refreshes here forever)
                        }
                        Ok(None) => {}
                        Err(e) => {
                            // Routing must fail SAFE: on a read error, mint under the tpk id
                            // (the pre-D7 behavior) rather than dropping the key. A pair is
                            // loud and healable; a lost key is not.
                            tracing::warn!(error = ?e, candidate = %candidate_id, "D7 candidate lookup failed — minting under tpk id");
                            break;
                        }
                    }
                }
            }
            let game = Game { id, /* …existing fields unchanged… */ };
```

The per-key routing adds one `get_game` read per choice-shaped tpk (~175 catalog-wide, ≤2 candidate reads each) every sync — ~1-2s, inside the A5 budget headroom (507s/900s) but real; noted so it's counted, not a surprise.

(`merge_sync` already handles the flip: fresh `requires_choice: false` wins, key fields refresh — domain :325-345. The routed write changes ONLY which existing row `upsert_game_from_sync` merges onto. **Id stability rests on THIS routing re-deriving `candidate_id` every sync — NOT on `merge_sync` preserving `machine_name`.** An offered row is written `status=Available`, which hits merge_sync's Available branch (`..fresh`) where fresh wins entirely, so `machine_name` becomes the tpk name post-flip; the id stays `GK:offered` only because the routing keeps targeting it. The A6a test asserts `id` + `requires_choice`, which is what actually matters — don't rely on the machine_name field staying the offered name.)

- [ ] **Step 4: Verify green; workspace suite.**
- [ ] **Step 5: Commit** — `git commit -S -m "feat(fulfillment): choice tpks route onto their offered row — the duplicate-pair factory closes (spec D7, A6a)"`

### Task 9: run summary + the refusal-ping tripwire (spec D5 + the D3 dependency)

**Files:**
- Modify: `crates/fulfillment/src/lib.rs` (`discover_choice_games` summary line)
- Test: `crates/fulfillment/tests/handler_test.rs`

**Interfaces:**
- Produces: one INFO summary per discovery pass: `months_walked`, `months_processed`, `months_skipped`, `stop_reason`, `canary_unmatched_tpks`; plus the tripwire test.

- [ ] **Step 1: Summary.** Accumulate counters through `discover_choice_games` (walked = targets len, processed = months reaching the write loop, skipped = the `continue` sites, canary = summed unmatched counts) and emit:

```rust
    tracing::info!(
        months_walked, months_processed, months_skipped,
        stop_reason = ?walk.stop, canary_unmatched_tpks,
        "choice discovery: pass summary"
    );
```

Test: extend an existing discovery test to assert the summary fields via the harness's log capture if the file has one; if it does not, the counters are exercised implicitly by the skip tests — do NOT build a log-capture harness for this task (YAGNI).

- [ ] **Step 2: The tripwire (spec D3 dependency — "this trade is sound only while claim-time refusals ping").** Find the existing dead-key-truth test coverage for refusal pings in `handler_test.rs` (search `RedeemRefused` / `ping`). Required behavior: a `choosecontent` refusal ("already chosen" / `success:false`) on a claim parks WITH a webhook ping.

  **The verification is MANDATORY IN BOTH BRANCHES (M4 — a comment is not a guard).** Whether you reuse an existing test or write a new one, you MUST prove it reds when the ping is removed before it counts as the tripwire:
  1. Comment out the ping call at the choose-refusal park site.
  2. Run the candidate test → it MUST fail (if it still passes, it is NOT the tripwire — it doesn't actually assert the ping; find or write the real one).
  3. Restore the ping. Test greens again.

  Only after step 2 reds may you tag it. If reusing an existing test, ALSO assert the webhook body names the game (if it doesn't already) — a park-without-ping-body test is too loose to be the tripwire. Then add:

```rust
// TRIPWIRE (lost-months spec D3): the order-authoritative claimed-set trade is sound
// only while this ping fires — an under-claimed game surfaces claimable, fails its
// choose loudly, and an operator sees it. VERIFIED to red when the ping is removed
// (plan Task 9 Step 2). If this assertion ever goes, that trade silently turns lossy.
// Do not delete; renegotiate the spec instead.
```

If no such test exists, write it following the file's park-and-ping idioms (mock humble `POST /humbler/choosecontent` → `{"success": false}`, drive a claim on a `requires_choice` game, assert the claim parks AND the mock webhook received a ping whose body names the game), then run the same comment-out/red/restore proof above.

- [ ] **Step 3: Verify green; commit** — `git commit -S -m "feat(fulfillment): discovery pass summary + refusal-ping tripwire test (spec D5, D3 dependency)"`

### Task 10: the heal script + runbook (spec Q5, A6b)

**Files:**
- Create: `crates/fulfillment/src/bin/heal_choice_pairs.rs` (modeled on the existing operator-bin precedent `crates/fulfillment/src/bin/backfill_details.rs` — copy its Deps/env bootstrapping)
- Create: `docs/runbook-choice-pair-heal.md`

**Interfaces:**
- Consumes: `choice_tpk_bases`/`choice_tpk_matches`, `Store::get_game`, the store's game-listing read (use whatever full-scan/list the backfill bin uses), `HumbleClient::order`.
- Produces: an operator bin — dry-run by default, `--execute` to delete; and the A6b procedure.

- [ ] **Step 1: Write the bin.** Behavior (spec Q5, all five bullets are REQUIREMENTS):

```rust
//! heal_choice_pairs — one-time sweep for the 15 legacy offered/tpk duplicate pairs
//! (spec Q5 / A6b, family-signed 2026-07-31). DRY-RUN BY DEFAULT; `--execute` deletes.
//!
//! This is NOT delete-on-absence (that contract stands, undiluted): every delete rests
//! on positive dual evidence, re-derived LIVE at execution time —
//!   1. the sibling's id derives to the offered id through domain::choice_tpk_bases, AND
//!   2. the offered row exists and carries the key fields post-flip (requires_choice ==
//!      false), AND
//!   3. the month's LIVE order (fetched now, not from the scheduling scan) carries a tpk
//!      matching the offered name — the order authorizes, the scan only schedules.
//! State-gate (skip + print any pair failing it): sibling.status == Available,
//! sibling.claim_id.is_none(), !sibling.hidden, sibling.appid_source != Some(Manual).
//! (mylittleuniverse's sibling fails the gate today — expected; heal by hand post-claim.)
```

main flow: list all GAME rows (`Store::list_all_games`, dynamo:1985) → group by gamekey → for each row whose machine_name is choice-shaped (`choice_tpk_bases(..).is_some()`), ladder its candidate ids; where a candidate row exists in the same gamekey → a PAIR. Print every pair with its gate verdict. With `--execute`: for gate-passing pairs, fetch the live order (`HumbleClient::order`), re-verify evidence 1-3, then delete the sibling row. Print a final `healed=N skipped=M` line.

**The delete (BLOCKER fix — feature-gated BOTH the method AND the bin, or the workspace won't build):**
The subtlety OMBB's step-5 gate caught: a plain auto-discovered `src/bin/heal_choice_pairs.rs` is compiled by EVERY default-feature build, including `cargo test --workspace`. If it calls `store.delete_game(..)` but `delete_game` is `#[cfg(feature="heal")]`, the default build gets E0599 — the workspace won't compile at all. The bin's compilation must be gated too, mirroring `backfill_details`:

1. `crates/dynamo/Cargo.toml` `[features]`: add `heal = []`.
2. `crates/fulfillment/Cargo.toml` `[features]`: add `heal = ["dynamo/heal"]` (`required-features` only accepts package-local features, so it forwards through fulfillment), AND declare an explicit bin target so the default build SKIPS it:
   ```toml
   [[bin]]
   name = "heal_choice_pairs"
   required-features = ["heal"]
   ```
3. In `crates/dynamo/src/lib.rs`, next to `delete_session` (:2176), add the feature-gated method (`schema::game_pk`/`schema::key_pair` are already `pub`):
   ```rust
   /// Delete a GAME row by id. Feature-gated to `heal` — compiled ONLY for the
   /// heal_choice_pairs operator bin, NEVER the lambda build, so "the sync path
   /// never deletes" is a compiler guarantee, not a discipline. (spec Q5)
   #[cfg(feature = "heal")]
   pub async fn delete_game(&self, id: &str) -> Result<(), StoreError> {
       let (pk, sk) = schema::key_pair(schema::game_pk(id), "META");
       self.client.delete_item().table_name(&self.table)
           .key("pk", pk).key("sk", sk).send().await?;
       Ok(())
   }
   ```
4. EVERY command that builds/runs the bin carries `--features heal`: the Step-4 build (`cargo build -p fulfillment --bin heal_choice_pairs --features heal`), and both runbook commands (`cargo run -p fulfillment --bin heal_choice_pairs --features heal [-- --execute]`). The default `--workspace` build skips the bin entirely (required-features unmet), so it stays green; a stray `deps.store.delete_game(..)` in sync code still fails to compile in the lambda build — the guarantee holds, the workspace builds.

- [ ] **Step 1a: TDD ordering (minor — define the testable core FIRST).** Before writing the bin's main flow, extract and TEST the pure decision so it's real red-green, not test-after: `fn pair_verdict(sibling: &Game, offered: Option<&Game>, live_order_tpks: &[String]) -> Verdict`. Write the four failing tests below, watch them fail, then implement `pair_verdict`, then build the main flow around it.

- [ ] **Step 2: Test the pairing/gate logic.** Unit-test `pair_verdict` in-file: gate-pass → `Heal`, claim_id present → `Skip("claim-entangled")`, offered row still `requires_choice=true` → `Skip("not flipped yet — run after a post-D7 sync")`, live order missing the matching tpk → `Skip("order does not corroborate")`. (These are the Step-1a tests; this step is where they go green.)

- [ ] **Step 3: Runbook.** `docs/runbook-choice-pair-heal.md`:

```markdown
# Choice duplicate-pair heal (one-time, spec Q5 / A6b)

Preconditions: the D7 sync change is DEPLOYED and at least one sync has run
(the flip must exist before the sweep — close the factory, then sweep).

1. `AWS_PROFILE=kitten-maintenance cargo run -p fulfillment --bin heal_choice_pairs`
   (dry-run). Read the printed pair list + gate verdicts. Expect ~14 Heal + 1
   claim-entangled Skip (mylittleuniverse).
2. Re-run with `--execute`. Every delete re-derives its evidence from the live
   order at execution time.
3. A6b verify: re-run the dry-run — expect ZERO gate-eligible pairs. Paste both
   outputs into the PR/issue thread.
4. mylittleuniverse: after its pending claim resolves, re-run steps 1-3; if the
   gate still refuses, heal by hand with four eyes on it.
```

- [ ] **Step 4: Verify** — `cargo build -p fulfillment --bin heal_choice_pairs --features heal` + `cargo test -p fulfillment --features heal pair_verdict` green + `cargo test --workspace` STILL green (the bin is skipped without `--features heal`, so the default build must be unaffected — this is the B1 check) + dry-run fails FAST/clear without AWS creds, not hang.
- [ ] **Step 5: Commit** — `git commit -S -m "feat(fulfillment): heal_choice_pairs operator bin + runbook — Q5 sweep, dry-run default, evidence re-derived live (A6b)"`

---

## Post-execution (arc steps, not plan tasks)

PR (layer 2) onto `kitten/client-timeouts` until layer 1 merges, then rebase onto `main`. Deploy = the existing terraform lambda path (`terraform/aws-lambda.tf`, `kitten-deploy` role) after both PRs merge. Prod acceptance = spec A1-A5 verified in CloudWatch (the queries from the spec header), then the Task-10 runbook for A6b. `#53` closes on the A1-A4 receipts.

## Self-Review + cold-review fold-in (2026-07-31)

- Spec coverage: D1→T4, D2→T7, D3→T3+T6, D4→T5, D5→T7+T9, D6→spread across every task's tests, D7→T8, Q3→T4, Q4→T1-2, Q5→T10, A6a→T8, A6b→T10, tripwire→T9. No gaps.
- Types: `choice_tpk_bases` returns `(String, Option<String>)` everywhere (T3/T8/T10); `ChoiceMonthsWalk.stop`/`complete_for_choice()` consistent T5/T6; `OrderIndex` fields consistent T6/T7.
- **Adversarial cold-subagent review (implementation-plan-review, 3 blockers + 4 majors) folded in:**
  - BLOCKER-1 (`ContentChoiceData: Default` compile break) → T4 Step 4a derives it explicitly.
  - BLOCKER-2 (the harness fiction — `sync_harness` etc. don't exist; `store_or_skip` skips silently without dynamodb-local) → T6 Step 0 builds concrete named helpers (`choice_order_json`, `sync_deps`, `games_for_gamekey`) against the REAL idiom (`handle(Sync)`, `list_all_games`, `order_json` extended for multi-tpk+product-machine-name); Global Constraints state the dynamodb-local + `DYNAMODB_LOCAL_URL` precondition.
  - BLOCKER-3 (Task 10 delete impossible-as-worded) → feature-gated `dynamo::Store::delete_game` under `heal`, concrete key-pair code, compiler-enforced sync-can't-delete.
  - MAJOR-A (T4/T5 invented client-test helpers + `fixture()` HTML panic) → rewritten against inline `Mock` mounts + `fixture_str`.
  - MAJOR-B (T5 append code/prose contradiction) → filter moved into the code block.
  - MAJOR-C (rung-3 order product machine_name unverified) → CONFIRMED via 2026-07-05 findings (`april_2026_choice`), + a rung-3-miss surfaces as a visible skip.
  - MAJOR-D (T6 signature not threaded to :3100 call site) → explicit call-site + `order_index` lifetime instruction.
  - Minors (async `client().await`, `HumbleError` import, the five `choice_months`/`complete` call sites at :908/959/988/1028/1062, T8 rationale, T6 `ProductWire` wording, T8 dynamo-read budget) all folded at their sites.
- **OMBB step-5 gate (READY AFTER FIXES: 1 blocker + 12 majors + ~10 minors) folded 2026-07-31:**
  - B1 (feature-gate broke `--workspace` build — bin not `required-features` gated) → `[[bin]] required-features=["heal"]` + `heal = ["dynamo/heal"]` in fulfillment, `--features heal` on all three commands, `cargo test --workspace` stays-green check added.
  - M1 (harness prose bodies) → `sync_deps`/`games_for_gamekey` given full wiremock-mount code + shorthand translation table.
  - M2 (T5 elided loop body) → month-building loop pasted verbatim with the filter in place.
  - M3 (mixed-boundary no fires-anyway) → `empty_slug_live_choice_product_is_kept_not_dropped` asserts the empty-slug product is KEPT.
  - M4 (tripwire decorative) → comment-out/red/restore proof MANDATORY in both branches before tagging.
  - M5 (D1 per-field not general) → Step 4b greps all membership-blob structs for strict fields.
  - M6 (walk-deadline arithmetic undone) → done: list 12s + detail-fan-out bounded by new 180s whole-pass `CHOICE_DISCOVERY_DEADLINE` + D7 2s ≈ 194s worst, inside 393s headroom; zero new order reads.
  - M7 (T4's new test not in T5 migration) → Step 1b lists SIX callers incl. `list_walk_gamekey_is_never_empty_string`, + grep-zero-1-arg check.
  - M8 (`slug` unbound) → bound from the existing `for (slug, is_probe) in &targets`, invariant stated.
  - M9 (open-ended test update) → three tests named (`:2647/:2736/:2798`) with each one's new assertion.
  - M10 (A6a one flip) → third sync added; id-stability across re-derivation proven.
  - M11 (chronology fixture trap) → monthly fixtures pinned pre-2019 slugs; threshold pinned to Choice-start (Dec 2019).
  - M12 (branch conditional) → dropped; Task 2 points at the concrete rebase-onto-Layer-1 flow.
  - Minors: T3 uppercase-platform charset test, T7 test#3 two-month distinguish, T6 canary in-task test, T8 state-preservation + loud + read-error-failsafe tests, T10 pair_verdict-first ordering, `== ""`→`None` existing-test enumeration — all folded.
