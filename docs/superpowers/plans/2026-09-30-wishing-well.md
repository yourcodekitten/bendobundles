# The Wishing Well ⭐ Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** On a gift link, mark (and on open shelves, float) the games that are on the connected friend's public Steam wishlist. The server keeps nothing at rest.

**Architecture:** A keyless `steam-client::get_wishlist` call, used by a new token-scoped proxy `GET /api/l/{token}/steam/wishlist/{steamid}`. The proxy runs the owned proxy's exact preamble order, then **intersects with the link's own live appids** before answering. The web fetches it whenever a Steam identity is on the page, and `GameGrid` renders a ⭐ pill. `LinkPage` briefly holds the first open-shelf render (≤400 ms) so the float lands before the cards do. A wishlist that arrives late adds stars and moves nothing.

**Tech Stack:** Rust (axum, reqwest, serde, wiremock tests, DynamoDB-local via `store_or_skip`), React + TypeScript + vitest.

**Spec:** `docs/spec-wishing-well.md` (at `fe74c92` or later). Read it first. §4 ("nothing at rest") and §2 (what the friend sees) are the contract.

## Global Constraints

- **Nothing at rest.** No `dynamo` crate change, no new `Store` method, no `terraform/` change, no IAM corpus change. **Task 6 asserts this from the diff.**
- **No appids in logs.** The new handler and `get_wishlist` emit **no** `tracing` events that carry wishlist data. Task 2 asserts this with a log capture that has a positive control.
- **Keyless.** `get_wishlist` sends **no** `key` query param. Task 1 asserts this on the recorded request.
- **Private == empty.** `{"response":{}}` ⇒ `Ok(vec![])`. No copy anywhere says "empty" or "private" about a wishlist.
- **Silent degrade.** Any wishlist failure (Steam 429/5xx/network ⇒ proxy `503`; client `FetchFailed`) renders exactly like "no match": no stars, no count line, no error text, and it never blocks owned.
- **Copy (verbatim):** pill `⭐ on your wishlist`. Pill `title` and accessible name `on your wishlist since <mon yyyy>`, lowercase, UTC month (e.g. `since mar 2023`). Count line `⭐ N of these are on your wishlist`, singular `⭐ 1 of these is on your wishlist`. The count line exists only for N ≥ 1.
- **Precedence:** owned wins (no star on an owned game). A `gone` (ghost) card never gets a star, and the server already excludes ghosts from the intersection.
- **Curated links never reorder.** Open shelves float starred games first, shuffled within each group. The order is frozen for the visit.
- Commits GPG-signed as `code kitten <yourcodekitten@gmail.com>`; branch `kitten/wishing-well`.

---

## File map

| File | Responsibility | Task |
|---|---|---|
| `crates/steam-client/src/lib.rs` | `WishItem`, wire types, `get_wishlist` | 1 |
| `crates/steam-client/tests/client_test.rs` | wishlist client tests | 1 |
| `crates/public-api/src/lib.rs` | route, `handle_steam_wishlist_proxy`, `link_live_app_ids`, shared `dead_link_conflict` | 2 |
| `crates/public-api/tests/api_test.rs` | proxy behaviour tests | 2 |
| `crates/public-api/tests/wishlist_logs_test.rs` (new) | no-appids-in-logs capture (own test binary, owns the global subscriber) | 2 |
| `web/src/api.ts` + `web/src/api.test.ts` | `WishItem`, `steamWishlistForLink` | 3 |
| `web/src/friend/GameGrid.tsx` + `.test.tsx` | ⭐ pill, `since` title, precedence | 4 |
| `web/src/wishlist.ts` + `web/src/wishlist.test.ts` (new) | pure helpers: `formatSince`, `floatRanks` | 4, 5 |
| `web/src/friend/LinkPage.tsx` + `.test.tsx` | fetch, hold, float, count line, disconnect | 5 |

---

### Task 1: `steam-client::get_wishlist` (keyless)

**Files:**
- Modify: `crates/steam-client/src/lib.rs` (types near `OwnedGames` ~L126; method after `get_owned_games` ~L496)
- Test: `crates/steam-client/tests/client_test.rs` (append)

**Interfaces:**
- Produces: `pub struct WishItem { pub appid: u32, pub date_added: i64 }` (derive `Debug, Clone, PartialEq, Eq`), and `impl SteamClient { pub async fn get_wishlist(&self, steamid: &SteamId64) -> Result<Vec<WishItem>, SteamError> }`.

- [ ] **Step 1: Write the failing tests** (append to `client_test.rs`; `test_client` already exists at the top of the file)

```rust
#[tokio::test]
async fn wishlist_returns_items_and_sends_no_key() {
    let server = wiremock::MockServer::start().await;
    wiremock::Mock::given(wiremock::matchers::method("GET"))
        .and(wiremock::matchers::path("/IWishlistService/GetWishlist/v1/"))
        .and(wiremock::matchers::query_param("steamid", "76561198000000001"))
        .respond_with(wiremock::ResponseTemplate::new(200).set_body_string(
            r#"{"response":{"items":[{"appid":413150,"priority":1,"date_added":1678000000},{"appid":1273400,"priority":0,"date_added":1700000000}]}}"#,
        ))
        .mount(&server)
        .await;
    let out = test_client(&server)
        .get_wishlist(&steam_client::SteamId64("76561198000000001".into()))
        .await
        .unwrap();
    assert_eq!(
        out,
        vec![
            steam_client::WishItem { appid: 413150, date_added: 1678000000 },
            steam_client::WishItem { appid: 1273400, date_added: 1700000000 },
        ]
    );
    // Keyless by construction: the recorded request carries NO key param.
    let reqs = server.received_requests().await.unwrap();
    assert_eq!(reqs.len(), 1);
    assert!(
        !reqs[0].url.query_pairs().any(|(k, _)| k == "key"),
        "wishlist must not send the api key: {}",
        reqs[0].url
    );
}

#[tokio::test]
async fn wishlist_empty_response_object_is_empty_vec_not_error() {
    // Measured 2026-09-30: private and empty are the SAME bytes. Not an error.
    let server = wiremock::MockServer::start().await;
    wiremock::Mock::given(wiremock::matchers::path("/IWishlistService/GetWishlist/v1/"))
        .respond_with(wiremock::ResponseTemplate::new(200).set_body_string(r#"{"response":{}}"#))
        .mount(&server)
        .await;
    let out = test_client(&server)
        .get_wishlist(&steam_client::SteamId64("76561198000000001".into()))
        .await
        .unwrap();
    assert!(out.is_empty());
}

#[tokio::test]
async fn wishlist_429_is_rate_limited() {
    let server = wiremock::MockServer::start().await;
    wiremock::Mock::given(wiremock::matchers::path("/IWishlistService/GetWishlist/v1/"))
        .respond_with(wiremock::ResponseTemplate::new(429))
        .mount(&server)
        .await;
    let err = test_client(&server)
        .get_wishlist(&steam_client::SteamId64("76561198000000001".into()))
        .await
        .unwrap_err();
    assert!(matches!(err, steam_client::SteamError::RateLimited), "{err:?}");
}

#[tokio::test]
async fn wishlist_malformed_body_is_parse_error() {
    let server = wiremock::MockServer::start().await;
    wiremock::Mock::given(wiremock::matchers::path("/IWishlistService/GetWishlist/v1/"))
        .respond_with(wiremock::ResponseTemplate::new(200).set_body_string("<html>nope</html>"))
        .mount(&server)
        .await;
    let err = test_client(&server)
        .get_wishlist(&steam_client::SteamId64("76561198000000001".into()))
        .await
        .unwrap_err();
    assert!(matches!(err, steam_client::SteamError::Parse(_)), "{err:?}");
}
```

- [ ] **Step 2: Run and confirm they fail.** `cargo test -p steam-client --test client_test wishlist` ⇒ compile error: `no method named get_wishlist` / `WishItem` not found.

- [ ] **Step 3: Implement.** Add the types next to `OwnedGames`:

```rust
/// One entry on a friend's public Steam wishlist (spec-wishing-well §1.1). `priority` is
/// deliberately not carried — v1 does not order by it (spec §6).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WishItem {
    pub appid: u32,
    /// Epoch seconds the game was wishlisted; 0 if Steam omitted it.
    pub date_added: i64,
}
```

and the wire structs next to `OwnedWire`:

```rust
#[derive(Deserialize)]
struct WishWire {
    response: WishResp,
}
/// `{"response":{}}` is BOTH a private and an empty wishlist (measured) — so `items`
/// defaults to empty and neither case is an error.
#[derive(Deserialize)]
struct WishResp {
    #[serde(default)]
    items: Vec<WishWireItem>,
}
#[derive(Deserialize)]
struct WishWireItem {
    appid: u32,
    #[serde(default)]
    date_added: i64,
}
```

and the method after `get_owned_games`:

```rust
    /// Public wishlist for `steamid`. **Keyless** — the endpoint does not need the Web API
    /// key (measured 2026-09-30), and a secret on a request that does not need it is only risk.
    /// Private and empty wishlists are indistinguishable on the wire; both are `Ok(vec![])`.
    /// Status mapping reuses `keyed_json` (a 401/403 surfaces as `KeyRejected`, which is a
    /// misnomer on a keyless call — every caller maps all errors to one silent 503).
    pub async fn get_wishlist(&self, steamid: &SteamId64) -> Result<Vec<WishItem>, SteamError> {
        let url = format!("{}/IWishlistService/GetWishlist/v1/", self.base_web_api);
        let resp = self
            .http
            .get(url)
            .query(&[("steamid", &steamid.0)])
            .send()
            .await
            .map_err(net)?;
        let wire: WishWire = keyed_json(resp).await?;
        Ok(wire
            .response
            .items
            .into_iter()
            .map(|i| WishItem { appid: i.appid, date_added: i.date_added })
            .collect())
    }
```

- [ ] **Step 4: Run.** `cargo test -p steam-client` ⇒ all pass, including the 4 new tests. Then `cargo clippy -p steam-client --all-targets -- -D warnings`.

- [ ] **Step 5: Commit.** `git add crates/steam-client && git commit -S -m "⭐ steam-client: get_wishlist — keyless, private==empty is Ok(vec![])"`

---

### Task 2: the token-scoped wishlist proxy

**Files:**
- Modify: `crates/public-api/src/lib.rs` (route table ~L280; new handler after `handle_steam_owned_proxy`; refactor its 409 match into `dead_link_conflict`)
- Test: `crates/public-api/tests/api_test.rs` (append; uses the existing `store_or_skip`, `steam_router`, `test_link`, `MockInvoker`, `body_json`, `CTX_TOKEN`, `TEST_STEAMID`)
- Create: `crates/public-api/tests/wishlist_logs_test.rs`

**Interfaces:**
- Consumes: `SteamClient::get_wishlist`, `WishItem` (Task 1). Existing: `Store::get_link`, `Store::batch_get_games(&[String]) -> Result<HashMap<String, Game>, StoreError>`, `Store::list_listable_games() -> Result<Vec<Game>, StoreError>`, `live_on_link(&Link, &Game) -> bool`, `link_not_found_response()`, `steam_client::is_valid_steam_id64`, `steam_client::STEAM_ID64_ERROR_MSG`.
- Produces: HTTP `GET /api/l/{token}/steam/wishlist/{steamid}` ⇒ `200 {"items":[{"appid":u32,"added":i64}]}` with `Cache-Control: private, max-age=3600` | `404` (byte-identical unknown-link) | `409 {"error":…}` | `400` | `500 {"error":"try again"}` | `503` (empty body).

- [ ] **Step 1: Write the failing proxy tests** (append to `api_test.rs`). Seed games the way the existing tests do (`store.put_game(&game)` with a `Game` whose `steam_app_id` is set and status listable; copy the `Game` literal from the nearest existing test that seeds a listable game with a `steam_app_id`, and change only `id`/`title`/`steam_app_id`).

```rust
fn wish_body(items: &[(u32, i64)]) -> String {
    let xs: Vec<String> = items
        .iter()
        .map(|(a, d)| format!(r#"{{"appid":{a},"priority":0,"date_added":{d}}}"#))
        .collect();
    format!(r#"{{"response":{{"items":[{}]}}}}"#, xs.join(","))
}

async fn mount_wishlist(server: &wiremock::MockServer, status: u16, body: &str) {
    wiremock::Mock::given(wiremock::matchers::method("GET"))
        .and(wiremock::matchers::path("/IWishlistService/GetWishlist/v1/"))
        .respond_with(wiremock::ResponseTemplate::new(status).set_body_string(body.to_string()))
        .mount(server)
        .await;
}

/// Unknown token ⇒ the byte-identical unknown-link 404, even with a malformed id64
/// (token beats id: no oracle upgrade).
#[tokio::test]
async fn wishlist_proxy_unknown_token_is_byte_identical_404_even_with_bad_id() {
    let Some(store) = store_or_skip("wish-404").await else { return };
    let server = wiremock::MockServer::start().await;
    let mock = MockInvoker::new(FulfillResponse::GiftUrl { url: "https://x.com/g".into() });
    let app = steam_router(Arc::clone(&store), mock, &server.uri());
    let a = app.clone().oneshot(Request::get(format!("/api/l/{CTX_TOKEN}/steam/wishlist/not-an-id")).body(Body::empty()).unwrap()).await.unwrap();
    let b = app.clone().oneshot(Request::get(format!("/api/l/{CTX_TOKEN}")).body(Body::empty()).unwrap()).await.unwrap();
    assert_eq!(a.status(), StatusCode::NOT_FOUND);
    let ab = axum::body::to_bytes(a.into_body(), usize::MAX).await.unwrap();
    let bb = axum::body::to_bytes(b.into_body(), usize::MAX).await.unwrap();
    assert_eq!(ab, bb);
}

/// Dead link ⇒ 409 before the id64 is looked at.
#[tokio::test]
async fn wishlist_proxy_revoked_link_is_409_even_with_bad_id() {
    let Some(store) = store_or_skip("wish-409").await else { return };
    let server = wiremock::MockServer::start().await;
    let mut lnk = test_link("wish-409-tok");
    lnk.revoked = true;
    store.create_link(&lnk).await.unwrap();
    let mock = MockInvoker::new(FulfillResponse::GiftUrl { url: "https://x.com/g".into() });
    let app = steam_router(Arc::clone(&store), mock, &server.uri());
    let r = app.oneshot(Request::get("/api/l/wish-409-tok/steam/wishlist/not-an-id").body(Body::empty()).unwrap()).await.unwrap();
    assert_eq!(r.status(), StatusCode::CONFLICT);
}

/// Live link + bad id64 ⇒ 400.
#[tokio::test]
async fn wishlist_proxy_bad_id_on_live_link_is_400() {
    let Some(store) = store_or_skip("wish-400").await else { return };
    let server = wiremock::MockServer::start().await;
    store.create_link(&test_link("wish-400-tok")).await.unwrap();
    let mock = MockInvoker::new(FulfillResponse::GiftUrl { url: "https://x.com/g".into() });
    let app = steam_router(Arc::clone(&store), mock, &server.uri());
    let r = app.oneshot(Request::get("/api/l/wish-400-tok/steam/wishlist/7656119800000000x").body(Body::empty()).unwrap()).await.unwrap();
    assert_eq!(r.status(), StatusCode::BAD_REQUEST);
}

/// Curated link: the response is the INTERSECTION with the link's LIVE games — a wishlisted
/// appid that is not on the shelf is absent, and a ghost (not live_on_link) is absent.
#[tokio::test]
async fn wishlist_proxy_curated_returns_only_live_shelf_overlap() {
    let Some(store) = store_or_skip("wish-curated").await else { return };
    let server = wiremock::MockServer::start().await;
    // A (appid 111) is live; B (appid 222) is hidden ⇒ a ghost on a curated link
    // (`live_on_link` requires `!game.hidden`, lib.rs:651).
    let mut a = test_game(9101);
    a.steam_app_id = Some(111);
    let mut b = test_game(9102);
    b.steam_app_id = Some(222);
    b.hidden = true;
    store.put_game(&a).await.unwrap();
    store.put_game(&b).await.unwrap();
    let mut lnk = test_link("wish-cur-tok");
    lnk.curated_game_ids = Some(vec![a.id.clone(), b.id.clone()]);
    store.create_link(&lnk).await.unwrap();
    // Wishlist carries A, B and 333 (not on the shelf at all).
    mount_wishlist(&server, 200, &wish_body(&[(111, 1678000000), (222, 1678000001), (333, 1678000002)])).await;
    let mock = MockInvoker::new(FulfillResponse::GiftUrl { url: "https://x.com/g".into() });
    let app = steam_router(Arc::clone(&store), mock, &server.uri());
    let r = app.oneshot(Request::get(format!("/api/l/wish-cur-tok/steam/wishlist/{TEST_STEAMID}")).body(Body::empty()).unwrap()).await.unwrap();
    assert_eq!(r.status(), StatusCode::OK);
    assert_eq!(r.headers()[header::CACHE_CONTROL], "private, max-age=3600");
    let j = body_json(r).await;
    assert_eq!(j, serde_json::json!({"items":[{"appid":111,"added":1678000000}]}));
}

/// Open shelf: intersection with the listable catalog.
#[tokio::test]
async fn wishlist_proxy_open_shelf_returns_catalog_overlap() {
    let Some(store) = store_or_skip("wish-open").await else { return };
    let server = wiremock::MockServer::start().await;
    // test_game is Available + giftable + !hidden ⇒ listable. Unique n: the table is shared.
    let mut g = test_game(9201);
    g.steam_app_id = Some(444);
    store.put_game(&g).await.unwrap();
    store.create_link(&test_link("wish-open-tok")).await.unwrap();
    mount_wishlist(&server, 200, &wish_body(&[(444, 1600000000), (555, 1600000001)])).await;
    let mock = MockInvoker::new(FulfillResponse::GiftUrl { url: "https://x.com/g".into() });
    let app = steam_router(Arc::clone(&store), mock, &server.uri());
    let r = app.oneshot(Request::get(format!("/api/l/wish-open-tok/steam/wishlist/{TEST_STEAMID}")).body(Body::empty()).unwrap()).await.unwrap();
    let j = body_json(r).await;
    let appids: Vec<u64> = j["items"].as_array().unwrap().iter().map(|i| i["appid"].as_u64().unwrap()).collect();
    assert!(appids.contains(&444));
    assert!(!appids.contains(&555));
}

/// Private/empty ⇒ 200 {"items":[]}; there is NO "private" flag (unmeasurable).
#[tokio::test]
async fn wishlist_proxy_empty_is_empty_items_no_private_flag() {
    let Some(store) = store_or_skip("wish-empty").await else { return };
    let server = wiremock::MockServer::start().await;
    store.create_link(&test_link("wish-empty-tok")).await.unwrap();
    mount_wishlist(&server, 200, r#"{"response":{}}"#).await;
    let mock = MockInvoker::new(FulfillResponse::GiftUrl { url: "https://x.com/g".into() });
    let app = steam_router(Arc::clone(&store), mock, &server.uri());
    let r = app.oneshot(Request::get(format!("/api/l/wish-empty-tok/steam/wishlist/{TEST_STEAMID}")).body(Body::empty()).unwrap()).await.unwrap();
    assert_eq!(r.status(), StatusCode::OK);
    assert_eq!(body_json(r).await, serde_json::json!({"items":[]}));
}

/// Steam 429 (shared Lambda egress IPs get rate-limited too) ⇒ plain 503; the client
/// renders that silent (Task 5 pins the client half).
#[tokio::test]
async fn wishlist_proxy_steam_429_is_503() {
    let Some(store) = store_or_skip("wish-429").await else { return };
    let server = wiremock::MockServer::start().await;
    store.create_link(&test_link("wish-429-tok")).await.unwrap();
    mount_wishlist(&server, 429, "").await;
    let mock = MockInvoker::new(FulfillResponse::GiftUrl { url: "https://x.com/g".into() });
    let app = steam_router(Arc::clone(&store), mock, &server.uri());
    let r = app.oneshot(Request::get(format!("/api/l/wish-429-tok/steam/wishlist/{TEST_STEAMID}")).body(Body::empty()).unwrap()).await.unwrap();
    assert_eq!(r.status(), StatusCode::SERVICE_UNAVAILABLE);
}
```

Seeding uses the file's own `test_game(n)` (`api_test.rs:85`: `Available`, `giftable`, `!hidden`, `steam_app_id: None`). Every test sets `steam_app_id` and uses a **unique `n` ≥ 9100**, because DynamoDB-local is shared across tests and the open-shelf path scans the whole listable index.

- [ ] **Step 2: Write the failing log-capture test** — new file `crates/public-api/tests/wishlist_logs_test.rs`. It is its own test binary, so it may own the **global** subscriber. (The fulfillment tests explain why a thread-local `set_default` is unsafe with tracing's callsite cache, `handler_test.rs:4327`.)

```rust
//! "No appids in logs" is a promise (spec-wishing-well §4) — this makes it an assertion.
//! Own test binary ⇒ owns the global subscriber; POSITIVE CONTROL first, so a capture that
//! sees nothing cannot pass vacuously.
use std::sync::{Arc, Mutex, OnceLock};

static BUF: OnceLock<Arc<Mutex<Vec<u8>>>> = OnceLock::new();

struct Cap;
impl std::io::Write for Cap {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        BUF.get().unwrap().lock().unwrap().extend_from_slice(b);
        Ok(b.len())
    }
    fn flush(&mut self) -> std::io::Result<()> { Ok(()) }
}

fn install() {
    BUF.get_or_init(|| Arc::new(Mutex::new(Vec::new())));
    let _ = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::TRACE)
        .with_writer(|| Cap)
        .try_init();
}

fn captured() -> String {
    String::from_utf8_lossy(&BUF.get().unwrap().lock().unwrap()).into_owned()
}
```

Then **one** `#[tokio::test]` that: calls `install()`; emits `tracing::info!("canary-9913377")` and asserts `captured().contains("canary-9913377")` (**positive control**); runs the wishlist proxy against a seeded link whose shelf and wishlist both carry appid **`8675309`**, with a wishlist-only appid **`7340033`** (seed with `test_game(9301)`, `steam_app_id: Some(8675309)`; copy `test_game`, `test_link`, `MockInvoker`, `steam_router`; reuse the helper bodies from Step 1 by copying them. They live in another test binary and cannot be imported); asserts a 200 whose body contains `8675309`; and finally asserts `!captured().contains("8675309") && !captured().contains("7340033")`. Use `store_or_skip` copied from `api_test.rs:24` (the first ~50 lines are self-contained helpers).

- [ ] **Step 3: Run and confirm they fail.** `cargo test -p public-api --test api_test wishlist` and `cargo test -p public-api --test wishlist_logs_test` ⇒ all 404 (no route) or fail to compile.

- [ ] **Step 4: Implement.** In `lib.rs`:

(a) **Extract the dead-link 409** out of `handle_steam_owned_proxy` so both proxies share one mapping:

```rust
/// The claim-path refusal ⇒ 409 mapping, shared by both steam proxies (one exhaustive match:
/// a new refusal variant forces a decision here at compile time).
fn dead_link_conflict(refusal: domain::ClaimRefusal) -> Response {
    use domain::ClaimRefusal;
    let msg = match refusal {
        ClaimRefusal::Revoked => "this link has been revoked",
        ClaimRefusal::Sealed => "this gift is still wrapped",
        ClaimRefusal::Expired => "this link has expired",
        ClaimRefusal::Exhausted => "no claims left on this link",
    };
    (StatusCode::CONFLICT, Json(serde_json::json!({"error": msg}))).into_response()
}
```

and replace the owned proxy's inline block with `if let Err(refusal) = link.can_claim(now) { return dead_link_conflict(refusal); }`. The existing owned-proxy tests must stay green unchanged.

(b) **The live-appid set:**

```rust
/// The steam appids a friend can actually see as LIVE cards on this link — the set the
/// wishlist proxy intersects against. Curated: members that pass `live_on_link` (ghosts are
/// excluded; a star on a ghost would point at nothing). Open: the listable catalog.
async fn link_live_app_ids(
    store: &Store,
    link: &domain::Link,
) -> Result<std::collections::HashSet<u32>, StoreError> {
    Ok(match &link.curated_game_ids {
        Some(ids) => store
            .batch_get_games(ids)
            .await?
            .values()
            .filter(|g| live_on_link(link, g))
            .filter_map(|g| g.steam_app_id)
            .collect(),
        None => store
            .list_listable_games()
            .await?
            .iter()
            .filter_map(|g| g.steam_app_id)
            .collect(),
    })
}
```

(c) **The handler** (after `handle_steam_owned_proxy`). **It contains no `tracing::` call at all:**

```rust
// ── GET /api/l/{token}/steam/wishlist/{steamid} ─────────────────────────────────

/// ⭐ Token-scoped wishlist proxy (docs/spec-wishing-well.md). Same preamble ORDER as the
/// owned proxy (token → liveness → id64). Then a LIVE, keyless Steam read — nothing is
/// stored (§4: the server holds nothing about a wishlist) — intersected with the link's
/// live appids so the proxy never serves anyone's full list. Every Steam failure is one
/// plain 503 that the client renders silent. No tracing here, by contract: appids are
/// the friend's data.
async fn handle_steam_wishlist_proxy(
    State(s): State<AppState>,
    Path((token, steamid)): Path<(String, String)>,
) -> Response {
    let steam = match s.steam.as_ref() {
        Some(c) => c,
        None => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(serde_json::json!({"error": "steam not configured"})),
            )
                .into_response();
        }
    };
    let link = match s.store.get_link(&token).await {
        Ok(Some(l)) => l,
        Ok(None) => return link_not_found_response(),
        Err(_) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": "try again"})),
            )
                .into_response();
        }
    };
    if let Err(refusal) = link.can_claim(OffsetDateTime::now_utc()) {
        return dead_link_conflict(refusal);
    }
    if !steam_client::is_valid_steam_id64(&steamid) {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": steam_client::STEAM_ID64_ERROR_MSG})),
        )
            .into_response();
    }
    let shelf = match link_live_app_ids(&s.store, &link).await {
        Ok(set) => set,
        Err(_) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": "try again"})),
            )
                .into_response();
        }
    };
    match steam
        .get_wishlist(&steam_client::SteamId64(steamid))
        .await
    {
        Ok(items) => {
            let items: Vec<serde_json::Value> = items
                .into_iter()
                .filter(|i| shelf.contains(&i.appid))
                .map(|i| serde_json::json!({"appid": i.appid, "added": i.date_added}))
                .collect();
            (
                StatusCode::OK,
                // private: per-friend, never shared-cacheable. The ONLY copy of the overlap
                // lives in the friend's own browser for an hour (spec §4).
                [(header::CACHE_CONTROL, "private, max-age=3600")],
                Json(serde_json::json!({"items": items})),
            )
                .into_response()
        }
        Err(_) => StatusCode::SERVICE_UNAVAILABLE.into_response(),
    }
}
```

(d) **Route**, next to the owned one:

```rust
        .route(
            "/api/l/{token}/steam/wishlist/{steamid}",
            get(handle_steam_wishlist_proxy),
        )
```

(e) Update the module doc comment at `lib.rs:6` to list the new route.

- [ ] **Step 5: Run.** Start DynamoDB-local the way the suite expects (`moto_server` on `:8000`; see `store_or_skip`'s doc comment for the env var). Then run `cargo test -p public-api` ⇒ all green, **and confirm the new tests actually RAN, not skipped:** each prints its skip reason when the store is absent, so check with `cargo test -p public-api wishlist -- --nocapture 2>&1 | grep -c skip` ⇒ `0`. Then run `cargo clippy -p public-api --all-targets -- -D warnings`.

- [ ] **Step 6: Commit.** `git add crates/public-api && git commit -S -m "⭐ public-api: token-scoped wishlist proxy — live, intersected, nothing at rest"`

---

### Task 3: web client `steamWishlistForLink`

**Files:** Modify `web/src/api.ts` (after `steamOwnedForLink` ~L870); Test `web/src/api.test.ts`.

**Interfaces:**
- Produces: `export type WishItem = { appid: number; added: number }` and `export async function steamWishlistForLink(token: string, steamid: string): Promise<WishItem[]>`. It throws `FetchFailed` on network error or any non-2xx.

- [ ] **Step 1: Failing tests** (follow the file's existing `steamOwnedForLink` tests for the `fetch` stub pattern):

```ts
describe("steamWishlistForLink", () => {
  it("returns the items on 200", async () => {
    vi.stubGlobal("fetch", vi.fn().mockResolvedValue(new Response(
      JSON.stringify({ items: [{ appid: 420, added: 1678000000 }] }), { status: 200 })));
    await expect(steamWishlistForLink("tok", "76561198000000001"))
      .resolves.toEqual([{ appid: 420, added: 1678000000 }]);
    expect(fetch).toHaveBeenCalledWith("/api/l/tok/steam/wishlist/76561198000000001");
  });
  it("throws FetchFailed on 503", async () => {
    vi.stubGlobal("fetch", vi.fn().mockResolvedValue(new Response("", { status: 503 })));
    await expect(steamWishlistForLink("tok", "1")).rejects.toBeInstanceOf(FetchFailed);
  });
  it("throws FetchFailed on network error", async () => {
    vi.stubGlobal("fetch", vi.fn().mockRejectedValue(new TypeError("offline")));
    await expect(steamWishlistForLink("tok", "1")).rejects.toBeInstanceOf(FetchFailed);
  });
});
```

- [ ] **Step 2: Run** `cd web && npx vitest run src/api.test.ts` ⇒ FAIL (not exported).
- [ ] **Step 3: Implement:**

```ts
export type WishItem = { appid: number; added: number };

/** ⭐ token-scoped wishlist overlap (spec-wishing-well §3). Server already intersected
 *  with this link's live games. Any failure throws FetchFailed — callers render it SILENT. */
export async function steamWishlistForLink(token: string, steamid: string): Promise<WishItem[]> {
  let response: Response;
  try {
    response = await fetch(`/api/l/${token}/steam/wishlist/${encodeURIComponent(steamid)}`);
  } catch {
    throw new FetchFailed();
  }
  if (!response.ok) throw new FetchFailed();
  const data = (await response.json()) as { items?: WishItem[] };
  return data.items ?? [];
}
```

- [ ] **Step 4: Run** ⇒ PASS. **Step 5: Commit** `⭐ web: steamWishlistForLink`.

---

### Task 4: the ⭐ pill in `GameGrid` + `formatSince`

**Files:** Create `web/src/wishlist.ts`, `web/src/wishlist.test.ts`; Modify `web/src/friend/GameGrid.tsx`, `web/src/friend/GameGrid.test.tsx`.

**Interfaces:**
- Produces: `export function formatSince(addedEpochSecs: number): string | null` (e.g. `1678000000` ⇒ `"mar 2023"`; `0` ⇒ `null`). New optional `GameGrid` prop `wished?: Map<number, number>` (appid ⇒ added epoch secs).

- [ ] **Step 1: Failing tests.** `wishlist.test.ts`:

```ts
import { formatSince } from "./wishlist";
it("formats UTC month + year, lowercase", () => {
  expect(formatSince(1678000000)).toBe("mar 2023"); // 2023-03-05T07:06:40Z
});
it("uses UTC, not local time, at a month boundary", () => {
  expect(formatSince(1680307200)).toBe("apr 2023"); // 2023-04-01T00:00:00Z
});
it("returns null for a missing date", () => {
  expect(formatSince(0)).toBeNull();
});
```

In `GameGrid.test.tsx` (reuse its `makeGame`):

```ts
it("stars a wishlisted game with its since-date as the accessible name", () => {
  render(<GameGrid games={[makeGame({ id: "1", title: "Portal", steam_app_id: 420 })]}
    wished={new Map([[420, 1678000000]])} onDetail={() => {}} />);
  const pill = screen.getByText("⭐ on your wishlist");
  expect(pill).toHaveAttribute("title", "on your wishlist since mar 2023");
  expect(pill).toHaveAccessibleName("on your wishlist since mar 2023");
});
it("owned wins: no star on an owned game", () => {
  render(<GameGrid games={[makeGame({ id: "1", title: "Portal", steam_app_id: 420 })]}
    owned={new Set([420])} wished={new Map([[420, 1678000000]])} onDetail={() => {}} />);
  expect(screen.getByText(/you own this/i)).toBeInTheDocument();
  expect(screen.queryByText("⭐ on your wishlist")).not.toBeInTheDocument();
});
it("never stars a ghost", () => {
  render(<GameGrid curated games={[makeGame({ id: "1", title: "Portal", steam_app_id: 420, gone: true })]}
    wished={new Map([[420, 1678000000]])} onDetail={() => {}} />);
  expect(screen.queryByText("⭐ on your wishlist")).not.toBeInTheDocument();
});
it("a missing date still stars, without a since", () => {
  render(<GameGrid games={[makeGame({ id: "1", title: "Portal", steam_app_id: 420 })]}
    wished={new Map([[420, 0]])} onDetail={() => {}} />);
  expect(screen.getByText("⭐ on your wishlist")).toHaveAttribute("title", "on your wishlist");
});
```

- [ ] **Step 2: Run** `npx vitest run src/wishlist.test.ts src/friend/GameGrid.test.tsx` ⇒ FAIL.
- [ ] **Step 3: Implement.** `wishlist.ts`:

```ts
// ⭐ the wishing well — pure helpers (docs/spec-wishing-well.md). No I/O here.

/** "mar 2023" from epoch seconds, in UTC (a wishlist date is a calendar fact, not local). */
export function formatSince(addedEpochSecs: number): string | null {
  if (!addedEpochSecs) return null;
  return new Date(addedEpochSecs * 1000)
    .toLocaleString("en-US", { month: "short", year: "numeric", timeZone: "UTC" })
    .toLowerCase();
}
```

`GameGrid.tsx`: add the prop to `GameGridProps` with a doc comment (`/** ⭐ appid ⇒ added epoch secs, already intersected server-side. Owned wins; ghosts never star. */ wished?: Map<number, number>;`), destructure it, and compute beside `youOwnThis`:

```tsx
        const wishedAt =
          !youOwnThis && game.gone !== true && game.steam_app_id !== null
            ? wished?.get(game.steam_app_id)
            : undefined;
        const wishLabel =
          wishedAt === undefined
            ? null
            : (() => {
                const since = formatSince(wishedAt);
                return since ? `on your wishlist since ${since}` : "on your wishlist";
              })();
```

and render right after the `youOwnThis` pill:

```tsx
            {wishLabel !== null && (
              <span
                className="rounded bg-amber-900 px-2 py-0.5 text-xs text-amber-200"
                title={wishLabel}
                aria-label={wishLabel}
              >
                ⭐ on your wishlist
              </span>
            )}
```

(Colour: the owned pill is blue on dark, and amber is its warm sibling. If `DESIGN.md` names a chip palette that forbids raw Tailwind colours, follow `DESIGN.md` and note the choice in the commit.)

- [ ] **Step 4: Run** ⇒ PASS, plus the whole `GameGrid.test.tsx`. **Step 5: Commit** `⭐ web: the wishlist star — owned wins, ghosts never star`.

---

### Task 5: `LinkPage` — fetch, hold, float, count line

**Files:** Modify `web/src/friend/LinkPage.tsx`, `web/src/friend/LinkPage.test.tsx`; add `floatRanks` to `web/src/wishlist.ts` (+ test).

**Interfaces:**
- Consumes: `steamWishlistForLink`, `WishItem` (Task 3); `formatSince`, `GameGrid.wished` (Task 4).
- Produces: `export function floatRanks(ids: string[], starred: Set<string>, rand: () => number): Map<string, number>`. Starred ids get ranks `0..k-1` in shuffled order, the rest get `k..n-1` in shuffled order. `rand` is injectable so the partition test is deterministic.

**Behaviour contract (from spec §2, §5 and the family review):**
1. The wishlist fetch starts when a Steam identity is present: on the OpenID-return path (with the fragment's steamid) or the restore path (with the stored steamid). It runs **in parallel** with `steamOwnedForLink`. State: `type Wish = { kind: "none" } | { kind: "pending" } | { kind: "done"; wished: Map<number, number> }`. On failure ⇒ `done` with an empty map (**silent**). It never touches `steamError` or `steamPrivate`.
2. **The hold (Lilith's layout-jump fix):** on an **open-shelf** link, when the link has loaded **and** `wish.kind === "pending"`, keep rendering the existing loading view for at most **400 ms** (`WISH_HOLD_MS = 400`). After the cap, render anyway. Curated links never hold. No identity means no hold.
3. **The freeze:** the shuffle ranks are computed **once**, at the first open-shelf render that is not held, with `floatRanks(ids, starredIds, Math.random)`. `starredIds` is the ids of games whose `steam_app_id` is in `wished` **and not in `ownedSet`**. If the wishlist lands later, **ranks are not recomputed**. Late stars appear and nothing moves.
4. The count line renders above the grid when `N ≥ 1`, where N is the number of **live, non-owned** games on the current shelf that are starred. It shows on both curated and open links. Absent at 0.
5. Disconnect (`clearIdentity` handler ~L480) also sets `wish` to `{ kind: "none" }`.

- [ ] **Step 1: Failing tests.** `wishlist.test.ts`:

```ts
import { floatRanks } from "./wishlist";
it("floatRanks puts every starred id before every unstarred id", () => {
  let seed = 7; const rand = () => ((seed = (seed * 16807) % 2147483647) / 2147483647);
  const ids = ["a", "b", "c", "d", "e", "f"];
  const r = floatRanks(ids, new Set(["e", "b"]), rand);
  const maxStar = Math.max(r.get("e")!, r.get("b")!);
  const minRest = Math.min(...["a", "c", "d", "f"].map((x) => r.get(x)!));
  expect(maxStar).toBeLessThan(minRest);
  expect(new Set(r.values()).size).toBe(6);
});
it("floatRanks with nothing starred is a plain shuffle of all ids", () => {
  const r = floatRanks(["a", "b", "c"], new Set(), Math.random);
  expect([...r.values()].sort()).toEqual([0, 1, 2]);
});
```

`LinkPage.test.tsx`: add `steamWishlistForLink: vi.fn()` to the `vi.mock("../api")` factory and to the import list. In the existing `beforeEach` (or a new one inside a `describe("wishing well")`), default it to `vi.mocked(steamWishlistForLink).mockResolvedValue([])` so older tests keep passing. Then:

```tsx
describe("wishing well", () => {
  const stored = { steamid: "76561198000000001", persona: "Alice", owned: [], fetched_at: 0 };
  const shelf = (n: number) => Array.from({ length: n }, (_, i) =>
    makeGame({ id: String(i), title: `G${i}`, steam_app_id: 1000 + i }));

  it("stars a wishlisted game and shows the count line (N ≥ 1)", async () => {
    vi.mocked(fetchLink).mockResolvedValue({ ...baseLink, games: shelf(5) });
    vi.mocked(loadIdentity).mockReturnValue(stored);
    vi.mocked(steamWishlistForLink).mockResolvedValue([
      { appid: 1003, added: 1678000000 }, { appid: 1001, added: 1678000000 }]);
    renderLinkPage();
    await waitFor(() => expect(screen.getByText("⭐ 2 of these are on your wishlist")).toBeInTheDocument());
    expect(screen.getAllByText("⭐ on your wishlist")).toHaveLength(2);
  });

  it("singular count copy", async () => {
    vi.mocked(fetchLink).mockResolvedValue({ ...baseLink, games: shelf(3) });
    vi.mocked(loadIdentity).mockReturnValue(stored);
    vi.mocked(steamWishlistForLink).mockResolvedValue([{ appid: 1002, added: 1678000000 }]);
    renderLinkPage();
    await waitFor(() => expect(screen.getByText("⭐ 1 of these is on your wishlist")).toBeInTheDocument());
  });

  it("no count line at 0 — and none on failure (silent, no error text)", async () => {
    vi.mocked(fetchLink).mockResolvedValue({ ...baseLink, games: shelf(3) });
    vi.mocked(loadIdentity).mockReturnValue(stored);
    vi.mocked(steamWishlistForLink).mockRejectedValue(new FetchFailed());
    renderLinkPage();
    await waitFor(() => expect(screen.getByText("G0")).toBeInTheDocument());
    expect(screen.queryByText(/of these (are|is) on your wishlist/)).not.toBeInTheDocument();
    expect(screen.queryByText(/wishlist/i)).not.toBeInTheDocument();
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
  });

  it("open shelf: starred cards render first", async () => {
    vi.mocked(fetchLink).mockResolvedValue({ ...baseLink, games: shelf(8) });
    vi.mocked(loadIdentity).mockReturnValue(stored);
    vi.mocked(steamWishlistForLink).mockResolvedValue([{ appid: 1006, added: 1678000000 }]);
    renderLinkPage();
    await waitFor(() => expect(screen.getByText("⭐ on your wishlist")).toBeInTheDocument());
    const titles = screen.getAllByRole("heading", { level: 3 }).map((h) => h.textContent);
    expect(titles[0]).toBe("G6");
  });

  it("curated: order is ben's, stars do not move, count line still shows", async () => {
    const games = shelf(4);
    vi.mocked(fetchLink).mockResolvedValue({ ...baseLink, curated: true, games });
    vi.mocked(loadIdentity).mockReturnValue(stored);
    vi.mocked(steamWishlistForLink).mockResolvedValue([{ appid: 1003, added: 1678000000 }]);
    renderLinkPage();
    await waitFor(() => expect(screen.getByText("⭐ 1 of these is on your wishlist")).toBeInTheDocument());
    const titles = screen.getAllByRole("heading", { level: 3 }).map((h) => h.textContent);
    expect(titles).toEqual(["G0", "G1", "G2", "G3"]);
  });

  it("a wishlist that lands AFTER the hold cap adds stars without moving any card", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    let resolveWish!: (v: { appid: number; added: number }[]) => void;
    vi.mocked(fetchLink).mockResolvedValue({ ...baseLink, games: shelf(8) });
    vi.mocked(loadIdentity).mockReturnValue(stored);
    vi.mocked(steamWishlistForLink).mockReturnValue(new Promise((r) => { resolveWish = r; }));
    renderLinkPage();
    await act(async () => { vi.advanceTimersByTime(450); });
    await waitFor(() => expect(screen.getByText("G0")).toBeInTheDocument());
    const before = screen.getAllByRole("heading", { level: 3 }).map((h) => h.textContent);
    await act(async () => { resolveWish([{ appid: 1007, added: 1678000000 }]); });
    await waitFor(() => expect(screen.getByText("⭐ on your wishlist")).toBeInTheDocument());
    const after = screen.getAllByRole("heading", { level: 3 }).map((h) => h.textContent);
    expect(after).toEqual(before);
    vi.useRealTimers();
  });

  it("wishlist failure never blocks owned", async () => {
    vi.mocked(fetchLink).mockResolvedValue({ ...baseLink, games: shelf(2) });
    vi.mocked(consumeReturnFragment).mockReturnValue({ steamid: "76561198000000001", persona: "Alice" });
    vi.mocked(steamOwnedForLink).mockResolvedValue([1000]);
    vi.mocked(steamWishlistForLink).mockRejectedValue(new FetchFailed());
    renderLinkPage();
    await waitFor(() => expect(screen.getByText(/you own this/i)).toBeInTheDocument());
  });
});
```

(If `baseLink` in the file already has a `curated` field, spread over it as written. If the heading level for card titles is not 3, match `GameGrid`'s `<h3>`, which is what it is today.)

- [ ] **Step 2: Run** `npx vitest run src/wishlist.test.ts src/friend/LinkPage.test.tsx` ⇒ the new ones FAIL, and the old ones still pass.

- [ ] **Step 3: Implement `floatRanks`** in `wishlist.ts`:

```ts
/** Per-visit shelf ranks with the ⭐ float (spec §2.2): starred ids first, each group
 *  Fisher–Yates-shuffled on its own, so the rummage survives inside both groups. */
export function floatRanks(ids: string[], starred: Set<string>, rand: () => number): Map<string, number> {
  const shuffle = (xs: string[]) => {
    for (let i = xs.length - 1; i > 0; i--) {
      const j = Math.floor(rand() * (i + 1));
      [xs[i], xs[j]] = [xs[j]!, xs[i]!];
    }
    return xs;
  };
  const ordered = [
    ...shuffle(ids.filter((id) => starred.has(id))),
    ...shuffle(ids.filter((id) => !starred.has(id))),
  ];
  return new Map(ordered.map((id, pos) => [id, pos]));
}
```

- [ ] **Step 4: Implement in `LinkPage.tsx`:**
  1. Import `steamWishlistForLink` and `floatRanks`. Add `const WISH_HOLD_MS = 400;` and the `Wish` type at module scope.
  2. State: `const [wish, setWish] = useState<Wish>({ kind: "none" });` and `const [holdOver, setHoldOver] = useState(false);`.
  3. Add a helper inside the component: `const loadWish = (steamid: string) => { setWish({ kind: "pending" }); steamWishlistForLink(token!, steamid).then((items) => setWish({ kind: "done", wished: new Map(items.map((i) => [i.appid, i.added])) })).catch(() => setWish({ kind: "done", wished: new Map() })); };`. Guard it with the steam effect's `cancelled` flag: pass `cancelled` through a closure, or check a ref, so an unmounted page never calls `setWish`.
  4. In the steam effect: restore path, after `setSteamIdentity(stored)`, add `if (stored) loadWish(stored.steamid);`. Return path: call `loadWish(steamid)` **before** `void fetchOwned();`, so the two run in parallel.
  5. The hold timer: `useEffect(() => { if (view.kind !== "loaded" || view.data.curated === true || wish.kind !== "pending") return; const t = setTimeout(() => setHoldOver(true), WISH_HOLD_MS); return () => clearTimeout(t); }, [view.kind, wish.kind]);`. Compute `const holding = view.kind === "loaded" && view.data.curated !== true && wish.kind === "pending" && !holdOver;`.
  6. `const wishedMap = wish.kind === "done" ? wish.wished : undefined;`
  7. In the `shelfGames` memo: `if (holding) return [];` before the rank block, and replace the inline Fisher–Yates with `shuffleRanksRef.current = floatRanks(games.map((g) => g.id), starredIds, Math.random)`, where `starredIds` is built from `games`, `wishedMap` and `ownedSet` (non-owned, `steam_app_id` in `wishedMap`). Add `holding`, `wishedMap` and `ownedSet` to the deps. Ranks are still assigned only while `shuffleRanksRef.current === null`, which is the freeze. **Do not reset the ref when the wishlist lands.**
  8. The render gate: extend the existing `if (view.kind === "loading")` to `if (view.kind === "loading" || holding)`.
  9. The count line: `const starCount = shelfGames.filter((g) => g.gone !== true && g.steam_app_id !== null && !ownedSet.has(g.steam_app_id) && wishedMap?.has(g.steam_app_id)).length;`. Render it directly above `<GameGrid …/>` when `starCount >= 1`: `<p className="px-6 pt-2 text-sm text-ink-soft">⭐ {starCount} of these {starCount === 1 ? "is" : "are"} on your wishlist</p>`. The text must be a **single text node** so `getByText` matches: build the string first, `const line = \`⭐ ${starCount} of these ${starCount === 1 ? "is" : "are"} on your wishlist\`;`, then render `{line}`.
  10. Pass `wished={wishedMap}` to `<GameGrid>`.
  11. In the disconnect handler next to `clearIdentity();`, add `setWish({ kind: "none" });`.

- [ ] **Step 5: Run the whole web suite:** `cd web && npx vitest run && npx tsc --noEmit && npx eslint src` ⇒ green. **Step 6: Commit** `⭐ web: the wishing well — float, hold ≤400ms, late stars never move, count line at ≥1`.

---

### Task 6: prove the "nothing at rest" boundary, then the full gate

- [ ] **Step 1: The diff boundary.** Run `git diff --name-only origin/main...HEAD`. The printed list must contain **no** path under `crates/dynamo/`, `terraform/` or `crates/fulfillment/`, and no `iam-request-corpus.json`. Assert it rather than eyeballing it:

```bash
git diff --name-only origin/main...HEAD | tee /dev/stderr \
  | grep -E '^(crates/dynamo/|crates/fulfillment/|terraform/)' \
  && { echo "🔴 NOTHING-AT-REST BOUNDARY BROKEN"; exit 1; } || echo "✅ boundary holds"
```

Before trusting that, run the positive control: `printf 'terraform/x.tf\n' | grep -E '^(crates/dynamo/|crates/fulfillment/|terraform/)'` ⇒ rc 0. That proves the pattern can see a hit.
- [ ] **Step 2: Full workspace.** `cargo fmt --all -- --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace` (with DynamoDB-local up) **and** `cd web && npx vitest run && npx tsc --noEmit && npx eslint src && npm run build`.
- [ ] **Step 3: IAM capture stays byte-identical:** `cargo test -p dynamo --test iam_capture` ⇒ PASS in default (assert) mode with **no** regeneration. The new proxy only uses store reads the public lambda already makes (`get_link`, `batch_get_games`, `list_listable_games`). This step proves that.

---

## Deploy (pounce step 11, after merge)

Full deploy per `terraform/README.md` → "Deploying as kitten". This changes lambda code **and** web, so it is not the web-only path. **Pre-register the plan shape from prod's last-deployed stamp, never from this diff** (a deploy ships prod→main, which may carry others' merges). The expected shape for this PR alone is **0 add / N change / 0 destroy**, where N is the lambda functions whose zip hash moved: `public-api` for certain, and every lambda that links `steam-client` if the build embeds it. Name N from the plan's own resource list before applying. Then run `deploy-web.sh` from the CI `web-dist` artifact of the merge commit.

## Post-deploy verification (pounce step 12)

1. `curl -sS "https://<prod-host>/api/l/<a live open-shelf token>/steam/wishlist/<a public id64 from the 2026-09-30 probe with items>"` ⇒ `200`, `cache-control: private, max-age=3600`. Every returned appid must be present in that link's `/api/l/<token>` `games[].steam_app_id`. Check this mechanically with jq set-difference ⇒ empty.
2. **Nothing at rest, measured:** `AWS_PROFILE=kitten-debug aws dynamodb get-item` for `pk=STEAMWISH#<id64>` **and** `pk=STEAMOWN#<id64>, sk=WISH` ⇒ no item. Also run a CloudWatch Logs Insights query over the public-api log group for the request window, filtering on one returned appid ⇒ 0 matches. Before that, run a positive control on the same query with a string known to be logged (the request path) ⇒ ≥1 match.
3. Load the page in a browser (Playwright) with a stored identity for that id64 and screenshot the stars and the count line.
