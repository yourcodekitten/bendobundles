# Media Carousel (trailer + screenshots) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** The game detail modal's media header becomes a carousel — trailer first, then Steam
screenshots — with the screenshots plumbed from Steam's appdetails payload through the dynamo
cache blob and both detail endpoints (issue #61).

**Architecture:** Four layers, back to front: (1) `steam-client` parses the appdetails
`screenshots[]` array into a new `SteamAppDetail.screenshots: Vec<Screenshot>` (capped 10,
`#[serde(default)]` for old-blob compat); (2) persistence + both detail endpoints need **zero
code** — the blob is full-struct JSON and both endpoints serialize `cache.detail` raw; (3) the
web modal's media header moves into a new `MediaHeader` component owning the carousel + all
video/HLS state; (4) the run-once backfill fn/bin is renamed generic (it already refetches
every app through the current parse, so rerunning it post-deploy backfills screenshots).

**Tech Stack:** Rust (axum lambdas, serde, wiremock, moto for dynamo tests), React 19 + TS +
Tailwind 4 (tokens in `web/src/index.css` `@theme`), vitest + happy-dom + testing-library.

**Spec:** `docs/superpowers/specs/2026-07-08-media-carousel-design.md` (read it for the why;
this plan is the how).

## Global Constraints

- Worktree: `~/bendobundles-wt/media-carousel`, branch `kitten/media-carousel`. All commands
  run from the worktree root.
- Rust env per shell: `export PATH="$HOME/.cargo/bin:$PATH"`.
- Web env per shell: `export PATH="$HOME/.local/node22/bin:$PATH"` (box node is 18; CI gate is
  node22 `npm run build` = `tsc -b` + vite).
- Rust tests need moto on `localhost:8000`. If `Corrupt("already exists")` appears, moto has
  accumulated state — restart it: `pgrep -x moto_server` then kill that exact pid (NEVER
  `pkill -f`, it self-matches), then `setsid nohup moto_server -p 8000 >/tmp/moto.log 2>&1 &`.
- Gates before ANY push: `cargo fmt --check` AND
  `cargo clippy --workspace --all-targets --all-features -- -D warnings` AND
  `cargo test --workspace` AND (in `web/`) `npm run build` AND `npx vitest run`.
- Commits: GPG-signed (`git commit -S`), author `code kitten <yourcodekitten@gmail.com>`
  (already the worktree's `git config user.email` — verify once, first commit only).
- UI copy is lowercase ("previous", "next", "play trailer", counter `1 / 6`). No hearts (One
  Heart Rule). No burgundy on carousel chrome (`give` is claim-button-only). No new shadows
  (Ceremony Rule) — the active frame marker is the pixel bezel `ring-1 ring-pixel`.
- No new npm or cargo dependencies.
- Never `git push --force`.

---

### Task 1: steam-client — `Screenshot` type + wire parse (TDD)

**Files:**
- Modify: `crates/steam-client/src/lib.rs` (struct ~line 31-46, wire struct ~line 175-191,
  parse ~line 419-463)
- Modify: `crates/steam-client/tests/fixtures/appdetails-413150-trimmed.json`
- Test: `crates/steam-client/tests/client_test.rs` (storefront section, after
  `app_details_tolerates_mistyped_category_ids` ~line 609)

**Interfaces:**
- Produces: `steam_client::Screenshot { pub thumbnail: String, pub full: String }` (derives
  `Debug, Clone, PartialEq, Serialize, Deserialize`); `SteamAppDetail.screenshots:
  Vec<Screenshot>` with `#[serde(default)]`; `const SCREENSHOT_CAP: usize = 10` (private).
  Every later task relies on these exact names.

- [ ] **Step 1: Add real captured screenshots to the fixture**

The spec REQUIRES the fixture be a real captured response (nothing in the repo has ever
carried a `screenshots` key — a hand-typed shape could ship the feature empty with green
tests). A capture already exists at
`/tmp/claude-1003/-home-code-kitten-code-kitten/bb5ac20e-c7ae-4a96-a240-31292ef452ec/scratchpad/appdetails-413150-full.json`;
if missing, re-capture ONCE (be gentle, this endpoint is throttled ~200 req/5min):

```bash
curl -s "https://store.steampowered.com/api/appdetails?appids=413150&cc=us&l=english" \
  -o /tmp/appdetails-413150-full.json
```

Then inject the real array into the trimmed fixture (adjust the capture path if you re-curled):

```bash
python3 - <<'EOF'
import json
cap = json.load(open('/tmp/claude-1003/-home-code-kitten-code-kitten/bb5ac20e-c7ae-4a96-a240-31292ef452ec/scratchpad/appdetails-413150-full.json'))
fix_path = 'crates/steam-client/tests/fixtures/appdetails-413150-trimmed.json'
fix = json.load(open(fix_path))
shots = cap['413150']['data']['screenshots']
assert len(shots) > 10, f"need >10 to exercise the cap, got {len(shots)}"
fix['413150']['data']['screenshots'] = shots
json.dump(fix, open(fix_path, 'w'), indent=1)
print('injected', len(shots), 'screenshots')
EOF
```

Expected: `injected 16 screenshots`. Verify the first entry's fields are named exactly
`path_thumbnail` and `path_full` (they are, from the live capture):
`python3 -c "import json; d=json.load(open('crates/steam-client/tests/fixtures/appdetails-413150-trimmed.json')); print(sorted(d['413150']['data']['screenshots'][0].keys()))"`
→ `['id', 'path_full', 'path_thumbnail']`.

- [ ] **Step 2: Write the failing tests**

Append to the storefront section of `crates/steam-client/tests/client_test.rs`:

```rust
#[tokio::test]
async fn app_details_parses_screenshots_thumb_and_full_capped_at_10() {
    // Fixture is a REAL captured appdetails response (16 screenshots) — the wire field
    // names (path_thumbnail/path_full) are pinned by capture, not by hand.
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
        steam_client::AppDetails::Delisted => panic!("expected Found, got Delisted"),
    };
    assert_eq!(
        detail.screenshots.len(),
        10,
        "16 in the fixture must cap at 10"
    );
    let first = &detail.screenshots[0];
    assert!(
        first.thumbnail.contains("ss_b887651a93b0525739049eb4194f633de2df75be.600x338"),
        "first thumbnail must be the capture's path_thumbnail; got {}",
        first.thumbnail
    );
    assert!(
        first.full.contains("ss_b887651a93b0525739049eb4194f633de2df75be.1920x1080"),
        "first full must be the capture's path_full; got {}",
        first.full
    );
}

#[tokio::test]
async fn app_details_missing_screenshots_key_is_empty() {
    // Pre-existing blobs / apps without screenshots: absent key must parse to [], never fail.
    let body = r#"{"413150":{"success":true,"data":{"name":"No Shots"}}}"#;
    let server = wiremock::MockServer::start().await;
    wiremock::Mock::given(wiremock::matchers::method("GET"))
        .and(wiremock::matchers::path("/api/appdetails"))
        .and(wiremock::matchers::query_param("appids", "413150"))
        .respond_with(wiremock::ResponseTemplate::new(200).set_body_string(body))
        .mount(&server)
        .await;
    let result = test_client(&server).get_app_details(413150).await.unwrap();
    let detail = match result {
        steam_client::AppDetails::Found(d) => d,
        steam_client::AppDetails::Delisted => panic!("expected Found, got Delisted"),
    };
    assert_eq!(detail.screenshots, vec![]);
}

#[tokio::test]
async fn app_details_screenshot_missing_either_tier_is_dropped() {
    // Both URLs or nothing — asymmetric fallbacks would create two sources of truth.
    let body = r#"{"413150":{"success":true,"data":{
        "name":"Partial Shots",
        "screenshots":[
            {"id":0,"path_thumbnail":"https://img.example/a.600x338.jpg","path_full":"https://img.example/a.1920x1080.jpg"},
            {"id":1,"path_thumbnail":"https://img.example/b.600x338.jpg"},
            {"id":2,"path_full":"https://img.example/c.1920x1080.jpg"}
        ]
    }}}"#;
    let server = wiremock::MockServer::start().await;
    wiremock::Mock::given(wiremock::matchers::method("GET"))
        .and(wiremock::matchers::path("/api/appdetails"))
        .and(wiremock::matchers::query_param("appids", "413150"))
        .respond_with(wiremock::ResponseTemplate::new(200).set_body_string(body))
        .mount(&server)
        .await;
    let result = test_client(&server).get_app_details(413150).await.unwrap();
    let detail = match result {
        steam_client::AppDetails::Found(d) => d,
        steam_client::AppDetails::Delisted => panic!("expected Found, got Delisted"),
    };
    assert_eq!(
        detail.screenshots,
        vec![steam_client::Screenshot {
            thumbnail: "https://img.example/a.600x338.jpg".into(),
            full: "https://img.example/a.1920x1080.jpg".into(),
        }],
        "entries missing either tier must drop, not fail or half-fill"
    );
}
```

- [ ] **Step 3: Run the tests, verify they fail to COMPILE (no `Screenshot` type)**

```bash
export PATH="$HOME/.cargo/bin:$PATH"
cargo test -p steam-client --test client_test app_details_parses_screenshots 2>&1 | tail -5
```
Expected: compile error — `no ` `Screenshot` ` in ` `steam_client` / no field `screenshots`.

- [ ] **Step 4: Implement**

In `crates/steam-client/src/lib.rs`:

(a) Domain type, immediately after the `SteamAppDetail` struct (~line 46):

```rust
/// One store screenshot: Steam's `path_thumbnail` (600x338) + `path_full` (1920x1080) tiers.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Screenshot {
    pub thumbnail: String,
    pub full: String,
}
```

(b) Field on `SteamAppDetail`, after `video_thumbnail` (line 45):

```rust
    /// First 10 store screenshots (thumbnail + full tiers). Empty when the app has none
    /// or the cached blob predates the field (issue #61).
    #[serde(default)]
    pub screenshots: Vec<Screenshot>,
```

(c) Wire struct, next to `MovieWire` (~line 234), plus the field on `AppDetailDataWire`
(after `movies`, ~line 190):

```rust
    #[serde(default)]
    screenshots: Vec<ScreenshotWire>,
```

```rust
/// Both tiers or the entry is dropped — a screenshot with only one URL would force the
/// UI to guess. Options (not defaults) so a missing key drops the entry, not the parse.
#[derive(Deserialize)]
struct ScreenshotWire {
    path_thumbnail: Option<String>,
    path_full: Option<String>,
}
```

(d) Cap const, next to `ALLOWED_CATEGORY_IDS` (~line 227):

```rust
/// Screenshots kept per app ("a handful", issue #61): bounds the cache blob; a cap change
/// reaches existing blobs only via a backfill rerun or the 30-day refresh.
const SCREENSHOT_CAP: usize = 10;
```

(e) Parse, in `get_app_details` right before `let first_movie = …` (~line 450):

```rust
        let screenshots: Vec<Screenshot> = data
            .screenshots
            .into_iter()
            .filter_map(|s| match (s.path_thumbnail, s.path_full) {
                (Some(thumbnail), Some(full)) => Some(Screenshot { thumbnail, full }),
                _ => None,
            })
            .take(SCREENSHOT_CAP)
            .collect();
```

and `screenshots,` in the `SteamAppDetail { … }` literal (after `video_thumbnail`).

- [ ] **Step 5: Sweep every `SteamAppDetail { … }` literal in the workspace**

Adding a struct field breaks every literal initializer. Find them all:

```bash
grep -rn "SteamAppDetail {" crates/ --include="*.rs"
```

Add `screenshots: vec![],` to each **test/helper** literal. The complete set (verified):
- `crates/dynamo/tests/store_test.rs:1760` (inside `steam_app_cache_full`)
- `crates/fulfillment/tests/handler_test.rs:4272` (inside `fresh_cache`)
- `crates/public-api/tests/api_test.rs:1256` (inside `test_steam_cache`)
- `crates/admin-api/tests/api_test.rs:1850` (inline seed)

Do NOT touch the one real construction site (steam-client's parse — already done in
Step 4). The grep + the `--all-targets` check below are the backstop if a new literal has
appeared since this plan was written:

```bash
cargo check --workspace --all-targets 2>&1 | tail -3
```
Expected: clean.

- [ ] **Step 6: Run the new tests, verify they pass**

```bash
cargo test -p steam-client --test client_test 2>&1 | tail -5
```
Expected: all client_test tests PASS (including the 3 new ones).

- [ ] **Step 7: Commit**

```bash
git add crates/steam-client crates/fulfillment/tests crates/dynamo/tests
git add -A crates/  # catches any other literal sweeps
git commit -S -m "steam-client: parse appdetails screenshots (thumb+full pairs, cap 10)

Screenshot { thumbnail, full } rides SteamAppDetail with #[serde(default)] so
every pre-#61 cached blob deserializes to []. Both-tiers-or-dropped wire
leniency; fixture is a real captured 413150 response (16 shots) so the
path_thumbnail/path_full field names are pinned by capture, not by hand (#61)"
```

---

### Task 2: dynamo — pre-#61 blob compat pin (TDD, no moto needed)

**Files:**
- Test: `crates/dynamo/tests/store_test.rs` (append near the other steam_app tests ~line 743)

**Interfaces:**
- Consumes: `steam_client::Screenshot`, `SteamAppDetail.screenshots` (Task 1).
- Produces: nothing new — this is a regression pin on the serde compat contract.

- [ ] **Step 1: Write the failing-or-passing pin test**

This test pins the load-bearing compat claim: a blob written BEFORE the field existed (no
`screenshots` key anywhere in the JSON) must deserialize with `screenshots: []`. It is a
plain `#[test]` — no moto, no tokio:

```rust
/// Pre-#61 blobs have no `screenshots` key in the detail JSON. `#[serde(default)]` must
/// fill `[]` — if this ever fails, every cached app in prod stops deserializing.
#[test]
fn steam_app_cache_pre_screenshots_blob_deserializes() {
    let body = r#"{"app_id":413150,"detail":{"app_id":413150,"name":"Stardew Valley","developers":["ConcernedApe"],"publishers":["ConcernedApe"],"genres":["Indie"],"release_date":"Feb 26, 2016","short_description":"farm.","header_image":null,"video_hls_url":null,"video_thumbnail":null},"overall":null,"recent":null,"fetched_at":100,"reviews_fetched_at":100}"#;
    let cache: dynamo::SteamAppCache =
        serde_json::from_str(body).expect("pre-screenshots blob must still deserialize");
    assert_eq!(cache.detail.expect("detail present").screenshots, vec![]);
}
```

`serde_json` is already in the dynamo crate's `[dependencies]` (Cargo.toml line 11) and is
therefore available to integration tests — do NOT add a redundant dev-dependency.

- [ ] **Step 2: Run it, verify it passes (the implementation landed in Task 1)**

```bash
cargo test -p dynamo --test store_test steam_app_cache_pre_screenshots 2>&1 | tail -3
```
Expected: `1 passed`. (This is a pin, not red-green — the serde attribute already exists;
the test's job is to fail loudly if anyone ever removes it.)

- [ ] **Step 3: Commit**

```bash
git add crates/dynamo
git commit -S -m "dynamo: pin pre-#61 blob compat — missing screenshots key deserializes to []"
```

---

### Task 3: fulfillment — backfill rename + screenshots ride the rebuild (TDD)

**Files:**
- Modify: `crates/fulfillment/src/lib.rs` (fn `backfill_steam_genres` ~line 2103, doc ~2094;
  `BackfillSummary` doc ~2079 mentions the fn name)
- Rename: `crates/fulfillment/src/bin/backfill_genres.rs` → `crates/fulfillment/src/bin/backfill_details.rs`
- Modify: `crates/fulfillment/Cargo.toml` (`[[bin]]` block ~line 37)
- Modify: `crates/fulfillment/tests/handler_test.rs` (import line 10, calls ~4662/4698/4729/…,
  `appdetails_found_body` ~line 4188)
- Modify: any other reference — sweep in Step 4.

**Interfaces:**
- Consumes: `steam_client::Screenshot` (Task 1).
- Produces: `pub async fn backfill_steam_details(store: &dynamo::Store, steam:
  &steam_client::SteamClient, pace: std::time::Duration, skip_fresh_secs: i64) ->
  Result<BackfillSummary, dynamo::StoreError>` (same signature as the old
  `backfill_steam_genres`, renamed); bin target `backfill_details`
  (`required-features = ["backfill"]` unchanged). The ops runbook (deploy step) invokes:
  `TABLE_NAME=<table> cargo run -p fulfillment --features backfill --bin backfill_details`.

- [ ] **Step 1: Extend the shared mock body + write the failing assertion**

In `crates/fulfillment/tests/handler_test.rs`, add a `screenshots` entry to
`appdetails_found_body` (~line 4188). The body is a `serde_json::json!` macro literal and
`"movies"` is currently the LAST key in `"data"` (its `}]` closes at ~line 4207) — so the
existing `}]` **gains a trailing comma**, then the new key follows:

```rust
                "movies": [{
                    "id": 1, "name": "Trailer",
                    "thumbnail": "https://img.example/thumb.jpg",
                    "hls_h264": "https://vid.example/master.m3u8"
                }],
                "screenshots": [{
                    "id": 0,
                    "path_thumbnail": "https://img.example/ss.600x338.jpg",
                    "path_full": "https://img.example/ss.1920x1080.jpg"
                }]
```

Then in `backfill_rewrites_dirty_detail_and_preserves_reviews` (~line 4634), after the
genres assertion, add:

```rust
    assert_eq!(
        detail.screenshots,
        vec![steam_client::Screenshot {
            thumbnail: "https://img.example/ss.600x338.jpg".into(),
            full: "https://img.example/ss.1920x1080.jpg".into(),
        }],
        "backfill must persist screenshots through the new parse (issue #61)"
    );
```

(If `steam_client` isn't imported in handler_test.rs, reference it as a full path — the
crate is a dependency of fulfillment, so `steam_client::Screenshot` resolves.)

- [ ] **Step 2: Run it, verify the new assertion passes**

Moto must be running on :8000 (see Global Constraints).

```bash
cargo test -p fulfillment --test handler_test backfill_rewrites 2>&1 | tail -3
```
Expected: PASS — Task 1's parse + the untouched put/get path make screenshots ride
automatically. (If it fails, the plumbing claim is broken — STOP and investigate; do not
proceed to the rename.)

- [ ] **Step 3: The rename**

```bash
git mv crates/fulfillment/src/bin/backfill_genres.rs crates/fulfillment/src/bin/backfill_details.rs
```

Then, in this exact order:
1. `crates/fulfillment/src/lib.rs`: rename `pub async fn backfill_steam_genres` →
   `backfill_steam_details`; update its doc comment header to
   `/// Run-once STEAMAPP# rebuild (issues #57, #61): refetch appdetails for EVERY catalog
   appid through the current parse and rewrite each item, preserving the reviews half`
   (keep the rest); update the `BackfillSummary` doc's `[backfill_steam_genres]` link and
   the `backfill_genres` bin mention (now `backfill_details`).
2. `crates/fulfillment/Cargo.toml` `[[bin]]`: `name = "backfill_details"` (path comment /
   feature comment mentions of the old name too).
3. `crates/fulfillment/src/bin/backfill_details.rs`: update the module doc — issue list
   `(issues #57, #61)`, invocation example
   `cargo run -p fulfillment --features backfill --bin backfill_details`, and the call site
   `fulfillment::backfill_steam_details(…)`.
4. `crates/fulfillment/tests/handler_test.rs`: import (line 10) and every call site
   (`grep -n backfill_steam_genres crates/fulfillment/tests/handler_test.rs`).

- [ ] **Step 4: Sweep every remaining old-name reference**

```bash
grep -rn "backfill_genres\|backfill_steam_genres" --include="*.rs" --include="*.toml" --include="*.md" . | grep -v docs/superpowers/plans | grep -v target/
```

Expected survivors: only historical docs (`docs/superpowers/specs/2026-07-07-*`,
`docs/superpowers/plans/2026-07-07-*` — leave history alone) and the new spec's narrative
mentions (fine). If `terraform/README.md` or any runbook carries the old bin name, update it
to `backfill_details`.

- [ ] **Step 5: Full fulfillment tests green**

```bash
cargo test -p fulfillment 2>&1 | tail -3
cargo build -p fulfillment --features backfill --bin backfill_details 2>&1 | tail -2
```
Expected: all PASS; bin builds.

- [ ] **Step 6: Commit**

```bash
git add -A crates/fulfillment terraform/ 2>/dev/null; git add -A crates/fulfillment
git commit -S -m "fulfillment: backfill_steam_genres → backfill_steam_details (bin backfill_details)

The rebuild was always generic — full appdetails refetch through the current
parse — so screenshots (#61) ride it with zero logic change; the name now says
so. Mock storefront body grows a screenshots entry and the rewrite test pins
that a backfilled item persists them (#57, #61)"
```

---

### Task 4: web — TS mirror types

**Files:**
- Modify: `web/src/api.ts` (Steam detail types section, ~line 450-463)

**Interfaces:**
- Produces: `export type Screenshot = { thumbnail: string; full: string }`;
  `SteamAppDetail.screenshots?: Screenshot[]` (**optional** — a new web bundle can race an
  old lambda during deploy; readers use `detail.screenshots ?? []`). Task 5 imports
  `Screenshot` from `./api`.

- [ ] **Step 1: Add the types**

In `web/src/api.ts`, immediately before `export type SteamAppDetail` (~line 452):

```typescript
/** One store screenshot — mirrors Rust steam_client::Screenshot. */
export type Screenshot = {
  thumbnail: string;
  full: string;
};
```

and on `SteamAppDetail`, after `video_thumbnail` (line 462):

```typescript
  /**
   * Optional, not `Screenshot[] | null`: an OLD lambda racing this bundle during deploy
   * omits the key entirely. Read as `detail.screenshots ?? []`.
   */
  screenshots?: Screenshot[];
```

- [ ] **Step 2: Verify it compiles**

```bash
export PATH="$HOME/.local/node22/bin:$PATH"
cd web && npm run build 2>&1 | tail -3 && cd ..
```
Expected: clean build.

- [ ] **Step 3: Commit**

```bash
git add web/src/api.ts
git commit -S -m "web: mirror Screenshot + optional SteamAppDetail.screenshots in api.ts (#61)"
```

---

### Task 5: web — `MediaHeader` carousel component (TDD)

**Files:**
- Create: `web/src/MediaHeader.tsx`
- Modify: `web/src/GameDetailModal.tsx` (remove video/HLS state + the media header block,
  render `<MediaHeader …>`; lines 1-5 imports, 91-96 state/refs, 140-192 effects+handler,
  274-317 render)
- Test: `web/src/GameDetailModal.test.tsx`

**Interfaces:**
- Consumes: `Screenshot`, `SteamAppDetail` from `./api` (Task 4); `titleColorClass` from
  `./titleColor`.
- Produces: `export function MediaHeader(props: { title: string; artworkUrl: string | null;
  detail: SteamAppDetail | null }): JSX element`. The modal is its only consumer. ALL
  video/HLS state (`videoPlaying`, `hlsFailed`, `videoRef`, `hlsRef`, `handlePlay`, the HLS
  unmount cleanup) moves INTO this component — the modal keeps none of it.
  **Invariant: when `hlsFailed` is true the trailer `<div>` must not render AT ALL** (not
  render-empty) — slideCount and the slide DOM both drop by one, which is what makes the
  fatal-HLS test's `1 / 2` counter and the inert-count test both hold.

- [ ] **Step 1: Write the failing tests**

In `web/src/GameDetailModal.test.tsx`:

(a) Add a screenshots-bearing fixture next to `steamDetailFixture` (~line 36). Do NOT add
screenshots to `steamDetailFixture` itself — the old fixture doubles as the pre-backfill
compat pin, and existing tests (e.g. the hls-fatal artwork fallback) depend on its shape:

```typescript
const screenshotsFixture = [
  {
    thumbnail: 'https://example.com/ss1.600x338.jpg',
    full: 'https://example.com/ss1.1920x1080.jpg',
  },
  {
    thumbnail: 'https://example.com/ss2.600x338.jpg',
    full: 'https://example.com/ss2.1920x1080.jpg',
  },
];

const steamDetailWithScreenshots = {
  ...steamDetailFixture,
  screenshots: screenshotsFixture,
};
```

(b) Append a new `describe('media carousel', …)` block inside the top-level describe. Reuse
the existing mocked-`fetchGameDetail` + render pattern from
`renders full detail variant from a mocked response` (~line 106) — same friend-mount render,
swapping the detail fixture per test:

```typescript
describe('media carousel', () => {
  function mockDetail(detail: object | null) {
    vi.mocked(fetchGameDetail).mockResolvedValue({
      game: friendGame,
      steam:
        detail === null
          ? null
          : { detail, overall: overallFixture, recent: recentFixture },
    });
  }

  function renderFriendModal() {
    return render(
      <GameDetailModal
        mount="friend"
        token="tok123"
        game={friendGame}
        active={true}
        onClaim={vi.fn()}
        onClose={vi.fn()}
        loadDetail={friendLoadDetail}
      />,
    );
  }

  it('trailer + screenshots: trailer is slide 1, arrows + counter present', async () => {
    mockDetail(steamDetailWithScreenshots);
    renderFriendModal();
    expect(await screen.findByLabelText('play trailer')).toBeInTheDocument();
    expect(screen.getByLabelText('previous')).toBeInTheDocument();
    expect(screen.getByLabelText('next')).toBeInTheDocument();
    expect(screen.getByText('1 / 3')).toBeInTheDocument();
  });

  it('next advances to a screenshot and wraps past the end', async () => {
    mockDetail(steamDetailWithScreenshots);
    renderFriendModal();
    const next = await screen.findByLabelText('next');
    await userEvent.click(next);
    expect(screen.getByText('2 / 3')).toBeInTheDocument();
    expect(screen.getByAltText('Stardew Valley screenshot 1')).toBeInTheDocument();
    await userEvent.click(next);
    expect(screen.getByText('3 / 3')).toBeInTheDocument();
    await userEvent.click(next);
    expect(screen.getByText('1 / 3')).toBeInTheDocument(); // wrap
  });

  it('one screenshot, no trailer: image alone, zero carousel chrome', async () => {
    mockDetail({
      ...steamDetailFixture,
      video_hls_url: null,
      screenshots: [screenshotsFixture[0]],
    });
    renderFriendModal();
    expect(
      await screen.findByAltText('Stardew Valley screenshot 1'),
    ).toBeInTheDocument();
    expect(screen.queryByLabelText('previous')).not.toBeInTheDocument();
    expect(screen.queryByLabelText('next')).not.toBeInTheDocument();
    expect(screen.queryByText('1 / 1')).not.toBeInTheDocument();
  });

  it('no trailer, no screenshots: plain header image, no carousel chrome', async () => {
    mockDetail({ ...steamDetailFixture, video_hls_url: null });
    renderFriendModal();
    expect(await screen.findByAltText('Stardew Valley')).toBeInTheDocument();
    expect(screen.queryByLabelText('next')).not.toBeInTheDocument();
  });

  it('navigating away from the trailer pauses the video', async () => {
    const pauseSpy = vi
      .spyOn(HTMLMediaElement.prototype, 'pause')
      .mockImplementation(() => {});
    mockDetail(steamDetailWithScreenshots);
    renderFriendModal();
    await userEvent.click(await screen.findByLabelText('next'));
    expect(pauseSpy).toHaveBeenCalled();
    pauseSpy.mockRestore();
  });

  it('fatal HLS error mid-carousel drops the trailer slide and clamps the index', async () => {
    mockDetail(steamDetailWithScreenshots);
    renderFriendModal();
    await userEvent.click(await screen.findByLabelText('play trailer'));
    await waitFor(() => expect(hlsCbCapture.errorCb).not.toBeNull());
    act(() => hlsCbCapture.errorCb?.('hlsError', { fatal: true }));
    // Trailer gone: 2 screenshots remain, counter consistent, no crash.
    expect(await screen.findByText('1 / 2')).toBeInTheDocument();
    expect(screen.queryByLabelText('play trailer')).not.toBeInTheDocument();
  });

  it('off-screen slides are inert and aria-hidden', async () => {
    mockDetail(steamDetailWithScreenshots);
    renderFriendModal();
    await screen.findByLabelText('play trailer');
    const region = screen.getByRole('region', { name: 'media' });
    const hidden = region.querySelectorAll('[aria-hidden="true"][inert]');
    expect(hidden.length).toBe(2); // both screenshot slides while trailer is active
  });

  it('reduced motion: no transition class; motion allowed: transition present', async () => {
    const mm = vi.spyOn(window, 'matchMedia');
    mm.mockReturnValue({ matches: true } as MediaQueryList);
    mockDetail(steamDetailWithScreenshots);
    const { unmount } = renderFriendModal();
    await screen.findByLabelText('play trailer');
    const strip = () =>
      screen.getByRole('region', { name: 'media' }).firstElementChild as HTMLElement;
    expect(strip().className).not.toContain('transition-transform');
    unmount();
    clearGameDetailCache();
    mm.mockReturnValue({ matches: false } as MediaQueryList);
    mockDetail(steamDetailWithScreenshots);
    renderFriendModal();
    await screen.findByLabelText('play trailer');
    expect(strip().className).toContain('transition-transform');
    mm.mockRestore();
  });
});
```

Note: `clearGameDetailCache()` already runs in the suite's `beforeEach`; the mid-test call
above is needed because the second render in the same test would otherwise hit the cache.

- [ ] **Step 2: Run them, verify they fail**

```bash
cd web && npx vitest run src/GameDetailModal.test.tsx 2>&1 | tail -10 && cd ..
```
Expected: the new `media carousel` tests FAIL (no `previous`/`next` buttons, no counter);
all pre-existing tests still PASS.

- [ ] **Step 3: Create `web/src/MediaHeader.tsx`**

```tsx
import { useState, useEffect, useRef } from 'react';
import type Hls from 'hls.js';
import type { SteamAppDetail } from './api';
import { titleColorClass } from './titleColor';

// ── Media header: trailer + screenshots carousel (issue #61) ──────────────────
// Owns ALL video/HLS state — the modal renders <MediaHeader> and knows nothing
// about slides. Thin fallback holds: no trailer + no screenshots renders the
// plain header image / title-hash block with zero carousel chrome; one media
// item renders alone (chrome appears only at ≥ 2 items). Delight never gates.

type MediaHeaderProps = {
  title: string;
  artworkUrl: string | null;
  detail: SteamAppDetail | null;
};

// Repo pattern for reduced motion (friend/CursorCompanion.tsx:71,
// friend/LinkPage.tsx:37): JS matchMedia, guarded — behavior-testable,
// unlike a media-query-only class.
function prefersReducedMotion(): boolean {
  if (typeof window === 'undefined' || !window.matchMedia) return false;
  return window.matchMedia('(prefers-reduced-motion: reduce)').matches;
}

export function MediaHeader({ title, artworkUrl, detail }: MediaHeaderProps) {
  const [mediaIndex, setMediaIndex] = useState(0);
  const [videoPlaying, setVideoPlaying] = useState(false);
  const [hlsFailed, setHlsFailed] = useState(false);
  const videoRef = useRef<HTMLVideoElement>(null);
  const hlsRef = useRef<Hls | null>(null);

  // ── HLS cleanup on unmount ──────────────────────────────────────────────────
  useEffect(() => {
    return () => {
      if (hlsRef.current) {
        hlsRef.current.destroy();
        hlsRef.current = null;
      }
    };
  }, []);

  const hlsUrl = !hlsFailed ? (detail?.video_hls_url ?? null) : null;
  const screenshots = detail?.screenshots ?? [];
  const slideCount = (hlsUrl !== null ? 1 : 0) + screenshots.length;
  const artwork = detail?.header_image ?? artworkUrl;

  // ── Thin fallback: no media at all → today's header, no carousel chrome ─────
  if (slideCount === 0) {
    return artwork !== null ? (
      <img src={artwork} alt={title} className="aspect-video w-full object-cover" />
    ) : (
      <div
        className={`aspect-video w-full ${titleColorClass(title)}`}
        aria-hidden="true"
      />
    );
  }

  // items can shrink under the index (fatal HLS drops the trailer slide) —
  // clamp at render, leave state alone.
  const index = Math.min(mediaIndex, slideCount - 1);

  const goTo = (next: number) => {
    const wrapped = (next + slideCount) % slideCount;
    if (wrapped === index) return;
    // Leaving the trailer slide pauses playback; the ▶ overlay returns so
    // coming back is an explicit resume, never an auto-play.
    if (hlsUrl !== null && index === 0) {
      videoRef.current?.pause();
      setVideoPlaying(false);
    }
    setMediaIndex(wrapped);
  };

  const handlePlay = async (url: string) => {
    if (!videoRef.current || videoPlaying) return;
    const video = videoRef.current;

    if (video.canPlayType('application/vnd.apple.mpegurl')) {
      // Native HLS — Safari. Only (re)assign on first play; resume keeps position.
      if (video.src === '') video.src = url;
    } else if (hlsRef.current === null) {
      // hls.js path — attach once; a resume after pause reuses the instance.
      const { default: HlsClass } = await import('hls.js');
      const hls = new HlsClass();
      hlsRef.current = hls;
      hls.loadSource(url);
      hls.attachMedia(video);
      hls.on(HlsClass.Events.ERROR, (_event, data) => {
        if (data.fatal) {
          setHlsFailed(true);
          hls.destroy();
          hlsRef.current = null;
        }
      });
    }

    setVideoPlaying(true);
    try {
      await video.play();
    } catch {
      // play() rejection (browser policy, no source in test env) — ignore
    }
  };

  const onKeyDown = (e: React.KeyboardEvent) => {
    // Native video controls own their arrow keys.
    if (e.target instanceof HTMLElement && e.target.closest('video')) return;
    if (e.key === 'ArrowLeft') {
      e.preventDefault();
      goTo(index - 1);
    } else if (e.key === 'ArrowRight') {
      e.preventDefault();
      goTo(index + 1);
    }
  };

  return (
    <div
      role="region"
      aria-roledescription="carousel"
      aria-label="media"
      onKeyDown={onKeyDown}
      className="relative overflow-hidden ring-1 ring-pixel"
    >
      {/* Slide strip — transform per index; reduced motion = instant swap */}
      <div
        className={`flex ${prefersReducedMotion() ? '' : 'transition-transform duration-300'}`}
        style={{ transform: `translateX(-${index * 100}%)` }}
      >
        {hlsUrl !== null && (
          <div
            className="relative w-full shrink-0"
            aria-hidden={index !== 0 || undefined}
            inert={index !== 0 || undefined}
          >
            <video
              ref={videoRef}
              poster={detail?.video_thumbnail ?? detail?.header_image ?? artworkUrl ?? undefined}
              className="aspect-video w-full object-cover"
              playsInline
            />
            {!videoPlaying && (
              <button
                type="button"
                aria-label="play trailer"
                onClick={() => void handlePlay(hlsUrl)}
                className="absolute inset-0 flex items-center justify-center bg-black/40 hover:bg-black/50"
              >
                <span className="text-5xl text-white">▶</span>
              </button>
            )}
          </div>
        )}
        {screenshots.map((shot, i) => {
          const slideIdx = (hlsUrl !== null ? 1 : 0) + i;
          return (
            <div
              key={shot.full}
              className="w-full shrink-0"
              aria-hidden={index !== slideIdx || undefined}
              inert={index !== slideIdx || undefined}
            >
              <img
                src={shot.full}
                loading="lazy"
                alt={`${title} screenshot ${i + 1}`}
                className="aspect-video w-full object-cover"
              />
            </div>
          );
        })}
      </div>

      {/* Chrome only at ≥ 2 items. Neutral control green — burgundy is giving-only. */}
      {slideCount > 1 && (
        <>
          <button
            type="button"
            aria-label="previous"
            onClick={() => goTo(index - 1)}
            className="absolute left-2 top-1/2 -translate-y-1/2 rounded bg-control px-2 py-1 text-sm hover:bg-control-bright"
          >
            ‹
          </button>
          <button
            type="button"
            aria-label="next"
            onClick={() => goTo(index + 1)}
            className="absolute right-2 top-1/2 -translate-y-1/2 rounded bg-control px-2 py-1 text-sm hover:bg-control-bright"
          >
            ›
          </button>
          <span
            aria-live="polite"
            className="absolute bottom-2 right-2 rounded bg-shelf px-2 py-0.5 text-xs text-ink-soft"
          >
            {index + 1} / {slideCount}
          </span>
        </>
      )}
    </div>
  );
}
```

- [ ] **Step 4: Gut the modal's media code and render `MediaHeader`**

In `web/src/GameDetailModal.tsx`:
1. Imports (lines 1-4): drop `useRef` if now unused and `import type Hls from 'hls.js'`;
   add `import { MediaHeader } from './MediaHeader';`. (Keep `useState`, `useEffect`;
   `containerRef` still needs `useRef` — it stays.)
2. Delete state/refs `videoPlaying`, `hlsFailed` (line 91-92), `videoRef`, `hlsRef`
   (95-96); delete the HLS-cleanup effect (142-149) and `handlePlay` (163-192).
3. Replace the whole "Video or artwork header" block (the `{!hlsFailed && hlsUrl !== null ?
   … : artwork !== null ? … : …}` ternary, lines 281-317) AND the two consts above it
   (`hlsUrl`, `artwork`, lines 276-277) with:

```tsx
                  <MediaHeader
                    title={game.title}
                    artworkUrl={game.artwork_url}
                    detail={detail}
                  />
```

4. The thin-fallback branch (`steam === null`, lines 246-271) stays byte-identical.

- [ ] **Step 5: Run the full web suite, verify green**

```bash
cd web && npx vitest run 2>&1 | tail -6 && npm run build 2>&1 | tail -3 && cd ..
```
Expected: ALL tests pass — the new carousel ones AND every pre-existing modal test (the
old fixture has no screenshots → single trailer slide → no chrome → old assertions hold;
the existing hls-fatal test now exercises slideCount 1→0 → artwork fallback). Build clean.
If `inert` fails the TS build (React 19 types accept it; if the project's @types lag,
use `inert={index !== 0 ? true : undefined}` — do NOT cast to any).

- [ ] **Step 6: Commit**

```bash
git add web/src/MediaHeader.tsx web/src/GameDetailModal.tsx web/src/GameDetailModal.test.tsx
git commit -S -m "web: media header becomes a trailer+screenshots carousel (#61)

MediaHeader owns all video/HLS state; index-state strip, wrap-around arrows +
lowercase n/m counter (no dots), pixel-bezel frame, control-green chrome, inert
off-screen slides, matchMedia reduced-motion (repo pattern), render-time index
clamp when a fatal HLS error drops the trailer slide. thin fallback unchanged:
no media = today's header, one item = no chrome"
```

---

### Task 6: web — dialog focus trap (TDD)

**Files:**
- Modify: `web/src/GameDetailModal.tsx` (dialog container div ~line 202-212)
- Test: `web/src/GameDetailModal.test.tsx`

**Interfaces:**
- Consumes: the dialog container ref/`tabIndex={-1}` structure (existing).
- Produces: Tab/Shift+Tab focus wrap inside the dialog. Contract (from the spec): focusable
  set computed at keydown time, `[inert]`/`aria-hidden="true"` subtrees and disabled
  elements excluded; Tab on last → first; Shift+Tab on first → last; from the container
  itself Tab → first, Shift+Tab → last; empty set → Tab swallowed.

- [ ] **Step 1: Write the failing tests**

Append to `web/src/GameDetailModal.test.tsx` (inside the top-level describe):

```typescript
describe('focus trap', () => {
  it('Tab from the container enters the dialog; Tab on last focusable wraps to first', async () => {
    vi.mocked(fetchGameDetail).mockResolvedValue({
      game: friendGame,
      steam: { detail: steamDetailFixture, overall: null, recent: null },
    });
    render(
      <GameDetailModal
        mount="friend"
        token="tok123"
        game={friendGame}
        active={true}
        onClaim={vi.fn()}
        onClose={vi.fn()}
        loadDetail={friendLoadDetail}
      />,
    );
    await screen.findByLabelText('play trailer');
    const dialog = screen.getByRole('dialog');
    expect(document.activeElement).toBe(dialog); // focus-on-open pin

    // Tab from the container lands on the FIRST focusable, not wherever the
    // browser default would go.
    await userEvent.tab();
    const first = document.activeElement as HTMLElement;
    expect(dialog.contains(first)).toBe(true);

    // Walk to the last focusable, then one more Tab must WRAP to the first.
    // (claim button is the last control in the friend footer)
    const claim = screen.getByRole('button', { name: 'claim' });
    claim.focus();
    await userEvent.tab();
    expect(document.activeElement).toBe(first);
  });

  it('Shift+Tab on the first focusable wraps to the last', async () => {
    vi.mocked(fetchGameDetail).mockResolvedValue({
      game: friendGame,
      steam: { detail: steamDetailFixture, overall: null, recent: null },
    });
    render(
      <GameDetailModal
        mount="friend"
        token="tok123"
        game={friendGame}
        active={true}
        onClaim={vi.fn()}
        onClose={vi.fn()}
        loadDetail={friendLoadDetail}
      />,
    );
    await screen.findByLabelText('play trailer');
    await userEvent.tab(); // container → first focusable
    const first = document.activeElement as HTMLElement;
    await userEvent.tab({ shift: true });
    const last = document.activeElement as HTMLElement;
    expect(last).not.toBe(first);
    const dialog = screen.getByRole('dialog');
    expect(dialog.contains(last)).toBe(true);
  });
});
```

- [ ] **Step 2: Run them, verify they fail**

```bash
cd web && npx vitest run src/GameDetailModal.test.tsx -t 'focus trap' 2>&1 | tail -6 && cd ..
```
Expected: FAIL — happy-dom's default Tab order escapes the dialog / no wrap.

- [ ] **Step 3: Implement the trap**

In `web/src/GameDetailModal.tsx`, above the component (module scope):

```tsx
// ── Focus trap (issue #61 acceptance) ─────────────────────────────────────────
// Computed at keydown time: the focusable set is dynamic (carousel slides are
// inert per-index, buttons appear/disappear), so a cached list would trap focus
// into hidden slides.

const FOCUSABLE_SELECTOR =
  'button, [href], input, select, textarea, video, [tabindex]:not([tabindex="-1"])';

function dialogFocusables(container: HTMLElement): HTMLElement[] {
  return Array.from(container.querySelectorAll<HTMLElement>(FOCUSABLE_SELECTOR)).filter(
    (el) =>
      el.closest('[inert], [aria-hidden="true"]') === null &&
      !el.hasAttribute('disabled'),
  );
}
```

Inside the component, next to the Escape effect (~line 151):

```tsx
  const handleTrapKeyDown = (e: React.KeyboardEvent) => {
    if (e.key !== 'Tab') return;
    const container = containerRef.current;
    if (container === null) return;
    const els = dialogFocusables(container);
    if (els.length === 0) {
      e.preventDefault(); // nowhere to go — focus stays on the container
      return;
    }
    const first = els[0];
    const last = els[els.length - 1];
    const active = document.activeElement;
    if (e.shiftKey) {
      if (active === first || active === container) {
        e.preventDefault();
        last.focus();
      }
    } else if (active === last || active === container) {
      e.preventDefault();
      first.focus();
    }
  };
```

and on the dialog container div (line 202-211), add `onKeyDown={handleTrapKeyDown}`.
(Escape stays the existing document-level listener; the carousel's Arrow handling stays on
the carousel region — Tab is the only key this handler owns.)

- [ ] **Step 4: Run the full web suite, verify green**

```bash
cd web && npx vitest run 2>&1 | tail -5 && cd ..
```
Expected: all PASS.

- [ ] **Step 5: Commit**

```bash
git add web/src/GameDetailModal.tsx web/src/GameDetailModal.test.tsx
git commit -S -m "web: dialog focus trap — keydown-time focusable set, inert/aria-hidden excluded (#61)"
```

---

### Task 7: full gates + push

**Files:** none new — verification only.

- [ ] **Step 1: All gates, in order**

```bash
export PATH="$HOME/.cargo/bin:$HOME/.local/node22/bin:$PATH"
cargo fmt --check
cargo clippy --workspace --all-targets --all-features -- -D warnings 2>&1 | tail -3
cargo test --workspace 2>&1 | tail -5
cd web && npm run build 2>&1 | tail -3 && npx vitest run 2>&1 | tail -5 && cd ..
```
Expected: fmt silent; clippy clean; every suite green. Fix anything that isn't (fmt
failures: run `cargo fmt` and amend the owning commit if unpushed, else a fixup commit).

- [ ] **Step 2: Old-name sweep, one last time**

```bash
grep -rn "backfill_genres\|backfill_steam_genres" --include="*.rs" --include="*.toml" . | grep -v target/
```
Expected: zero hits in code (docs/history-only hits are fine).

- [ ] **Step 3: Push the branch**

```bash
git push -u origin kitten/media-carousel
```

(PR creation, CI watch, /review, merge, deploy, and the `backfill_details` prod run are
pipeline steps OUTSIDE this plan — see the spec's Rollout & ops for the deploy/backfill
contract, especially `SKIP_FRESH_SECS=0` on the first run.)
