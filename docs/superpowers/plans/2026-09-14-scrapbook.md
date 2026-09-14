# The Scrapbook 📖 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A ben-facing `/admin/scrapbook` page composing fifteen years of giving — keepsake cards, chosen-and-waiting, doors-left-open — from data that already exists, read-only.

**Architecture:** One new store method (`list_claims`, paginated Scan), one new admin-api route (`GET /admin/api/scrapbook`) whose composition is a **pure function taking `now: OffsetDateTime`** (every filter server-side, clock injected for boundary tests), one new admin SPA page + fifth nav tab. Zero write paths, zero infra changes.

**Tech Stack:** Rust (axum admin-api, aws-sdk-dynamodb store), React + TypeScript (vite SPA), vitest + testing-library, dynamodb-local for store-backed tests.

**Spec:** `docs/spec-scrapbook.md` (v2.2, `584fd12`) — the plan argues from the spec; executors read both. The spec's **"tests owed by this spec"** section is a contract: every one of those tests appears in a task below.

## Global Constraints

- Commits GPG-signed (`-S`), authored `code kitten <yourcodekitten@gmail.com>` — verify `git config user.email` before first commit.
- `cargo clippy --workspace --all-targets -- -D warnings` and `cargo fmt --check` must pass at every commit (the #231 arc went red on fmt; don't repeat it).
- No new dependencies in any crate or in `web/package.json`.
- Brand voice in all UI copy: lowercase, warm, ♡ is canon. No metric-card grids, no SaaS chrome (PRODUCT.md anti-references).
- Store-backed Rust tests use `store_or_skip` (skip without `DYNAMODB_LOCAL_URL`; CI runs them). Composition-logic tests are **pure** (no store) by design — they must run everywhere.
- Wire format: `ClaimState` serializes `snake_case` (`"pending"` / `"fulfilled"` / `"compensated"` / `"failed"` — matches `web/src/api.ts:143`). All timestamps RFC3339 strings.

---

### Task 1: `Store::list_claims()` — the one new store method

**Files:**
- Modify: `crates/dynamo/src/lib.rs` (add method near `list_links`, ~line 2864)
- Test: `crates/dynamo/tests/store_test.rs` (append)

**Interfaces:**
- Consumes: existing `schema::parse_body`, `StoreError`, `AwsFault` — all already imported in `lib.rs`.
- Produces: `pub async fn list_claims(&self) -> Result<Vec<Claim>, StoreError>` — every claim in the table across all `LINK#` partitions, **including `LINK#SELF` claims** (the method is deliberately wide; exclusion is the composition's job, Task 2). Each `Claim` carries its own `link_token` field, so no pk recovery is needed.

- [ ] **Step 1: Write the failing test**

Append to `crates/dynamo/tests/store_test.rs` (reuse the file's existing `store_or_skip`, `game`, and link/claim helpers — read the file's own fixtures first and follow its idioms; `SELF_LINK_TOKEN` is already imported at the top):

```rust
#[tokio::test]
async fn list_claims_spans_links_and_includes_self() {
    let Some(store) = store_or_skip("list-claims-spans").await else {
        return;
    };
    // two ordinary links, one claim each
    for (tok, gid) in [("tok-a", "gk1:game_a"), ("tok-b", "gk1:game_b")] {
        let mut l = link(tok);
        l.claims_allowed = 2;
        store.create_link(&l).await.unwrap();
        let c = Claim {
            id: format!("c-{tok}"),
            link_token: tok.into(),
            game_id: gid.into(),
            state: ClaimState::Pending,
            gift_url: None,
            revealed_key: None,
            created_at: datetime!(2026-09-01 12:00 UTC),
            choice_pre_tpks: None,
            failure_reason: None,
        };
        store.put_claim(&c).await.unwrap();
    }
    // one SELF claim — must ALSO be returned (wide on purpose)
    let self_claim = Claim {
        id: "c-self".into(),
        link_token: SELF_LINK_TOKEN.into(),
        game_id: "gk1:game_c".into(),
        state: ClaimState::Pending,
        gift_url: None,
        revealed_key: None,
        created_at: datetime!(2026-09-02 12:00 UTC),
        choice_pre_tpks: None,
        failure_reason: None,
    };
    store.put_claim(&self_claim).await.unwrap();

    let mut got = store.list_claims().await.unwrap();
    got.sort_by(|a, b| a.id.cmp(&b.id));
    assert_eq!(got.len(), 3, "all claims across all LINK# partitions");
    assert_eq!(got[0].link_token, SELF_LINK_TOKEN);
    assert_eq!(got[1].link_token, "tok-a");
    assert_eq!(got[2].link_token, "tok-b");
    // link META items must NOT leak in as claims (the sk filter's job)
    assert!(got.iter().all(|c| !c.id.is_empty()));
}
```

The `Claim` literal lists every field, **verified against `crates/domain/src/lib.rs:298-330`**
(9 fields incl. `failure_reason: Option<String>` at `:330`). `store_test.rs` **has** the
`link(token)` helper at `:82` — use it as written above.

- [ ] **Step 2: Run it to make sure it fails**

Run: `cd ~/bendobundles && cargo test -p dynamo --test store_test list_claims_spans -- --nocapture`
Expected: compile FAIL — `list_claims` not found on `Store` (or SKIP without dynamodb-local; if skipping, still confirm the compile failure is the method, then continue — CI is the enforcing run).

- [ ] **Step 3: Implement `list_claims`**

In `crates/dynamo/src/lib.rs`, directly after `list_links` (~line 2896):

```rust
    /// Every CLAIM item across all `LINK#` partitions — the scrapbook's read
    /// (docs/spec-scrapbook.md). Same completeness loop as `list_links` /
    /// `list_all_games` (`last_evaluated_key` until exhausted) with a
    /// deliberately WIDER filter: it spans partitions on purpose, which is why
    /// `LINK#SELF` claims arrive in the result and must be excluded downstream
    /// by the composition. Both key halves are constrained (`pk` begins_with
    /// "LINK#" AND `sk` begins_with "CLAIM#") — nothing in the schema forbids
    /// a future non-LINK partition growing a CLAIM# sort key, and this filter
    /// must not be the thing that discovers it. No GSI can answer this query:
    /// `gsi2pk = "PENDINGCLAIM"` is written only while pending and consumed on
    /// transition, so fulfilled claims leave that index. Claim body is the
    /// authoritative record (whole-item PutItem transitions) — `parse_body` is
    /// the same parse `get_claim` / `claims_for_link` use.
    pub async fn list_claims(&self) -> Result<Vec<Claim>, StoreError> {
        let mut claims: Vec<Claim> = Vec::new();
        let mut last_key: Option<HashMap<String, aws_sdk_dynamodb::types::AttributeValue>> = None;
        loop {
            let out = self
                .client
                .scan()
                .table_name(&self.table)
                .filter_expression("begins_with(pk, :lpfx) AND begins_with(sk, :cpfx)")
                .expression_attribute_values(
                    ":lpfx",
                    aws_sdk_dynamodb::types::AttributeValue::S("LINK#".into()),
                )
                .expression_attribute_values(
                    ":cpfx",
                    aws_sdk_dynamodb::types::AttributeValue::S("CLAIM#".into()),
                )
                .set_exclusive_start_key(last_key.take())
                .send()
                .await
                .map_err(|e| StoreError::Aws(AwsFault::from_sdk_error("scan", &e)))?;
            for item in out.items() {
                claims.push(parse_body(item)?);
            }
            match out.last_evaluated_key() {
                None => break,
                Some(k) => last_key = Some(k.clone()),
            }
        }
        Ok(claims)
    }
```

(If `parse_body` is referenced as `schema::parse_body` elsewhere in the file, match that spelling.)

- [ ] **Step 4: Run the test and the crate suite**

Run: `cargo test -p dynamo --test store_test list_claims_spans -- --nocapture && cargo clippy -p dynamo --all-targets -- -D warnings && cargo fmt --check`
Expected: PASS (or clean SKIP line + clean clippy/fmt).

- [ ] **Step 5: Commit**

```bash
git add crates/dynamo/src/lib.rs crates/dynamo/tests/store_test.rs
git commit -S -m "📖 dynamo: list_claims() — all claims across LINK# partitions, wide on purpose (scrapbook T1)"
```

---

### Task 2: composition + `GET /admin/api/scrapbook`

**Files:**
- Create: `crates/admin-api/src/scrapbook.rs` (composition — pure, clock-injected)
- Modify: `crates/admin-api/src/lib.rs` (module decl, route, handler)
- Test: composition unit tests **inside `scrapbook.rs`** (`#[cfg(test)]` — pure fixtures, no store); one store-backed endpoint test appended to `crates/admin-api/tests/api_test.rs`

**Interfaces:**
- Consumes: `Store::list_claims()` (Task 1), existing `list_links` / `list_friends` / `batch_get_games`, `domain::{Claim, ClaimState, Friend, Game, Link, SELF_LINK_TOKEN}`.
- Produces (exact names — Tasks 3–5 depend on this JSON):
  - `pub fn compose_scrapbook(claims: Vec<Claim>, links: Vec<Link>, friends: Vec<Friend>, games: HashMap<String, Game>, now: OffsetDateTime) -> ScrapbookView`
  - `ScrapbookView { entries, waiting, doors_open, orphan_claim_count, stale_pending_count }` — field-for-field the spec's response object (spec "admin-api" section). `STALE_PENDING_HOURS: i64 = 48` as a named const.
  - Route `GET /admin/api/scrapbook` inside the session-protected block.

- [ ] **Step 1: Write the failing composition tests** (`crates/admin-api/src/scrapbook.rs`, bottom, `#[cfg(test)] mod tests`)

Fixture helpers first — every test builds from these, with a fixed
`NOW: datetime!(2026-09-14 12:00 UTC)`. The test module imports `domain::GameStatus` (the
production module does not need it — `can_claim`/`is_listable` hide it there):

```rust
    fn fx_game_with(id: &str, status: GameStatus, giftable: bool, hidden: bool) -> Game { /* every field explicit; artwork Some, acquired_at Some(datetime!(2014-08-15 0:00 UTC)). SHAPE-only reference: store_test.rs's game() helper shows a full Game literal — different signature (game(n: u32, listable: bool)), different crate, cannot be imported; copy the field list, not the arity */ }
    fn fx_game(id: &str) -> Game { fx_game_with(id, GameStatus::Available, true, false) }
    fn fx_link(token: &str) -> Link { /* claims_allowed 2, claims_used 0, revoked false, no expiry/unlock, no friend, no curation, created_at datetime!(2024-09-01 0:00 UTC) */ }
    fn fx_claim(id: &str, token: &str, gid: &str, state: ClaimState, at: OffsetDateTime) -> Claim { /* every field explicit, gift_url None */ }
```

The test set — one test per spec-owed contract, names fixed (the review gate greps for them):

```rust
    #[test]
    fn waiting_is_listability_not_claim_absence() {
        // live link curates 5 games: listable / Failed-retired (status Expired) /
        // hidden / non-giftable / stale-pending (status Pending).
        // waiting must contain EXACTLY the listable one. (B1/B2 + stale-Pending arm.)
    }
    #[test]
    fn self_claims_dropped_before_join_and_not_orphans() {
        // one SELF claim + one ordinary fulfilled claim →
        // entries has 1, orphan_claim_count == 0.
    }
    #[test]
    fn non_self_orphan_claim_is_counted_never_skipped() {
        // claim with link_token "ghost" (no such link) →
        // entries empty, orphan_claim_count == 1.
    }
    #[test]
    fn revoked_link_keeps_entry_loses_waiting() {
        // ONE fixture: revoked link with one Fulfilled claim AND one
        // curated-unclaimed listable game → the claim IS in entries,
        // waiting is EMPTY. Two assertions, one fixture. (Q4.)
    }
    #[test]
    fn pending_boundary_47h_badges_49h_drops_and_counts() {
        // now = NOW; pending claims created_at NOW-47h and NOW-49h.
        // 47h → in entries with state pending; 49h → absent,
        // stale_pending_count == 1. Fixtures ON the boundary. (Q2 frozen clock.)
    }
    #[test]
    fn recipient_prefers_friend_name_falls_back_to_label() {
        // link A carries friend_id→Friend{name:"sam"}; link B no friend, label "sarah bday".
        // entries' recipients: "sam" and "sarah bday".
    }
    #[test]
    fn entries_sorted_claimed_at_then_game_id() {
        // three fulfilled claims, two sharing one created_at second →
        // order (claimed_at asc, game_id asc). (Q1 tiebreak.)
    }
    #[test]
    fn doors_are_uncurated_live_links_with_created_at() {
        // uncurated live link → doors_open row {claims_left = allowed-used, created_at rfc3339};
        // uncurated REVOKED link → absent; curated link → absent from doors.
    }
    #[test]
    fn sealed_two_half_waits_labeled_never_doors() {
        // ONE fixture, two assertions (the revoked two-half's sibling — step-5 B1):
        // a SEALED curated link (unlock_at = NOW + 10 days, one listable curated game)
        // IS in waiting with sealed_until == Some(rfc3339 of that unlock);
        // a SEALED uncurated link is NOT in doors_open (and nowhere else).
        // Control: an unsealed curated link's waiting row has sealed_until == None,
        // and a PAST unlock_at (NOW - 1 day) also yields None — the filter is
        // sealed-AT-now, not attribute-present.
    }
    #[test]
    fn tag_read_only_for_entry_games() {
        // curated_notes carries a note for an entry's game AND a note for a
        // game no longer in curated_game_ids → entry.tag is Some, and the
        // orphaned note changes nothing anywhere (spec: notes can outlive curation).
    }
```

Every `/* ... */` above is a *fixture recipe*, not a placeholder — the executor writes the
literal following the recipe; expected values are stated in each comment. Assertions compare
whole fields (`assert_eq!(view.waiting.len(), 1)` etc.), never substring-matching on JSON.

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p admin-api scrapbook -- --nocapture`
Expected: compile FAIL — module doesn't exist.

- [ ] **Step 3: Implement `scrapbook.rs`**

```rust
use domain::{Claim, ClaimState, Friend, Game, Link, SELF_LINK_TOKEN};
use serde::Serialize;
use std::collections::HashMap;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

/// A Pending older than this is a fulfillment defect, not a gift mid-open
/// (docs/spec-scrapbook.md "claim states on the page"; the 70-day prod
/// specimen is why this bound exists). Provisional and layout-invariant.
pub const STALE_PENDING_HOURS: i64 = 48;

#[derive(Serialize, Clone)]
pub struct ScrapbookGame {
    pub id: String,
    pub title: String,
    pub artwork_url: Option<String>,
    pub acquired_at: Option<String>,
}

#[derive(Serialize)]
pub struct ScrapbookEntry {
    pub claimed_at: String,
    pub state: ClaimState,
    pub game: ScrapbookGame,
    pub recipient: String,
    pub gift_note: Option<String>,
    pub tag: Option<String>,
    pub thank_note: Option<String>,
    pub thanked_at: Option<String>,
    pub link_token: String,
    pub link_label: String,
}

#[derive(Serialize)]
pub struct ScrapbookWaitingLink {
    pub link_token: String,
    pub link_label: String,
    pub recipient: String,
    /// RFC3339 unlock moment, Some iff the link is sealed AT `now` — the one
    /// forward-pointing field on the page ("wrapped until ⟨date⟩", spec Q1 ruling).
    pub sealed_until: Option<String>,
    pub games: Vec<ScrapbookGame>,
}

#[derive(Serialize)]
pub struct ScrapbookDoor {
    pub link_token: String,
    pub link_label: String,
    pub recipient: String,
    pub claims_left: u32,
    pub created_at: String,
}

#[derive(Serialize)]
pub struct ScrapbookView {
    pub entries: Vec<ScrapbookEntry>,
    pub waiting: Vec<ScrapbookWaitingLink>,
    pub doors_open: Vec<ScrapbookDoor>,
    pub orphan_claim_count: u32,
    pub stale_pending_count: u32,
}

fn rfc3339(t: OffsetDateTime) -> String {
    t.format(&Rfc3339).unwrap_or_default()
}

fn scrapbook_game(id: &str, games: &HashMap<String, Game>) -> ScrapbookGame {
    match games.get(id) {
        Some(g) => ScrapbookGame {
            id: g.id.clone(),
            title: g.title.clone(),
            artwork_url: g.artwork_url.clone(),
            acquired_at: g.acquired_at.map(rfc3339),
        },
        // degradation, not lying: a missing game record renders by id
        None => ScrapbookGame {
            id: id.into(),
            title: id.into(),
            artwork_url: None,
            acquired_at: None,
        },
    }
}

/// TWO predicates, deliberately — the seal splits them (step-5 B1, decided in spec):
/// a curated sealed link's games are chosen-and-waiting (the friend just can't open
/// yet); a sealed uncurated link is NOT an open door — it is wrapped. Both call the
/// canonical `Link::can_claim` (domain:372) rather than restating a subset of its
/// four arms — a hand-rolled copy that drops one arm is the drift class this fixes.
fn link_waits(l: &Link, now: OffsetDateTime) -> bool {
    // tolerates the seal AND NOTHING ELSE — a future fifth refusal lands
    // excluded-by-default and must be NAMED to be tolerated (Lilith's form;
    // the exhaustiveness argument Notify::resolve's own doc makes one crate over)
    matches!(l.can_claim(now), Ok(()) | Err(domain::ClaimRefusal::Sealed))
}
fn link_is_open_door(l: &Link, now: OffsetDateTime) -> bool {
    l.can_claim(now).is_ok()
}

fn recipient(l: &Link, friends: &HashMap<String, String>) -> String {
    l.friend_id
        .as_ref()
        .and_then(|id| friends.get(id).cloned())
        .unwrap_or_else(|| l.label.clone())
}

pub fn compose_scrapbook(
    claims: Vec<Claim>,
    links: Vec<Link>,
    friends: Vec<Friend>,
    games: HashMap<String, Game>,
    now: OffsetDateTime,
) -> ScrapbookView {
    let friend_names: HashMap<String, String> =
        friends.into_iter().map(|f| (f.id, f.name)).collect();
    let links_by_token: HashMap<String, &Link> =
        links.iter().map(|l| (l.token.clone(), l)).collect();

    // Sort BEFORE formatting: string-sorting mixed-precision RFC3339 missorts
    // ("…08.123Z" < "…08Z" lexically while 08.123 > 08 in time) — plan-review find.
    let mut claims = claims;
    claims.sort_by(|a, b| (a.created_at, a.game_id.as_str()).cmp(&(b.created_at, b.game_id.as_str())));

    let mut entries = Vec::new();
    let mut orphan_claim_count = 0u32;
    let mut stale_pending_count = 0u32;

    for c in &claims {
        if c.link_token == SELF_LINK_TOKEN {
            continue; // structural drop, by pk-equivalent, BEFORE the join (spec B3)
        }
        let Some(link) = links_by_token.get(&c.link_token) else {
            // an orphan that is not SELF is a data fact — counted, never skipped
            orphan_claim_count += 1;
            continue;
        };
        match c.state {
            ClaimState::Fulfilled => {}
            ClaimState::Pending => {
                if now - c.created_at > time::Duration::hours(STALE_PENDING_HOURS) {
                    stale_pending_count += 1;
                    continue;
                }
            }
            ClaimState::Compensated | ClaimState::Failed => continue,
        }
        entries.push(ScrapbookEntry {
            claimed_at: rfc3339(c.created_at),
            state: c.state,
            game: scrapbook_game(&c.game_id, &games),
            recipient: recipient(link, &friend_names),
            gift_note: link.gift_note.clone(),
            tag: link
                .curated_notes
                .as_ref()
                .and_then(|m| m.get(&c.game_id).cloned()),
            thank_note: link.thank_note.clone(),
            thanked_at: link.thanked_at.map(rfc3339),
            link_token: link.token.clone(),
            link_label: link.label.clone(),
        });
    }
    // entries inherit the pre-format sort — no string sort here (see above).

    let mut waiting = Vec::new();
    let mut doors_open = Vec::new();
    let mut sorted_links: Vec<&&Link> = links_by_token.values().collect();
    sorted_links.sort_by(|a, b| (a.created_at, &a.token).cmp(&(b.created_at, &b.token)));
    for link in sorted_links {
        let curated = link.curated_game_ids.as_deref().unwrap_or(&[]);
        if curated.is_empty() {
            if !link_is_open_door(link, now) {
                continue; // revoked, expired, exhausted — or SEALED: wrapped is not open
            }
            doors_open.push(ScrapbookDoor {
                link_token: link.token.clone(),
                link_label: link.label.clone(),
                recipient: recipient(link, &friend_names),
                claims_left: link.claims_allowed.saturating_sub(link.claims_used),
                created_at: rfc3339(link.created_at),
            });
            continue;
        }
        if !link_waits(link, now) {
            continue; // dead links leave waiting; a SEAL alone does not
        }
        // THE LISTABILITY TEST, not a claim-absence test (spec B1/B2): Failed
        // retires, hidden/non-giftable were taken off the shelf, a claimed or
        // stuck-pending game has left `Available` — is_listable covers all of it.
        let games_waiting: Vec<ScrapbookGame> = curated
            .iter()
            .filter(|id| games.get(*id).is_some_and(|g| g.is_listable()))
            .map(|id| scrapbook_game(id, &games))
            .collect();
        if !games_waiting.is_empty() {
            waiting.push(ScrapbookWaitingLink {
                link_token: link.token.clone(),
                link_label: link.label.clone(),
                recipient: recipient(link, &friend_names),
                sealed_until: link.unlock_at.filter(|u| *u > now).map(rfc3339),
                games: games_waiting,
            });
        }
    }

    ScrapbookView { entries, waiting, doors_open, orphan_claim_count, stale_pending_count }
}
```

(`is_none_or` is fine: the workspace toolchain is pinned `1.97.1` in `rust-toolchain.toml`,
well past its 1.82 stabilization — verified at plan review.)

- [ ] **Step 4: Wire the module + route + handler in `lib.rs`**

Module: `mod scrapbook;` + `pub use scrapbook::*;` near the other module decls.
Route: the router builds a `protected` sub-router (`let protected = Router::new()` …) that ends
with `.route_layer(… session_middleware)`; login/logout sit OUTSIDE it. **Add the route inside
the `protected` chain, anywhere before `.route_layer`** — never by line number, the block has
grown before:

```rust
        .route("/admin/api/scrapbook", get(handle_scrapbook))
```

Handler, near `handle_list_links` (~line 823) — same error idiom as its neighbors:

```rust
async fn handle_scrapbook(State(s): State<AppState>) -> Response {
    let (claims, links, friends) = match tokio::try_join!(
        s.store.list_claims(),
        s.store.list_links(),
        s.store.list_friends(),
    ) {
        Ok(t) => t,
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };
    // ids the view needs: every claim's game + every curated id on a live-or-not link
    // (over-fetching a dead link's ids is harmless; the composition filters).
    let mut ids: Vec<String> = claims.iter().map(|c| c.game_id.clone()).collect();
    for l in &links {
        if let Some(cur) = &l.curated_game_ids {
            ids.extend(cur.iter().cloned());
        }
    }
    ids.sort();
    ids.dedup();
    let games = match s.store.batch_get_games(&ids).await {
        Ok(g) => g,
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };
    let view = compose_scrapbook(claims, links, friends, games, OffsetDateTime::now_utc());
    (StatusCode::OK, Json(view)).into_response()
}
```

**Verified** (plan review, `crates/dynamo/src/lib.rs:782`): `batch_get_games(&self, ids:
&[String]) -> Result<HashMap<String, Game>, StoreError>` — already the map keyed by id; pass it
straight into `compose_scrapbook`. No conversion.

- [ ] **Step 5: Store-backed endpoint test** (append to `crates/admin-api/tests/api_test.rs`, following the file's existing store-backed idiom — session cookie via the file's login helper, `router(...)` construction copied from a neighboring store-backed test):

```rust
#[tokio::test]
async fn scrapbook_endpoint_composes() {
    // store_or_skip; seed: one friend "sam", one link with friend_id + one
    // Fulfilled claim on a listable game + gift_note + curated tag for that
    // game; one uncurated live link. GET /admin/api/scrapbook with a valid
    // session → 200; body: entries.len()==1, entry.recipient=="sam",
    // entry.tag is the curated note, doors_open.len()==1,
    // orphan_claim_count==0, stale_pending_count==0.
    // Build fixtures exactly like the file's other store-backed tests build
    // links/games/claims — same helpers, same style.
}
```

- [ ] **Step 6: Run everything**

Run: `cargo test -p admin-api -- --nocapture && cargo clippy -p admin-api --all-targets -- -D warnings && cargo fmt --check`
Expected: all composition tests PASS (pure — they run without dynamo); endpoint test PASS or clean SKIP.

- [ ] **Step 7: Commit**

```bash
git add crates/admin-api/src/scrapbook.rs crates/admin-api/src/lib.rs crates/admin-api/tests/api_test.rs
git commit -S -m "📖 admin-api: GET /admin/api/scrapbook — pure clock-injected composition, every filter server-side (scrapbook T2)"
```

---

### Task 3: web plumbing — `adminScrapbook()` + the postmark call-site contract

**Files:**
- Modify: `web/src/api.ts` (types + fetcher, after `adminFriends` ~line 597)
- Modify: `web/src/postmark.ts` (doc comment only, line ~38)
- Test: `web/src/api.test.ts` (append), `web/src/postmark.test.ts` (append)

**Interfaces:**
- Consumes: Task 2's JSON contract.
- Produces (Tasks 4–5 import these exact names from `../api` / `../postmark`):
  - `export type ScrapbookGame = { id: string; title: string; artwork_url: string | null; acquired_at: string | null }`
  - `export type ScrapbookEntry = { claimed_at: string; state: 'pending' | 'fulfilled' | 'compensated' | 'failed'; game: ScrapbookGame; recipient: string; gift_note: string | null; tag: string | null; thank_note: string | null; thanked_at: string | null; link_token: string; link_label: string }`
  - `export type ScrapbookWaitingLink = { link_token: string; link_label: string; recipient: string; sealed_until: string | null; games: ScrapbookGame[] }`
  - `export type ScrapbookDoor = { link_token: string; link_label: string; recipient: string; claims_left: number; created_at: string }`
  - `export type ScrapbookView = { entries: ScrapbookEntry[]; waiting: ScrapbookWaitingLink[]; doors_open: ScrapbookDoor[]; orphan_claim_count: number; stale_pending_count: number }`
  - `export async function adminScrapbook(): Promise<ScrapbookView>`

- [ ] **Step 1: Failing fetcher test** (append to `api.test.ts`, mirroring the `adminLinks` describe at line 544 — same mockFetch idiom, same `Unauthorized` rejection arm):

```ts
describe('adminScrapbook', () => {
  it('fetches and returns the view', async () => {
    const view = {
      entries: [], waiting: [], doors_open: [],
      orphan_claim_count: 0, stale_pending_count: 0,
    };
    const mockResponse = {
      ok: true,
      status: 200,
      json: vi.fn().mockResolvedValue(view),
    };
    mockFetch.mockResolvedValueOnce(mockResponse);
    const result = await adminScrapbook();
    expect(mockFetch).toHaveBeenCalledWith('/admin/api/scrapbook');
    expect(result.stale_pending_count).toBe(0);
  });
  it('throws Unauthorized on 401', async () => {
    mockFetch.mockResolvedValueOnce({ ok: false, status: 401, json: vi.fn() });
    await expect(adminScrapbook()).rejects.toBeInstanceOf(Unauthorized);
  });
});
```

(**Verified idiom**: `api.test.ts` has NO response-builder helper — every test builds an inline
`{ ok, status, json: vi.fn().mockResolvedValue(...) }` literal against the file's shared
`mockFetch` (`vi.stubGlobal('fetch', mockFetch)` in `beforeEach`). The blocks above ARE that
idiom; import `adminScrapbook` in the file's existing import list.)

- [ ] **Step 2: Run to fail** — `cd web && npx vitest run src/api.test.ts` → FAIL (no export).

- [ ] **Step 3: Implement** (after `adminFriends`, same shape):

```ts
/** The scrapbook — ben's story of giving (docs/spec-scrapbook.md). Read-only. */
export async function adminScrapbook(): Promise<ScrapbookView> {
  const response = await fetch('/admin/api/scrapbook');
  await checkOk(response, 'scrapbook');
  return (await response.json()) as ScrapbookView;
}
```

- [ ] **Step 4: Pin the existing `waitedYears` semantics** (characterization — these tests
**PASS immediately**, they pin behavior the scrapbook relies on, not new code. ⚠️ If any FAILS,
STOP: the plan's understanding of `waitedYears` is wrong — re-read `postmark.ts` and reconcile
before continuing). Append to `postmark.test.ts`:

```ts
describe('waitedYears — the two scrapbook call shapes (frozen clock)', () => {
  const acquired = '2014-08-15T00:00:00Z';
  it('keepsake card: span ends at the unwrap, not today', () => {
    // opened sep 2023 — anniversary passed → 9 (the spec's own example line)
    expect(waitedYears(acquired, Date.parse('2023-09-20T00:00:00Z'))).toBe(9);
  });
  it('same acquired_at, waiting-shaped now, DIFFERENT answer — the divergence is the bug class', () => {
    expect(waitedYears(acquired, Date.parse('2026-09-14T00:00:00Z'))).toBe(12);
  });
  it('anniversary not reached rounds down (mar 2023 → 8, not 9)', () => {
    expect(waitedYears(acquired, Date.parse('2023-03-10T00:00:00Z'))).toBe(8);
  });
});
```

- [ ] **Step 5: Run** — `npx vitest run src/postmark.test.ts` → all three PASS (see Step 4's characterization note; a FAIL here is a STOP, not a fix-forward).

- [ ] **Step 6: Amend the doc comment** in `postmark.ts` — replace the line
`` *  `now` is injectable for tests. `` with:

```ts
 *  `now` is injectable — for tests AND for production callers measuring a span
 *  that ended in the past: the scrapbook keepsake card MUST pass the claim's
 *  claimed_at (a default-now call there drifts +1 every January). The waiting
 *  and doors sections correctly take the default; the door span feeds
 *  link.created_at, a different field (docs/spec-scrapbook.md). */
```

(A doc that labels the parameter test-only certifies no production caller exists — OMBB's
crossfire layer on Lilith's blocker. This edit and the tests above are one commit: both
findings, one edit, or neither is finished.)

- [ ] **Step 7: Run + typecheck** — `npx vitest run src/api.test.ts src/postmark.test.ts && npx tsc --noEmit` → PASS.

- [ ] **Step 8: Commit**

```bash
git add web/src/api.ts web/src/api.test.ts web/src/postmark.ts web/src/postmark.test.ts
git commit -S -m "📖 web: adminScrapbook() + waitedYears call-site contract pinned frozen-clock, doc comment de-certified (scrapbook T3)"
```

---

### Task 4: `Scrapbook.tsx` — the story: years, jump-list, keepsake cards

**Files:**
- Create: `web/src/admin/Scrapbook.tsx`
- Modify: `web/src/App.tsx` (route, in the `/admin` children ~line 29), `web/src/admin/AdminApp.tsx` (fifth NavLink after `friends` ~line 70; **fix the comment at line 16 to `// One place for the nav active/inactive style — every NavLink shares it.` — drop the number, don't increment it**), `web/src/admin/Links.tsx` (row anchors + hash-scroll, for the card deep-link — spec: "the card deep-links to its owning links-tab row")
- Test: `web/src/admin/Scrapbook.test.tsx`, `web/src/admin/Links.test.tsx` (consuming-side deep-link arm — step-5 B2)

**Interfaces:**
- Consumes: `adminScrapbook`, `ScrapbookView`, `ScrapbookEntry` from `../api`; `postmark`, `waitedYears` from `../postmark`.
- Produces: `export function Scrapbook()` (named export, matching `Friends`/`Links` convention); route `/admin/scrapbook`; nav label `scrapbook`. Task 5 extends **this same file** with the waiting/doors/summary sections — keep the component's data fetch in one `useEffect` and render sections from one `view` state so Task 5 only adds JSX + helpers.

- [ ] **Step 1: Failing tests** (`Scrapbook.test.tsx`, mock idiom copied from `Friends.test.tsx:1-30` — partial `vi.mock('../api')` stubbing `adminScrapbook` only):

```tsx
// fixture: entries across 2019 and 2023, one 2023 pair sharing claimed_at
// second (tiebreak visible), one pending ≤48h entry, one entry with
// gift_note+tag+thank_note, one entry with acquired_at null (span omitted),
// one entry whose acquired_at POSTDATES claimed_at (waited clause omitted).
it('groups by year of unwrap, oldest year first, with a jump-list', ...);
it('orders within a year by (claimed_at, game_id)', ...);       // assert DOM order of titles
it('keepsake card shows recipient, notes, thanks as one unit', ...);
it('postmark span ends at the unwrap: "bought aug 2014 · opened sep 2023 — waited 9 years"', ...);
it('span line omitted when acquired_at is null; waited clause omitted when negative', ...);
it('pending entry wears the unwrapping… badge', ...);
it('shows the soft empty state when there are no entries', ...); // "no gifts opened yet ♡" tone
```

Each `it` body renders `<MemoryRouter><Scrapbook /></MemoryRouter>` with `adminScrapbook`
resolving the fixture, then asserts via `screen.getByText` / DOM order (`getAllByRole` or
`within` on year sections). Write real assertions with the fixture's literal strings.

- [ ] **Step 2: Run to fail** — `npx vitest run src/admin/Scrapbook.test.tsx` → FAIL (no module).

- [ ] **Step 3: Implement the page (entries only)** — structure:

```tsx
export function Scrapbook() {
  // useState<ScrapbookView | null> + error state; useEffect fetch on mount —
  // copy Friends.tsx's load/error/retry idiom exactly.
  // derive: entriesByYear = Map<number, ScrapbookEntry[]> from
  //   new Date(Date.parse(e.claimed_at)).getUTCFullYear(), years sorted asc,
  //   entries within a year sorted [claimed_at, game.id] asc.
  // render: year jump-list (<a href="#y2019">2019</a> …) · per-year section
  //   (<section id={`y${year}`}>) · keepsake cards.
}
```

Card content rules (each is a test above): recipient line "for {recipient}"; gift_note then
tag (the sticker) each in its own quiet style; thank-you rendered only as a unit
(`thank_note` + date via `postmark(thanked_at)`), nothing invented when absent; span line —

```tsx
const bought = postmark(e.game.acquired_at ?? undefined);
const opened = postmark(e.claimed_at);
const waited =
  e.game.acquired_at && Date.parse(e.game.acquired_at) <= Date.parse(e.claimed_at)
    ? waitedYears(e.game.acquired_at, Date.parse(e.claimed_at))
    : null; // negative span = bad data → omit the clause, never "waited −2 years"
```

rendered as `bought {bought} · opened {opened}` + (waited ? ` — waited {waited} years` : ``),
whole line omitted when `bought` is null (card still shows `opened {opened}`). Badge:
`e.state === 'pending'` → `<span>unwrapping…</span>` (soft styling, amber family per
statusBadge's pending). Styling: follow the admin pages' existing className/inline-style
idiom — warm, no metric cards, lowercase copy throughout.

**Card deep-link (spec: "deep-links to its owning links-tab row"):** the card's recipient/label
line is a react-router `<Link to={{ pathname: '/admin/links', hash: `#link-${e.link_token}` }}>`.
In `Links.tsx`: give each link row `id={`link-${l.token}`}`, and add a mount effect —

```tsx
const { hash } = useLocation();
useEffect(() => {
  if (hash) document.getElementById(hash.slice(1))?.scrollIntoView();
}, [hash, links]);   // re-run when rows land — the anchor doesn't exist until data loads
```

— because react-router does not scroll to hashes on its own. Tests, BOTH sides (step-5 B2 —
the producing-side href test alone lets a subagent skip `Links.tsx` entirely with every suite
green; *a fix that cannot go red is the same shape as the gap it closed*):
- Producing side (`Scrapbook.test.tsx`): the card link's `href` ends with
  `/admin/links#link-<token>`.
- Consuming side (append to `web/src/admin/Links.test.tsx`, which already mocks the api —
  follow its existing render idiom):

```tsx
it('deep-link hash scrolls to the owning row', async () => {
  const scrollSpy = vi.fn();
  HTMLElement.prototype.scrollIntoView = scrollSpy; // jsdom doesn't implement it — unstubbed it throws, which is the honest red
  // render Links inside <MemoryRouter initialEntries={['/admin/links#link-tok-a']}>
  // with adminLinks resolving two links (tokens 'tok-a', 'tok-b');
  await waitFor(() => expect(screen.getByText(/tok-a-label/)).toBeInTheDocument());
  expect(document.getElementById('link-tok-a')).not.toBeNull();  // the row carries the anchor id
  expect(scrollSpy).toHaveBeenCalled();                           // and the effect actually scrolled
});
```

- [ ] **Step 4: Route + nav.** `App.tsx`: `<Route path="scrapbook" element={<Scrapbook />} />` with the other admin children. `AdminApp.tsx`: NavLink `scrapbook` after `friends`, plus the line-16 comment fix (Global note: **drop the number**).

- [ ] **Step 5: Run** — `npx vitest run src/admin/Scrapbook.test.tsx src/admin/AdminApp.test.tsx && npx tsc --noEmit` → PASS (AdminApp tests may assert nav contents — update its expectations if the fifth tab breaks them, that's a legitimate ripple, not scope creep).

- [ ] **Step 6: Commit**

```bash
git add web/src/admin/Scrapbook.tsx web/src/admin/Scrapbook.test.tsx web/src/App.tsx web/src/admin/AdminApp.tsx web/src/admin/AdminApp.test.tsx
git commit -S -m "📖 web: /admin/scrapbook — years, jump-list, keepsake cards, unwrap-anchored spans (scrapbook T4)"
```

---

### Task 5: waiting · doors left open · summary sentence · footnotes

**Files:**
- Modify: `web/src/admin/Scrapbook.tsx` (add sections), `web/src/admin/Scrapbook.test.tsx` (append)

**Interfaces:**
- Consumes: Task 4's component + `view.waiting` / `view.doors_open` / the two counts; `waitedYears` default-call for waiting (`game.acquired_at`) and doors (`link.created_at` — **a different field, never symmetry-copied**).
- Produces: the finished page.

- [ ] **Step 1: Failing tests** (append):

```tsx
it('chosen and waiting: recipient-grouped, listable games only arrive (server filtered)', ...);
it('a sealed waiting group says "wrapped until ⟨date⟩"; unsealed groups do not', ...);
   // fixture: one group sealed_until '2026-12-25T15:00:00Z' → heading carries
   // "wrapped until dec 25"; one group sealed_until null → string absent
it('doors left open is its own heading — recipient named, never under chosen', ...);
   // asserts "the door ben left open for sam · open 2 years · 2 claims left"
   // shape, with the open-clause omitted for a door younger than a year
it('summary sentence counts distinct recipients as people', ...);
   // fixture: 3 entries, 2 distinct recipients, 1 thank-you →
   // "3 gifts opened by 2 people across N years · 1 thank-you ♡"
it('quiet footnotes render ONLY when counts are nonzero', ...);
   // orphan_claim_count 0 + stale_pending_count 0 → neither string in DOM;
   // re-render with 1 + 2 → both present, machinery-toned but honest
```

- [ ] **Step 2: Run to fail.**

- [ ] **Step 3: Implement.** Waiting section: per `ScrapbookWaitingLink`, heading "for
{recipient}", game rows with `postmark(g.acquired_at ?? undefined)` + default-now
`waitedYears(g.acquired_at ?? undefined)` ("waiting N years" clause omitted when null) —
the `?? undefined` coercion is required: API types are `string | null`, the postmark helpers
take `string | undefined`. A group with `sealed_until` non-null renders "wrapped until
{sealDate(sealed_until)}" in its heading — `sealDate` is a tiny local helper in
`Scrapbook.tsx` (`const d = new Date(Date.parse(iso));` → `` `${MONTHS_SHORT[d.getUTCMonth()]} ${d.getUTCDate()}` `` with a lowercase
month list matching postmark's style — day-level because a seal has a *date*, unlike the
month-grain postmark; null/junk → omit the clause). Doors: own `<section>` headed `doors left open`,
one line per door — `the door ben left open for {recipient}` + (waitedYears(created_at) ?
` · open {n} years` : ``) + ` · {claims_left} claims left` (singular "claim" when 1). Summary
sentence above everything, derived client-side from `view` (distinct `recipient` strings ⇒
"people"; year span = max−min entry year, "across N years" omitted when all one year;
`thank_note != null` count ⇒ "K thank-yous", singular when 1). Footnotes at the page bottom,
each gated `count > 0`: `{n} claims reference links this page can't find` /
`{n} gifts are stuck mid-unwrap — see ops` (lowercase, honest, no alarm styling).

- [ ] **Step 4: Run the full web suite** — `npx vitest run && npx tsc --noEmit` → PASS.

- [ ] **Step 5: Commit**

```bash
git add web/src/admin/Scrapbook.tsx web/src/admin/Scrapbook.test.tsx
git commit -S -m "📖 web: waiting + doors left open + summary + honest footnotes — the scrapbook is whole (scrapbook T5)"
```

---

### Task 6: sweep + PR

**Files:** none new — verification and the PR.

- [ ] **Step 1: The sweep, in order, each to completion:**

```bash
cd ~/bendobundles
cargo fmt --check                                        # the #231 lesson: fmt is IN the sweep
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace                                   # store-backed tests skip without dynamodb-local; CI enforces
cd web && npx vitest run && npx tsc --noEmit && cd ..
```

Every command's real exit code checked — no `| tail`, no swallowed rc. Any red: fix, re-run
the WHOLE sweep, then continue.

- [ ] **Step 2: Push and open the PR** (base `main`, head `scrapbook`):

```bash
git push -u origin scrapbook
gh pr create -R yourcodekitten/bendobundles --base main --head scrapbook \
  --title "the scrapbook 📖 — ben-facing story of fifteen years of giving" \
  --body "<decision-record body>"
```

PR body is a decision record, per house pattern: what it composes and why (spec pointer),
the crossfire deltas (B1/B2 listability, B3 orphan counting, waitedYears call sites +
48h frozen clock, doors-span field, stale_pending_count), **named gaps**: store-backed tests
skip locally (CI is the enforcing run, per test-file doctrine); #234 filed separately for the
stuck-Pending systemic gap; 48h + STALE_PENDING_HOURS provisional. End the body with the
session link per commit convention.

- [ ] **Step 3: Watch CI to terminal state** (`gh pr checks --watch` or the runs endpoint —
NEVER `statusCheckRollup`). Green required before requesting review.

---

## Self-review (run, not performed by a subagent)

- **Spec coverage:** every "tests owed by this spec" item has a named test: waiting-is-listability + stale-Pending arm (T2 `waiting_is_listability_not_claim_absence`), 48h boundary frozen-clock (T2 `pending_boundary_47h_badges_49h_drops_and_counts`), revoked two-half (T2 `revoked_link_keeps_entry_loses_waiting`), SELF-drop + orphan (T2, two tests), frozen-clock postmark divergence (T3). Doors `created_at`, `stale_pending_count`, footnotes, summary-as-derivation, jump-list, comment fixes — all in T3–T5. ✅
- **Placeholders:** T2 Step-1 test bodies and T4/T5 `it(...)` lists are fixture *recipes with stated expected values* — the executor writes literals from them; no TBDs remain. ✅
- **Type consistency:** `ScrapbookView` field names identical across T2 (serde), T3 (TS types), T4/T5 (consumption); `compose_scrapbook` signature stated once and consumed once; `STALE_PENDING_HOURS` named in T2 and referenced nowhere else (the web never re-derives it — counts arrive computed). ✅

## Plan-review record (implementation-plan-review, 2026-09-14, integrated in place)

Cold-subagent walkthrough + reality checks against the tree. **Verdict after fixes: ready to
execute.** Findings, all fixed above:
- **B1 (spec coverage)**: the card's deep-link-to-links-row was in the spec and in no task →
  T4 gains the `Links.tsx` anchors + hash-scroll effect + href test.
- **B2 (nonexistent helper)**: T3's fetcher test used a `jsonResponse` builder `api.test.ts`
  does not have (verified: inline `{ok, status, json}` literals only) → rewritten in the real
  idiom.
- **M1 (latent missort)**: sorting formatted RFC3339 strings missorts mixed subsecond
  precision (`…08.123Z` < `…08Z` lexically) → sort on parsed `OffsetDateTime` BEFORE
  formatting; string sort removed.
- **M2 (deferred signature)**: `batch_get_games` hedge resolved by reading it —
  `&[String] → HashMap<String, Game>`, pass-through.
- **M3 (line-number anchor)**: route placement re-anchored structurally (inside `protected`
  before `.route_layer`), not by line.
- **M4 (cross-crate fixture temptation)**: T2's fixture recipes say copy-the-shape; sharpened
  with the reminder that dynamo's test helpers cannot be imported across crates.
- **m1**: T1's `link()` helper confirmed to exist (`store_test.rs:82`) — conditional removed.
  **m2**: `Claim` 9-field literal verified against `domain:298-330`. **m3**: `is_none_or` MSRV
  hedge removed (toolchain pinned 1.97.1). **m4**: T5's `?? undefined` coercion stated.
- Interface table: T1→T2 (`list_claims`), T2→T3 (JSON contract), T3→T4/T5
  (`adminScrapbook` + types), T4→T5 (single-fetch component) — all matched, no forward
  references, no orphan Produces.

## Step-5 record (OMBB sign-off pass @ `4086645`, integrated 2026-09-14)

Verdict was **ready after fixes** — five edits (his four + the payload field his Q1-consequence
note and Lilith's tense ruling made necessary), all in:
- **B1**: `link_is_live` dropped `can_claim`'s Sealed arm (a live feature — 1 of 2,043 prod
  items carries `unlock_at`, measured). Fix: TWO predicates, both through the canonical
  `can_claim` — `link_waits` tolerates exactly `Sealed` (`matches!`, excluded-by-default for
  any future refusal), `link_is_open_door` forgives nothing. Ruling in spec: curated+sealed
  stays in waiting AND says so ("wrapped until ⟨date⟩", the page's only forward-pointing
  item); uncurated+sealed appears nowhere in v1, stated. New payload field `sealed_until`
  (Some iff sealed at `now`) + `sealed_two_half_waits_labeled_never_doors` test + T5 render
  arm + `sealDate` helper.
- **B2**: the deep-link fix could not go red — consuming-side `Links.test.tsx` arm added
  (row `id` + `scrollIntoView` spy; jsdom's missing impl is the honest red), file added to
  T4's Files and run command.
- **M1**: T3 Step 4 retitled characterization, STOP moved into it. **M2/m1/m2**:
  `fx_game_with(id, status, giftable, hidden)` named, `GameStatus` into the test-mod imports,
  "shape only, not signature" on the cross-crate reference.
- His refuted-hunt is on the record too: the T1 scan-count flake he went looking for is
  already prevented by per-test tables (`store_or_skip`'s own comment).
