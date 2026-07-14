# Steam Tags + Content Descriptors + 18+ Auto-Hide Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace publisher-genre chips with Steam community tags on all game cards, store content
descriptors, and auto-hide adult-only games during sync — per spec
`docs/superpowers/specs/2026-07-14-steam-tags-descriptors-design.md` (issue #71).

**Architecture:** `steam-client` learns two keyless endpoints (GetItems batched tags,
GetTagList names) and starts parsing appdetails' `content_descriptors`. Enrichment merges
top-10 tag names + descriptor ids into the existing `SteamAppCache` blobs; a new
`hidden_source` provenance field (mirroring the `appid_source` Manual pattern, including the
top-level DDB attribute) lets sync auto-hide descriptor-{3,4} games without ever fighting an
admin toggle. API views and the web app surface tags (genre fallback), an admin 🔞 badge, a
mature filter, and auto-hide labeling. A backfill run resyncs the existing catalog.

**Tech Stack:** Rust (edition 2024) lambdas — serde/reqwest/wiremock/tokio; DynamoDB single-table;
React 19 + Vite + TS + vitest in `web/`.

## Global Constraints

- All commits GPG-signed (`git commit -S`), author `code kitten <yourcodekitten@gmail.com>`.
- Gates before PR: `cargo fmt --check` · `cargo clippy --workspace --all-targets
  --all-features -- -D warnings` · `cargo test --workspace` · `cd web && npm run build && npx
  vitest run`.
- Never `git push --force`.
- New `SteamAppDetail` / `Game` fields MUST be `#[serde(default)]` — existing dynamo blobs
  must keep deserializing (issue #61 precedent).
- Serde snake_case trap (decisions 2026-07-07): enum values on the wire/DDB are lowercase
  (`"sync"`, `"admin"`) — every DDB condition value must match the serde output exactly.
- Conditional-write tests MUST exercise the DDB-level condition with a state the in-memory
  guard cannot catch (plan-2 void-conditional-write lesson).
- Auto-hide policy constants: hide set `{3, 4}`; admin badge set `{1, 3, 4}`; id 5 renders as
  a detail-view note only; id 2 (violence) surfaces nowhere.
- Tag storage cap 10; display = char-budget fit (min 3, max 6, budget 36).
- Dynamo integration tests need dynamodb-local at `localhost:8000` (`store_or_skip` skips
  gracefully when absent; do not treat a skip as proof).
- Rust tests for one crate: `cargo test -p <crate>`. Web tests: `cd web && npx vitest run
  <file>`.

---

### Task 1: steam-client — content descriptors from appdetails + new SteamAppDetail fields

**Files:**
- Modify: `crates/steam-client/src/lib.rs` (SteamAppDetail ~:30-50, AppDetailDataWire ~:185-204, get_app_details ~:444-507)
- Modify: `crates/steam-client/tests/fixtures/appdetails-413150-trimmed.json` — the fixture
  behind `APPDETAILS_FIXTURE` (`client_test.rs:541` is
  `include_str!("fixtures/appdetails-413150-trimmed.json")`)
- Test: `crates/steam-client/tests/client_test.rs`
- Modify (compile sweep): every `SteamAppDetail {` literal in the workspace — run
  `grep -rn "SteamAppDetail {" crates/` and add the three new fields (tests/fixtures in
  `admin-api` :2050/:2160, `fulfillment` :4278, `dynamo` :1956, `public-api` :1341
  construct it).

**Interfaces:**
- Consumes: existing `AppDetails::Found(Box<SteamAppDetail>)` flow.
- Produces: `SteamAppDetail.tags: Vec<String>` (always empty from `get_app_details` — filled
  by enrichment in Task 6), `SteamAppDetail.content_descriptor_ids: Vec<u32>`,
  `SteamAppDetail.content_notes: Option<String>`. Tasks 6-9 rely on these exact names.

- [ ] **Step 1: Write the failing tests**

In `crates/steam-client/tests/fixtures/appdetails-413150-trimmed.json` (the file behind
`APPDETAILS_FIXTURE`), add a key inside the `"data": {` object **as a single line, exactly
as below** — the second test's `replace` needle must match it byte-for-byte:

```json
   "content_descriptors": { "ids": [1, 5], "notes": "Some nudity." },
```

Add tests (same idiom as `app_details_found_parses_fields` at :548 — wiremock GET
`/api/appdetails`):

```rust
#[tokio::test]
async fn app_details_parses_content_descriptors() {
    let server = wiremock::MockServer::start().await;
    wiremock::Mock::given(wiremock::matchers::method("GET"))
        .and(wiremock::matchers::path("/api/appdetails"))
        .and(wiremock::matchers::query_param("appids", "413150"))
        .respond_with(wiremock::ResponseTemplate::new(200).set_body_string(APPDETAILS_FIXTURE))
        .mount(&server)
        .await;
    let result = test_client(&server).get_app_details(413150).await.unwrap();
    let detail = match result {
        steam_client::AppDetails::Found(d) => d,
        steam_client::AppDetails::Delisted => panic!("expected Found"),
    };
    assert_eq!(detail.content_descriptor_ids, vec![1, 5]);
    assert_eq!(detail.content_notes, Some("Some nudity.".to_string()));
    // get_app_details never fills tags — enrichment owns them (GetItems, Task 6).
    assert!(detail.tags.is_empty());
}

#[tokio::test]
async fn app_details_tolerates_missing_content_descriptors() {
    // Fixture WITHOUT the key — most apps. Serde default must yield empties, not a parse error.
    let server = wiremock::MockServer::start().await;
    let body = APPDETAILS_FIXTURE.replace(
        r#""content_descriptors": { "ids": [1, 5], "notes": "Some nudity." },"#,
        "",
    );
    assert_ne!(body, APPDETAILS_FIXTURE, "needle must have matched — fixture line drifted?");
    wiremock::Mock::given(wiremock::matchers::method("GET"))
        .and(wiremock::matchers::path("/api/appdetails"))
        .respond_with(wiremock::ResponseTemplate::new(200).set_body_string(body))
        .mount(&server)
        .await;
    let result = test_client(&server).get_app_details(413150).await.unwrap();
    let detail = match result {
        steam_client::AppDetails::Found(d) => d,
        steam_client::AppDetails::Delisted => panic!("expected Found"),
    };
    assert!(detail.content_descriptor_ids.is_empty());
    assert_eq!(detail.content_notes, None);
}

#[test]
fn steam_app_detail_blob_backcompat() {
    // A cache blob written before this build (no tags/descriptor fields) must deserialize.
    let old = r#"{"app_id":1,"name":"x","developers":[],"publishers":[],"genres":["RPG"],
        "release_date":null,"short_description":"","header_image":null,
        "video_hls_url":null,"video_thumbnail":null}"#;
    let d: steam_client::SteamAppDetail = serde_json::from_str(old).unwrap();
    assert!(d.tags.is_empty());
    assert!(d.content_descriptor_ids.is_empty());
    assert_eq!(d.content_notes, None);
}
```

(If comma placement in the fixture forces a different exact line, update the `replace`
needle to match it byte-for-byte — the `assert_ne!` guards against a silent non-match.)

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p steam-client --test client_test app_details_parses_content -- --nocapture`
Expected: FAIL — `no field content_descriptor_ids on SteamAppDetail` (compile error counts as
the failing state).

- [ ] **Step 3: Implement**

In `lib.rs` — `SteamAppDetail` gains (after `screenshots`):

```rust
    /// Top user-defined store tags by popularity (names, capped at 10 by the enrichment
    /// pass — GetItems order preserved). Empty when the app is gated/delisted from the
    /// browse surface or the blob predates the field; cards fall back to `genres`.
    #[serde(default)]
    pub tags: Vec<String>,
    /// Raw Steam content descriptor ids (1 nudity/sexual · 2 violence/gore · 3 adult-only
    /// sexual · 4 gratuitous sexual · 5 general mature). Semantics live in the consumers:
    /// domain::ADULT_HIDE_DESCRIPTOR_IDS drives auto-hide, the web's badge set drives 🔞.
    #[serde(default)]
    pub content_descriptor_ids: Vec<u32>,
    /// Steam's free-text descriptor note, verbatim (grammar and all). Admin detail only.
    #[serde(default)]
    pub content_notes: Option<String>,
```

`AppDetailDataWire` gains:

```rust
    #[serde(default)]
    content_descriptors: Option<ContentDescriptorsWire>,
```

New wire struct next to the other wire types:

```rust
/// appdetails' `content_descriptors: {ids, notes}` — dev-selected checkboxes. `required_age`
/// stays ignored (self-reported; Puss! says 0 — issue #71).
#[derive(Deserialize)]
struct ContentDescriptorsWire {
    #[serde(default)]
    ids: Vec<u32>,
    notes: Option<String>,
}
```

In `get_app_details`, before the final `Ok(...)`, and add the three fields to the
`SteamAppDetail` literal:

```rust
        let (content_descriptor_ids, content_notes) = match data.content_descriptors {
            Some(cd) => (
                cd.ids,
                // "" would render a phantom note line — collapse at the wire like the movie URLs.
                cd.notes.filter(|s| !s.trim().is_empty()),
            ),
            None => (Vec::new(), None),
        };
```

```rust
            tags: Vec::new(),
            content_descriptor_ids,
            content_notes,
```

Sweep: `grep -rn "SteamAppDetail {" crates/` — add
`tags: Vec::new(), content_descriptor_ids: Vec::new(), content_notes: None,` (or fixture
values where a test needs them) to every literal so the workspace compiles.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p steam-client --test client_test`
Expected: PASS (all, including the pre-existing appdetails tests).
Then: `cargo test --workspace` — expected PASS (sweep complete).

- [ ] **Step 5: Commit**

```bash
git add crates/
git commit -S -m "steam-client: parse appdetails content descriptors; SteamAppDetail grows tags/descriptor fields (#71)"
```

---

### Task 2: steam-client — GetTagList (tagid → name)

**Files:**
- Modify: `crates/steam-client/src/lib.rs`
- Test: `crates/steam-client/tests/client_test.rs`

**Interfaces:**
- Produces: `SteamClient::get_tag_list(&self) -> Result<std::collections::HashMap<u32, String>, SteamError>`.
  Task 6/7 call it once per run.

- [ ] **Step 1: Write the failing test**

```rust
#[tokio::test]
async fn tag_list_parses_id_name_map() {
    let server = wiremock::MockServer::start().await;
    wiremock::Mock::given(wiremock::matchers::method("GET"))
        .and(wiremock::matchers::path("/IStoreService/GetTagList/v1/"))
        .and(wiremock::matchers::query_param("language", "english"))
        .respond_with(wiremock::ResponseTemplate::new(200).set_body_string(
            r#"{"response":{"version_hash":"abc","tags":[
                {"tagid":19,"name":"Action"},{"tagid":701,"name":"Sports"}]}}"#,
        ))
        .mount(&server)
        .await;
    let map = test_client(&server).get_tag_list().await.unwrap();
    assert_eq!(map.get(&19).map(String::as_str), Some("Action"));
    assert_eq!(map.get(&701).map(String::as_str), Some("Sports"));
    assert_eq!(map.len(), 2);
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p steam-client --test client_test tag_list -- --nocapture`
Expected: FAIL — `no method named get_tag_list`.

- [ ] **Step 3: Implement**

Wire types next to the others:

```rust
#[derive(Deserialize)]
struct TagListWire {
    response: TagListResp,
}
#[derive(Deserialize)]
struct TagListResp {
    #[serde(default)]
    tags: Vec<TagEntry>,
}
#[derive(Deserialize)]
struct TagEntry {
    tagid: u32,
    name: String,
}
```

Method on `impl SteamClient` (keyless — no `key` param, like the store endpoints):

```rust
    /// Fetch the global tagid→name map (`IStoreService/GetTagList`, keyless, ~450 tags).
    /// Callers fetch once per enrichment/backfill run and hold it in memory — the
    /// `version_hash` the endpoint offers isn't worth persisting for one call a day (#71).
    pub async fn get_tag_list(&self) -> Result<std::collections::HashMap<u32, String>, SteamError> {
        let url = format!("{}/IStoreService/GetTagList/v1/", self.base_web_api);
        let resp = self
            .http
            .get(url)
            .query(&[("language", "english")])
            .send()
            .await
            .map_err(net)?;
        let wire: TagListWire = keyed_json(resp).await?;
        Ok(wire
            .response
            .tags
            .into_iter()
            .map(|t| (t.tagid, t.name))
            .collect())
    }
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p steam-client --test client_test tag_list`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/steam-client
git commit -S -m "steam-client: GetTagList — tagid to name map (#71)"
```

---

### Task 3: steam-client — GetItems batched store tags

**Files:**
- Modify: `crates/steam-client/src/lib.rs`
- Test: `crates/steam-client/tests/client_test.rs`

**Interfaces:**
- Produces: `pub struct StoreItemTags { pub tagids: Vec<u32>, pub content_descriptorids: Vec<u32> }`
  and `SteamClient::get_store_items(&self, app_ids: &[u32]) -> Result<std::collections::HashMap<u32, StoreItemTags>, SteamError>`.
  Tasks 6/7 consume both. Chunk size 50 is an internal constant.

- [ ] **Step 1: Write the failing tests**

```rust
#[tokio::test]
async fn store_items_parses_tags_in_popularity_order() {
    let server = wiremock::MockServer::start().await;
    wiremock::Mock::given(wiremock::matchers::method("GET"))
        .and(wiremock::matchers::path("/IStoreBrowseService/GetItems/v1/"))
        .respond_with(wiremock::ResponseTemplate::new(200).set_body_string(
            r#"{"response":{"store_items":[
                {"appid":1294420,"tagids":[19,701,1774],"content_descriptorids":[5]},
                {"appid":981300,"tagids":[12095],"content_descriptorids":[1,3,5,4]},
                {"appid":404,"visible":false,"tagids":[],"content_descriptorids":[]}
            ]}}"#,
        ))
        .mount(&server)
        .await;
    let map = test_client(&server)
        .get_store_items(&[1294420, 981300, 404])
        .await
        .unwrap();
    assert_eq!(map[&1294420].tagids, vec![19, 701, 1774]); // popularity order preserved
    assert_eq!(map[&981300].content_descriptorids, vec![1, 3, 5, 4]);
    // visible:false still lands in the map with empty tags — a SUCCESSFUL fetch of
    // "no tags", which callers may legitimately store (genre fallback takes over).
    assert!(map[&404].tagids.is_empty());
}

#[tokio::test]
async fn store_items_chunks_batches_of_fifty() {
    let server = wiremock::MockServer::start().await;
    wiremock::Mock::given(wiremock::matchers::method("GET"))
        .and(wiremock::matchers::path("/IStoreBrowseService/GetItems/v1/"))
        .respond_with(wiremock::ResponseTemplate::new(200)
            .set_body_string(r#"{"response":{"store_items":[]}}"#))
        .expect(2) // 51 ids → 2 chunks
        .mount(&server)
        .await;
    let ids: Vec<u32> = (1..=51).collect();
    let map = test_client(&server).get_store_items(&ids).await.unwrap();
    assert!(map.is_empty());
    // wiremock asserts the .expect(2) on drop
}

#[tokio::test]
async fn store_items_rate_limited_maps_to_error() {
    let server = wiremock::MockServer::start().await;
    wiremock::Mock::given(wiremock::matchers::method("GET"))
        .and(wiremock::matchers::path("/IStoreBrowseService/GetItems/v1/"))
        .respond_with(wiremock::ResponseTemplate::new(429))
        .mount(&server)
        .await;
    let err = test_client(&server).get_store_items(&[1]).await.unwrap_err();
    assert!(matches!(err, steam_client::SteamError::RateLimited));
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p steam-client --test client_test store_items -- --nocapture`
Expected: FAIL — `no method named get_store_items`.

- [ ] **Step 3: Implement**

Public type near `SteamAppDetail`:

```rust
/// One app's community-tag payload from `IStoreBrowseService/GetItems`: tag ids in
/// popularity order + content descriptor ids. Names resolve via [`SteamClient::get_tag_list`].
#[derive(Debug, Clone, PartialEq)]
pub struct StoreItemTags {
    pub tagids: Vec<u32>,
    pub content_descriptorids: Vec<u32>,
}
```

Wire types:

```rust
#[derive(Deserialize)]
struct StoreItemsWire {
    response: StoreItemsResp,
}
#[derive(Deserialize)]
struct StoreItemsResp {
    #[serde(default)]
    store_items: Vec<StoreItemWire>,
}
#[derive(Deserialize)]
struct StoreItemWire {
    appid: Option<u32>,
    #[serde(default)]
    tagids: Vec<u32>,
    #[serde(default)]
    content_descriptorids: Vec<u32>,
}
```

Method (keyless; chunked like `get_app_list` pages — no inter-chunk pacing, this is the lax
api.steampowered.com host, not throttled appdetails. DECIDED DEVIATION: this supersedes the
spec's "pace batches at 250ms" line — `get_app_list` already pulls 5 unpaced 50k-row pages
from this host daily; ~15 GetItems chunks are strictly gentler. Do not add pacing back):

```rust
    /// Batched community tags + descriptor ids for `app_ids` (`GetItems`, keyless, chunks
    /// of 50). Apps Steam omits (never existed) are absent from the map; gated/delisted
    /// apps come back `visible:false` with EMPTY tagids and are present-with-empty — the
    /// caller stores the empty and lets genre fallback take over (#71 caveat).
    pub async fn get_store_items(
        &self,
        app_ids: &[u32],
    ) -> Result<std::collections::HashMap<u32, StoreItemTags>, SteamError> {
        const CHUNK: usize = 50;
        let url = format!("{}/IStoreBrowseService/GetItems/v1/", self.base_web_api);
        let mut out = std::collections::HashMap::new();
        for chunk in app_ids.chunks(CHUNK) {
            let input = serde_json::json!({
                "ids": chunk.iter().map(|id| serde_json::json!({"appid": id})).collect::<Vec<_>>(),
                "context": {"language": "english", "country_code": "US"},
                "data_request": {"include_tag_count": 20}
            });
            let resp = self
                .http
                .get(&url)
                .query(&[("input_json", input.to_string().as_str())])
                .send()
                .await
                .map_err(net)?;
            let wire: StoreItemsWire = keyed_json(resp).await?;
            for item in wire.response.store_items {
                if let Some(appid) = item.appid {
                    out.insert(
                        appid,
                        StoreItemTags {
                            tagids: item.tagids,
                            content_descriptorids: item.content_descriptorids,
                        },
                    );
                }
            }
        }
        Ok(out)
    }
```

(`serde_json` is already a dependency of the crate — `lenient_category_id` uses it. If it is
`dev-dependencies`-only, move it to `[dependencies]`.)

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p steam-client --test client_test store_items`
Expected: PASS (3 tests).

- [ ] **Step 5: Commit**

```bash
git add crates/steam-client
git commit -S -m "steam-client: GetItems batched community tags + descriptor ids (#71)"
```

---

### Task 4: domain — HiddenSource provenance + policy constant

**Files:**
- Modify: `crates/domain/src/lib.rs` (enum near `AppidSource` :25-34, `Game` :36-92,
  `merge_sync` :223-283; inline tests at :316+)
- Modify (compile sweep): every `Game {` literal in the workspace — run
  `grep -rn "hidden: " crates/ | grep -v hidden_source` to find constructors; add
  `hidden_source: None,` to each.

**Interfaces:**
- Produces: `domain::HiddenSource` (`Admin | Sync`, serde snake_case → `"admin"`/`"sync"`),
  `Game.hidden_source: Option<HiddenSource>` (`#[serde(default)]`),
  `domain::ADULT_HIDE_DESCRIPTOR_IDS: [u32; 2]`. Tasks 5-7, 9 consume these exact names.

- [ ] **Step 1: Write the failing tests** (inline `#[cfg(test)]` block, next to the existing
  merge tests)

```rust
    #[test]
    fn hidden_source_serde_is_snake_case() {
        assert_eq!(
            serde_json::to_string(&HiddenSource::Sync).unwrap(),
            r#""sync""#
        );
        assert_eq!(
            serde_json::to_string(&HiddenSource::Admin).unwrap(),
            r#""admin""#
        );
    }

    #[test]
    fn game_blob_backcompat_hidden_source_defaults_none() {
        // Serialize a pre-field game shape by stripping the key from a current one.
        let mut v = serde_json::to_value(fresh_game()).unwrap();
        v.as_object_mut().unwrap().remove("hidden_source");
        let g: Game = serde_json::from_value(v).unwrap();
        assert_eq!(g.hidden_source, None);
    }

    #[test]
    fn merge_sync_carries_hidden_source_both_branches() {
        // Humble-owned branch (Available). fresh MUST differ on a refreshed field
        // (title) — if hidden/hidden_source were the only diffs, a correct merge
        // carries both from existing, merged == existing, and merge_sync returns
        // None (the no-op contract). The assertions below then prove carry-over
        // happened on a merge that actually wrote.
        let mut existing = fresh_game();
        existing.hidden = true;
        existing.hidden_source = Some(HiddenSource::Sync);
        let mut fresh = fresh_game();
        fresh.title = "renamed".into();
        let merged = merge_sync(Some(&existing), fresh).expect("title differs → Some");
        assert_eq!(merged.hidden_source, Some(HiddenSource::Sync));
        assert!(merged.hidden);

        // App-owned branch (Gifted)
        let mut existing = fresh_game();
        existing.status = GameStatus::Gifted;
        existing.hidden_source = Some(HiddenSource::Admin);
        let mut fresh = fresh_game();
        fresh.title = "renamed".into();
        let merged = merge_sync(Some(&existing), fresh).expect("title differs → Some");
        assert_eq!(merged.hidden_source, Some(HiddenSource::Admin));
    }
```

(`fresh_game()` at `crates/domain/src/lib.rs:420` is the test module's existing Game
fixture — use it, don't invent another.)

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p domain hidden_source -- --nocapture`
Expected: FAIL — `cannot find type HiddenSource` (compile error).

- [ ] **Step 3: Implement**

Enum after `AppidSource`:

```rust
/// Who last decided a game's `hidden` flag. `Admin` is Ben's toggle and is FINAL: the
/// auto-hide sweep never overrides it in either direction — his unhide of an adult game
/// stays unhidden forever (#71 "never fights Ben"). `Sync` marks an automatic hide
/// (adult content descriptors) so admin can label it. `None` (legacy / never touched)
/// is auto-hide-eligible: every pre-existing unhidden game is untouched-by-Ben by
/// definition; his first toggle stamps `Admin` and immunizes the record.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HiddenSource {
    Admin,
    Sync,
}

/// Content descriptor ids that auto-hide a game at sync/backfill time: 3 = adult-only
/// sexual content, 4 = gratuitous sexual content (Puss! carries both). NOT 1 (some
/// nudity — Witcher 3), NOT 5 (general mature — Rollerdrome), NOT 2 (violence).
/// Ben tightens/loosens by editing this list (#71).
pub const ADULT_HIDE_DESCRIPTOR_IDS: [u32; 2] = [3, 4];
```

`Game` gains (after `owned_by_ben`):

```rust
    /// Provenance of [`hidden`](Self::hidden) — see [`HiddenSource`]. `None` iff no
    /// admin toggle or auto-hide has ever run on this record.
    /// `#[serde(default)]`: records written before this field existed deserialize to `None`.
    #[serde(default)]
    pub hidden_source: Option<HiddenSource>,
```

`merge_sync`: in the Pending/Gifted branch's `Game { ... }` literal add
`hidden_source: existing_game.hidden_source,` (next to `hidden`); in the
Available/BenRedeemed/Expired branch add `hidden_source: existing_game.hidden_source,`
alongside the existing `hidden: existing_game.hidden,` — sync NEVER moves provenance.

Sweep every `Game {` literal (fulfillment sync builders, all test fixtures):
`hidden_source: None,`.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p domain` then `cargo test --workspace`
Expected: PASS (sweep complete → workspace compiles).

- [ ] **Step 5: Commit**

```bash
git add crates/
git commit -S -m "domain: HiddenSource provenance on Game + adult auto-hide descriptor policy (#71)"
```

---

### Task 5: dynamo — hidden_source mirror, Admin stamping, guarded auto_hide_game

**Files:**
- Modify: `crates/dynamo/src/schema.rs` (`game_item` :42-85)
- Modify: `crates/dynamo/src/lib.rs` (`HiddenWrite` :50-58 area, `set_game_hidden`
  :1284-1330)
- Test: `crates/dynamo/tests/store_test.rs` (`store_or_skip` idiom; dynamodb-local)

**Interfaces:**
- Consumes: `domain::HiddenSource`, `Game.hidden_source` (Task 4).
- Produces: `pub enum AutoHideWrite { Written, NotFound, AlreadyHidden, AdminOwned, Contested }`
  and `Store::auto_hide_game(&self, game_id: &str) -> Result<AutoHideWrite, StoreError>`.
  `set_game_hidden` now stamps `hidden_source = Some(Admin)` on every admin toggle.
  Tasks 6/7 consume `auto_hide_game`; Task 9 reads `hidden_source` off `Game`.

- [ ] **Step 1: Write the failing tests**

The file's fixtures (verified): `game(n: u32, listable: bool) -> Game` at
`store_test.rs:55` (id derives from `gk{n}`), `store_or_skip(test)` creates a per-test
table named `t-{test}`, and `raw_client(test)` at `store_test.rs:14` hands back a raw
`aws_sdk_dynamodb::Client` (the `appid_source_is_top_level_attribute` test at ~:1557 shows
the raw get/update idiom). Add two free helper fns to the TEST FILE (NOT methods on
`Store` — raw attribute surgery has no business on the production type):

```rust
async fn raw_set_top_level_string(test: &str, game_id: &str, attr: &str, val: &str) {
    let client = raw_client(test).await;
    client
        .update_item()
        .table_name(format!("t-{test}"))
        .key("pk", AttributeValue::S(format!("GAME#{game_id}")))
        .key("sk", AttributeValue::S("META".into()))
        .update_expression("SET #a = :v")
        .expression_attribute_names("#a", attr)
        .expression_attribute_values(":v", AttributeValue::S(val.into()))
        .send()
        .await
        .unwrap();
}

async fn raw_get_top_level_string(test: &str, game_id: &str, attr: &str) -> Option<String> {
    let client = raw_client(test).await;
    let out = client
        .get_item()
        .table_name(format!("t-{test}"))
        .key("pk", AttributeValue::S(format!("GAME#{game_id}")))
        .key("sk", AttributeValue::S("META".into()))
        .send()
        .await
        .unwrap();
    out.item()
        .and_then(|i| i.get(attr))
        .and_then(|v| v.as_s().ok())
        .cloned()
}
```

(Match the `GAME#`/`META` key strings against the file's existing raw test before trusting
them — the schema helpers are the source of truth.)

The tests:

```rust
#[tokio::test]
async fn auto_hide_sets_hidden_and_sync_source() {
    let Some(store) = store_or_skip("auto_hide_sets_hidden_and_sync_source").await else { return };
    let g = game(1, true);
    store.put_game(&g).await.unwrap();
    assert_eq!(store.auto_hide_game(&g.id).await.unwrap(), AutoHideWrite::Written);
    let read = store.get_game(&g.id).await.unwrap().unwrap();
    assert!(read.hidden);
    assert_eq!(read.hidden_source, Some(domain::HiddenSource::Sync));
}

#[tokio::test]
async fn auto_hide_respects_admin_unhide_forever() {
    let Some(store) = store_or_skip("auto_hide_respects_admin_unhide_forever").await else { return };
    let g = game(1, true);
    store.put_game(&g).await.unwrap();
    // Ben hides then unhides — both stamp Admin.
    assert_eq!(store.set_game_hidden(&g.id, true).await.unwrap(), HiddenWrite::Written);
    assert_eq!(store.set_game_hidden(&g.id, false).await.unwrap(), HiddenWrite::Written);
    // Auto-hide must refuse.
    assert_eq!(store.auto_hide_game(&g.id).await.unwrap(), AutoHideWrite::AdminOwned);
    let read = store.get_game(&g.id).await.unwrap().unwrap();
    assert!(!read.hidden, "ben's unhide must stand");
    assert_eq!(read.hidden_source, Some(domain::HiddenSource::Admin));
}

#[tokio::test]
async fn auto_hide_already_hidden_is_noop() {
    let Some(store) = store_or_skip("auto_hide_already_hidden_is_noop").await else { return };
    let mut g = game(1, true);
    g.hidden = true; // ben's manual hide from before provenance existed (Puss! today)
    store.put_game(&g).await.unwrap();
    assert_eq!(store.auto_hide_game(&g.id).await.unwrap(), AutoHideWrite::AlreadyHidden);
    let read = store.get_game(&g.id).await.unwrap().unwrap();
    assert_eq!(read.hidden_source, None, "no-op must not stamp provenance");
}

#[tokio::test]
async fn auto_hide_mid_claim_is_contested() {
    let Some(store) = store_or_skip("auto_hide_mid_claim_is_contested").await else { return };
    let mut g = game(1, true);
    g.status = GameStatus::Pending; // a claim is in flight
    store.put_game(&g).await.unwrap();
    assert_eq!(store.auto_hide_game(&g.id).await.unwrap(), AutoHideWrite::Contested);
    let read = store.get_game(&g.id).await.unwrap().unwrap();
    assert!(!read.hidden, "mid-claim games are left alone");
}

#[tokio::test]
async fn auto_hide_ddb_condition_fires_without_in_memory_guard() {
    // The plan-2 lesson: prove the DDB-LEVEL guard, not the fast path. Seed an item whose
    // BODY says hidden_source: null but whose TOP-LEVEL attribute says "admin" — only the
    // condition expression can see the mismatch. (This state can't arise from our writers;
    // it's a scalpel for the guard.)
    let test = "auto_hide_ddb_condition_fires";
    let Some(store) = store_or_skip(test).await else { return };
    let g = game(1, true);
    store.put_game(&g).await.unwrap();
    raw_set_top_level_string(test, &g.id, "hidden_source", "admin").await;
    assert_eq!(store.auto_hide_game(&g.id).await.unwrap(), AutoHideWrite::Contested);
    let read = store.get_game(&g.id).await.unwrap().unwrap();
    assert!(!read.hidden, "the guarded put must not have landed");
}

#[tokio::test]
async fn admin_toggle_stamps_admin_source_top_level_snake_case() {
    let test = "admin_toggle_stamps_admin";
    let Some(store) = store_or_skip(test).await else { return };
    let g = game(1, true);
    store.put_game(&g).await.unwrap();
    store.set_game_hidden(&g.id, true).await.unwrap();
    // Raw attribute check: the mirror must exist and be serde-snake_case ("admin"),
    // or every future condition against it is dead (void-conditional-write lesson).
    let attr = raw_get_top_level_string(test, &g.id, "hidden_source").await;
    assert_eq!(attr.as_deref(), Some("admin"));
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p dynamo --test store_test auto_hide -- --nocapture` (dynamodb-local must
be reachable at `localhost:8000` — `curl -s localhost:8000` answers; if not, start it before
trusting any result)
Expected: FAIL — `no method named auto_hide_game` / `no variant AutoHideWrite`.

- [ ] **Step 3: Implement**

`schema.rs` `game_item`, after the `appid_source` mirror block:

```rust
    // Top-level `hidden_source` mirrors the body so auto-hide's PutItem condition can guard
    // against racing an admin toggle. Only written when Some — attribute_not_exists then
    // correctly matches legacy items (never admin-touched → auto-hide eligible).
    if let Some(src) = g.hidden_source {
        item.insert(
            "hidden_source".into(),
            s(serde_json::to_value(src)
                .expect("hidden_source serializes")
                .as_str()
                .expect("hidden_source is a string")
                .to_string()),
        );
    }
```

`lib.rs` — `set_game_hidden`, after `game.hidden = hidden;`:

```rust
        // Every admin toggle — hide OR unhide — stamps Admin: from this moment the
        // auto-hide sweep defers to Ben on this record forever (#71).
        game.hidden_source = Some(domain::HiddenSource::Admin);
```

New enum next to `HiddenWrite`:

```rust
/// Outcome of the sync auto-hide write. Non-`Written` variants are all "leave it alone":
/// the sweep re-evaluates next run, and Admin provenance is permanent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AutoHideWrite {
    /// Game is now hidden with `hidden_source = Sync`.
    Written,
    /// No game with that ID exists.
    NotFound,
    /// Already hidden (by anyone) — no write, provenance untouched.
    AlreadyHidden,
    /// `hidden_source == Admin` — Ben decided; the sweep never overrides him.
    AdminOwned,
    /// Optimistic-lock CCF (claim flipped status mid-window, or an admin write landed
    /// between our read and put). Skip; next sync retries.
    Contested,
}
```

New method after `set_game_hidden`:

```rust
    /// Sync auto-hide: the ONLY writer allowed to set `hidden` without admin intent.
    /// One-way (never unhides). Same status optimistic-lock as `set_game_hidden`, plus a
    /// condition on the top-level `hidden_source` mirror so an admin toggle landing inside
    /// the read→write window wins (#71 "never fights Ben"; `appid_source` Manual-guard
    /// pattern).
    pub async fn auto_hide_game(&self, game_id: &str) -> Result<AutoHideWrite, StoreError> {
        let Some(mut game) = self.get_game(game_id).await? else {
            return Ok(AutoHideWrite::NotFound);
        };
        if game.hidden {
            return Ok(AutoHideWrite::AlreadyHidden);
        }
        if game.hidden_source == Some(domain::HiddenSource::Admin) {
            return Ok(AutoHideWrite::AdminOwned);
        }
        if game.status == GameStatus::Pending {
            return Ok(AutoHideWrite::Contested);
        }

        game.hidden = true;
        game.hidden_source = Some(domain::HiddenSource::Sync);

        let status_str = serde_json::to_value(game.status)
            .expect("status serializes")
            .as_str()
            .expect("status is a string")
            .to_string();

        let res = self
            .client
            .put_item()
            .table_name(&self.table)
            .set_item(Some(game_item(&game)))
            .expression_attribute_names("#st", "status")
            .expression_attribute_names("#hsrc", "hidden_source")
            .expression_attribute_values(":expected", schema::s(status_str))
            .expression_attribute_values(":admin", schema::s("admin".to_string()))
            .condition_expression(
                "#st = :expected AND (attribute_not_exists(#hsrc) OR #hsrc <> :admin)",
            )
            .send()
            .await;

        match res {
            Ok(_) => Ok(AutoHideWrite::Written),
            Err(sdk_err) => {
                if is_ccf_put(&sdk_err) {
                    Ok(AutoHideWrite::Contested)
                } else {
                    Err(StoreError::Aws(format!("{sdk_err:?}")))
                }
            }
        }
    }
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p dynamo --test store_test` (dynamodb-local up — do not accept a skip as
green for THESE tests; confirm they actually ran in the output)
Expected: PASS, new tests listed as `ok` not skipped.

- [ ] **Step 5: Commit**

```bash
git add crates/dynamo
git commit -S -m "dynamo: hidden_source mirror + admin stamping + guarded auto_hide_game (#71)"
```

---

### Task 6: fulfillment — enrichment merges tags/descriptors + auto-hide sweep

**Files:**
- Modify: `crates/fulfillment/src/lib.rs` (`enrich_steam_apps` :1894-2077; new helpers +
  constant near `STEAM_ENRICH_MAX_APPS` :36)
- Test: `crates/fulfillment/tests/handler_test.rs`

**Interfaces:**
- Consumes: `steam.get_store_items`, `steam.get_tag_list` (Tasks 2-3),
  `store.auto_hide_game` + `AutoHideWrite` (Task 5), `domain::ADULT_HIDE_DESCRIPTOR_IDS`
  (Task 4).
- Produces: `pub const STEAM_TAG_STORE_CAP: usize = 10;` and
  `fn tags_for_app(app_id, tag_data, prev_detail) -> Vec<String>` (private, shared with
  Task 7 in this same file).

- [ ] **Step 1: Write the failing tests**

The file's enrichment-test wiring (verified — copy it, don't invent): `store_or_skip` at
`handler_test.rs:67`; `steam_client_at(uri: &str) -> Arc<steam_client::SteamClient>` at
`:3680`; `seed_steam_game(...)` at `:3692` (read its signature and use it to seed a mapped
game); `far_deadline()` at `:4185`; `mount_steam_ok(steam: &MockServer, app_id: u32)` at
`:4255` mounts happy-path appdetails+reviews+histogram. Deps are built as:

```rust
let mut d = deps(store, "http://unused", None);
d.steam = Some(steam_client_at(&steam_mock.uri()));
```

For descriptor-carrying appdetails, mount your own `/api/appdetails` mock (200, body with
`"content_descriptors": {"ids":[1,3,5,4],"notes":"This game have Nudity and Sexual
Content."}` inside `data`) instead of `mount_steam_ok`'s fixture, or extend a copied body —
mirror how `mount_steam_ok` builds its response. Every test below ALSO mounts
`/IStoreService/GetTagList/v1/` (200: ids 19→"Action", 12095→"Sexual Content") and
`/IStoreBrowseService/GetItems/v1/` unless the test says otherwise — the implementation is
both-or-nothing, so an unmocked GetTagList 404s and silently flips the test into the
"batch failed" path.

```rust
#[tokio::test]
async fn enrich_merges_tags_descriptors_and_auto_hides_adult() {
    // seed: one Available game mapped to appid 981300 (hidden:false, hidden_source:None);
    // mocks: adult appdetails + reviews/histogram + GetItems (tagids [12095, 19]) + GetTagList.
    // run: enrich_steam_apps(&d, far_deadline()).await;
    let cache = store.get_steam_app(981300).await.unwrap().unwrap();
    let detail = cache.detail.unwrap();
    assert_eq!(detail.tags, vec!["Sexual Content".to_string(), "Action".to_string()]); // GetItems order
    assert_eq!(detail.content_descriptor_ids, vec![1, 3, 5, 4]);
    assert!(detail.content_notes.unwrap().contains("Nudity"));
    let g = store.get_game(&gid).await.unwrap().unwrap();
    assert!(g.hidden);
    assert_eq!(g.hidden_source, Some(domain::HiddenSource::Sync));
}

#[tokio::test]
async fn enrich_does_not_hide_non_adult_descriptors() {
    // same shape; appdetails descriptors [1,5] (Witcher-like), GetItems tagids [19].
    // assert: cache.detail.tags == ["Action"]; game.hidden stays false; hidden_source None.
}

#[tokio::test]
async fn enrich_never_rehides_admin_unhidden_game() {
    // seed mapped game; store.set_game_hidden(&gid, true) then set_game_hidden(&gid, false)
    // (ben unhid an adult game); adult mocks as in the first test.
    // run enrich; assert: hidden == false, hidden_source == Some(Admin).
}

#[tokio::test]
async fn enrich_one_way_never_unhides_when_descriptors_clear() {
    // seed mapped game ALREADY hidden with hidden_source Sync (a prior auto-hide), and a
    // stale cache (fetched_at 0). Mocks: appdetails WITHOUT content_descriptors + GetItems
    // + GetTagList. run enrich; assert: game stays hidden, hidden_source stays Sync —
    // descriptors disappearing later never un-hides (#71 one-way rule).
}

#[tokio::test]
async fn enrich_preserves_old_tags_when_getitems_fails() {
    // seed STEAMAPP cache for the appid: detail.tags = vec!["Old Tag".into()], fetched_at 0
    // (stale → need_detail). Mocks: appdetails 200, GetTagList 200, GetItems → 500.
    // run enrich; assert cache.detail.tags == ["Old Tag"] (batch failure must not wipe)
    // AND an appdetails-refreshed field (e.g. name) DID update — proves the detail write
    // happened and only tags were preserved.
}

#[tokio::test]
async fn enrich_stores_empty_tags_when_app_absent_from_successful_batch() {
    // seed STEAMAPP cache with detail.tags = vec!["Old Tag".into()] and fetched_at 0 —
    // NON-EMPTY seed is the point: an empty seed would pass against a no-op.
    // Mocks: appdetails 200, GetTagList 200, GetItems 200 with store_items: [] (the
    // gated/delisted case — successful batch, app absent).
    // run enrich; assert cache.detail.tags.is_empty() — a successful "no tags" answer
    // OVERWRITES (genre fallback takes over client-side).
}
```

(The three prose-bodied tests follow the first test's arrange/act/assert shape exactly —
same seeding helpers, same mock set, different fixture values; write them out fully.)

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p fulfillment --test handler_test enrich_merges -- --nocapture`
Expected: FAIL — tags empty / game not hidden (enrichment doesn't fetch GetItems yet).

- [ ] **Step 3: Implement**

Constant near the other STEAM_ consts:

```rust
/// Top-N user tags stored per app. Display caps are client-side (char-budget fit), so
/// tuning what cards SHOW never needs a backfill — only widening storage does (#71).
pub const STEAM_TAG_STORE_CAP: usize = 10;
```

Helper near `enrich_steam_apps`:

```rust
/// Resolve one app's stored tag list from the batch results. `tag_data` None = the
/// GetItems/GetTagList batch failed → preserve the previous blob's tags (a network hiccup
/// must never strip chips). Present-but-absent appid in a SUCCESSFUL batch = Steam hides
/// the app from the browse surface → store empty and let genre fallback take over (#71).
fn tags_for_app(
    app_id: u32,
    tag_data: Option<&(
        std::collections::HashMap<u32, steam_client::StoreItemTags>,
        std::collections::HashMap<u32, String>,
    )>,
    prev_detail: Option<&steam_client::SteamAppDetail>,
) -> Vec<String> {
    match tag_data {
        Some((items, names)) => match items.get(&app_id) {
            Some(item) => item
                .tagids
                .iter()
                .filter_map(|id| names.get(id).cloned())
                .take(STEAM_TAG_STORE_CAP)
                .collect(),
            None => Vec::new(),
        },
        None => prev_detail.map(|d| d.tags.clone()).unwrap_or_default(),
    }
}
```

In `enrich_steam_apps`, after `worklist.truncate(STEAM_ENRICH_MAX_APPS);`:

```rust
    // Community tags ride ONE batched keyless call pair per pass (GetItems chunks +
    // GetTagList), not per-app storefront calls. Both-or-nothing: resolving tag names
    // with a partial map would silently store a truncated tag list.
    // DECIDED DEVIATION from the spec's "GetItems 429 aborts the pass": ANY tag-batch
    // failure (429 included) logs, preserves existing tags, and lets the pass continue —
    // the spec's own Error-handling section ("abort tags for the pass, detail writes keep
    // their old tags") is the semantics we implement; a keyless tag endpoint hiccup must
    // not starve appdetails/reviews refreshes.
    let detail_ids: Vec<u32> = worklist
        .iter()
        .filter(|w| w.need_detail)
        .map(|w| w.app_id)
        .collect();
    let tag_data = if detail_ids.is_empty() {
        None
    } else {
        match (
            steam.get_store_items(&detail_ids).await,
            steam.get_tag_list().await,
        ) {
            (Ok(items), Ok(names)) => Some((items, names)),
            (items_res, names_res) => {
                tracing::warn!(
                    items_err = items_res.is_err(),
                    names_err = names_res.is_err(),
                    "steam enrichment: tag batch failed — preserving existing tags this pass"
                );
                None
            }
        }
    };

    // appid → game ids, for the auto-hide sweep (games is already in memory).
    let mut games_by_appid: std::collections::HashMap<u32, Vec<String>> =
        std::collections::HashMap::new();
    for g in &games {
        if let Some(id) = g.steam_app_id {
            games_by_appid.entry(id).or_default().push(g.id.clone());
        }
    }
    let mut auto_hidden = 0u32;
```

In the detail-fetch arm, replace the `Ok(steam_client::AppDetails::Found(d))` body:

```rust
                Ok(steam_client::AppDetails::Found(d)) => {
                    let mut detail = *d;
                    detail.tags = tags_for_app(app_id, tag_data.as_ref(), cache.detail.as_ref());
                    cache.detail = Some(detail);
                    cache.fetched_at = now;
                    fresh += 1;
                }
```

After the successful `put_steam_app` (`fetched += 1;`), the sweep:

```rust
        // Auto-hide: adult descriptors on the (possibly just-refreshed) detail hide every
        // mapped game that Ben hasn't personally decided on. Runs even on reviews-only
        // refreshes over CACHED descriptors — deliberate: a descriptor doesn't need to be
        // fresh to be true, and the sweep is idempotent. One-way; all non-Written
        // outcomes are deliberate leave-it-alones (see AutoHideWrite).
        let adult = cache.detail.as_ref().is_some_and(|d| {
            d.content_descriptor_ids
                .iter()
                .any(|id| domain::ADULT_HIDE_DESCRIPTOR_IDS.contains(id))
        });
        if adult {
            for gid in games_by_appid.get(&app_id).map(Vec::as_slice).unwrap_or(&[]) {
                match deps.store.auto_hide_game(gid).await {
                    Ok(dynamo::AutoHideWrite::Written) => {
                        auto_hidden += 1;
                        tracing::info!(app_id, game_id = %gid, "steam enrichment: auto-hid adult game");
                    }
                    Ok(outcome) => {
                        tracing::debug!(app_id, game_id = %gid, ?outcome, "steam enrichment: auto-hide left alone");
                    }
                    Err(e) => {
                        tracing::warn!(app_id, game_id = %gid, error = ?e, "steam enrichment: auto-hide write failed");
                    }
                }
            }
        }
```

Extend the summary log line with `auto_hidden={auto_hidden}` (and add `auto_hidden` to the
doc comment's documented line shape at :1891).

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p fulfillment --test handler_test enrich`
Expected: PASS — new tests plus every pre-existing enrichment test (pace ZERO keeps them fast).

- [ ] **Step 5: Commit**

```bash
git add crates/fulfillment
git commit -S -m "fulfillment: enrichment merges community tags/descriptors + adult auto-hide sweep (#71)"
```

---

### Task 7: fulfillment — backfill does tags + auto-hide (the DB resync tool)

**Files:**
- Modify: `crates/fulfillment/src/lib.rs` (`backfill_steam_details` :2103-2180,
  `BackfillSummary` :2080-2092)
- Modify: `crates/fulfillment/src/bin/backfill_details.rs` — the summary `println!` at ~:50
  prints explicit fields (`fetched={} negative={} skipped={} failed={} aborted_429={}`),
  NOT Debug; extend it with ` auto_hidden={}` + `summary.auto_hidden`.
- Test: `crates/fulfillment/tests/handler_test.rs`

**Interfaces:**
- Consumes: `tags_for_app` + the auto-hide sweep block, both already committed in
  `enrich_steam_apps` in THIS file (copy the sweep from there — you cannot see the task
  that wrote it); `get_store_items`, `get_tag_list`, `auto_hide_game` (Tasks 2-5).
- Produces: `BackfillSummary.auto_hidden: u32`, AND the bin's summary line grows an
  `auto_hidden={}` field — the operator runbook greps for it.

- [ ] **Step 1: Write the failing test**

```rust
#[tokio::test]
async fn backfill_populates_tags_and_auto_hides() {
    // seed: game mapped to appid 981300 with an EXISTING cache whose fetched_at is old
    // (outside skip_fresh) and whose detail has empty tags/descriptors.
    // mocks: appdetails with adult descriptors, GetItems, GetTagList — as Task 6.
    // run: backfill_steam_details(&store, &steam, Duration::ZERO, 43200).await.unwrap()
    // assert: summary.fetched == 1 && summary.auto_hidden == 1;
    //         cache.detail.tags non-empty; game hidden with Sync source.
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p fulfillment --test handler_test backfill_populates -- --nocapture`
Expected: FAIL — `no field auto_hidden on BackfillSummary`.

- [ ] **Step 3: Implement**

`BackfillSummary` gains:

```rust
    /// Games auto-hidden by the adult-descriptor sweep during this run.
    pub auto_hidden: u32,
```

In `backfill_steam_details`, after the `appids` BTreeSet is built, add the same batched
tag fetch + games map as Task 6 (the whole catalog's appids this time):

```rust
    let all_ids: Vec<u32> = appids.iter().copied().collect();
    let tag_data = if all_ids.is_empty() {
        None
    } else {
        match (steam.get_store_items(&all_ids).await, steam.get_tag_list().await) {
            (Ok(items), Ok(names)) => Some((items, names)),
            (items_res, names_res) => {
                tracing::warn!(
                    items_err = items_res.is_err(),
                    names_err = names_res.is_err(),
                    "backfill: tag batch failed — preserving existing tags"
                );
                None
            }
        }
    };
    let mut games_by_appid: std::collections::HashMap<u32, Vec<String>> =
        std::collections::HashMap::new();
    for g in &games {
        if let Some(id) = g.steam_app_id {
            games_by_appid.entry(id).or_default().push(g.id.clone());
        }
    }
```

Replace the `Ok(steam_client::AppDetails::Found(d))` arm body:

```rust
            Ok(steam_client::AppDetails::Found(d)) => {
                let mut detail = *d;
                detail.tags = tags_for_app(app_id, tag_data.as_ref(), cache.detail.as_ref());
                cache.detail = Some(detail);
                cache.fetched_at = now;
            }
```

After the successful `put_steam_app` (before the `done` accounting), copy the auto-hide
sweep block from `enrich_steam_apps` in this same file (the `let adult = ...; if adult {`
block after its `fetched += 1;`), with two mechanical swaps: `summary.auto_hidden += 1;`
instead of `auto_hidden += 1;`, and `store.auto_hide_game(gid)` instead of
`deps.store.auto_hide_game(gid)`. Update the run-summary `tracing::info!` at the end of the
fn to include `auto_hidden = summary.auto_hidden`, and extend the bin's `println!` (see
Files) so the operator sees the count.

NOTE the skip-fresh interaction: an app skipped for freshness also skips the sweep — fine
for the resync (fresh items were just enriched by Task 6's code, which sweeps), but document
it in the fn doc comment:

```rust
/// Skip-fresh items skip the auto-hide sweep too — acceptable: anything fresh was written
/// by the enrichment pass, which runs the identical sweep (#71).
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p fulfillment --test handler_test backfill`
Expected: PASS — new + all pre-existing backfill tests.

- [ ] **Step 5: Commit**

```bash
git add crates/fulfillment
git commit -S -m "fulfillment: backfill resync carries tags/descriptors + auto-hide (#71)"
```

---

### Task 8: public-api — tags on the friend list payload

**Files:**
- Modify: `crates/public-api/src/lib.rs` (`GameView` :76-107, list join ~:495-520; detail
  endpoint's `from_game` call site)
- Test: `crates/public-api/tests/` (find the list-endpoint test:
  `grep -rn "genres" crates/public-api/tests/`)

**Interfaces:**
- Consumes: `SteamAppCache.detail.tags` (Tasks 1/6).
- Produces: wire field `tags: string[]` on the friend list's game objects (omitted when
  empty). Task 10's TS mirror relies on the name `tags`.

- [ ] **Step 1: Write the failing test**

Add a NEW test `link_list_carries_tags_from_steam_cache` next to the existing
`link_list_carries_genres_from_steam_cache` (`crates/public-api/tests/api_test.rs:1717`),
cloned from it: seed the STEAMAPP cache detail with
`tags: vec!["Roguelike".into(), "Sci-fi".into()]`, assert the response game object carries
`"tags": ["Roguelike","Sci-fi"]`, and that a game whose cache has empty tags omits the
`tags` key entirely (serde skip) — same assertion style as the genres test.

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p public-api --test api_test link_list_carries_tags -- --nocapture`
Expected: FAIL with an assertion on the missing `tags` key — NOT "0 tests run" (if the
filter matches nothing, the test name is wrong; a 0-run green is not a red).

- [ ] **Step 3: Implement**

`GameView` gains (after `genres`):

```rust
    /// Top user tags (popularity order, ≤10) from the enrichment cache — the card chips.
    /// Genres stay as the fallback for tag-less apps AND for deploy-window back-compat
    /// (an older cached SPA bundle still reads `genres`). Empty → omitted.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tags: Vec<String>,
```

`from_game` becomes `from_game(g: domain::Game, genres: Vec<String>, tags: Vec<String>)`
with `tags,` in the literal; the detail endpoint's call site passes `Vec::new()` for both
(unchanged contract: the modal reads the full steam blob). List join gains, next to the
genres extraction:

```rust
                    let tags = g
                        .steam_app_id
                        .and_then(|id| caches.get(&id))
                        .and_then(|c| c.detail.as_ref())
                        .map(|d| d.tags.clone())
                        .unwrap_or_default();
                    GameView::from_game(g, genres, tags)
```

(Stored tags are already capped at 10 — no `.take()` here.)

Deliberate non-change: the friend DETAIL endpoint (`:784` call site) serializes the whole
`SteamAppDetail` blob, which now carries descriptor ids/notes on the friend wire. Accepted
in the spec (public Steam metadata; friend UI never renders it) — do NOT add a strip layer.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p public-api`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/public-api
git commit -S -m "public-api: friend list carries community tags (genre fallback stays) (#71)"
```

---

### Task 9: admin-api — tags/descriptors on catalog rows + hidden_source

**Files:**
- Modify: `crates/admin-api/src/lib.rs` (`CatalogGameView` :253-268, `SteamSummaryView`
  :273-286, `steam_summary` :291-316, the `CatalogGameView` construction inside
  `handle_catalog`)
- Test: `crates/admin-api/tests/api_test.rs` (catalog tests at ~:243+)

**Interfaces:**
- Consumes: `Game.hidden_source` (Task 4), `SteamAppDetail.tags`/`content_descriptor_ids`
  (Task 1).
- Produces: wire fields `steam.tags: string[]`, `steam.content_descriptor_ids: number[]`,
  `hidden_source: "admin"|"sync"|null` on catalog rows. Task 10's TS mirrors rely on these
  names.

- [ ] **Step 1: Write the failing test**

Extend the existing catalog join test (fixture builds a `SteamAppCache`): set
`detail.tags = vec!["Roguelike".into()]`, `detail.content_descriptor_ids = vec![1, 5]`, and
seed the game with `hidden_source: Some(domain::HiddenSource::Sync)`. Assert the row JSON:
`steam.tags == ["Roguelike"]`, `steam.content_descriptor_ids == [1,5]`,
`hidden_source == "sync"`; and a provenance-less game serializes `hidden_source: null`.

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p admin-api --test api_test catalog -- --nocapture`
Expected: FAIL — missing fields.

- [ ] **Step 3: Implement**

`SteamSummaryView` gains:

```rust
    /// Top user tags (popularity order, ≤10) — the toolkit's chips + tag filter.
    tags: Vec<String>,
    /// Raw content descriptor ids — the 🔞 badge ({1,3,4} ∩) and mature filter are
    /// client-side policy over these.
    content_descriptor_ids: Vec<u32>,
```

`steam_summary` fills them:

```rust
        tags: d.map(|d| d.tags.clone()).unwrap_or_default(),
        content_descriptor_ids: d.map(|d| d.content_descriptor_ids.clone()).unwrap_or_default(),
```

`CatalogGameView` gains:

```rust
    /// Provenance of `hidden` — "sync" rows get the "auto-hidden: adult content" label.
    hidden_source: Option<domain::HiddenSource>,
```

and the construction in `handle_catalog` copies `hidden_source: g.hidden_source,` (mirror
however the other `Game` fields are moved there).

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p admin-api`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/admin-api
git commit -S -m "admin-api: catalog rows carry tags, descriptor ids, hidden provenance (#71)"
```

---

### Task 10: web — API type mirrors + tags.ts policy/fit helpers

**Files:**
- Modify: `web/src/api.ts` (`GameView` :2-11, `SteamSummary` :41-52, `AdminGame` :54-68, and
  the `SteamAppDetail` mirror near :508)
- Create: `web/src/tags.ts`
- Test: `web/src/tags.test.ts` (new; vitest)

**Interfaces:**
- Consumes: wire shapes from Tasks 8-9.
- Produces (Tasks 11-14 import these exact names from `../tags` / `./tags`):
  `fitTags(tags: string[], budget?: number): string[]` · `displayTags(x: { tags?: string[]; genres?: string[] }): string[]`
  · `isMature(ids: number[] | undefined): boolean` · `DESCRIPTOR_LABELS: Record<number, string>`
  · `TAG_CHAR_BUDGET`. And the type updates: `GameView.tags?: string[]`,
  `SteamSummary.tags: string[]`, `SteamSummary.content_descriptor_ids: number[]`,
  `AdminGame.hidden_source: 'admin' | 'sync' | null`, `SteamAppDetail` mirror gains
  `tags?: string[]; content_descriptor_ids?: number[]; content_notes?: string | null;` —
  OPTIONAL, like the mirror's `screenshots?` (api.ts:518-523 documents why: an OLD lambda
  racing this bundle during deploy omits the keys entirely; a required type would be a lie
  tsc can't catch). Consumers read `detail.content_descriptor_ids ?? []`.

- [ ] **Step 1: Write the failing tests** (`web/src/tags.test.ts`)

```ts
import { describe, expect, it } from 'vitest';
import { DESCRIPTOR_LABELS, displayTags, fitTags, isMature } from './tags';

describe('fitTags', () => {
  it('fills by character budget in given order', () => {
    // 6+6+7=19 fits; "Resource Management" (19) would blow a 36 budget at position 4
    expect(
      fitTags(['Action', 'Sports', 'Shooter', 'Resource Management', 'Indie'], 36),
    ).toEqual(['Action', 'Sports', 'Shooter']);
  });
  it('always shows at least 3 tags even over budget', () => {
    const long = ['Interactive Fiction', 'Psychological Horror', 'Resource Management', 'Indie'];
    expect(fitTags(long, 10)).toEqual(long.slice(0, 3));
  });
  it('never shows more than 6', () => {
    expect(fitTags(['a', 'b', 'c', 'd', 'e', 'f', 'g', 'h'], 999)).toHaveLength(6);
  });
  it('handles fewer than 3 tags', () => {
    expect(fitTags(['Action'], 36)).toEqual(['Action']);
    expect(fitTags([], 36)).toEqual([]);
  });
});

describe('displayTags', () => {
  it('prefers tags over genres', () => {
    expect(displayTags({ tags: ['Roguelike'], genres: ['Action'] })).toEqual(['Roguelike']);
  });
  it('falls back to genres when tags are empty or absent', () => {
    expect(displayTags({ tags: [], genres: ['Action'] })).toEqual(['Action']);
    expect(displayTags({ genres: ['Action'] })).toEqual(['Action']);
  });
  it('returns empty when neither exists', () => {
    expect(displayTags({})).toEqual([]);
  });
});

describe('isMature', () => {
  it('true for the sexual-content family {1,3,4}', () => {
    expect(isMature([1])).toBe(true);
    expect(isMature([3])).toBe(true);
    expect(isMature([2, 4])).toBe(true);
  });
  it('false for violence-only, general-mature-only, none, undefined', () => {
    expect(isMature([2])).toBe(false);
    expect(isMature([5])).toBe(false); // Rollerdrome must not badge
    expect(isMature([])).toBe(false);
    expect(isMature(undefined)).toBe(false);
  });
});

describe('DESCRIPTOR_LABELS', () => {
  it('labels all five known ids', () => {
    for (const id of [1, 2, 3, 4, 5]) expect(DESCRIPTOR_LABELS[id]).toBeTruthy();
  });
});
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cd web && npx vitest run src/tags.test.ts`
Expected: FAIL — cannot resolve `./tags`.

- [ ] **Step 3: Implement**

`web/src/tags.ts`:

```ts
/** Steam community-tag display + content-descriptor policy (#71).
 * Descriptor semantics (verified live 2026-07-14): 1 some nudity/sexual ·
 * 2 frequent violence/gore · 3 adult-ONLY sexual · 4 gratuitous sexual · 5 general mature. */

/** Sexual-content family — drives the admin 🔞 badge and the mature filter.
 * Deliberately NOT 5 (Rollerdrome/Witcher carry it) and NOT 2 (violence). */
export const MATURE_DESCRIPTOR_IDS: readonly number[] = [1, 3, 4];

export const DESCRIPTOR_LABELS: Record<number, string> = {
  1: 'some nudity or sexual content',
  2: 'frequent violence or gore',
  3: 'adult-only sexual content',
  4: 'gratuitous sexual content',
  5: 'general mature content',
};

export function isMature(descriptorIds: number[] | undefined): boolean {
  return (descriptorIds ?? []).some((id) => MATURE_DESCRIPTOR_IDS.includes(id));
}

/** Steam's store page shows tags by width-fit, not by count (Rollerdrome fits 6 short
 * ones). Deterministic mirror: popularity-order prefix within a character budget —
 * short tags ⇒ more chips. Always at least 3 (when available), never more than 6. */
export const TAG_CHAR_BUDGET = 36;
const TAG_MIN = 3;
const TAG_MAX = 6;

export function fitTags(tags: string[], budget: number = TAG_CHAR_BUDGET): string[] {
  const out: string[] = [];
  let used = 0;
  for (const t of tags) {
    if (out.length >= TAG_MAX) break;
    if (out.length >= TAG_MIN && used + t.length > budget) break;
    out.push(t);
    used += t.length;
  }
  return out;
}

/** Community tags when present, publisher genres otherwise (#71: genres are the
 * degradation path for gated/delisted apps and pre-backfill cache blobs). */
export function displayTags(x: { tags?: string[]; genres?: string[] }): string[] {
  return x.tags?.length ? x.tags : (x.genres ?? []);
}
```

`web/src/api.ts` — `GameView` gains
`/** Top user tags (popularity order); absent when unknown. */ tags?: string[];`;
`SteamSummary` gains `tags: string[];` and `content_descriptor_ids: number[];`;
`AdminGame` gains `/** Who last set hidden — 'sync' rows are auto-hides. */ hidden_source: 'admin' | 'sync' | null;`;
the `SteamAppDetail` mirror gains (with the `screenshots?`-style deploy-window comment —
copy its rationale):

```ts
  /** Optional like screenshots: an OLD lambda racing this bundle during deploy omits
   * these keys. Read as `detail.tags ?? []` etc. */
  tags?: string[];
  content_descriptor_ids?: number[];
  content_notes?: string | null;
```

Then fix every fixture the compiler flags
(`npm run build` lists them — Catalog.test.tsx / ToolkitBar.test.tsx / catalogToolkit.test.ts
fixtures need the new `SteamSummary`/`AdminGame` fields; use empty arrays / null).

- [ ] **Step 4: Run tests to verify they pass**

Run: `cd web && npx vitest run src/tags.test.ts && npm run build`
Expected: PASS + clean tsc build.

- [ ] **Step 5: Commit**

```bash
git add web/src
git commit -S -m "web: tag fit/display helpers + descriptor policy + API type mirrors (#71)"
```

---

### Task 11: web — friend cards chip on tags

**Files:**
- Modify: `web/src/friend/GameGrid.tsx` (:60-97)
- Test: `web/src/friend/GameGrid.test.tsx`

**Interfaces:**
- Consumes: `fitTags`, `displayTags` from `../tags` (Task 10); `GameView.tags` (Task 10).

- [ ] **Step 1: Write the failing tests**

In `GameGrid.test.tsx` (existing fixture style):

```tsx
it('chips community tags over genres, in payload order', () => {
  // fixture game with tags: ['Roguelike', 'Sci-fi'], genres: ['Action']
  // render; assert 'Roguelike' and 'Sci-fi' chips present, 'Action' absent
});

it('falls back to genre chips when tags are absent', () => {
  // game with genres only → genre chips render (existing behavior preserved)
});

it('width-budget caps the chip row', () => {
  // game with 8 short tags → exactly 6 chips render
});
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cd web && npx vitest run src/friend/GameGrid.test.tsx`
Expected: FAIL — tags not rendered.

- [ ] **Step 3: Implement**

In `GameGrid.tsx`, import `{ displayTags, fitTags } from '../tags';` and replace the genres
line (:62):

```tsx
        // community tags replace genres on the chips (#71); genres remain the fallback
        // for gated/delisted/pre-backfill apps. width-budget fit mirrors steam's store box.
        const chipTags = fitTags(displayTags(game));
        const genres = chipTags.length ? chipTags : null;
```

(Keep the `genres` variable name so the chipsRow JSX below is untouched — it maps whatever
list it's given.)

- [ ] **Step 4: Run tests to verify they pass**

Run: `cd web && npx vitest run src/friend/GameGrid.test.tsx`
Expected: PASS (new + existing).

- [ ] **Step 5: Commit**

```bash
git add web/src/friend
git commit -S -m "web: friend cards chip community tags, genre fallback (#71)"
```

---

### Task 12: web — admin catalog 🔞 badge + auto-hidden label

**Files:**
- Modify: `web/src/admin/Catalog.tsx` (badge cluster :395-419, hidden toggle area :447-461)
- Test: `web/src/admin/Catalog.test.tsx`

**Interfaces:**
- Consumes: `isMature` from `../tags` (Task 10); `AdminGame.hidden_source`,
  `SteamSummary.content_descriptor_ids` (Task 10).

- [ ] **Step 1: Write the failing tests**

```tsx
it('shows 🔞 badge for sexual-content descriptors, not for violence/mature-only', () => {
  // row with steam.content_descriptor_ids [1,5] → badge present (accessible name below)
  // row with [5] → absent; row with [2] → absent; steam null → absent
});

it('labels sync auto-hides next to the hidden toggle', () => {
  // hidden: true + hidden_source: 'sync' → text 'auto-hidden: adult content' present
  // hidden: true + hidden_source: 'admin' → absent; hidden: false + 'sync' → absent
});
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cd web && npx vitest run src/admin/Catalog.test.tsx`
Expected: FAIL.

- [ ] **Step 3: Implement**

Import `{ isMature } from '../tags';`. In the badge cluster (after the giftable chip, before
`owned_by_ben`):

```tsx
                {/* 🔞 — sexual-content descriptor family {1,3,4}; violence-only (2) and
                    general-mature-only (5) deliberately don't badge (#71) */}
                {game.steam !== null && isMature(game.steam.content_descriptor_ids) && (
                  <span
                    role="img"
                    aria-label="mature content"
                    title="steam content descriptors: sexual content"
                    className="rounded bg-red-950 px-2 py-0.5 text-xs text-red-200"
                  >
                    🔞
                  </span>
                )}
```

Next to the hidden-toggle label (inside the same flex row, after the `</label>`):

```tsx
                {/* not-silent auto-hide (#71): sync-hidden rows say so */}
                {game.hidden && game.hidden_source === 'sync' && (
                  <span
                    className="text-xs text-dust"
                    title="hidden automatically by sync — adult content descriptors; toggling makes your choice permanent"
                  >
                    auto-hidden: adult content
                  </span>
                )}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cd web && npx vitest run src/admin/Catalog.test.tsx`
Expected: PASS (new + existing — fixtures already updated in Task 10).

- [ ] **Step 5: Commit**

```bash
git add web/src/admin
git commit -S -m "web: admin catalog gets the mature badge + auto-hidden labeling (#71)"
```

---

### Task 13: web — toolkit chips on displayTags + mature filter

**Files:**
- Modify: `web/src/admin/catalogToolkit.ts` (`ToolkitState` :12-26, `collectTagOptions`
  :49-56, `applyToolkit` :66-100)
- Modify: `web/src/admin/ToolkitBar.tsx` (selects block :89-135)
- Modify: `web/src/admin/Catalog.tsx` (URL param wiring :75-94)
- Test: `web/src/admin/catalogToolkit.test.ts`, `web/src/admin/ToolkitBar.test.tsx`

**Interfaces:**
- Consumes: `displayTags`, `isMature` from `../tags` (Task 10).
- Produces: `ToolkitState.mature: MatureFilter` with
  `export type MatureFilter = 'all' | 'hide' | 'only';` — `IDLE_TOOLKIT.mature = 'all'`; URL
  param `mature` (`Catalog.tsx` follows the existing `rating` param pattern exactly).

- [ ] **Step 1: Write the failing tests** (`catalogToolkit.test.ts`)

```ts
it('tag options and tag filter run on displayTags (tags, genre fallback)', () => {
  // game A: steam.tags ['Roguelike'], genres ['Action'] → option 'Roguelike', NOT 'Action'
  // game B: steam.tags [], genres ['Action'] → option 'Action'
  // filtering by 'Roguelike' keeps A only; by 'Action' keeps B only
});

it("mature 'hide' drops flagged games, keeps unmapped", () => {
  // flagged = content_descriptor_ids ∩ {1,3,4}; steam:null rows KEPT (not mature),
  // no excludedNoData increment for them under 'hide'
});

it("mature 'only' keeps flagged games, counts unmapped as excludedNoData", () => {});
```

Plus a `ToolkitBar.test.tsx` case: the mature `<select>` renders with options
all/hide/only and calls the state setter (mirror the rating select's existing test).

Plus a `Catalog.test.tsx` case: extend the existing URL-params test (`restores toolkit
state from URL params`, Catalog.test.tsx:596) so `?mature=hide` restores
`state.mature === 'hide'` — the spec's mature-filter URL round-trip requirement.

- [ ] **Step 2: Run tests to verify they fail**

Run: `cd web && npx vitest run src/admin/catalogToolkit.test.ts`
Expected: FAIL — `mature` missing from `ToolkitState`.

- [ ] **Step 3: Implement**

`catalogToolkit.ts`: import `{ displayTags, isMature } from '../tags';`. Add:

```ts
export type MatureFilter = 'all' | 'hide' | 'only';
```

`ToolkitState` gains `mature: MatureFilter;`, `IDLE_TOOLKIT` gains `mature: 'all',`.
`collectTagOptions` body swaps `g.steam?.genres ?? []` for `displayTags(g.steam ?? {})`.
In `applyToolkit`'s filter, the tags block swaps `g.steam?.genres` for
`displayTags(g.steam ?? {})` (same empty-→excludedNoData semantics), and after the rating
block:

```ts
    if (state.mature !== 'all') {
      const flagged = isMature(g.steam?.content_descriptor_ids);
      if (state.mature === 'hide' && flagged) return false;
      if (state.mature === 'only' && !flagged) {
        // unmapped rows aren't provably mature — under 'only' they're no-data exclusions
        if (g.steam === null) excludedNoData++;
        return false;
      }
    }
```

`ToolkitBar.tsx`: add a fourth `<select>` cloned from the rating one:

```tsx
        <label className="flex items-center gap-1.5 text-xs text-dust">
          mature
          <select
            value={state.mature}
            onChange={(e) => onChange({ ...state, mature: e.target.value as MatureFilter })}
            className={controlClass /* the file's shared select class, ToolkitBar.tsx:10 */}
          >
            <option value="all">show all</option>
            <option value="hide">hide 🔞</option>
            <option value="only">only 🔞</option>
          </select>
        </label>
```

(Match the surrounding selects' exact markup — copy the rating select including its
`aria-label` and label classes, and adjust.)

`Catalog.tsx`: wire the `mature` URL param exactly like `rating` (the `keyOf`/valid-keys
pattern at Catalog.tsx:29-39 + the param read at :80; write when ≠ 'all'. `setToolkit`
rebuilds URLSearchParams from scratch, so there is no separate clear-all site to touch).

One semantics note for the tag-filter swap: `displayTags(g.steam ?? {})` never returns
undefined, so the existing `!genres || genres.length === 0` check becomes
`tags.length === 0` — KEEP the `excludedNoData++` + `return false` behavior for that empty
case; only the data source changes.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cd web && npx vitest run src/admin`
Expected: PASS (new + all existing toolkit/catalog tests).

- [ ] **Step 5: Commit**

```bash
git add web/src/admin
git commit -S -m "web: toolkit filters on community tags + mature show/hide/only (#71)"
```

---

### Task 14: web — detail modal tags + descriptor note (admin)

**Files:**
- Modify: `web/src/GameDetailModal.tsx` (genres block :443-458; admin-mount section)
- Test: `web/src/GameDetailModal.test.tsx`

**Interfaces:**
- Consumes: `displayTags`, `DESCRIPTOR_LABELS` from `./tags` (Task 10); `SteamAppDetail`
  mirror fields (Task 10). Check the file's actual import depth (it lives in `web/src/`, so
  `./tags`).

- [ ] **Step 1: Write the failing tests**

```tsx
it('modal chips prefer tags over genres', () => {
  // detail with tags ['Roguelike'] + genres ['Action'] → 'Roguelike' chip, no 'Action'
});

it('admin mount shows descriptor labels + steam note', () => {
  // mount="admin", detail.content_descriptor_ids [1,5], content_notes 'Some nudity.'
  // → text matches /some nudity or sexual content/ and /Some nudity./
});

it('friend mount never shows descriptor info', () => {
  // mount="friend" (or whatever the non-admin mount value actually is in this file),
  // same detail → no descriptor text
});
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cd web && npx vitest run src/GameDetailModal.test.tsx`
Expected: FAIL.

- [ ] **Step 3: Implement**

Replace `detail.genres` in the chips block (:443) with a `const chips = displayTags(detail);`
just above the JSX and map over `chips` (condition `chips.length > 0`). After the
`short_description` paragraph, add:

```tsx
                            {mount === 'admin' && (detail.content_descriptor_ids ?? []).length > 0 && (
                              <p className="text-xs text-dust">
                                content:{' '}
                                {(detail.content_descriptor_ids ?? [])
                                  .map((id) => DESCRIPTOR_LABELS[id] ?? `descriptor ${id}`)
                                  .join(' · ')}
                                {detail.content_notes ? ` — ${detail.content_notes}` : ''}
                              </p>
                            )}
```

(The `?? []` guards are load-bearing: the mirror fields are optional for the deploy window —
an old-lambda response omits them and an unguarded `.length` throws on every modal open.)

(Verify the mount prop's actual name/values in the file — `grep -n "mount" GameDetailModal.tsx`
— and use those.)

- [ ] **Step 4: Run tests to verify they pass**

Run: `cd web && npx vitest run src/GameDetailModal.test.tsx`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add web/src
git commit -S -m "web: detail modal chips tags; admin sees descriptor labels + steam note (#71)"
```

---

### Task 15: full gates + docs

**Files:**
- Create (already written, commit them): `docs/superpowers/specs/2026-07-14-steam-tags-descriptors-design.md`,
  `docs/superpowers/plans/2026-07-14-steam-tags-descriptors.md`

- [ ] **Step 1: Run the full gate battery**

```bash
cargo fmt --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace
cd web && npm run build && npx vitest run && cd ..
```

Expected: all green. Fix anything that isn't (fmt/clippy nits from the new code) and amend
into sensible commits.

- [ ] **Step 2: Commit the docs**

```bash
git add docs/superpowers/specs/2026-07-14-steam-tags-descriptors-design.md docs/superpowers/plans/2026-07-14-steam-tags-descriptors.md
git commit -S -m "docs: spec + plan for steam tags/descriptors/auto-hide (#71)"
```

---

### Task 16: deploy + database resync (operator runbook — post-merge, kitten runs it)

Not subagent work — this is the live-prod sequence after the PR merges to main, following
the standing deploy runbook (state/decisions.md:1038 lineage). Recorded here because Ben
explicitly required the resync in the plan.

- [ ] **Step 1:** Pull artifacts (3 lambda zips + web-dist) from the GREEN main CI run of the
  merge commit.
- [ ] **Step 2:** Terraform: recreate tfvars (boundary ARN, secrets from `~/.secrets`, admin
  hash from SSM verbatim), `terraform plan` read line-by-line — expected: 3 lambda updates +
  the two KNOWN no-op churn lines only. Apply. Verify all lambdas' live CodeSha256 == zip
  sha256. Shred tfvars/plan.
- [ ] **Step 3:** Web: `aws s3 sync web/dist s3://<bucket> --delete --dryrun` (deletes = old
  hashed js/css pair ONLY), then real sync + CloudFront invalidation; curl the live bundle
  hash.
- [ ] **Step 4 (THE RESYNC):** outside the 09:00Z cron window, with `~/.secrets` steam key +
  deploy-role AWS creds:

```bash
cd /path/to/worktree
TABLE_NAME=<prod table> AWS_PROFILE=kitten-deploy \
  cargo run -p fulfillment --features backfill --bin backfill_details
```

  Expected: summary line with `fetched≈<mapped-app count>`, `auto_hidden ≥ 0`; rerun on a
  429 abort (resume via skip-fresh window). Check the bin's actual env contract
  (`crates/fulfillment/src/bin/backfill_details.rs`) before running — it may take the steam
  key via env/SSM; follow what it reads.
- [ ] **Step 5:** Verify live (playwright): tags on friend cards, 🔞 badges + mature filter
  in admin, an auto-hidden row labeled, Puss! still hidden (Ben-hidden, untouched — its
  `hidden_source` stays null until Ben toggles).

---

## Review round (2026-07-14, adversarial plan review — integrated)

Cold-reviewed by a context-free agent against the repo; verdict "ready after fixes"; all 4
blockers + 6 majors integrated: fixture names corrected to reality (`fresh_game()`,
`game(n, listable)`, `steam_client_at`, `controlClass`), the un-passable Available-branch
merge test fixed (fresh must differ on a refreshed field), raw DDB helpers given as free
fns with the file's real idiom, backfill bin's `println!` extension added (it does NOT
Debug-print), Task 8's zero-match red command replaced, TS mirror fields made optional per
the file's own deploy-window rule (+ `?? []` guards in the modal), spec deviations
(GetItems unpaced / tag-batch-failure continues) promoted to signed-off decisions in both
docs, and three spec-required tests added (mature URL round-trip, mid-claim Contested,
one-way never-unhides).

## Self-review (done at write time)

- Spec coverage: tags fetch/store (T1-3,6), provenance + never-fights-Ben (T4-5), enrich +
  backfill/resync (T6-7), API views (T8-9), friend chips (T10-11), badge/label (T12), mature
  filter (T13), modal note (T14), gates/docs (T15), deploy+resync (T16). Descriptor policy
  constants: server {3,4} (domain), client {1,3,4} (tags.ts) — both documented with the
  corrected id semantics.
- Type consistency: `tags_for_app` (T6) is consumed verbatim in T7; `AutoHideWrite` variants
  match between T5 def and T6/7 match arms; `HiddenSource` serde `"sync"`/`"admin"` strings
  match the DDB condition value `:admin = "admin"` and the TS union.
- Known judgment calls the executor may hit: exact fixture-helper names in each test file
  (greps provided), the modal's mount prop values, ToolkitBar's class constants — all are
  "read the file, mirror the local idiom" moments, flagged inline.
