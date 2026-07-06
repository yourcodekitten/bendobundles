# Steam Integration Implementation Plan (OpenID + Ownership Badges + the Steam Foundation)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ben and gift-link friends sign in through Steam; the catalog shows `owned_by_ben` badges (admin) and "you own this" badges (friends) — plus this plan builds the shared Steam foundation (steam-client crate + appid mapper) that the modal plan consumes.

**Architecture:** A new `steam-client` crate (plain reqwest — deliberately NOT humble-client's wreq fingerprint machinery) provides OpenID assertion verification, GetOwnedGames (with privacy detection pinned to response shape), persona, and GetAppList. The appid mapper rides the existing sync: tier 1 flows `steam_app_id` straight off humble's tpk wire (78% coverage, HAR-proven), a post-walk pass does unique-exact-title matching, and an admin override endpoint wins over everything. Friend identity is browser-only (one shared localStorage key, same origin); Ben's steamid persists in a net-new `CONFIG#STEAM` item so sync stamps `owned_by_ben`. The owned-games proxy is token-scoped (never an open proxy through the key — review B1).

**Tech Stack:** Rust (axum, reqwest, aws-sdk-dynamodb, aws-sdk-ssm), wiremock + dynamodb-local tests, React 18 + TS + Vitest, Terraform.

**Specs:** `docs/superpowers/specs/2026-07-06-steam-integration-design.md` (primary) + the mapper section of `2026-07-06-game-detail-modal-design.md` §2 (the mapper builds HERE because this plan now runs before the modal — Ben's ordering, 2026-07-06). Read both first. Plan-format template: `docs/superpowers/plans/2026-07-06-self-claim.md`.

## Global Constraints

- All commits GPG-signed, authored `code kitten <yourcodekitten@gmail.com>`. Branch: `kitten/steam-integration` (after the self-claim feature merges; rebase on main).
- The Steam Web API key NEVER reaches the browser, a log line, or a response body. In Rust it lives in `SteamApiKey` (redacted `Debug`, same discipline as humble-client's `SessionCookie` :13).
- A steamid is trusted only after the server-side `check_authentication` round-trip (never parsed off a URL alone).
- Ownership is ADVISORY: no claim/auth decision ever reads `owned_by_ben` or a badge.
- `SteamError` enum is exhaustive at every match site — no `_` catch-all (crate convention).
- Steam Web-API budget: ≤2 keyed calls per friend connect (persona + owned on cache miss); server cache 24h-fresh/7d-TTL; sync refresh for Ben once per run.
- Box can't build boring from scratch — per-crate `cargo test -p`, CI is the full builder.

---

### Task 1: steam-client crate — scaffold, key/id newtypes, owned-games (privacy-pinned), persona, vanity

**Files:**
- Create: `crates/steam-client/Cargo.toml`, `crates/steam-client/src/lib.rs`, `crates/steam-client/tests/client_test.rs`
- Modify: workspace `Cargo.toml` members list

**Interfaces (produced — Tasks 2/3/6/8/9/10 and the modal plan consume these exactly):**

```rust
pub struct SteamApiKey(String);                    // ::new(String); Debug prints "SteamApiKey(REDACTED)"
#[derive(Debug, Clone, PartialEq, Eq)] pub struct SteamId64(pub String);
pub enum OwnedGames { Private, Games(Vec<u32>) }   // appids
pub struct Persona { pub name: String, pub avatar_url: Option<String> }
pub struct SteamClient { /* base_web_api: String, base_store: String, http: reqwest::Client, key: SteamApiKey */ }
impl SteamClient {
    pub fn new(web_api_base: &str, store_base: &str, key: SteamApiKey) -> Result<Self, SteamError>;
    pub async fn get_owned_games(&self, steamid: &SteamId64) -> Result<OwnedGames, SteamError>;
    pub async fn get_player_summary(&self, steamid: &SteamId64) -> Result<Persona, SteamError>;
    pub async fn resolve_vanity(&self, name: &str) -> Result<SteamId64, SteamError>;
}
#[derive(Debug, thiserror::Error)] pub enum SteamError {
    #[error("steam api http {0}")] Api(u16),
    #[error("network: {0}")] Network(String),
    #[error("parse: {0}")] Parse(String),
    #[error("rate limited")] RateLimited,       // 429
    #[error("bad api key")] KeyRejected,        // 401/403 from the keyed api
    #[error("no such vanity/steamid")] NotFound,
    #[error("openid verification failed: {0}")] OpenIdRejected(String), // used from Task 2
}
```

Bases are constructor params (wiremock needs them): prod wiring passes `https://api.steampowered.com` + `https://store.steampowered.com`.

- [ ] **Step 1: Failing tests** (`crates/steam-client/tests/client_test.rs`):

```rust
fn test_client(server: &wiremock::MockServer) -> steam_client::SteamClient {
    steam_client::SteamClient::new(&server.uri(), &server.uri(),
        steam_client::SteamApiKey::new("TESTKEY".into())).unwrap()
}

#[tokio::test]
async fn owned_games_public_returns_appids() {
    let server = wiremock::MockServer::start().await;
    wiremock::Mock::given(wiremock::matchers::method("GET"))
        .and(wiremock::matchers::path("/IPlayerService/GetOwnedGames/v0001/"))
        .and(wiremock::matchers::query_param("key", "TESTKEY"))
        .and(wiremock::matchers::query_param("steamid", "76561198000000001"))
        .and(wiremock::matchers::query_param("include_played_free_games", "1"))
        .respond_with(wiremock::ResponseTemplate::new(200).set_body_string(
            r#"{"response":{"game_count":2,"games":[{"appid":413150,"playtime_forever":100},{"appid":1273400,"playtime_forever":0}]}}"#,
        ))
        .mount(&server).await;
    let out = test_client(&server)
        .get_owned_games(&steam_client::SteamId64("76561198000000001".into())).await.unwrap();
    assert_eq!(out, steam_client::OwnedGames::Games(vec![413150, 1273400]));
}

#[tokio::test]
async fn owned_games_private_is_absent_game_count() {
    // M4 pin: privacy = response WITHOUT game_count. NOT an error, NOT empty.
    let server = wiremock::MockServer::start().await;
    wiremock::Mock::given(wiremock::matchers::method("GET"))
        .and(wiremock::matchers::path("/IPlayerService/GetOwnedGames/v0001/"))
        .respond_with(wiremock::ResponseTemplate::new(200).set_body_string(r#"{"response":{}}"#))
        .mount(&server).await;
    let out = test_client(&server)
        .get_owned_games(&steam_client::SteamId64("7656".into())).await.unwrap();
    assert_eq!(out, steam_client::OwnedGames::Private);
}

#[tokio::test]
async fn owned_games_zero_count_is_genuinely_empty() {
    let server = wiremock::MockServer::start().await;
    wiremock::Mock::given(wiremock::matchers::method("GET"))
        .and(wiremock::matchers::path("/IPlayerService/GetOwnedGames/v0001/"))
        .respond_with(wiremock::ResponseTemplate::new(200)
            .set_body_string(r#"{"response":{"game_count":0,"games":[]}}"#))
        .mount(&server).await;
    let out = test_client(&server)
        .get_owned_games(&steam_client::SteamId64("7656".into())).await.unwrap();
    assert_eq!(out, steam_client::OwnedGames::Games(vec![]));
}

#[tokio::test]
async fn key_rejection_and_rate_limit_are_typed() {
    let server = wiremock::MockServer::start().await;
    wiremock::Mock::given(wiremock::matchers::method("GET"))
        .and(wiremock::matchers::path("/IPlayerService/GetOwnedGames/v0001/"))
        .respond_with(wiremock::ResponseTemplate::new(403)).mount(&server).await;
    let out = test_client(&server).get_owned_games(&steam_client::SteamId64("x".into())).await;
    assert!(matches!(out, Err(steam_client::SteamError::KeyRejected)));
}

#[tokio::test]
async fn api_key_debug_is_redacted() {
    let k = steam_client::SteamApiKey::new("SECRET123".into());
    assert!(!format!("{k:?}").contains("SECRET123"));
}

#[tokio::test]
async fn persona_and_vanity_parse() {
    let server = wiremock::MockServer::start().await;
    wiremock::Mock::given(wiremock::matchers::path("/ISteamUser/GetPlayerSummaries/v0002/"))
        .respond_with(wiremock::ResponseTemplate::new(200).set_body_string(
            r#"{"response":{"players":[{"steamid":"7656","personaname":"bendoerr","avatarfull":"https://a/b.jpg"}]}}"#,
        )).mount(&server).await;
    wiremock::Mock::given(wiremock::matchers::path("/ISteamUser/ResolveVanityURL/v0001/"))
        .respond_with(wiremock::ResponseTemplate::new(200).set_body_string(
            r#"{"response":{"success":1,"steamid":"76561198000000001"}}"#,
        )).mount(&server).await;
    let c = test_client(&server);
    let p = c.get_player_summary(&steam_client::SteamId64("7656".into())).await.unwrap();
    assert_eq!(p.name, "bendoerr");
    let id = c.resolve_vanity("bendoerr").await.unwrap();
    assert_eq!(id, steam_client::SteamId64("76561198000000001".into()));
}
```

- [ ] **Step 2: Verify failure** — `cargo test -p steam-client 2>&1 | tail -5` → crate doesn't exist / compile FAIL.

- [ ] **Step 3: Implement.** `Cargo.toml`: `reqwest = { version = "0.12", default-features = false, features = ["rustls-tls", "json"] }`, `serde`, `serde_json`, `thiserror`, `tracing`, `tokio` (match workspace versions; add to workspace members). Core of `lib.rs`:

```rust
use serde::Deserialize;

pub struct SteamApiKey(String);
impl SteamApiKey {
    pub fn new(v: String) -> Self { Self(v) }
    fn expose(&self) -> &str { &self.0 }
}
impl std::fmt::Debug for SteamApiKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SteamApiKey(REDACTED)")
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SteamId64(pub String);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OwnedGames {
    /// "game details" privacy hides the library: the response carries NO `game_count` at all.
    /// Distinct from an empty library (`game_count: 0`) — spec M4; do NOT infer privacy from
    /// GetPlayerSummaries' communityvisibilitystate (profile visibility is a different setting).
    Private,
    Games(Vec<u32>),
}

#[derive(Deserialize)]
struct OwnedWire { response: OwnedResp }
#[derive(Deserialize)]
struct OwnedResp {
    game_count: Option<u64>,
    #[serde(default)]
    games: Vec<OwnedGame>,
}
#[derive(Deserialize)]
struct OwnedGame { appid: u32 }

impl SteamClient {
    pub async fn get_owned_games(&self, steamid: &SteamId64) -> Result<OwnedGames, SteamError> {
        let url = format!("{}/IPlayerService/GetOwnedGames/v0001/", self.base_web_api);
        let resp = self.http.get(url)
            .query(&[("key", self.key.expose()), ("steamid", &steamid.0),
                     ("include_played_free_games", "1"), ("format", "json")])
            .send().await.map_err(net)?;
        let wire: OwnedWire = keyed_json(resp).await?;
        match wire.response.game_count {
            None => Ok(OwnedGames::Private),
            Some(_) => Ok(OwnedGames::Games(wire.response.games.into_iter().map(|g| g.appid).collect())),
        }
    }
}

/// Shared keyed-endpoint status mapping: 429 → RateLimited, 401/403 → KeyRejected,
/// other non-2xx → Api(status), body → serde or Parse. The key never appears in any error string.
async fn keyed_json<T: serde::de::DeserializeOwned>(resp: reqwest::Response) -> Result<T, SteamError> {
    match resp.status().as_u16() {
        200 => resp.json::<T>().await.map_err(|e| SteamError::Parse(e.to_string())),
        429 => Err(SteamError::RateLimited),
        401 | 403 => Err(SteamError::KeyRejected),
        s => Err(SteamError::Api(s)),
    }
}
fn net(e: reqwest::Error) -> SteamError { SteamError::Network(e.to_string()) }
```

`get_player_summary` / `resolve_vanity` follow the same shape (`players[0]` → Persona{personaname, avatarfull}; ResolveVanityURL `success==1` → SteamId64 else `NotFound`).

- [ ] **Step 4: Verify** — `cargo test -p steam-client 2>&1 | tail -5` → PASS.
- [ ] **Step 5: Commit** — `git add crates/steam-client Cargo.toml && git commit -S -m "feat(steam-client): new crate — owned-games (privacy pinned to absent game_count), persona, vanity"`

---

### Task 2: steam-client — OpenID assertion verification

**Files:** Modify `crates/steam-client/src/lib.rs`; test `crates/steam-client/tests/client_test.rs`

**Interfaces:**
- Produces: `pub async fn verify_openid_assertion(&self, params: &[(String, String)], expected_return_to: &str) -> Result<SteamId64, SteamError>` — `params` are the query pairs Steam appended to the return URL; `expected_return_to` is the exact URL the endpoint reconstructed for itself. Also `pub fn steam_openid_redirect_url(realm: &str, return_to: &str) -> String` (pure helper the SPA/endpoint uses to build the login redirect). Task 10 consumes both.

- [ ] **Step 1: Failing tests:**

```rust
fn assertion_params(claimed: &str, return_to: &str) -> Vec<(String, String)> {
    vec![
        ("openid.ns".into(), "http://specs.openid.net/auth/2.0".into()),
        ("openid.mode".into(), "id_res".into()),
        ("openid.claimed_id".into(), claimed.into()),
        ("openid.identity".into(), claimed.into()),
        ("openid.return_to".into(), return_to.into()),
        ("openid.response_nonce".into(), "2026-07-06T00:00:00Znonce".into()),
        ("openid.assoc_handle".into(), "h".into()),
        ("openid.signed".into(), "signed,fields".into()),
        ("openid.sig".into(), "sig".into()),
    ]
}

#[tokio::test]
async fn openid_valid_assertion_returns_steamid() {
    let server = wiremock::MockServer::start().await;
    // check_authentication: Steam echoes is_valid:true in key-value form.
    wiremock::Mock::given(wiremock::matchers::method("POST"))
        .and(wiremock::matchers::path("/openid/login"))
        .and(wiremock::matchers::body_string_contains("openid.mode=check_authentication"))
        .respond_with(wiremock::ResponseTemplate::new(200)
            .set_body_string("ns:http://specs.openid.net/auth/2.0\nis_valid:true\n"))
        .mount(&server).await;
    let c = test_openid_client(&server); // SteamClient with store_base = server (openid lives on steamcommunity; make the openid base a constructor param or reuse store_base — pick ONE and document)
    let params = assertion_params(
        "https://steamcommunity.com/openid/id/76561198000000001",
        "https://bendobundles.com/api/steam/return?ctx=%2Fl%2Fabc",
    );
    let id = c.verify_openid_assertion(&params, "https://bendobundles.com/api/steam/return?ctx=%2Fl%2Fabc").await.unwrap();
    assert_eq!(id, steam_client::SteamId64("76561198000000001".into()));
}

#[tokio::test]
async fn openid_invalid_is_rejected() {
    // is_valid:false → OpenIdRejected.
    // (same mounting, body "is_valid:false\n") — assert matches!(out, Err(SteamError::OpenIdRejected(_)))
}

#[tokio::test]
async fn openid_wrong_claimed_id_shape_rejected_without_network() {
    // claimed_id "https://evil.example/openid/id/123" → OpenIdRejected BEFORE any HTTP call
    // (mount NOTHING; a network attempt would error differently).
}

#[tokio::test]
async fn openid_return_to_mismatch_rejected() {
    // params say return_to=https://evil.example/... but expected is bendobundles → OpenIdRejected.
}
```

- [ ] **Step 2: Verify failure.**
- [ ] **Step 3: Implement:**

```rust
/// Build the "Sign in through Steam" redirect. Pure; both surfaces use it via the return endpoint.
pub fn steam_openid_redirect_url(realm: &str, return_to: &str) -> String {
    let q = [
        ("openid.ns", "http://specs.openid.net/auth/2.0"),
        ("openid.mode", "checkid_setup"),
        ("openid.claimed_id", "http://specs.openid.net/auth/2.0/identifier_select"),
        ("openid.identity", "http://specs.openid.net/auth/2.0/identifier_select"),
        ("openid.return_to", return_to),
        ("openid.realm", realm),
    ];
    let qs = q.iter()
        .map(|(k, v)| format!("{k}={}", urlencoding::encode(v)))
        .collect::<Vec<_>>().join("&");
    format!("https://steamcommunity.com/openid/login?{qs}")
}

impl SteamClient {
    /// Verify a Steam OpenID assertion. Trust ladder (spec §2, ALL must hold):
    /// 1. `openid.return_to` in the params EXACTLY equals the URL we're handling (standard
    ///    OpenID rule — makes ctx tampering visible).
    /// 2. `openid.claimed_id` matches `https://steamcommunity.com/openid/id/<17-digit>`.
    /// 3. Steam's own `check_authentication` echo answers `is_valid:true` (this also enforces
    ///    single-use response_nonce server-side — the replay defense).
    pub async fn verify_openid_assertion(
        &self,
        params: &[(String, String)],
        expected_return_to: &str,
    ) -> Result<SteamId64, SteamError> {
        let get = |k: &str| params.iter().find(|(pk, _)| pk == k).map(|(_, v)| v.as_str());
        let return_to = get("openid.return_to").unwrap_or("");
        if return_to != expected_return_to {
            return Err(SteamError::OpenIdRejected("return_to mismatch".into()));
        }
        let claimed = get("openid.claimed_id").unwrap_or("");
        let id = claimed
            .strip_prefix("https://steamcommunity.com/openid/id/")
            .filter(|rest| rest.len() == 17 && rest.bytes().all(|b| b.is_ascii_digit()))
            .ok_or_else(|| SteamError::OpenIdRejected("claimed_id shape".into()))?;
        // Echo the assertion back with mode=check_authentication (form-encoded).
        let mut form: Vec<(String, String)> = params.to_vec();
        for (k, v) in &mut form {
            if k == "openid.mode" { *v = "check_authentication".into(); }
        }
        let resp = self.http.post(format!("{}/openid/login", self.base_openid))
            .form(&form).send().await.map_err(net)?;
        if resp.status().as_u16() != 200 {
            return Err(SteamError::Api(resp.status().as_u16()));
        }
        let body = resp.text().await.map_err(net)?;
        if body.lines().any(|l| l.trim() == "is_valid:true") {
            Ok(SteamId64(id.to_string()))
        } else {
            Err(SteamError::OpenIdRejected("is_valid:false".into()))
        }
    }
}
```

Add `base_openid` as a third constructor param (prod: `https://steamcommunity.com`); update Task 1's `new()` signature + tests accordingly (`SteamClient::new(web_api, store, openid, key)`). Add `urlencoding` dep (or hand-roll percent-encoding of the two URL params).

- [ ] **Step 4: Verify** — crate green.
- [ ] **Step 5: Commit** — `git commit -S -m "feat(steam-client): openid assertion verify — return_to exact-match, claimed_id shape, check_authentication echo"`

---

### Task 3: steam-client — GetAppList (mapper tier 2)

**Files:** Modify `crates/steam-client/src/lib.rs` + tests.

**Interfaces:** `pub async fn get_app_list(&self) -> Result<Vec<(u32, String)>, SteamError>` — `(appid, name)` pairs from `ISteamApps/GetAppList/v2` (keyless, web-api base). Task 6 consumes.

- [ ] **Step 1: Failing test** — mock `/ISteamApps/GetAppList/v2/` returning `{"applist":{"apps":[{"appid":413150,"name":"Stardew Valley"},{"appid":999,"name":"Stardew Valley"},{"appid":602320,"name":"Train Valley 2"}]}}`; assert the pairs come back verbatim (dup names INCLUDED — dedup is the mapper's job, with its unique-only rule).
- [ ] **Step 2: Verify failure.** **Step 3: Implement** (same `keyed_json` mapping; no key param needed). **Step 4: Verify green.**
- [ ] **Step 5: Commit** — `git commit -S -m "feat(steam-client): get_app_list for the title-match mapper tier"`

---

### Task 4: humble-client — `TpkWire.steam_app_id` flows to the order model

**Files:** Modify `crates/humble-client/src/model.rs` (TpkWire :31-42 + the public key-entry struct it converts into); test `crates/humble-client/tests/client_test.rs` + fixture `crates/humble-client/tests/fixtures/order_detail.json`.

**Interfaces:** the public order key entry (the type `handle_gift_choice` reads as `order.keys[…]` with `.machine_name`/`.redeemed`/`.redeemed_key_val`) gains `pub steam_app_id: Option<u32>`. Task 6 consumes. (Find the exact public struct: grep `pub keys` / the `Order` definition in model.rs; mirror how `redeemed_key_val` flows wire→public.)

- [ ] **Step 1: Failing test** — extend the fixture: give `stardew_valley_steam` a `"steam_app_id": 413150` and leave `already_revealed_steam` without one; assert `order.keys[0].steam_app_id == Some(413150)` and `order.keys[1].steam_app_id == None` in the existing order-parse test (HAR reference: `docs/superpowers/specs/captures/2026-07-06-steam/humble-order-tpk-sample-scrubbed.json` — 78% of steam tpks carry it).
- [ ] **Step 2/3/4:** fail → add `#[serde(default)] pub steam_app_id: Option<u32>` to `TpkWire` + the public struct + the conversion → green (whole crate).
- [ ] **Step 5: Commit** — `git commit -S -m "feat(humble-client): surface tpk steam_app_id (HAR-proven, 78% of steam tpks)"`

---

### Task 5: domain — `steam_app_id`, `appid_source`, `owned_by_ben` + merge_sync preservation

**Files:** Modify `crates/domain/src/lib.rs` (Game :24-60, merge_sync :150-202); tests in-file.

**Interfaces (Tasks 6-11 + modal plan consume):**

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AppidSource { Humble, Title, Manual }

// Game gains (all #[serde(default)] for backcompat):
pub steam_app_id: Option<u32>,
pub appid_source: Option<AppidSource>,   // None ⇔ steam_app_id is None (unmapped)
pub owned_by_ben: bool,
```

- [ ] **Step 1: Failing tests:**

```rust
#[test]
fn merge_sync_preserves_steam_fields_in_both_branches() {
    // Branch A (Available — "fresh wins entirely except hidden"): steam fields + owned_by_ben
    // must ALSO survive, or every sync clobbers the mapper's work and the admin's overrides.
    let mut existing = sample_game();
    existing.status = GameStatus::Available;
    existing.steam_app_id = Some(413150);
    existing.appid_source = Some(AppidSource::Manual);
    existing.owned_by_ben = true;
    let fresh = sample_game(); // steam fields default: None/false
    let merged = merge_sync(Some(&existing), fresh).unwrap();
    assert_eq!(merged.steam_app_id, Some(413150));
    assert_eq!(merged.appid_source, Some(AppidSource::Manual));
    assert!(merged.owned_by_ben);

    // Branch B (Pending): same assertions with existing.status = GameStatus::Pending.
}

#[test]
fn old_game_json_without_steam_fields_deserializes() {
    // Copy an existing old-game JSON fixture from the current tests; assert
    // steam_app_id == None, appid_source == None, owned_by_ben == false.
}
```

- [ ] **Step 2/3/4:** fail → add the fields; in `merge_sync` BOTH branches carry them from `existing_game` (in the Available branch use struct-update carefully — `Game { hidden: existing.hidden, steam_app_id: existing.steam_app_id, appid_source: existing.appid_source, owned_by_ben: existing.owned_by_ben, ..fresh }`); fix every `Game { … }` literal workspace-wide (compiler-guided) → green + `cargo build` the dependent crates.
- [ ] **Step 5: Commit** — `git commit -S -m "feat(domain): steam_app_id/appid_source/owned_by_ben — app-owned, merge_sync-preserved both branches"`

---

### Task 6: fulfillment — the appid mapper (tier 1 in the walk, title pass after it)

**Files:** Modify `crates/fulfillment/src/lib.rs` (the order-walk Game construction ~:1261-1330; new `map_missing_appids` pass called from `run_sync` after the walk, before/alongside `discover_choice_games` :1334); `Deps` gains `steam: Option<Arc<steam_client::SteamClient>>` (Option: sync must run keyless if the param is absent); `crates/fulfillment/src/main.rs` wiring in Task 12. Test: `crates/fulfillment/tests/handler_test.rs`.

**Interfaces:** none new beyond the pass; produces mapped `Game.steam_app_id` in the store. Coverage log line: `steam appid mapping: mapped=<n> unmapped=<n> manual=<n>` once per sync.

**Behavior (spec modal-§2):**
- In the walk's wire→Game conversion: `steam_app_id: tpk.steam_app_id, appid_source: tpk.steam_app_id.map(|_| AppidSource::Humble)` — tier 1 is free. (`merge_sync` preserves existing manual values because the mapper's fresh value only lands through the SAME preservation rule — verify: preservation must prefer EXISTING when `existing.appid_source == Some(Manual)`, else prefer FRESH when fresh is `Some`. Adjust Task 5's merge rule to: manual-existing wins; otherwise fresh `Some` wins over existing; otherwise keep existing. Add a merge test for "humble-sourced fresh value updates a stale title-sourced existing".)
- `map_missing_appids(deps)`: list games (`list_all_games`), collect steam-keytype games with `steam_app_id == None` (skip `appid_source == Some(Manual)` — a cleared override is `None/None` so it participates); if none, log + return WITHOUT fetching the app list (lazy). Else `get_app_list()`, build `name_lower → Vec<appid>`, resolve only UNIQUE exact matches (normalized: lowercase + trim + strip `™`/`®`), write via a guarded update (mirror `set_game_hidden`'s conditional-write pattern — never clobber status/claim mid-claim), `appid_source = Title`. Ambiguous/missing → left unmapped, counted.
- 429/network failure from `get_app_list` → log + skip the pass this run (never fail the sync).

- [ ] **Step 1: Failing tests** — three moto+wiremock tests: (a) walk carries tier-1 appid onto the stored game; (b) title pass maps a unique name, leaves a duplicate name unmapped; (c) manual-sourced game untouched by both. Mount GetAppList on the wiremock server; build `Deps` with the steam client pointed at it (extend the test harness's Deps constructor with the new field).
- [ ] **Step 2/3/4:** fail → implement → whole crate green.
- [ ] **Step 5: Commit** — `git commit -S -m "feat(fulfillment): appid mapper — tier-1 tpk flow-through + lazy unique-exact-title pass + coverage log"`

---

### Task 7: dynamo — CONFIG#STEAM, STEAMOWN cache, guarded owned-stamp write

**Files:** Modify `crates/dynamo/src/schema.rs` + `crates/dynamo/src/lib.rs`; test `crates/dynamo/tests/store_test.rs`.

**Interfaces (Tasks 8/9/10 consume):**

```rust
pub async fn put_steam_identity(&self, steamid: &str) -> Result<(), StoreError>;      // pk=CONFIG#STEAM sk=META
pub async fn get_steam_identity(&self) -> Result<Option<String>, StoreError>;
pub async fn delete_steam_identity(&self) -> Result<(), StoreError>;
pub async fn put_steam_owned(&self, steamid: &str, appids: &[u32], now_epoch: i64) -> Result<(), StoreError>; // pk=STEAMOWN#<id>, ttl=now+7d
pub async fn get_steam_owned(&self, steamid: &str) -> Result<Option<(Vec<u32>, i64)>, StoreError>; // (appids, fetched_at)
pub async fn set_game_owned_by_ben(&self, id: &str, owned: bool) -> Result<OwnedWrite, StoreError>; // mirror set_game_hidden's guarded write + Written/NotFound/Contested enum
```

- [ ] **Step 1: Failing tests** — config round-trip (put/get/delete); owned cache round-trip incl. `fetched_at` and the DDB `ttl` attribute present (= now+7d, reuse how SESSION items write `ttl` — schema :143); `set_game_owned_by_ben` flips the flag on an available game, `Contested` on a pending one (copy the set_game_hidden test shape).
- [ ] **Step 2/3/4:** fail → implement (CONFIG/STEAMOWN items follow SYNC#STATE's put/get shape :126-138; the guarded stamp is a structural copy of `set_game_hidden` :894 with `owned_by_ben` in place of `hidden`) → green.
- [ ] **Step 5: Commit** — `git commit -S -m "feat(dynamo): CONFIG#STEAM identity, STEAMOWN 7d-ttl cache, guarded owned_by_ben stamp"`

---

### Task 8: fulfillment — `owned_by_ben` sync stamping (M1 semantics)

**Files:** Modify `crates/fulfillment/src/lib.rs` (new `refresh_ben_ownership(deps)` called from `run_sync` after the mapper pass); test `handler_test.rs`.

**Behavior (spec §3, M1):** read `get_steam_identity()` — absent ⇒ skip silently. Present ⇒ `get_owned_games(ben)`:
- `Ok(Games(appids))` → `put_steam_owned` + stamp: for every game with a `steam_app_id`, `set_game_owned_by_ben(id, appids.contains(appid))` (only write when the value CHANGES — read the game list once, diff, write the delta).
- `Ok(Private)` → **keep prior stamps**, log `steam owned refresh skipped: ben library reads private`, `ping` ONCE ("your steam 'game details' privacy or the key's account changed — owned badges are frozen until fixed") — dedupe the ping via a marker on SyncState or a simple "only ping when the previous run succeeded" check (read STEAMOWN's presence as the previous-success signal; document the choice inline).
- `Err(_)` → keep prior stamps, log, no ping (transient).
- Disconnect (config absent but stamps exist): clear stamps on the next run (diff handles it — absent identity ⇒ skip; spec says stamps clear via "next successful refresh" after reconnect, and delete_steam_identity's handler (Task 9) also kicks off a clear — implement clear-on-disconnect in the HANDLER (admin-api sets all owned_by_ben=false via the guarded write? NO — that's O(catalog) HTTP-path writes). Simplest honest behavior: handler deletes config; stamps go stale-but-frozen; the admin UI hides badges when no identity is connected (Task 11 checks identity presence). Document in code.)

- [ ] **Step 1: Failing tests** — (a) successful fetch stamps intersect + unstamps disjoint; (b) `Private` keeps stamps + logs; (c) transient error keeps stamps. Drive via the sync entry with wiremock GetOwnedGames variants.
- [ ] **Step 2/3/4:** fail → implement → green.
- [ ] **Step 5: Commit** — `git commit -S -m "feat(fulfillment): owned_by_ben stamping — recompute only on successful non-private fetch (spec M1)"`

---

### Task 9: admin-api — identity endpoints, override endpoint, admin owned proxy, view fields

**Files:** Modify `crates/admin-api/src/lib.rs` (+ `main.rs` steam wiring); test `api_test.rs`.

**Interfaces (Task 11 web consumes):**
- `POST /admin/api/steam/identity` `{steamid: string}` → `{ok:true}` (validates 17-digit; `put_steam_identity`).
- `DELETE /admin/api/steam/identity` → `{ok:true}` (`delete_steam_identity`).
- `GET /admin/api/steam/identity` → `{steamid: string|null}` (UI needs to know state).
- `GET /admin/api/steam/owned/:steamid` → `{appids: number[]} | {private: true}` — session-guarded proxy: serve `get_steam_owned` if fresh (≤24h via `fetched_at`), else `get_owned_games` + cache + serve; `Private` → `{private:true}`.
- `POST /admin/api/games/:id/steam-app-id` `{app_id: number|null}` → `{ok:true}` — null clears (`steam_app_id=None, appid_source=None` — auto-resolution reruns next sync); Some sets `appid_source=Manual`. Guarded write (new `set_game_steam_app_id` on Store — same `set_game_hidden` pattern; add it here with its own moto test, or fold into Task 7 if the implementer prefers).
- `CatalogGameView` gains `steam_app_id: Option<u32>` + `owned_by_ben: bool`.
- AppState gains `steam: Option<Arc<SteamClient>>` (absent ⇒ steam endpoints return 503 `{"error":"steam not configured"}`).

- [ ] **Step 1: Failing tests** — identity round-trip via the endpoints; owned proxy serves cache-fresh without hitting the mock, fetches+caches on stale (wiremock expect(1)); override endpoint sets Manual / clears to None; catalog view carries the two new fields; ALL steam endpoints 401 without a session cookie (route in the protected block).
- [ ] **Step 2/3/4:** fail → implement (handlers follow the existing State/Path/Json axum shapes; doc-comment route list updated) → green.
- [ ] **Step 5: Commit** — `git commit -S -m "feat(admin-api): steam identity + owned proxy + appid override + catalog fields"`

---

### Task 10: public-api — OpenID return endpoint + token-scoped owned proxy + GameView field

**Files:** Modify `crates/public-api/src/lib.rs` (+ `main.rs` steam wiring); test `api_test.rs`.

**Interfaces (Task 11 consumes):**
- `GET /api/steam/login?ctx=<path>` → 302 to Steam's OpenID endpoint (`steam_openid_redirect_url(realm=BASE_URL, return_to=BASE_URL + "/api/steam/return?ctx=" + enc(ctx))`) — ctx validated against the allowlist HERE TOO (initiation-side, spec §3).
- `GET /api/steam/return?ctx=<path>&openid.*` → per the spec §3 failure contract:
  - ctx fails allowlist → 302 `/` (no fragment).
  - verify OK → `get_player_summary` (best-effort; failure ⇒ empty persona) → 302 `{ctx}#steam=<id64>&persona=<urlencoded>`.
  - verify rejected → 302 `{ctx}#steam_error=verify_failed`; steam unreachable → `{ctx}#steam_error=steam_unreachable`.
  - ctx allowlist: `^/l/[0-9a-f]{64}$` or exactly `/admin`.
- `GET /api/l/:token/steam/owned/:steamid` → `{appids:[…]} | {private:true}` — **link-token-scoped (review B1)**: resolve the link first; unknown token → the byte-identical 404; dead link (revoked/expired/exhausted) → 409 like the claim path refusals. Then cache-or-fetch exactly like the admin proxy.
- `GameView` gains `steam_app_id: Option<u32>` (badges compute client-side).

- [ ] **Step 1: Failing tests:**

```rust
#[tokio::test]
async fn owned_proxy_404s_without_live_link_byte_identical() {
    // unknown token → same body as any unknown-token 404 (no oracle).
}
#[tokio::test]
async fn owned_proxy_serves_cache_then_fetches_on_stale() { /* wiremock expect(1) discipline */ }
#[tokio::test]
async fn steam_return_valid_redirects_with_fragment() {
    // mock check_authentication is_valid:true + persona; GET /api/steam/return?ctx=%2Fl%2F<64hex>&openid...
    // → 302, Location = "/l/<64hex>#steam=7656...&persona=bendoerr"
}
#[tokio::test]
async fn steam_return_bad_ctx_redirects_root_no_fragment() { /* ctx=/evil → Location "/" */ }
#[tokio::test]
async fn steam_return_invalid_assertion_gets_steam_error_fragment() { /* is_valid:false → #steam_error=verify_failed */ }
#[tokio::test]
async fn game_view_carries_steam_app_id() { /* LinkView.games[0].steam_app_id present */ }
```

- [ ] **Step 2/3/4:** fail → implement → green. `expected_return_to` reconstruction: `format!("{base_url}/api/steam/return?ctx={}", urlencoding::encode(&ctx))` — MUST byte-match what the login endpoint emitted (one helper builds it, both endpoints call the helper). BASE_URL reaches the router via config (main.rs env — it already exists as an env var on the lambdas; thread it into AppState).
- [ ] **Step 5: Commit** — `git commit -S -m "feat(public-api): steam openid login/return (ctx allowlist, error fragments) + token-scoped owned proxy + GameView.steam_app_id"`

---

### Task 11: web — shared identity module, connect flows, badges

**Files:**
- Create: `web/src/steamIdentity.ts` (+ `web/src/steamIdentity.test.ts`)
- Modify: `web/src/api.ts`, `web/src/friend/LinkPage.tsx`, `web/src/friend/GameGrid.tsx`, `web/src/admin/Ops.tsx`, `web/src/admin/Catalog.tsx`
- Tests: co-located `.test.tsx` files (follow existing patterns)

**Interfaces:**

```typescript
// steamIdentity.ts — the ONE shared localStorage key (spec §3), both surfaces, same origin.
export type SteamIdentity = { steamid: string; persona: string; owned: number[]; fetched_at: number };
export function loadIdentity(): SteamIdentity | null;
export function saveIdentity(i: SteamIdentity): void;
export function clearIdentity(): void;                    // "not you? disconnect"
export function consumeReturnFragment(): { steamid: string; persona: string } | { error: string } | null;
  // parses #steam=…&persona=… or #steam_error=… off location.hash, then clears the hash.
export function beginConnect(ctx: string): void;          // location.href = `/api/steam/login?ctx=${encodeURIComponent(ctx)}`
```

`api.ts` additions: `steamOwnedForLink(token, steamid): Promise<number[] | 'private'>`, `adminSteamOwned(steamid)`, `adminSteamIdentity(): Promise<string|null>`, `adminSetSteamIdentity(steamid)`, `adminClearSteamIdentity()`, `adminSetAppId(gameId, appId: number|null)`; `GameView`/`AdminGame` gain `steam_app_id: number | null`, AdminGame gains `owned_by_ben: boolean`.

**Behavior:**
- **LinkPage:** on mount, `consumeReturnFragment()` — on steamid: fetch owned via `steamOwnedForLink`, `saveIdentity`, render persona chip + disconnect; on error: one polite line (`verify_failed` / `steam_unreachable` mapped to copy); else `loadIdentity()`. Connect button in the header when no identity (`beginConnect('/l/' + token)`). Pass `owned: Set<number>` down to GameGrid → "you own this" pill on cards whose `steam_app_id` is in the set. `private` result → the privacy message (spec §4 wording: "couldn't read your library — check Steam's *game details* privacy setting").
- **Ops:** connect/disconnect panel — same fragment consumption on `/admin` (AdminApp route mounts it), `adminSetSteamIdentity` after a verified connect (the admin extra step), current state from `adminSteamIdentity()`.
- **Catalog:** `owned_by_ben` badge chip; the self-claim confirm (from the self-claim feature) adds "you already own this on steam — sure?" to the ARMED label when `game.owned_by_ben` (badges hidden entirely when `adminSteamIdentity()` is null — the frozen-stamps caveat from Task 8).

- [ ] **Step 1: Failing tests** — steamIdentity round-trip + fragment parsing (`#steam=…`, `#steam_error=…`, hash cleared after consume); LinkPage shows connect button → renders pills for owned appids (mock fetch); privacy message on `'private'`; Ops connect persists identity (mock `adminSetSteamIdentity` called); Catalog renders owned badge.
- [ ] **Step 2/3/4:** fail → implement → `npx vitest run && npx tsc --noEmit` green.
- [ ] **Step 5: Commit** — `git commit -S -m "feat(web): steam connect (shared identity, one localStorage key) + ownership badges on both surfaces"`

---

### Task 12: terraform + lambda wiring — the key reaches the lambdas

**Files:**
- Modify: `terraform/aws-ssm.tf` (new param, container-only like humble_cookie :22-44), `terraform/aws-lambda.tf` (env + IAM for fulfillment AND public-api AND admin-api), `terraform/tf-variables.tf` if a var is needed
- Modify: `crates/fulfillment/src/main.rs`, `crates/public-api/src/main.rs`, `crates/admin-api/src/main.rs` (load key from SSM at startup — mirror fulfillment's `get_secret` helper :12-30; construct `SteamClient` with prod bases; absent/"UNSET" param ⇒ `None`, features off)

```hcl
# terraform/aws-ssm.tf — container only; value set out of band (same pattern as humble_cookie):
#   aws ssm put-parameter --name "/<param-prefix>/steam-web-api-key" --type SecureString --overwrite --value "<key>"
resource "aws_ssm_parameter" "steam_web_api_key" {
  name  = "/${local.param_prefix}/steam-web-api-key"   # match the file's actual prefix local
  type  = "SecureString"
  value = "UNSET"
  lifecycle { ignore_changes = [value] }
}
```

Env: `STEAM_KEY_PARAM = aws_ssm_parameter.steam_web_api_key.name` on all three lambdas; IAM: append the param ARN to each lambda's ssm-read statement (fulfillment :39, admin :130, public-api's equivalent). Match the file's existing locals/patterns exactly — read `aws-ssm.tf` + `aws-lambda.tf` fully before editing.

- [ ] **Step 1:** `terraform validate` + `terraform plan` (with deploy.tfvars) shows ONLY the new param + env/IAM changes, **0 destroy**.
- [ ] **Step 2:** main.rs wiring per above; `cargo build -p fulfillment -p public-api -p admin-api` green; startup log line says `steam client: configured|absent` (never the key).
- [ ] **Step 3: Commit** — `git commit -S -m "feat(infra): steam-web-api-key SSM container + env/IAM plumbing for all three lambdas"`

---

### Task 13: ship — CI, PR, DEPLOY, live check (fork-to-deployed; Ben not involved until live)

- [ ] **Step 1:** branch `kitten/steam-integration` from updated main; every commit signed (`git log --format='%h %GK %s' main..HEAD`).
- [ ] **Step 2:** workspace tests + web suite + tsc; push; `gh pr create`; CI green; merge per HR#1.
- [ ] **Step 3: put the real key into SSM** (value from `~/.secrets/steam-web-api-key.env`, NEVER echoed):
  `source ~/.secrets/steam-web-api-key.env && AWS_PROFILE=kitten-deploy aws ssm put-parameter --name "/<param-prefix>/steam-web-api-key" --type SecureString --overwrite --value "$STEAM_WEB_API_KEY"` — check kitten-deploy can put-parameter; if denied, this one step goes to Ben with the exact command.
- [ ] **Step 4: DEPLOY** per the proven procedure (self-claim plan Task 12 step 6): download lambda-zips, stage **fulfillment + public-api + admin-api**, recreate deploy.tfvars, targeted plan (THIS deploy includes the terraform additions — plan UN-targeted first to read the new param/IAM diff; expect adds, **0 destroy**), apply, verify CodeShas, shred. Deploy the SPA (build + s3 sync + CF invalidation).
- [ ] **Step 5: LIVE CHECK** (spec §6.5): sync → mapper coverage log line sane (mapped ≈78%+ of steam games); Ben's steamid arrives via HIS one connect click on /admin (this is the single Ben interaction, at the END, per his instruction — bundle it into the completion report); after his connect: sync → `owned_by_ben` stamps land; open a live gift link in the same browser → badges light up (cross-surface carry-over); a friend-door connect on a second browser; CloudWatch: no key material in any log.
- [ ] **Step 6: report ONCE** on discord with receipts.

---

## Self-Review Notes (applied)

- Spec coverage: §1 why-OpenID (documented, no task needed) · §2 crate (T1-3) + M4 privacy pin (T1) + M2 key-account (live check) · §3 dance both doors (T10/T9/T11), B1 token-scoping (T10), STEAMOWN cache (T7), CONFIG#STEAM (T7/T9), M1 stamping (T8), M3 failure contract (T10), BASE_URL return_to (T10) · §4 surfaces (T9/T10/T11) · §5 caveats (T8 ping, T11 privacy copy) · mapper from modal-spec §2 (T4/T5/T6 + override in T9).
- Type consistency: `SteamId64(pub String)` everywhere; `OwnedGames::{Private,Games}` from T1 consumed by T8/T9/T10; `AppidSource::{Humble,Title,Manual}` from T5 consumed by T6/T9; ctx allowlist regex identical in T10's two endpoints (one shared fn).
- Deliberate deviations from the spec text, with reasons: (a) the mapper lives HERE not in the modal plan (Ben's build re-order); (b) added `GET /api/steam/login` as the initiation endpoint (the spec has the SPA build the redirect — centralizing return_to construction server-side is what makes the exact-match check in T2 reliable); (c) disconnect freezes stamps + hides badges rather than mass-clearing (documented in T8; O(catalog) writes on an HTTP path is the alternative and it's worse).
- Placeholder scan: T8's ping-dedupe and T9's `set_game_steam_app_id` placement are flagged inline as implementer decisions with the options named — not TBDs.
