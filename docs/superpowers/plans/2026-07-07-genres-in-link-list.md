# Genres in the Link Games List Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** `GET /api/l/{token}` carries each game's first 5 steam genres so the friend grid renders genre chips with zero per-card detail requests (closes yourcodekitten/bendobundles#55).

**Architecture:** The link-view handler already maps listable games into `GameView`; it gains a cache-only read of the existing dynamo steam enrichment cache (`Store::get_steam_app`, the same blob the detail endpoint reads) per distinct `steam_app_id`, memoized within the request because duplicate copies of one title are common. The shared `GameView` struct gains `genres: Vec<String>` with `skip_serializing_if = "Vec::is_empty"`, so games without an appid/cache omit the key and the detail endpoint (which passes an empty vec) stays byte-identical on the wire. The web client's `GameView` type gains `genres?: string[]` and `GameGrid` drops its entire per-card fetch + module cache (`GenreChips`, `genreCache`, `useSyncExternalStore` plumbing), rendering chips straight from the payload.

**Tech Stack:** Rust (axum handlers in `crates/public-api`, dynamo store in `crates/dynamo`), integration tests against dynamodb-local (moto), React + TypeScript + vitest in `web/`.

**Spec:** the issue body of yourcodekitten/bendobundles#55 (view with `gh issue view 55 -R yourcodekitten/bendobundles`).

## Global Constraints

- **Cache-only:** Steam's HTTP API is NEVER called at request time — genres come only from `Store::get_steam_app` (the dynamo cache written at sync time).
- **Best-effort:** any cache miss, negative-cache stub (`detail: None`), or store error degrades to empty genres for that game — never an error response. Same posture as the detail endpoint.
- **Detail endpoint wire-unchanged:** `GET /api/l/{token}/games/{id}/detail` must serialize a `game` object with NO `genres` key (the modal reads the full `steam.detail.genres` blob instead).
- **Empty means omitted:** `genres` is skipped from JSON when empty (`#[serde(skip_serializing_if = "Vec::is_empty")]` / optional TS field).
- **Server caps genres at 5** (issue: "first ~5"); the client displays at most 4 (existing display rule).
- **Gates before every push:** `cargo fmt --check`, `cargo clippy --workspace --all-targets --all-features -- -D warnings`, and in `web/`: `npm run lint && npm run typecheck && npm test -- --run && npm run build`.
- **Commits are GPG-signed** (`git commit -S`), authored as `code kitten <yourcodekitten@gmail.com>`.
- **PATH exports needed on this box:** `export PATH="$HOME/.cargo/bin:$PATH"` for cargo; `export PATH="$HOME/.local/node22/bin:$PATH"` for npm/node.

---

### Task 1: public-api — genres in the link list payload

**Files:**
- Modify: `crates/public-api/src/lib.rs:76-83` (the `GameView` struct)
- Modify: `crates/public-api/src/lib.rs:459-477` (the games `async` arm of the `tokio::join!` in `handle_get_link`)
- Modify: `crates/public-api/src/lib.rs:735-742` (the `GameView` construction in `handle_game_detail`; line numbers are pre-change — this site shifts down after the handler edit, find it by searching `let game_view = GameView`)
- Test: `crates/public-api/tests/api_test.rs` (append a new test at the end of the file)

**Interfaces:**
- Consumes (existing code, verified signatures):
  - `Store::get_steam_app(&self, app_id: u32) -> Result<Option<SteamAppCache>, StoreError>` (crates/dynamo)
  - `SteamAppCache { detail: Option<SteamAppDetail>, .. }` with `SteamAppDetail { genres: Vec<String>, .. }` (already deduped, order-preserving)
  - test helpers already in `api_test.rs`: `store_or_skip`, `test_game`, `test_link`, `test_steam_cache`, `plain_router`, `body_json`, `MockInvoker`
- Produces (wire contract Task 2 relies on): each element of `LinkView.games` MAY carry `"genres": [string, ...]` (max 5); the key is ABSENT when the game has no appid, no cache entry, a negative-cache stub, or a store error. The detail endpoint's `game` object never carries the key.

- [ ] **Step 1: Ensure dynamodb-local (moto) is up on :8155**

The store-backed test needs a local dynamo. Two SEPARATE shell invocations (never combine pkill with the start in one compound command — pkill matches its own command line and kills the shell):

First invocation — clear the port:
```bash
pkill -9 -f 'moto_server -p 8155'; sleep 1; pgrep -af moto_server; ss -ltn | grep 8155 || echo "port 8155 free"
```
Expected: no moto_server processes listed, "port 8155 free".

Second invocation — start fresh:
```bash
nohup ~/.local/bin/moto_server -p 8155 > /tmp/moto8155.log 2>&1 & sleep 2; curl -s -o /dev/null -w "%{http_code}\n" http://localhost:8155
```
Expected: an HTTP status (e.g. `403` or `200`) proving the port answers.

- [ ] **Step 2: Write the failing test**

Append to `crates/public-api/tests/api_test.rs`:

```rust
/// GET /api/l/:token — games in the list payload carry `genres` from the steam
/// cache (first 5, cache-only), games without an appid omit the key entirely,
/// and the detail endpoint's `game` object stays wire-identical (no `genres`).
#[tokio::test]
async fn link_list_carries_genres_from_steam_cache() {
    let uid = uuid::Uuid::new_v4().simple().to_string();
    let Some(store) = store_or_skip(&format!("gnr{}", &uid[..10])).await else {
        return;
    };

    // game A: steam appid + warm cache with 6 genres (proves the 5-cap)
    let mut a = test_game(60);
    a.steam_app_id = Some(99101);
    let aid = a.id.clone();
    store.put_game(&a).await.unwrap();
    let mut cache = test_steam_cache(99101);
    cache.detail.as_mut().unwrap().genres = vec![
        "Action".into(),
        "Indie".into(),
        "Platformer".into(),
        "Adventure".into(),
        "Casual".into(),
        "Sports".into(),
    ];
    store.put_steam_app(&cache).await.unwrap();

    // game B: no steam appid → genres key must be absent
    let b = test_game(61);
    let bid = b.id.clone();
    store.put_game(&b).await.unwrap();

    // game C: appid but cache-cold (no put_steam_app) → genres key absent too
    let mut c = test_game(62);
    c.steam_app_id = Some(99102);
    let cid = c.id.clone();
    store.put_game(&c).await.unwrap();

    let tok = format!("gnr{}", &uid[..28]);
    store.create_link(&test_link(&tok)).await.unwrap();

    let mock = MockInvoker::new(FulfillResponse::GiftUrl {
        url: "https://x.com/g".into(),
    });
    let req = Request::get(format!("/api/l/{tok}"))
        .body(Body::empty())
        .unwrap();
    let resp = plain_router(Arc::clone(&store), mock.clone())
        .oneshot(req)
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let j = body_json(resp).await;

    let games = j["games"].as_array().expect("games must be an array");
    let ga = games
        .iter()
        .find(|g| g["id"] == aid.as_str())
        .expect("game A must be in the list");
    let gb = games
        .iter()
        .find(|g| g["id"] == bid.as_str())
        .expect("game B must be in the list");

    assert_eq!(
        ga["genres"],
        serde_json::json!(["Action", "Indie", "Platformer", "Adventure", "Casual"]),
        "cache-warm game carries the first 5 genres, in cache order"
    );
    assert!(
        gb.get("genres").is_none(),
        "game without appid must omit the genres key entirely"
    );
    let gc = games
        .iter()
        .find(|g| g["id"] == cid.as_str())
        .expect("game C must be in the list");
    assert!(
        gc.get("genres").is_none(),
        "appid with a cold cache must degrade to no genres key (best-effort)"
    );

    // detail endpoint wire shape unchanged: game object has NO genres key,
    // and the modal still reads the full steam blob.
    let dreq = Request::get(format!("/api/l/{tok}/games/{aid}/detail"))
        .body(Body::empty())
        .unwrap();
    let dresp = plain_router(Arc::clone(&store), mock)
        .oneshot(dreq)
        .await
        .unwrap();
    assert_eq!(dresp.status(), StatusCode::OK);
    let dj = body_json(dresp).await;
    assert!(
        dj["game"].get("genres").is_none(),
        "detail game object must stay wire-identical (no genres key)"
    );
    assert_eq!(dj["steam"]["detail"]["genres"][0], "Action");
}
```

- [ ] **Step 3: Run the test to verify it fails for the right reason**

```bash
cd ~/bendobundles && export PATH="$HOME/.cargo/bin:$PATH" && \
DYNAMODB_LOCAL_URL=http://localhost:8155 cargo test -p public-api --test api_test link_list_carries_genres_from_steam_cache
```
Expected: compiles, then FAILS on the first genres assertion — `assertion 'left == right' failed: cache-warm game carries the first 5 genres` with `left: Null`. (A compile error means the test references a helper wrongly — fix the test, not the code. A PANIC saying "refusing to skip" means moto isn't up — with `DYNAMODB_LOCAL_URL` set, `store_or_skip` panics rather than skips; re-run Step 1.)

- [ ] **Step 4: Implement**

4a. `crates/public-api/src/lib.rs` — `GameView` (lines 76-83) gains the field:

```rust
struct GameView {
    id: String,
    title: String,
    bundle: String,
    key_type: String,
    artwork_url: Option<String>,
    steam_app_id: Option<u32>,
    /// First ~5 steam genres from the enrichment cache (cache-only,
    /// best-effort). Empty → omitted from the wire. The detail endpoint
    /// always leaves this empty — the modal reads the full steam blob.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    genres: Vec<String>,
}
```

4b. `handle_get_link` — replace the games arm of the `tokio::join!` (the `async { ... }` block currently at lines 458-477 that maps `list_listable_games` into `GameView`s) with:

```rust
        async {
            if hide_games {
                return vec![];
            }
            let gs = match s.store.list_listable_games().await {
                Ok(gs) => gs,
                Err(_) => return vec![],
            };
            // Genres ride the same steam cache the detail endpoint reads —
            // cache-only, best-effort: any miss/stub/error degrades to no
            // genres for that game. Memoized per appid because duplicate
            // copies of one title are common in the catalog.
            let mut memo: std::collections::HashMap<u32, Vec<String>> =
                std::collections::HashMap::new();
            let mut views = Vec::with_capacity(gs.len());
            for g in gs {
                let genres = match g.steam_app_id {
                    None => Vec::new(),
                    Some(app_id) => {
                        if let Some(known) = memo.get(&app_id) {
                            known.clone()
                        } else {
                            let fetched: Vec<String> =
                                match s.store.get_steam_app(app_id).await {
                                    Ok(Some(cache)) => cache
                                        .detail
                                        .map(|d| {
                                            d.genres.into_iter().take(5).collect()
                                        })
                                        .unwrap_or_default(),
                                    Ok(None) | Err(_) => Vec::new(),
                                };
                            memo.insert(app_id, fetched.clone());
                            fetched
                        }
                    }
                };
                views.push(GameView {
                    id: g.id,
                    title: g.title,
                    bundle: g.bundle,
                    key_type: g.key_type,
                    artwork_url: g.artwork_url,
                    steam_app_id: g.steam_app_id,
                    genres,
                });
            }
            views
        },
```

4c. `handle_game_detail` — the `let game_view = GameView { ... }` construction (search for it; pre-change lines 735-742) gains one field so the struct compiles, with the wire staying identical (empty → key omitted):

```rust
    let game_view = GameView {
        id: game.id,
        title: game.title,
        bundle: game.bundle,
        key_type: game.key_type,
        artwork_url: game.artwork_url,
        steam_app_id: game.steam_app_id,
        // Deliberately empty (key omitted on the wire): the modal reads
        // steam.detail.genres from the full blob below instead.
        genres: vec![],
    };
```

- [ ] **Step 5: Run the test to verify it passes, then the crate suite**

```bash
cd ~/bendobundles && export PATH="$HOME/.cargo/bin:$PATH" && \
DYNAMODB_LOCAL_URL=http://localhost:8155 cargo test -p public-api --test api_test link_list_carries_genres_from_steam_cache
```
Expected: `test link_list_carries_genres_from_steam_cache ... ok`.

Then the whole crate (guards the detail tests and every other endpoint):
```bash
DYNAMODB_LOCAL_URL=http://localhost:8155 cargo test -p public-api
```
Expected: all tests pass, 0 failed (store-backed tests must RUN, not skip — the env var forces that).

- [ ] **Step 6: Gates + commit**

```bash
cd ~/bendobundles && cargo fmt --check && cargo clippy --workspace --all-targets --all-features -- -D warnings
```
Expected: both clean. Then:

```bash
git add crates/public-api/src/lib.rs crates/public-api/tests/api_test.rs
git commit -S -m "feat(public-api): include steam genres in the link games list (#55)

GameView gains genres (first 5 from the dynamo steam cache, cache-only,
best-effort, memoized per appid); empty is omitted from the wire so the
detail endpoint's game object stays byte-identical."
```

### Task 2: web — render chips from the payload, kill the per-card fetch

**Files:**
- Modify: `web/src/api.ts:2-9` (the `GameView` type)
- Modify: `web/src/friend/GameGrid.tsx` (delete lines 1-48 infra: the `GenreChips` component, `genreCache`/`genreVersion`/listener plumbing, and the `useEffect`/`useSyncExternalStore`/`useParams`/`fetchGameDetail` imports; rewrite the chips row at lines 112-144)
- Test: `web/src/friend/GameGrid.test.tsx` (append two tests inside the existing `describe('GameGrid', ...)`)

**Interfaces:**
- Consumes (from Task 1, via the wire): `LinkView.games[n].genres?: string[]` — max 5 entries, absent when unknown.
- Consumes (existing code): `titleColorClass(title: string): string` from `web/src/titleColor` (unchanged).
- Produces: nothing later tasks rely on (this is the last task).

- [ ] **Step 1: Write the failing tests**

Append inside `describe('GameGrid', ...)` in `web/src/friend/GameGrid.test.tsx`:

```tsx
  it('renders up to 4 genre chips straight from the list payload — no fetch', () => {
    const games = [
      makeGame({
        id: '1',
        title: 'Celeste',
        genres: ['Action', 'Indie', 'Platformer', 'Adventure', 'Casual'],
      }),
    ];
    render(<GameGrid games={games} onDetail={vi.fn()} />);
    expect(screen.getByText('Action')).toBeInTheDocument();
    expect(screen.getByText('Adventure')).toBeInTheDocument();
    // display cap is 4 — the 5th genre from the payload is not rendered
    expect(screen.queryByText('Casual')).not.toBeInTheDocument();
    // genre chips replace the key_type chip
    expect(screen.queryByText('steam')).not.toBeInTheDocument();
  });

  it('falls back to the key_type chip when the payload has no genres', () => {
    render(<GameGrid games={[makeGame({ id: '1', title: 'Game' })]} onDetail={vi.fn()} />);
    expect(screen.getByText('steam')).toBeInTheDocument();
  });
```

- [ ] **Step 2: Run the new tests to verify the first fails for the right reason**

```bash
cd ~/bendobundles/web && export PATH="$HOME/.local/node22/bin:$PATH" && npx vitest run src/friend/GameGrid.test.tsx
```
Expected: the payload test FAILS with `Unable to find an element with the text: Action` (the current grid ignores `game.genres` — `GenreChips` finds no cache entry and renders the key_type fallback). The fallback test PASSES already (it pins current behavior so the rewrite can't break it). Note: vitest strips types without checking them, so the not-yet-typed `genres` override does not error here — `npm run typecheck` would, which is why the type lands in Step 3 before the gates.

- [ ] **Step 3: Implement**

3a. `web/src/api.ts` — `GameView` (lines 2-9) gains the optional field:

```ts
export type GameView = {
  id: string;
  title: string;
  bundle: string;
  key_type: string;
  artwork_url: string | null;
  steam_app_id: number | null;
  /** First ~5 steam genres from the server's enrichment cache; absent when unknown. */
  genres?: string[];
};
```

3b. `web/src/friend/GameGrid.tsx` — delete the fetch infrastructure and render from the payload. **All line numbers below are relative to the ORIGINAL file and drift as you edit — anchor every edit on the quoted content (exact old text → new text), never on line numbers.**

- Replace the import block (original lines 1-4) with:

```tsx
import { type GameView } from '../api';
import { titleColorClass } from '../titleColor';
```

- Delete the whole block from the comment `// Steam genres per game, fetched once and cached for the page's lifetime.` down through the closing `}` of the `GenreChips` component (original lines 6-48 — the `genreCache`/`genreVersion`/`genreListeners`/`genreSubscribe`/`genreSnapshot`/`genreNotify` module state and the entire `GenreChips` function).

- Inside the `.map(({ game, count }) => { ... })` body, before `const chipsRow`, add:

```tsx
        // genres ride the list payload now (issue #55) — no per-card fetch.
        // max 4 chips on the card; absent/empty falls back to the key_type chip.
        const genres =
          game.genres !== undefined && game.genres.length > 0
            ? game.genres.slice(0, 4)
            : null;
```

- Inside `chipsRow`, replace the old three-line `{/* genre chips replace the key_type chip when steam genres are cached; ... */}` comment TOGETHER WITH the entire `<GenreChips game={game}>...</GenreChips>` render-prop block that follows it (original lines 114-144 — comment included, or it survives as a lie about a cache that no longer exists) with:

```tsx
            {/* genre chips replace the key_type chip when the payload carries
                genres; tag colors ride the shared title-hash palette
                (The Title-Hash Rule) tinted toward floor for chip duty */}
            {genres === null ? (
              /* floor chip — the shelf chip vanishes on the shelf card */
              <span className="rounded bg-floor px-2 py-0.5 text-xs text-ink-soft">
                {game.key_type}
              </span>
            ) : (
              genres.map((genre) => {
                const hue = `var(${titleColorClass(genre).replace('bg-', '--color-')})`;
                return (
                  <span
                    key={genre}
                    className="rounded px-2 py-0.5 text-xs"
                    style={{
                      background: `color-mix(in oklch, ${hue}, var(--color-floor) 70%)`,
                      color: `color-mix(in oklch, ${hue}, oklch(15% 0.02 110) 35%)`,
                    }}
                  >
                    {genre}
                  </span>
                );
              })
            )}
```

- [ ] **Step 4: Run the grid tests, then the full web suite + gates**

```bash
cd ~/bendobundles/web && npx vitest run src/friend/GameGrid.test.tsx
```
Expected: all GameGrid tests PASS (both new ones green; every pre-existing test untouched and green).

```bash
npm run lint && npm run typecheck && npm test -- --run && npm run build
```
Expected: all four clean. (`npm test -- --run` runs the whole vitest suite — LinkPage's `fetchGameDetail` mocks still satisfy the modal, which keeps its own fetch.)

- [ ] **Step 5: Commit**

```bash
cd ~/bendobundles
git add web/src/api.ts web/src/friend/GameGrid.tsx web/src/friend/GameGrid.test.tsx
git commit -S -m "feat(web): genre chips ride the list payload — drop the per-card detail fetch (#55)

GameView gains genres?: string[]; GameGrid loses GenreChips + module
cache + per-card fetchGameDetail. Zero extra requests on the gift page."
```
