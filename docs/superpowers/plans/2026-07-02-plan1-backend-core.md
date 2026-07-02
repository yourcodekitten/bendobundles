# bendobundles Plan 1: Backend Core Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** The rust workspace foundation — domain types, the humble-client wrapper for the
unofficial API, and the dynamodb storage layer including the exactly-once claim transaction.

**Architecture:** Cargo workspace with three library crates (`domain`, `humble-client`,
`dynamo`). No lambdas yet (Plan 2). Everything here is testable without AWS credentials:
humble-client against wiremock fixtures, dynamo against dynamodb-local. One feature-gated probe
binary lets ben verify the unofficial API contract against his real session before Plan 2 builds
on it.

**Tech Stack:** rust (edition 2024), tokio, serde/serde_json, thiserror, reqwest (rustls-tls),
wiremock (dev), aws-sdk-dynamodb + aws-config, uuid, time. CI: github actions with a
dynamodb-local service container.

**Spec:** `docs/superpowers/specs/2026-07-02-bendobundles-design.md` — read it first; the
invariant that rules everything: *a humble key burns exactly once, and a burned key's gift URL is
never lost.*

## Global Constraints

- All commits GPG-signed (`git commit -S`), authored `code kitten <yourcodekitten@gmail.com>`.
- `cargo fmt --check` and `cargo clippy --workspace --all-targets -- -D warnings` must pass at every commit.
- No live humble calls in any test, ever. Fixtures + wiremock only. The probe binary is the ONLY
  thing that touches the real API, and only when a human runs it.
- The humble session cookie is a secret: never logged, never in fixtures, never in error messages.
  Wrap it so Debug/Display redact it.
- DynamoDB item shapes defined in Task 6 are the contract for Plan 2 — change them there and you
  break the lambdas' assumptions; don't.
- Unofficial-API base URL is configurable (tests point it at wiremock): default
  `https://www.humblebundle.com`.

## File Structure (locked by this plan)

```
Cargo.toml                      # workspace root
rust-toolchain.toml
.github/workflows/ci.yml
crates/
  domain/src/lib.rs             # types + transitions (no I/O, no deps beyond serde/time/thiserror)
  humble-client/src/lib.rs      # client + error taxonomy
  humble-client/src/model.rs    # wire types (serde) → domain conversion
  humble-client/src/bin/probe.rs# feature-gated live probe CLI
  humble-client/tests/fixtures/ # recorded JSON shapes
  humble-client/tests/client_test.rs
  dynamo/src/lib.rs             # Store: CRUD + claim transaction
  dynamo/src/schema.rs          # table/index names, key builders, (de)serialization
  dynamo/tests/store_test.rs    # integration tests vs dynamodb-local
```

---

### Task 1: Workspace scaffold + CI

**Files:**
- Create: `Cargo.toml`, `rust-toolchain.toml`, `.gitignore`, `.github/workflows/ci.yml`
- Create: `crates/domain/Cargo.toml`, `crates/domain/src/lib.rs` (stub)

**Interfaces:**
- Produces: a workspace where `cargo test --workspace` runs green and CI enforces fmt/clippy/test.

- [ ] **Step 1: Write the workspace files**

`Cargo.toml`:
```toml
[workspace]
resolver = "3"
members = ["crates/domain", "crates/humble-client", "crates/dynamo"]

[workspace.package]
edition = "2024"
license = "MIT"
publish = false

[workspace.dependencies]
serde = { version = "1", features = ["derive"] }
serde_json = "1"
thiserror = "2"
time = { version = "0.3", features = ["serde", "formatting", "parsing", "macros"] }
tokio = { version = "1", features = ["macros", "rt-multi-thread"] }
uuid = { version = "1", features = ["v4"] }
```

`rust-toolchain.toml`:
```toml
[toolchain]
channel = "stable"
components = ["rustfmt", "clippy"]
```

`.gitignore`:
```
/target
```

`crates/domain/Cargo.toml`:
```toml
[package]
name = "domain"
version = "0.1.0"
edition.workspace = true
publish.workspace = true

[dependencies]
serde.workspace = true
thiserror.workspace = true
time.workspace = true
```

`crates/domain/src/lib.rs`:
```rust
//! bendobundles domain types and state transitions. No I/O lives here.
```

Note: `humble-client` and `dynamo` are workspace members but don't exist until Tasks 3 and 6 —
temporarily list only `crates/domain` in `members` and extend the list in those tasks.

- [ ] **Step 2: Verify the workspace builds**

Run: `cd /home/code-kitten/bendobundles && cargo test --workspace && cargo fmt --check && cargo clippy --workspace --all-targets -- -D warnings`
Expected: PASS (0 tests, but compiles clean)

- [ ] **Step 3: Write CI**

`.github/workflows/ci.yml`:
```yaml
name: ci
on:
  pull_request:
  push:
    branches: [main]
permissions:
  contents: read
jobs:
  test:
    runs-on: ubuntu-latest
    services:
      dynamodb:
        image: amazon/dynamodb-local:latest
        ports: ["8000:8000"]
    env:
      DYNAMODB_LOCAL_URL: http://localhost:8000
    steps:
      - uses: actions/checkout@v4
        with:
          persist-credentials: false
      - uses: dtolnay/rust-toolchain@stable
        with:
          components: rustfmt, clippy
      - uses: Swatinem/rust-cache@v2
      - run: cargo fmt --check
      - run: cargo clippy --workspace --all-targets -- -D warnings
      - run: cargo test --workspace
```

- [ ] **Step 4: Commit**

```bash
git add -A && git commit -S -m "chore: cargo workspace scaffold + CI (fmt, clippy, test, dynamodb-local)"
```

---

### Task 2: domain crate — types + transitions

**Files:**
- Modify: `crates/domain/src/lib.rs`

**Interfaces:**
- Produces (Plan 2 and Tasks 6-7 consume exactly these):
  - `GameStatus { Available, Pending, Gifted, BenRedeemed, Expired }`
  - `ClaimState { Pending, Fulfilled, Compensated }`
  - `Game { id: String, title: String, bundle: String, gamekey: String, machine_name: String, key_type: String, giftable: bool, hidden: bool, status: GameStatus, claim_id: Option<String>, artwork_url: Option<String> }`
  - `Link { token: String, label: String, claims_allowed: u32, claims_used: u32, revoked: bool, expires_at: Option<OffsetDateTime>, created_at: OffsetDateTime }`
  - `Claim { id: String, link_token: String, game_id: String, state: ClaimState, gift_url: Option<String>, created_at: OffsetDateTime }`
  - `Game::is_listable(&self) -> bool`
  - `Link::can_claim(&self, now: OffsetDateTime) -> Result<(), ClaimRefusal>`
  - `ClaimRefusal { Revoked, Expired, Exhausted }`
  - `game_id(gamekey: &str, machine_name: &str) -> String` (= `format!("{gamekey}:{machine_name}")`)

- [ ] **Step 1: Write the failing tests** (bottom of `crates/domain/src/lib.rs`)

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use time::macros::datetime;

    fn link() -> Link {
        Link {
            token: "tok".into(),
            label: "dave".into(),
            claims_allowed: 2,
            claims_used: 0,
            revoked: false,
            expires_at: None,
            created_at: datetime!(2026-07-02 00:00 UTC),
        }
    }

    #[test]
    fn listable_iff_available_giftable_unhidden() {
        let mut g = Game {
            id: game_id("gk", "mn"),
            title: "T".into(),
            bundle: "B".into(),
            gamekey: "gk".into(),
            machine_name: "mn".into(),
            key_type: "steam".into(),
            giftable: true,
            hidden: false,
            status: GameStatus::Available,
            claim_id: None,
            artwork_url: None,
        };
        assert!(g.is_listable());
        g.hidden = true;
        assert!(!g.is_listable());
        g.hidden = false;
        g.status = GameStatus::Gifted;
        assert!(!g.is_listable());
        g.status = GameStatus::Available;
        g.giftable = false;
        assert!(!g.is_listable());
    }

    #[test]
    fn link_claim_gates() {
        let now = datetime!(2026-07-02 12:00 UTC);
        assert!(link().can_claim(now).is_ok());

        let mut l = link();
        l.revoked = true;
        assert_eq!(l.can_claim(now), Err(ClaimRefusal::Revoked));

        let mut l = link();
        l.expires_at = Some(datetime!(2026-07-01 00:00 UTC));
        assert_eq!(l.can_claim(now), Err(ClaimRefusal::Expired));

        let mut l = link();
        l.claims_used = 2;
        assert_eq!(l.can_claim(now), Err(ClaimRefusal::Exhausted));
    }

    #[test]
    fn game_id_shape() {
        assert_eq!(game_id("abc", "def_tpk"), "abc:def_tpk");
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p domain`
Expected: FAIL — types not defined.

- [ ] **Step 3: Implement**

Top of `crates/domain/src/lib.rs` (above the tests):
```rust
//! bendobundles domain types and state transitions. No I/O lives here.
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GameStatus {
    Available,
    Pending,
    Gifted,
    BenRedeemed,
    Expired,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClaimState {
    Pending,
    Fulfilled,
    Compensated,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Game {
    pub id: String,
    pub title: String,
    pub bundle: String,
    pub gamekey: String,
    pub machine_name: String,
    pub key_type: String,
    pub giftable: bool,
    pub hidden: bool,
    pub status: GameStatus,
    pub claim_id: Option<String>,
    pub artwork_url: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Link {
    pub token: String,
    pub label: String,
    pub claims_allowed: u32,
    pub claims_used: u32,
    pub revoked: bool,
    #[serde(with = "time::serde::rfc3339::option")]
    pub expires_at: Option<OffsetDateTime>,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Claim {
    pub id: String,
    pub link_token: String,
    pub game_id: String,
    pub state: ClaimState,
    pub gift_url: Option<String>,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ClaimRefusal {
    #[error("link revoked")]
    Revoked,
    #[error("link expired")]
    Expired,
    #[error("all claims used")]
    Exhausted,
}

impl Game {
    pub fn is_listable(&self) -> bool {
        self.status == GameStatus::Available && self.giftable && !self.hidden
    }
}

impl Link {
    pub fn can_claim(&self, now: OffsetDateTime) -> Result<(), ClaimRefusal> {
        if self.revoked {
            return Err(ClaimRefusal::Revoked);
        }
        if let Some(exp) = self.expires_at {
            if exp <= now {
                return Err(ClaimRefusal::Expired);
            }
        }
        if self.claims_used >= self.claims_allowed {
            return Err(ClaimRefusal::Exhausted);
        }
        Ok(())
    }
}

pub fn game_id(gamekey: &str, machine_name: &str) -> String {
    format!("{gamekey}:{machine_name}")
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p domain && cargo clippy -p domain --all-targets -- -D warnings && cargo fmt --check`
Expected: PASS (3 tests)

- [ ] **Step 5: Commit**

```bash
git add crates/domain && git commit -S -m "feat(domain): core types, listability + claim-gate rules"
```

---

### Task 3: humble-client — orders parsing against fixtures

**Files:**
- Create: `crates/humble-client/Cargo.toml`, `crates/humble-client/src/lib.rs`,
  `crates/humble-client/src/model.rs`,
  `crates/humble-client/tests/fixtures/user_order.json`,
  `crates/humble-client/tests/fixtures/order_detail.json`,
  `crates/humble-client/tests/client_test.rs`
- Modify: root `Cargo.toml` (add member `crates/humble-client`)

**Interfaces:**
- Produces:
  - `HumbleClient::new(base_url: &str, session_cookie: SessionCookie) -> Result<HumbleClient, HumbleError>`
  - `SessionCookie(pub secrecy-free redacting wrapper)` — `SessionCookie::new(String)`, Debug prints `SessionCookie(REDACTED)`
  - `client.gamekeys() -> Result<Vec<String>, HumbleError>`
  - `client.order(gamekey: &str) -> Result<Order, HumbleError>`
  - `Order { gamekey: String, bundle_name: String, keys: Vec<KeyEntry> }`
  - `KeyEntry { machine_name: String, human_name: String, key_type: String, redeemed: bool, expired: bool, giftable: bool }`
- API contract (community-documented unofficial API; probe task verifies):
  - `GET {base}/api/v1/user/order` with `Cookie: _simpleauth_sess=<cookie>` and
    `X-Requested-By: hb_android_app` → `[{"gamekey": "..."}]`
  - `GET {base}/api/v1/order/{gamekey}?all_tpkds=true` → order JSON; keys live in
    `tpkd_dict.all_tpks[]` with `machine_name`, `human_name`, `key_type`,
    optional `redeemed_key_val` (present = ben already revealed it), optional `is_expired`.
  - `giftable` for v1 = `!redeemed && !expired` (refined by probe findings if needed).
  - non-2xx: 401/403 or a 302 to `/login` → `HumbleError::Unauthorized` (dead cookie).

- [ ] **Step 1: Write the fixtures**

`crates/humble-client/tests/fixtures/user_order.json`:
```json
[
  { "gamekey": "AAAAbbbbCCCC" },
  { "gamekey": "DDDDeeeeFFFF" }
]
```

`crates/humble-client/tests/fixtures/order_detail.json`:
```json
{
  "gamekey": "AAAAbbbbCCCC",
  "product": { "human_name": "Humble Indie Bundle 99", "machine_name": "hib99_bundle" },
  "tpkd_dict": {
    "all_tpks": [
      {
        "machine_name": "stardew_valley_steam",
        "human_name": "Stardew Valley",
        "key_type": "steam"
      },
      {
        "machine_name": "already_revealed_steam",
        "human_name": "Already Revealed Game",
        "key_type": "steam",
        "redeemed_key_val": "AAAA-BBBB-CCCC"
      },
      {
        "machine_name": "dead_key_steam",
        "human_name": "Dead Old Game",
        "key_type": "steam",
        "is_expired": true
      }
    ]
  }
}
```

- [ ] **Step 2: Write the failing test**

`crates/humble-client/tests/client_test.rs`:
```rust
use humble_client::{HumbleClient, SessionCookie};
use wiremock::matchers::{header, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn fixture(name: &str) -> serde_json::Value {
    let raw = std::fs::read_to_string(format!(
        "{}/tests/fixtures/{name}",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap();
    serde_json::from_str(&raw).unwrap()
}

async fn client(server: &MockServer) -> HumbleClient {
    HumbleClient::new(&server.uri(), SessionCookie::new("sekrit".into())).unwrap()
}

#[tokio::test]
async fn lists_gamekeys() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/user/order"))
        .and(header("cookie", "_simpleauth_sess=sekrit"))
        .and(header("x-requested-by", "hb_android_app"))
        .respond_with(ResponseTemplate::new(200).set_body_json(fixture("user_order.json")))
        .mount(&server)
        .await;

    let keys = client(&server).await.gamekeys().await.unwrap();
    assert_eq!(keys, vec!["AAAAbbbbCCCC", "DDDDeeeeFFFF"]);
}

#[tokio::test]
async fn parses_order_key_states() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/order/AAAAbbbbCCCC"))
        .and(query_param("all_tpkds", "true"))
        .respond_with(ResponseTemplate::new(200).set_body_json(fixture("order_detail.json")))
        .mount(&server)
        .await;

    let order = client(&server).await.order("AAAAbbbbCCCC").await.unwrap();
    assert_eq!(order.bundle_name, "Humble Indie Bundle 99");
    assert_eq!(order.keys.len(), 3);

    let fresh = &order.keys[0];
    assert!(fresh.giftable && !fresh.redeemed && !fresh.expired);

    let revealed = &order.keys[1];
    assert!(revealed.redeemed && !revealed.giftable);

    let dead = &order.keys[2];
    assert!(dead.expired && !dead.giftable);
}

#[tokio::test]
async fn dead_cookie_is_unauthorized() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/user/order"))
        .respond_with(ResponseTemplate::new(401))
        .mount(&server)
        .await;

    let err = client(&server).await.gamekeys().await.unwrap_err();
    assert!(matches!(err, humble_client::HumbleError::Unauthorized));
}

#[test]
fn cookie_redacts_in_debug() {
    let c = SessionCookie::new("sekrit".into());
    assert_eq!(format!("{c:?}"), "SessionCookie(REDACTED)");
}
```

- [ ] **Step 3: Write the crate manifest, run test to verify it fails**

`crates/humble-client/Cargo.toml`:
```toml
[package]
name = "humble-client"
version = "0.1.0"
edition.workspace = true
publish.workspace = true

[dependencies]
reqwest = { version = "0.12", default-features = false, features = ["json", "rustls-tls"] }
serde.workspace = true
serde_json.workspace = true
thiserror.workspace = true
tokio.workspace = true

[dev-dependencies]
wiremock = "0.6"
```
Add `"crates/humble-client"` to root `Cargo.toml` members.

Run: `cargo test -p humble-client`
Expected: FAIL — lib doesn't exist.

- [ ] **Step 4: Implement**

`crates/humble-client/src/model.rs`:
```rust
//! Wire shapes of the unofficial humble API. Field names are theirs, not ours.
use serde::Deserialize;

#[derive(Deserialize)]
pub(crate) struct GamekeyEntry {
    pub gamekey: String,
}

#[derive(Deserialize)]
pub(crate) struct OrderWire {
    pub gamekey: String,
    pub product: ProductWire,
    #[serde(default)]
    pub tpkd_dict: TpkdDict,
}

#[derive(Deserialize)]
pub(crate) struct ProductWire {
    pub human_name: String,
}

#[derive(Deserialize, Default)]
pub(crate) struct TpkdDict {
    #[serde(default)]
    pub all_tpks: Vec<TpkWire>,
}

#[derive(Deserialize)]
pub(crate) struct TpkWire {
    pub machine_name: String,
    pub human_name: String,
    #[serde(default)]
    pub key_type: String,
    #[serde(default)]
    pub redeemed_key_val: Option<String>,
    #[serde(default)]
    pub is_expired: bool,
}
```

`crates/humble-client/src/lib.rs`:
```rust
//! Client for the community-documented unofficial Humble Bundle API.
//! No test touches the real API — see the probe binary for live verification.
mod model;

use model::{GamekeyEntry, OrderWire};

pub struct SessionCookie(String);

impl SessionCookie {
    pub fn new(value: String) -> Self {
        Self(value)
    }
}

impl std::fmt::Debug for SessionCookie {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "SessionCookie(REDACTED)")
    }
}

#[derive(Debug, thiserror::Error)]
pub enum HumbleError {
    #[error("session cookie rejected — needs a fresh paste")]
    Unauthorized,
    #[error("humble rate-limited us")]
    RateLimited,
    #[error("humble returned status {0}")]
    Api(u16),
    #[error("network error talking to humble: {0}")]
    Network(#[from] reqwest::Error),
    #[error("could not parse humble response: {0}")]
    Parse(#[from] serde_json::Error),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Order {
    pub gamekey: String,
    pub bundle_name: String,
    pub keys: Vec<KeyEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyEntry {
    pub machine_name: String,
    pub human_name: String,
    pub key_type: String,
    pub redeemed: bool,
    pub expired: bool,
    pub giftable: bool,
}

pub struct HumbleClient {
    http: reqwest::Client,
    base: String,
    cookie: SessionCookie,
}

impl HumbleClient {
    pub fn new(base_url: &str, cookie: SessionCookie) -> Result<Self, HumbleError> {
        let http = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none()) // a 302-to-login must surface, not follow
            .build()?;
        Ok(Self { http, base: base_url.trim_end_matches('/').to_string(), cookie })
    }

    async fn get_json<T: serde::de::DeserializeOwned>(&self, path_q: &str) -> Result<T, HumbleError> {
        let resp = self
            .http
            .get(format!("{}{path_q}", self.base))
            .header("Cookie", format!("_simpleauth_sess={}", self.cookie.0))
            .header("X-Requested-By", "hb_android_app")
            .send()
            .await?;
        match resp.status().as_u16() {
            200 => Ok(resp.json::<T>().await?),
            401 | 403 | 302 => Err(HumbleError::Unauthorized),
            429 => Err(HumbleError::RateLimited),
            s => Err(HumbleError::Api(s)),
        }
    }

    pub async fn gamekeys(&self) -> Result<Vec<String>, HumbleError> {
        let entries: Vec<GamekeyEntry> = self.get_json("/api/v1/user/order").await?;
        Ok(entries.into_iter().map(|e| e.gamekey).collect())
    }

    pub async fn order(&self, gamekey: &str) -> Result<Order, HumbleError> {
        let wire: OrderWire = self
            .get_json(&format!("/api/v1/order/{gamekey}?all_tpkds=true"))
            .await?;
        Ok(Order {
            gamekey: wire.gamekey,
            bundle_name: wire.product.human_name,
            keys: wire
                .tpkd_dict
                .all_tpks
                .into_iter()
                .map(|t| {
                    let redeemed = t.redeemed_key_val.is_some();
                    let expired = t.is_expired;
                    KeyEntry {
                        giftable: !redeemed && !expired,
                        machine_name: t.machine_name,
                        human_name: t.human_name,
                        key_type: t.key_type,
                        redeemed,
                        expired,
                    }
                })
                .collect(),
        })
    }
}
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p humble-client && cargo clippy -p humble-client --all-targets -- -D warnings && cargo fmt --check`
Expected: PASS (4 tests)

- [ ] **Step 6: Commit**

```bash
git add Cargo.toml crates/humble-client && git commit -S -m "feat(humble-client): orders + key-state parsing, redacting cookie, error taxonomy"
```

---

### Task 4: humble-client — redeem-as-gift

**Files:**
- Modify: `crates/humble-client/src/lib.rs`
- Modify: `crates/humble-client/tests/client_test.rs`

**Interfaces:**
- Produces: `client.redeem_as_gift(gamekey: &str, machine_name: &str) -> Result<GiftUrl, HumbleError>`
  where `GiftUrl(pub String)`; plus `HumbleError::AlreadyRedeemed`.
- API contract (FailSpy's redeemer + community docs; probe verifies):
  `POST {base}/humbler/redeemkey` form-encoded `keytype=<machine_name>&key=<gamekey>&keyindex=0&gift=true`
  → 200 `{"success": true, "giftkey": "<token>"}` → gift URL
  `https://www.humblebundle.com/gift?key=<token>`. A key already redeemed comes back as a
  non-success JSON (`{"success": false, ...}`) → `AlreadyRedeemed`.

- [ ] **Step 1: Write the failing tests** (append to `client_test.rs`)

```rust
use wiremock::matchers::body_string_contains;

#[tokio::test]
async fn redeems_as_gift() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/humbler/redeemkey"))
        .and(body_string_contains("keytype=stardew_valley_steam"))
        .and(body_string_contains("key=AAAAbbbbCCCC"))
        .and(body_string_contains("gift=true"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "success": true,
            "giftkey": "g1ftt0k3n"
        })))
        .mount(&server)
        .await;

    let gift = client(&server)
        .await
        .redeem_as_gift("AAAAbbbbCCCC", "stardew_valley_steam")
        .await
        .unwrap();
    assert_eq!(gift.0, "https://www.humblebundle.com/gift?key=g1ftt0k3n");
}

#[tokio::test]
async fn already_redeemed_is_typed() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/humbler/redeemkey"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "success": false
        })))
        .mount(&server)
        .await;

    let err = client(&server)
        .await
        .redeem_as_gift("AAAAbbbbCCCC", "already_revealed_steam")
        .await
        .unwrap_err();
    assert!(matches!(err, humble_client::HumbleError::AlreadyRedeemed));
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p humble-client`
Expected: FAIL — `redeem_as_gift` not defined.

- [ ] **Step 3: Implement** (append to `lib.rs`)

Add variant to `HumbleError`:
```rust
    #[error("key already redeemed on humble")]
    AlreadyRedeemed,
```

Add to `impl HumbleClient` and a wire struct:
```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GiftUrl(pub String);

#[derive(serde::Deserialize)]
struct RedeemResponse {
    #[serde(default)]
    success: bool,
    #[serde(default)]
    giftkey: Option<String>,
}

impl HumbleClient {
    pub async fn redeem_as_gift(
        &self,
        gamekey: &str,
        machine_name: &str,
    ) -> Result<GiftUrl, HumbleError> {
        let resp = self
            .http
            .post(format!("{}/humbler/redeemkey", self.base))
            .header("Cookie", format!("_simpleauth_sess={}", self.cookie.0))
            .header("X-Requested-By", "hb_android_app")
            .form(&[
                ("keytype", machine_name),
                ("key", gamekey),
                ("keyindex", "0"),
                ("gift", "true"),
            ])
            .send()
            .await?;
        match resp.status().as_u16() {
            200 => {
                let body: RedeemResponse = resp.json().await?;
                match (body.success, body.giftkey) {
                    (true, Some(token)) => Ok(GiftUrl(format!(
                        "https://www.humblebundle.com/gift?key={token}"
                    ))),
                    _ => Err(HumbleError::AlreadyRedeemed),
                }
            }
            401 | 403 | 302 => Err(HumbleError::Unauthorized),
            429 => Err(HumbleError::RateLimited),
            s => Err(HumbleError::Api(s)),
        }
    }
}
```
(Note: `impl HumbleClient` blocks merge — put the method inside the existing block.)

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p humble-client && cargo clippy -p humble-client --all-targets -- -D warnings && cargo fmt --check`
Expected: PASS (6 tests)

- [ ] **Step 5: Commit**

```bash
git add crates/humble-client && git commit -S -m "feat(humble-client): redeem-as-gift with typed already-redeemed"
```

---

### Task 5: probe binary — live contract verification (human-run only)

**Files:**
- Create: `crates/humble-client/src/bin/probe.rs`
- Modify: `crates/humble-client/Cargo.toml`

**Interfaces:**
- Produces: `cargo run -p humble-client --features probe --bin probe -- orders|order <gamekey>`
  reading `HUMBLE_SESSION` from env. READ-ONLY: no gift/redeem mode exists in the probe — burning
  a key happens only through the real app, deliberately. Ben runs this once before Plan 2 to
  confirm the wire shapes; any drift found gets folded back into Task 3/4 fixtures.

- [ ] **Step 1: Add the feature gate**

In `crates/humble-client/Cargo.toml`:
```toml
[features]
probe = ["dep:tokio"]

[dependencies]
tokio = { workspace = true, optional = true }

[[bin]]
name = "probe"
required-features = ["probe"]
```
(Move the existing `tokio.workspace = true` dev-dependency usage: tokio stays a dev-dependency
for tests AND an optional dependency for the probe feature.)

- [ ] **Step 2: Write the probe**

`crates/humble-client/src/bin/probe.rs`:
```rust
//! READ-ONLY live probe of the unofficial humble API. Run by a human, never CI:
//!   HUMBLE_SESSION='<cookie>' cargo run -p humble-client --features probe --bin probe -- orders
//!   HUMBLE_SESSION='<cookie>' cargo run -p humble-client --features probe --bin probe -- order <gamekey>
use humble_client::{HumbleClient, SessionCookie};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cookie = std::env::var("HUMBLE_SESSION").expect("set HUMBLE_SESSION");
    let client = HumbleClient::new("https://www.humblebundle.com", SessionCookie::new(cookie))?;
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.as_slice() {
        [cmd] if cmd == "orders" => {
            let keys = client.gamekeys().await?;
            println!("{} orders", keys.len());
            for k in keys.iter().take(5) {
                println!("  {k}");
            }
        }
        [cmd, gamekey] if cmd == "order" => {
            let order = client.order(gamekey).await?;
            println!("{} — {} keys", order.bundle_name, order.keys.len());
            for k in &order.keys {
                println!(
                    "  [{}] {} ({}) redeemed={} expired={} giftable={}",
                    k.key_type, k.human_name, k.machine_name, k.redeemed, k.expired, k.giftable
                );
            }
        }
        _ => eprintln!("usage: probe orders | probe order <gamekey>"),
    }
    Ok(())
}
```

- [ ] **Step 3: Verify it compiles (no live run in CI)**

Run: `cargo clippy -p humble-client --features probe --all-targets -- -D warnings && cargo test --workspace`
Expected: PASS — probe compiles; tests unaffected.

- [ ] **Step 4: Commit**

```bash
git add crates/humble-client && git commit -S -m "feat(humble-client): read-only live probe bin (human-run, feature-gated)"
```

---

### Task 6: dynamo crate — schema + CRUD vs dynamodb-local

**Files:**
- Create: `crates/dynamo/Cargo.toml`, `crates/dynamo/src/lib.rs`, `crates/dynamo/src/schema.rs`,
  `crates/dynamo/tests/store_test.rs`
- Modify: root `Cargo.toml` (add member `crates/dynamo`)

**Interfaces:**
- Produces (Plan 2 consumes exactly these):
  - `Store::new(client: aws_sdk_dynamodb::Client, table: String) -> Store`
  - `store.create_table_for_tests() -> Result<()>` (test helper: table + GSI, used by tests only)
  - `store.put_game(&Game)`, `store.get_game(id: &str) -> Result<Option<Game>>`
  - `store.put_link(&Link)`, `store.get_link(token: &str) -> Result<Option<Link>>`
  - `store.get_claim(link_token: &str, claim_id: &str) -> Result<Option<Claim>>`
  - `store.list_listable_games() -> Result<Vec<Game>>` (queries sparse GSI `listable`)
  - `store.claims_for_link(token: &str) -> Result<Vec<Claim>>`
- Item shapes (THE storage contract):
  - GAME: `pk = "GAME#<id>"`, `sk = "META"`, `body` = serde_json of `domain::Game`; when
    `game.is_listable()`: `gsi1pk = "LISTABLE"`, `gsi1sk = "<title lowercased>#<id>"` (sparse).
  - LINK: `pk = "LINK#<token>"`, `sk = "META"`, `body` = serde_json of `domain::Link`.
  - CLAIM: `pk = "LINK#<token>"`, `sk = "CLAIM#<claim_id>"`, `body` = serde_json of
    `domain::Claim`; while `state == pending`: `gsi2pk = "PENDINGCLAIM"`, `gsi2sk = created_at`
    RFC3339 (sparse — reconcile pass in Plan 2 queries it).
  - Table: PK `pk` (S), SK `sk` (S); GSI `listable` on (`gsi1pk`,`gsi1sk`); GSI `pending-claims`
    on (`gsi2pk`,`gsi2sk`); all projections ALL. Billing on-demand (terraform, Plan 4).
- Tests require dynamodb-local at `DYNAMODB_LOCAL_URL` (default `http://localhost:8000`); tests
  are `#[ignore]`-free but SKIP with a clear message if the endpoint is unreachable, so plain
  `cargo test` on a dev box without docker still passes. CI always runs them (service container).

- [ ] **Step 1: Write the failing tests**

`crates/dynamo/tests/store_test.rs`:
```rust
use domain::{game_id, Claim, ClaimState, Game, GameStatus, Link};
use dynamo::Store;
use time::macros::datetime;

async fn store_or_skip(test: &str) -> Option<Store> {
    let url = std::env::var("DYNAMODB_LOCAL_URL")
        .unwrap_or_else(|_| "http://localhost:8000".into());
    let config = aws_config::defaults(aws_config::BehaviorVersion::latest())
        .endpoint_url(&url)
        .region("us-east-1")
        .test_credentials()
        .load()
        .await;
    let client = aws_sdk_dynamodb::Client::new(&config);
    if client.list_tables().send().await.is_err() {
        eprintln!("SKIP {test}: no dynamodb-local at {url}");
        return None;
    }
    // one table per test = no cross-test interference
    let store = Store::new(client, format!("t-{test}"));
    store.create_table_for_tests().await.unwrap();
    Some(store)
}

fn game(n: u32, listable: bool) -> Game {
    Game {
        id: game_id(&format!("gk{n}"), "mn"),
        title: format!("Game {n}"),
        bundle: "B".into(),
        gamekey: format!("gk{n}"),
        machine_name: "mn".into(),
        key_type: "steam".into(),
        giftable: listable,
        hidden: false,
        status: GameStatus::Available,
        claim_id: None,
        artwork_url: None,
    }
}

fn link(token: &str) -> Link {
    Link {
        token: token.into(),
        label: "dave".into(),
        claims_allowed: 1,
        claims_used: 0,
        revoked: false,
        expires_at: None,
        created_at: datetime!(2026-07-02 00:00 UTC),
    }
}

#[tokio::test]
async fn game_roundtrip_and_listable_index() {
    let Some(store) = store_or_skip("game-roundtrip").await else { return };
    store.put_game(&game(1, true)).await.unwrap();
    store.put_game(&game(2, false)).await.unwrap();

    let got = store.get_game(&game_id("gk1", "mn")).await.unwrap().unwrap();
    assert_eq!(got, game(1, true));
    assert_eq!(store.get_game("nope").await.unwrap(), None);

    let listable = store.list_listable_games().await.unwrap();
    assert_eq!(listable.len(), 1);
    assert_eq!(listable[0].id, game_id("gk1", "mn"));
}

#[tokio::test]
async fn link_and_claim_roundtrip() {
    let Some(store) = store_or_skip("link-claim").await else { return };
    store.put_link(&link("tok1")).await.unwrap();
    assert_eq!(store.get_link("tok1").await.unwrap().unwrap(), link("tok1"));

    let claim = Claim {
        id: "c1".into(),
        link_token: "tok1".into(),
        game_id: game_id("gk1", "mn"),
        state: ClaimState::Pending,
        gift_url: None,
        created_at: datetime!(2026-07-02 01:00 UTC),
    };
    store.put_claim(&claim).await.unwrap();
    assert_eq!(
        store.get_claim("tok1", "c1").await.unwrap().unwrap(),
        claim
    );
    assert_eq!(store.claims_for_link("tok1").await.unwrap(), vec![claim]);
}
```

- [ ] **Step 2: Write the manifest, run tests to verify they fail**

`crates/dynamo/Cargo.toml`:
```toml
[package]
name = "dynamo"
version = "0.1.0"
edition.workspace = true
publish.workspace = true

[dependencies]
aws-config = { version = "1", features = ["behavior-version-latest", "test-util"] }
aws-sdk-dynamodb = "1"
domain = { path = "../domain" }
serde_json.workspace = true
thiserror.workspace = true
time.workspace = true
uuid.workspace = true

[dev-dependencies]
tokio.workspace = true
```
Add `"crates/dynamo"` to root members.

Run: `cargo test -p dynamo`
Expected: FAIL — Store not defined.

- [ ] **Step 3: Implement**

`crates/dynamo/src/schema.rs`:
```rust
//! Key builders + item (de)serialization. The item shapes here are the storage contract.
use aws_sdk_dynamodb::types::AttributeValue;
use domain::{Claim, ClaimState, Game, Link};
use std::collections::HashMap;

pub const GSI_LISTABLE: &str = "listable";
pub const GSI_PENDING: &str = "pending-claims";

pub fn game_pk(id: &str) -> String {
    format!("GAME#{id}")
}
pub fn link_pk(token: &str) -> String {
    format!("LINK#{token}")
}
pub fn claim_sk(claim_id: &str) -> String {
    format!("CLAIM#{claim_id}")
}

fn s(v: impl Into<String>) -> AttributeValue {
    AttributeValue::S(v.into())
}

pub fn game_item(g: &Game) -> HashMap<String, AttributeValue> {
    let mut item = HashMap::from([
        ("pk".into(), s(game_pk(&g.id))),
        ("sk".into(), s("META")),
        ("body".into(), s(serde_json::to_string(g).expect("game serializes"))),
    ]);
    if g.is_listable() {
        item.insert("gsi1pk".into(), s("LISTABLE"));
        item.insert(
            "gsi1sk".into(),
            s(format!("{}#{}", g.title.to_lowercase(), g.id)),
        );
    }
    item
}

pub fn link_item(l: &Link) -> HashMap<String, AttributeValue> {
    HashMap::from([
        ("pk".into(), s(link_pk(&l.token))),
        ("sk".into(), s("META")),
        ("body".into(), s(serde_json::to_string(l).expect("link serializes"))),
    ])
}

pub fn claim_item(c: &Claim) -> HashMap<String, AttributeValue> {
    let mut item = HashMap::from([
        ("pk".into(), s(link_pk(&c.link_token))),
        ("sk".into(), s(claim_sk(&c.id))),
        ("body".into(), s(serde_json::to_string(c).expect("claim serializes"))),
    ]);
    if c.state == ClaimState::Pending {
        item.insert("gsi2pk".into(), s("PENDINGCLAIM"));
        let ts = c
            .created_at
            .format(&time::format_description::well_known::Rfc3339)
            .expect("rfc3339");
        item.insert("gsi2sk".into(), s(ts));
    }
    item
}

pub fn parse_body<T: serde::de::DeserializeOwned>(
    item: &HashMap<String, AttributeValue>,
) -> Result<T, crate::StoreError> {
    let body = item
        .get("body")
        .and_then(|v| v.as_s().ok())
        .ok_or(crate::StoreError::Corrupt("missing body"))?;
    serde_json::from_str(body).map_err(|_| crate::StoreError::Corrupt("bad body json"))
}
```

`crates/dynamo/src/lib.rs`:
```rust
//! DynamoDB storage. Single table; see schema.rs for the item contract.
pub mod schema;

use aws_sdk_dynamodb::types::{
    AttributeDefinition, BillingMode, GlobalSecondaryIndex, KeySchemaElement, KeyType,
    Projection, ProjectionType, ScalarAttributeType,
};
use aws_sdk_dynamodb::Client;
use domain::{Claim, Game, Link};
use schema::{claim_sk, game_item, game_pk, link_item, link_pk, parse_body, claim_item};

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("dynamodb error: {0}")]
    Aws(String),
    #[error("corrupt item: {0}")]
    Corrupt(&'static str),
}

impl<E: std::fmt::Debug, R: std::fmt::Debug> From<aws_sdk_dynamodb::error::SdkError<E, R>>
    for StoreError
{
    fn from(e: aws_sdk_dynamodb::error::SdkError<E, R>) -> Self {
        StoreError::Aws(format!("{e:?}"))
    }
}

pub struct Store {
    client: Client,
    table: String,
}

impl Store {
    pub fn new(client: Client, table: String) -> Self {
        Self { client, table }
    }

    pub async fn put_game(&self, g: &Game) -> Result<(), StoreError> {
        self.client
            .put_item()
            .table_name(&self.table)
            .set_item(Some(game_item(g)))
            .send()
            .await?;
        Ok(())
    }

    pub async fn get_game(&self, id: &str) -> Result<Option<Game>, StoreError> {
        self.get_meta(&game_pk(id)).await
    }

    pub async fn put_link(&self, l: &Link) -> Result<(), StoreError> {
        self.client
            .put_item()
            .table_name(&self.table)
            .set_item(Some(link_item(l)))
            .send()
            .await?;
        Ok(())
    }

    pub async fn get_link(&self, token: &str) -> Result<Option<Link>, StoreError> {
        self.get_meta(&link_pk(token)).await
    }

    pub async fn put_claim(&self, c: &Claim) -> Result<(), StoreError> {
        self.client
            .put_item()
            .table_name(&self.table)
            .set_item(Some(claim_item(c)))
            .send()
            .await?;
        Ok(())
    }

    pub async fn get_claim(
        &self,
        link_token: &str,
        claim_id: &str,
    ) -> Result<Option<Claim>, StoreError> {
        let out = self
            .client
            .get_item()
            .table_name(&self.table)
            .key("pk", aws_sdk_dynamodb::types::AttributeValue::S(link_pk(link_token)))
            .key("sk", aws_sdk_dynamodb::types::AttributeValue::S(claim_sk(claim_id)))
            .send()
            .await?;
        out.item.map(|i| parse_body(&i)).transpose()
    }

    pub async fn list_listable_games(&self) -> Result<Vec<Game>, StoreError> {
        let out = self
            .client
            .query()
            .table_name(&self.table)
            .index_name(schema::GSI_LISTABLE)
            .key_condition_expression("gsi1pk = :p")
            .expression_attribute_values(
                ":p",
                aws_sdk_dynamodb::types::AttributeValue::S("LISTABLE".into()),
            )
            .send()
            .await?;
        out.items().iter().map(parse_body).collect()
    }

    pub async fn claims_for_link(&self, token: &str) -> Result<Vec<Claim>, StoreError> {
        let out = self
            .client
            .query()
            .table_name(&self.table)
            .key_condition_expression("pk = :p AND begins_with(sk, :c)")
            .expression_attribute_values(
                ":p",
                aws_sdk_dynamodb::types::AttributeValue::S(link_pk(token)),
            )
            .expression_attribute_values(
                ":c",
                aws_sdk_dynamodb::types::AttributeValue::S("CLAIM#".into()),
            )
            .send()
            .await?;
        out.items().iter().map(parse_body).collect()
    }

    async fn get_meta<T: serde::de::DeserializeOwned>(
        &self,
        pk: &str,
    ) -> Result<Option<T>, StoreError> {
        let out = self
            .client
            .get_item()
            .table_name(&self.table)
            .key("pk", aws_sdk_dynamodb::types::AttributeValue::S(pk.into()))
            .key("sk", aws_sdk_dynamodb::types::AttributeValue::S("META".into()))
            .send()
            .await?;
        out.item.map(|i| parse_body(&i)).transpose()
    }

    /// Test-only helper: create the table + GSIs (mirrors the Plan 4 terraform).
    pub async fn create_table_for_tests(&self) -> Result<(), StoreError> {
        let attr = |name: &str| {
            AttributeDefinition::builder()
                .attribute_name(name)
                .attribute_type(ScalarAttributeType::S)
                .build()
                .expect("attr")
        };
        let key = |name: &str, kt: KeyType| {
            KeySchemaElement::builder()
                .attribute_name(name)
                .key_type(kt)
                .build()
                .expect("key")
        };
        let gsi = |name: &str, pk: &str, sk: &str| {
            GlobalSecondaryIndex::builder()
                .index_name(name)
                .key_schema(key(pk, KeyType::Hash))
                .key_schema(key(sk, KeyType::Range))
                .projection(
                    Projection::builder()
                        .projection_type(ProjectionType::All)
                        .build(),
                )
                .build()
                .expect("gsi")
        };
        let _ = self
            .client
            .create_table()
            .table_name(&self.table)
            .billing_mode(BillingMode::PayPerRequest)
            .attribute_definitions(attr("pk"))
            .attribute_definitions(attr("sk"))
            .attribute_definitions(attr("gsi1pk"))
            .attribute_definitions(attr("gsi1sk"))
            .attribute_definitions(attr("gsi2pk"))
            .attribute_definitions(attr("gsi2sk"))
            .key_schema(key("pk", KeyType::Hash))
            .key_schema(key("sk", KeyType::Range))
            .global_secondary_indexes(gsi(schema::GSI_LISTABLE, "gsi1pk", "gsi1sk"))
            .global_secondary_indexes(gsi(schema::GSI_PENDING, "gsi2pk", "gsi2sk"))
            .send()
            .await; // ignore ResourceInUse on re-run
        Ok(())
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `docker ps | grep dynamodb || docker run -d -p 8000:8000 amazon/dynamodb-local` then
`cargo test -p dynamo && cargo clippy -p dynamo --all-targets -- -D warnings && cargo fmt --check`
Expected: PASS (2 tests; or SKIP lines if no docker — CI always runs them)

- [ ] **Step 5: Commit**

```bash
git add Cargo.toml crates/dynamo && git commit -S -m "feat(dynamo): single-table store, sparse listable/pending GSIs, local-test harness"
```

---

### Task 7: dynamo — the claim transaction (exactly-once intake)

**Files:**
- Modify: `crates/dynamo/src/lib.rs`
- Modify: `crates/dynamo/tests/store_test.rs`

**Interfaces:**
- Produces (Plan 2's public-api calls exactly these):
  - `store.claim_game(link_token: &str, game_id: &str, claim_id: &str, now: OffsetDateTime) -> Result<(), ClaimTxError>`
    — the atomic intake: GAME available→pending + LINK counter + CLAIM(pending) put.
  - `store.fulfill_claim(link_token: &str, claim_id: &str, game_id: &str, gift_url: &str) -> Result<(), StoreError>`
    — writes gift_url onto CLAIM (state→fulfilled, drops pending-GSI attrs) THEN flips GAME→gifted.
    Two writes in this order on purpose: the spec invariant says the gift URL becomes durable first.
  - `store.compensate_claim(link_token: &str, claim_id: &str, game_id: &str) -> Result<(), StoreError>`
    — CLAIM→compensated, GAME→available (re-adds listable GSI attrs), LINK counter decrement.
  - `ClaimTxError { GameUnavailable, LinkNotClaimable, DuplicateClaim, Store(StoreError) }`
- Semantics: `claim_game` conditions — GAME `body`-status is authoritative but conditions can't
  parse JSON, so GAME items also carry a top-level `status` attribute (add to `game_item` in
  schema.rs: `("status", s(json status string))`) used ONLY in condition expressions; LINK items
  carry top-level `claims_allowed` (N), `claims_used` (N), `revoked` (BOOL), optional
  `expires_at` (S, RFC3339 — lexicographic compare works). `game_item`/`link_item` must keep
  these in sync with `body` (single writer path = these builders, so they can't drift).

- [ ] **Step 1: Extend schema items** (in `schema.rs`)

In `game_item`, add before the listable block:
```rust
    item.insert(
        "status".into(),
        s(serde_json::to_value(g.status)
            .expect("status serializes")
            .as_str()
            .expect("status is a string")
            .to_string()),
    );
```
In `link_item`, change to:
```rust
pub fn link_item(l: &Link) -> HashMap<String, AttributeValue> {
    let mut item = HashMap::from([
        ("pk".into(), s(link_pk(&l.token))),
        ("sk".into(), s("META")),
        ("body".into(), s(serde_json::to_string(l).expect("link serializes"))),
        ("claims_allowed".into(), AttributeValue::N(l.claims_allowed.to_string())),
        ("claims_used".into(), AttributeValue::N(l.claims_used.to_string())),
        ("revoked".into(), AttributeValue::Bool(l.revoked)),
    ]);
    if let Some(exp) = l.expires_at {
        let ts = exp
            .format(&time::format_description::well_known::Rfc3339)
            .expect("rfc3339");
        item.insert("expires_at".into(), s(ts));
    }
    item
}
```

- [ ] **Step 2: Write the failing tests** (append to `store_test.rs`)

```rust
use dynamo::ClaimTxError;

#[tokio::test]
async fn claim_happy_path_then_race_loses() {
    let Some(store) = store_or_skip("claim-race").await else { return };
    store.put_game(&game(1, true)).await.unwrap();
    store.put_link(&link("tok1")).await.unwrap(); // claims_allowed = 1
    let now = datetime!(2026-07-02 12:00 UTC);
    let gid = game_id("gk1", "mn");

    store.claim_game("tok1", &gid, "c1", now).await.unwrap();

    // game is now pending + off the listable index; link slot consumed
    assert_eq!(store.list_listable_games().await.unwrap(), vec![]);
    let g = store.get_game(&gid).await.unwrap().unwrap();
    assert_eq!(g.status, GameStatus::Pending);
    assert_eq!(g.claim_id.as_deref(), Some("c1"));

    // second claim on the same game: game already pending → unavailable
    store.put_link(&link("tok2")).await.unwrap();
    let err = store.claim_game("tok2", &gid, "c2", now).await.unwrap_err();
    assert!(matches!(err, ClaimTxError::GameUnavailable));

    // exhausted link: tok1 had exactly 1 claim
    store.put_game(&game(3, true)).await.unwrap();
    let err = store
        .claim_game("tok1", &game_id("gk3", "mn"), "c3", now)
        .await
        .unwrap_err();
    assert!(matches!(err, ClaimTxError::LinkNotClaimable));
}

#[tokio::test]
async fn fulfill_writes_gift_url_then_flips_game() {
    let Some(store) = store_or_skip("fulfill").await else { return };
    store.put_game(&game(1, true)).await.unwrap();
    store.put_link(&link("tok1")).await.unwrap();
    let now = datetime!(2026-07-02 12:00 UTC);
    let gid = game_id("gk1", "mn");
    store.claim_game("tok1", &gid, "c1", now).await.unwrap();

    store
        .fulfill_claim("tok1", "c1", &gid, "https://www.humblebundle.com/gift?key=x")
        .await
        .unwrap();

    let c = store.get_claim("tok1", "c1").await.unwrap().unwrap();
    assert_eq!(c.state, ClaimState::Fulfilled);
    assert_eq!(c.gift_url.as_deref(), Some("https://www.humblebundle.com/gift?key=x"));
    let g = store.get_game(&gid).await.unwrap().unwrap();
    assert_eq!(g.status, GameStatus::Gifted);
}

#[tokio::test]
async fn compensate_returns_everything() {
    let Some(store) = store_or_skip("compensate").await else { return };
    store.put_game(&game(1, true)).await.unwrap();
    store.put_link(&link("tok1")).await.unwrap();
    let now = datetime!(2026-07-02 12:00 UTC);
    let gid = game_id("gk1", "mn");
    store.claim_game("tok1", &gid, "c1", now).await.unwrap();

    store.compensate_claim("tok1", "c1", &gid).await.unwrap();

    // game listable again, link slot returned, claim marked compensated
    assert_eq!(store.list_listable_games().await.unwrap().len(), 1);
    let l = store.get_link("tok1").await.unwrap().unwrap();
    assert_eq!(l.claims_used, 0);
    let c = store.get_claim("tok1", "c1").await.unwrap().unwrap();
    assert_eq!(c.state, ClaimState::Compensated);
}
```

- [ ] **Step 3: Run tests to verify they fail**

Run: `cargo test -p dynamo`
Expected: FAIL — `claim_game` etc. not defined.

- [ ] **Step 4: Implement** (append to `lib.rs`)

```rust
use aws_sdk_dynamodb::types::{Put, TransactWriteItem, Update};
use domain::{ClaimState, GameStatus};
use time::OffsetDateTime;

#[derive(Debug, thiserror::Error)]
pub enum ClaimTxError {
    #[error("game is not available")]
    GameUnavailable,
    #[error("link cannot claim (revoked/expired/exhausted)")]
    LinkNotClaimable,
    #[error("duplicate claim id")]
    DuplicateClaim,
    #[error(transparent)]
    Store(#[from] StoreError),
}

impl Store {
    /// Atomic claim intake. Cancellation reasons map positionally to the three writes.
    pub async fn claim_game(
        &self,
        link_token: &str,
        game_id: &str,
        claim_id: &str,
        now: OffsetDateTime,
    ) -> Result<(), ClaimTxError> {
        // read current game/link bodies so the updated `body` JSON stays in sync
        let game = self
            .get_game(game_id)
            .await?
            .ok_or(ClaimTxError::GameUnavailable)?;
        let link = self
            .get_link(link_token)
            .await?
            .ok_or(ClaimTxError::LinkNotClaimable)?;

        let mut pending = game.clone();
        pending.status = GameStatus::Pending;
        pending.claim_id = Some(claim_id.to_string());
        let mut bumped = link.clone();
        bumped.claims_used += 1;
        let claim = domain::Claim {
            id: claim_id.to_string(),
            link_token: link_token.to_string(),
            game_id: game_id.to_string(),
            state: ClaimState::Pending,
            gift_url: None,
            created_at: now,
        };
        let now_ts = now
            .format(&time::format_description::well_known::Rfc3339)
            .expect("rfc3339");

        let s = |v: &str| aws_sdk_dynamodb::types::AttributeValue::S(v.to_string());
        let game_update = Update::builder()
            .table_name(&self.table)
            .key("pk", s(&schema::game_pk(game_id)))
            .key("sk", s("META"))
            .update_expression("SET body = :b, #st = :pending REMOVE gsi1pk, gsi1sk")
            .condition_expression("#st = :available")
            .expression_attribute_names("#st", "status")
            .expression_attribute_values(":b", s(&serde_json::to_string(&pending).expect("game")))
            .expression_attribute_values(":pending", s("pending"))
            .expression_attribute_values(":available", s("available"))
            .build()
            .expect("update");
        let link_update = Update::builder()
            .table_name(&self.table)
            .key("pk", s(&schema::link_pk(link_token)))
            .key("sk", s("META"))
            .update_expression("SET body = :b ADD claims_used :one")
            .condition_expression(
                "revoked = :f AND claims_used < claims_allowed \
                 AND (attribute_not_exists(expires_at) OR expires_at > :now)",
            )
            .expression_attribute_values(":b", s(&serde_json::to_string(&bumped).expect("link")))
            .expression_attribute_values(":one", aws_sdk_dynamodb::types::AttributeValue::N("1".into()))
            .expression_attribute_values(":f", aws_sdk_dynamodb::types::AttributeValue::Bool(false))
            .expression_attribute_values(":now", s(&now_ts))
            .build()
            .expect("update");
        let claim_put = Put::builder()
            .table_name(&self.table)
            .set_item(Some(schema::claim_item(&claim)))
            .condition_expression("attribute_not_exists(pk)")
            .build()
            .expect("put");

        let result = self
            .client
            .transact_write_items()
            .transact_items(TransactWriteItem::builder().update(game_update).build())
            .transact_items(TransactWriteItem::builder().update(link_update).build())
            .transact_items(TransactWriteItem::builder().put(claim_put).build())
            .send()
            .await;

        match result {
            Ok(_) => Ok(()),
            Err(sdk_err) => {
                let svc = sdk_err.as_service_error();
                if let Some(cancelled) =
                    svc.and_then(|e| e.as_transaction_canceled_exception())
                {
                    let reasons = cancelled.cancellation_reasons();
                    let failed = |i: usize| {
                        reasons
                            .get(i)
                            .and_then(|r| r.code())
                            .is_some_and(|c| c == "ConditionalCheckFailed")
                    };
                    if failed(0) {
                        return Err(ClaimTxError::GameUnavailable);
                    }
                    if failed(1) {
                        return Err(ClaimTxError::LinkNotClaimable);
                    }
                    if failed(2) {
                        return Err(ClaimTxError::DuplicateClaim);
                    }
                }
                Err(ClaimTxError::Store(StoreError::Aws(format!("{sdk_err:?}"))))
            }
        }
    }

    /// Spec invariant: gift URL becomes durable BEFORE the game flips to gifted.
    pub async fn fulfill_claim(
        &self,
        link_token: &str,
        claim_id: &str,
        game_id: &str,
        gift_url: &str,
    ) -> Result<(), StoreError> {
        let mut claim = self
            .get_claim(link_token, claim_id)
            .await?
            .ok_or(StoreError::Corrupt("fulfill: claim missing"))?;
        claim.state = ClaimState::Fulfilled;
        claim.gift_url = Some(gift_url.to_string());
        self.put_claim(&claim).await?; // write 1: URL durable (claim_item drops pending GSI)

        let mut game = self
            .get_game(game_id)
            .await?
            .ok_or(StoreError::Corrupt("fulfill: game missing"))?;
        game.status = GameStatus::Gifted;
        self.put_game(&game).await // write 2: game flips
    }

    pub async fn compensate_claim(
        &self,
        link_token: &str,
        claim_id: &str,
        game_id: &str,
    ) -> Result<(), StoreError> {
        let mut claim = self
            .get_claim(link_token, claim_id)
            .await?
            .ok_or(StoreError::Corrupt("compensate: claim missing"))?;
        claim.state = ClaimState::Compensated;
        self.put_claim(&claim).await?;

        let mut game = self
            .get_game(game_id)
            .await?
            .ok_or(StoreError::Corrupt("compensate: game missing"))?;
        game.status = GameStatus::Available;
        game.claim_id = None;
        self.put_game(&game).await?; // put_game re-adds listable GSI attrs via game_item

        let mut link = self
            .get_link(link_token)
            .await?
            .ok_or(StoreError::Corrupt("compensate: link missing"))?;
        link.claims_used = link.claims_used.saturating_sub(1);
        self.put_link(&link).await
    }
}
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p dynamo && cargo clippy --workspace --all-targets -- -D warnings && cargo fmt --check`
Expected: PASS (5 tests in dynamo)

- [ ] **Step 6: Commit + push + open the PR**

```bash
git add crates/dynamo && git commit -S -m "feat(dynamo): exactly-once claim transaction, fulfill (URL-durable-first) + compensate"
git push -u origin kitten/plan1-backend-core
gh pr create --title "plan 1: backend core (domain, humble-client, dynamo)" \
  --body "implements docs/superpowers/plans/2026-07-02-plan1-backend-core.md — @bendoerr review when you have a minute ♡"
```

---

## Self-Review (done at write time)

1. **Spec coverage (plan-1 scope):** domain types/statuses ✓ (Task 2), humble API list/detail ✓
   (Task 3), redeem-as-gift + AlreadyRedeemed ✓ (Task 4), live-contract de-risk ✓ (Task 5),
   single-table + sparse GSIs ✓ (Task 6), claim transaction + gift-URL-durable-first + compensate
   ✓ (Task 7). NOT in this plan (by decomposition): lambdas/API routes, sync loop, reconcile pass,
   cookie SSM plumbing, discord webhook, frontend, terraform → Plans 2-4.
2. **Placeholder scan:** none — every step has full code/commands.
3. **Type consistency:** `Game.claim_id: Option<String>` used by Tasks 2/6/7 consistently;
   `game_id()` helper shared; `claim_item` referenced in Task 7's Put matches Task 6's schema fn;
   `status`/`claims_*` condition attributes introduced in Task 7 Step 1 before use.
