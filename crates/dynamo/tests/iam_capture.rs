//! IAM request-shape capture harness (#70).
//!
//! Drives every `Store` method **exactly as each lambda calls it** with an SDK interceptor
//! logging the serialized DynamoDB requests, then derives from that traffic:
//!
//!   1. `terraform/iam-request-corpus.json` — per-lambda, per-method request shapes: the
//!      IAM action, key prefixes (`dynamodb:LeadingKeys`), and attribute names
//!      (`dynamodb:Attributes` — update/condition/projection expression names, aliases
//!      resolved, plus key + item attribute names, which the context key includes).
//!   2. `terraform/policies/dynamo-rw-{public,admin,fulfillment}.json.tpl` — the actual
//!      per-lambda policy documents, generated so the attribute allowlists and deny
//!      prefixes are *derived from real traffic*, never hand-enumerated. Terraform
//!      consumes these via `templatefile()`; `terraform/policies/simulate.sh` consumes
//!      the same bytes for the `aws iam simulate-custom-policy` proof matrix.
//!
//! Default mode **asserts the committed files match the code** — a new or widened store
//! call fails this test instead of 403ing in prod after the policy ships. Regenerate with:
//!
//! ```text
//! IAM_CORPUS_WRITE=1 cargo test -p dynamo --test iam_capture
//! ```
//!
//! Lambda attribution is the audited caller map (issue #70): every entry cites the crate
//! that calls it. `cached_owned_or_fetch` is captured via its two dynamo legs
//! (`get_steam_owned` + `put_steam_owned`) to keep the harness off the Steam HTTP client —
//! same requests, same shapes. `delete_game` is deliberately absent: it is compiled only
//! into the heal operator bin (`heal` feature), never the lambda runtime, so it is not
//! lambda surface. `put_claim`/`get_claim`/`put_game`/`list_steam_app_ids` have no lambda
//! callers at all (test seeding helpers) and are likewise excluded.

use aws_smithy_runtime_api::box_error::BoxError;
use aws_smithy_runtime_api::client::interceptors::Intercept;
use aws_smithy_runtime_api::client::interceptors::context::BeforeTransmitInterceptorContextRef;
use aws_smithy_runtime_api::client::runtime_components::RuntimeComponents;
use aws_smithy_types::config_bag::ConfigBag;

use domain::{AppidSource, Game, GameStatus, Link, game_id};
use dynamo::{SteamAppCache, SteamAppPutGuard, Store, SyncState};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::future::Future;
use std::sync::{Arc, Mutex};
use time::macros::datetime;

// ---------------------------------------------------------------------------
// capture plumbing
// ---------------------------------------------------------------------------

/// Logs (x-amz-target, request body) for every request the captured client sends.
#[derive(Debug, Clone, Default)]
struct Capture {
    buf: Arc<Mutex<Vec<(String, String)>>>,
}

impl Capture {
    fn drain(&self) -> Vec<(String, String)> {
        self.buf.lock().unwrap().drain(..).collect()
    }
}

impl Intercept for Capture {
    fn name(&self) -> &'static str {
        "iam-capture"
    }

    fn read_before_transmit(
        &self,
        context: &BeforeTransmitInterceptorContextRef<'_>,
        _rc: &RuntimeComponents,
        _cfg: &mut ConfigBag,
    ) -> Result<(), BoxError> {
        let req = context.request();
        let target = req
            .headers()
            .get("x-amz-target")
            .unwrap_or_default()
            .to_string();
        let body = String::from_utf8_lossy(req.body().bytes().unwrap_or_default()).to_string();
        self.buf.lock().unwrap().push((target, body));
        Ok(())
    }
}

/// One store per rig on a shared table: `store` (intercepted — everything it sends lands in
/// the corpus under the method being driven) and `seed` (plain — fixture setup that must NOT
/// be attributed to the lambda under capture, e.g. public-api's claim seeding fulfillment's
/// pending claims).
struct Rig {
    store: Store,
    seed: Store,
    cap: Capture,
}

async fn rig_or_skip(test: &str) -> Option<Rig> {
    let (url, explicit) = match std::env::var("DYNAMODB_LOCAL_URL") {
        Ok(v) => (v, true),
        Err(_) => ("http://localhost:8000".into(), false),
    };
    let config = aws_config::defaults(aws_config::BehaviorVersion::latest())
        .endpoint_url(&url)
        .region("us-east-1")
        .test_credentials()
        .load()
        .await;
    let plain = aws_sdk_dynamodb::Client::new(&config);
    if plain.list_tables().send().await.is_err() {
        if explicit {
            panic!(
                "DYNAMODB_LOCAL_URL is set but dynamodb-local is unreachable — \
                 refusing to skip (this would forge a green run)"
            );
        }
        eprintln!("SKIP {test}: no dynamodb-local at {url}");
        return None;
    }
    let cap = Capture::default();
    let intercepted = aws_sdk_dynamodb::Client::from_conf(
        aws_sdk_dynamodb::config::Builder::from(&config)
            .interceptor(cap.clone())
            .build(),
    );
    let table = format!("t-{test}");
    let seed = Store::new(plain, table.clone());
    seed.create_table_for_tests().await.unwrap();
    Some(Rig {
        store: Store::new(intercepted, table),
        seed,
        cap,
    })
}

// ---------------------------------------------------------------------------
// request-shape extraction
// ---------------------------------------------------------------------------

/// One captured request shape — the unit the corpus and the simulator matrix share.
/// `leading_keys` holds `dynamodb:LeadingKeys` context values (key-prefix through the
/// first `#`); for GSI queries it is the *index* leading key and `index` names the GSI.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, serde::Serialize)]
struct Op {
    action: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    via: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    index: Option<String>,
    leading_keys: BTreeSet<String>,
    attributes: BTreeSet<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    select: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    return_values: Option<String>,
}

/// Expression operator keywords — every one is a DynamoDB **reserved word**, so the
/// service itself rejects an un-aliased attribute by any of these names (they can only
/// reach an expression via a `#alias`, which resolves before this list is consulted).
/// Skipping them unconditionally is therefore provably safe. (#142 item 4)
const EXPR_OPERATORS: &[&str] = &[
    "set", "remove", "add", "delete", "and", "or", "not", "between", "in",
];

/// Expression function names — skipped ONLY when the token is actually a call (next
/// non-space char is `(`). Some of these are also reserved words (`contains`, `size`),
/// others are not (`attribute_exists`, `begins_with`, `if_not_exists`, `list_append`,
/// `attribute_type`) — a future un-aliased attribute named like an unreserved function
/// would otherwise be silently dropped from a `dynamodb:Attributes` allowlist, which is
/// exactly the 403 class this harness exists to prevent. Gating on `(` makes the
/// reserved-or-not distinction moot. (#142 item 4)
const EXPR_FUNCTIONS: &[&str] = &[
    "attribute_exists",
    "attribute_not_exists",
    "attribute_type",
    "begins_with",
    "contains",
    "size",
    "if_not_exists",
    "list_append",
];

/// Pull attribute names out of an expression string: identifiers minus `:values` and
/// keywords, `#aliases` resolved via ExpressionAttributeNames, document paths reduced to
/// their top-level name (which is what `dynamodb:Attributes` carries).
fn expr_attr_names(expr: &str, aliases: &BTreeMap<String, String>, out: &mut BTreeSet<String>) {
    let chars: Vec<char> = expr.chars().collect();
    fn flush(
        tok: &mut String,
        is_call: bool,
        aliases: &BTreeMap<String, String>,
        out: &mut BTreeSet<String>,
    ) {
        if tok.is_empty() {
            return;
        }
        let t = std::mem::take(tok);
        if t.starts_with(':') || t.chars().all(|c| c.is_ascii_digit()) {
            return;
        }
        let base = t.split('.').next().unwrap();
        let lower = base.to_ascii_lowercase();
        if let Some(alias) = base.strip_prefix('#') {
            let resolved = aliases
                .get(&format!("#{alias}"))
                .unwrap_or_else(|| panic!("unresolved expression alias #{alias} in {t:?}"));
            out.insert(resolved.clone());
        } else if EXPR_OPERATORS.contains(&lower.as_str())
            || (is_call && EXPR_FUNCTIONS.contains(&lower.as_str()))
        {
            // operator: reserved word, never an attribute; function name: only a call
        } else {
            out.insert(base.to_string());
        }
    }
    let mut tok = String::new();
    for i in 0..chars.len() {
        let c = chars[i];
        if c.is_ascii_alphanumeric() || matches!(c, '_' | '#' | ':' | '.') {
            tok.push(c);
        } else {
            // is the token we just finished a function call? scan past whitespace for `(`
            let is_call = chars[i..]
                .iter()
                .find(|ch| !ch.is_ascii_whitespace())
                .is_some_and(|ch| *ch == '(');
            flush(&mut tok, is_call, aliases, out);
        }
    }
    // end-of-string terminator: nothing follows, so a trailing token is never a call
    flush(&mut tok, false, aliases, out);
}

/// The paren-gating contract, pinned (#142 item 4): a function-NAMED token is dropped
/// only when it is actually a call; anywhere else it is an attribute and must survive
/// into the allowlist. No live expression exercises the bare-name case (that's exactly
/// why the gap was latent), so this test is the only thing keeping it true.
#[test]
fn expr_extractor_keeps_bare_function_names_and_drops_calls() {
    let aliases = BTreeMap::from([("#st".to_string(), "status".to_string())]);
    let mut out = BTreeSet::new();
    expr_attr_names(
        "attribute_not_exists(gsi1pk) AND size = :v AND begins_with (sk, :c) AND #st = :s AND contains(tags, :t)",
        &aliases,
        &mut out,
    );
    let got: Vec<&str> = out.iter().map(String::as_str).collect();
    // kept: gsi1pk (call ARG), size (bare attr despite being a function name), sk (arg),
    //       status (alias-resolved), tags (arg). dropped: the three calls + operators.
    assert_eq!(got, ["gsi1pk", "size", "sk", "status", "tags"]);

    // operator keywords stay dropped even bare — they're reserved words.
    let mut out2 = BTreeSet::new();
    expr_attr_names("a BETWEEN :lo AND :hi", &BTreeMap::new(), &mut out2);
    assert_eq!(out2.iter().collect::<Vec<_>>(), [&"a".to_string()]);
}

/// Key prefix through the first `#` — `"GAME#x"` → `"GAME#"`, `"SYNC#STATE"` → `"SYNC#"`;
/// a value with no `#` (GSI partition constants) is used whole.
fn key_prefix(v: &str) -> String {
    match v.find('#') {
        Some(i) => v[..=i].to_string(),
        None => v.to_string(),
    }
}

fn string_attr(map: &Value, name: &str) -> Option<String> {
    map.get(name)?.get("S")?.as_str().map(str::to_string)
}

/// Extract one Op from a single-item request payload (GetItem/PutItem/UpdateItem/
/// DeleteItem/Query/Scan bodies, and the per-element payloads inside TransactWriteItems).
fn shape(action: &str, via: Option<&str>, v: &Value) -> Op {
    let aliases: BTreeMap<String, String> = v
        .get("ExpressionAttributeNames")
        .and_then(Value::as_object)
        .map(|m| {
            m.iter()
                .map(|(k, val)| (k.clone(), val.as_str().unwrap().to_string()))
                .collect()
        })
        .unwrap_or_default();
    let mut attributes = BTreeSet::new();
    let mut leading_keys = BTreeSet::new();

    for source in ["Key", "Item"] {
        if let Some(map) = v.get(source).and_then(Value::as_object) {
            for k in map.keys() {
                attributes.insert(k.clone());
            }
            if let Some(pk) = string_attr(v.get(source).unwrap(), "pk") {
                leading_keys.insert(key_prefix(&pk));
            }
        }
    }
    for field in [
        "UpdateExpression",
        "ConditionExpression",
        "FilterExpression",
        "ProjectionExpression",
        "KeyConditionExpression",
    ] {
        if let Some(expr) = v.get(field).and_then(Value::as_str) {
            expr_attr_names(expr, &aliases, &mut attributes);
        }
    }
    // Query: resolve the partition-key equality to a LeadingKeys context value. Base-table
    // queries key on `pk`; GSI queries key on the index's own partition attribute.
    if let Some(kce) = v.get("KeyConditionExpression").and_then(Value::as_str) {
        let eav = v.get("ExpressionAttributeValues");
        for segment in kce.split(" AND ") {
            let Some((lhs, rhs)) = segment.split_once('=') else {
                continue;
            };
            let (lhs, rhs) = (lhs.trim(), rhs.trim());
            if !rhs.starts_with(':') {
                continue;
            }
            let name = lhs
                .strip_prefix('#')
                .map(|a| aliases[&format!("#{a}")].clone())
                .unwrap_or_else(|| lhs.to_string());
            let is_partition = match v.get("IndexName").and_then(Value::as_str) {
                None => name == "pk",
                Some(_) => name.ends_with("pk"),
            };
            if is_partition && let Some(val) = eav.and_then(|m| string_attr(m, rhs)) {
                leading_keys.insert(key_prefix(&val));
            }
        }
    }
    Op {
        action: format!("dynamodb:{action}"),
        via: via.map(str::to_string),
        index: v
            .get("IndexName")
            .and_then(Value::as_str)
            .map(str::to_string),
        leading_keys,
        attributes,
        select: v.get("Select").and_then(Value::as_str).map(str::to_string),
        return_values: v
            .get("ReturnValues")
            .and_then(Value::as_str)
            .map(str::to_string),
    }
}

fn ops_from_request(target: &str, body: &str) -> Vec<Op> {
    let v: Value = serde_json::from_str(body).expect("request body is JSON");
    let op = target.rsplit('.').next().unwrap_or(target);
    match op {
        "GetItem" | "PutItem" | "UpdateItem" | "DeleteItem" | "Query" | "Scan" => {
            vec![shape(op, None, &v)]
        }
        "BatchGetItem" => {
            let tables = v["RequestItems"].as_object().unwrap();
            let mut ops = Vec::new();
            for req in tables.values() {
                let mut attributes = BTreeSet::new();
                let mut leading_keys = BTreeSet::new();
                for key in req["Keys"].as_array().unwrap() {
                    for k in key.as_object().unwrap().keys() {
                        attributes.insert(k.clone());
                    }
                    if let Some(pk) = string_attr(key, "pk") {
                        leading_keys.insert(key_prefix(&pk));
                    }
                }
                let aliases = BTreeMap::new();
                if let Some(pe) = req.get("ProjectionExpression").and_then(Value::as_str) {
                    expr_attr_names(pe, &aliases, &mut attributes);
                }
                ops.push(Op {
                    action: "dynamodb:BatchGetItem".into(),
                    via: None,
                    index: None,
                    leading_keys,
                    attributes,
                    select: None,
                    return_values: None,
                });
            }
            ops
        }
        "TransactWriteItems" => v["TransactItems"]
            .as_array()
            .unwrap()
            .iter()
            .map(|item| {
                let (kind, payload) = item.as_object().unwrap().iter().next().unwrap();
                let action = match kind.as_str() {
                    "Put" => "PutItem",
                    "Update" => "UpdateItem",
                    "Delete" => "DeleteItem",
                    "ConditionCheck" => "ConditionCheckItem",
                    other => panic!("unhandled transact element {other}"),
                };
                shape(action, Some("TransactWriteItems"), payload)
            })
            .collect(),
        other => panic!("unhandled x-amz-target op {other} — extend ops_from_request"),
    }
}

// ---------------------------------------------------------------------------
// driver
// ---------------------------------------------------------------------------

type MethodOps = BTreeMap<String, BTreeSet<Op>>;

/// Run one store method under capture and fold its request shapes into the corpus.
/// Returns THIS leg's parsed ops so a caller can assert leg-specific shapes — the merged
/// corpus map can't do that once entries from earlier legs share the method key (#142).
async fn capture<F: Future>(
    cap: &Capture,
    methods: &mut MethodOps,
    method: &str,
    fut: F,
) -> Vec<Op> {
    cap.drain();
    fut.await;
    let reqs = cap.drain();
    assert!(!reqs.is_empty(), "{method}: captured no requests");
    let entry = methods.entry(method.to_string()).or_default();
    let mut leg = Vec::new();
    for (target, body) in reqs {
        for op in ops_from_request(&target, &body) {
            entry.insert(op.clone());
            leg.push(op);
        }
    }
    leg
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
        keyindex: 0,
        requires_choice: false,
        steam_app_id: None,
        appid_source: None,
        owned_by_ben: false,
        hidden_source: None,
    }
}

fn link(token: &str) -> Link {
    Link {
        token: token.into(),
        label: "dave".into(),
        gift_note: None,
        thank_note: None,
        thanked_at: None,
        claims_allowed: 1,
        claims_used: 0,
        revoked: false,
        expires_at: None,
        created_at: datetime!(2026-07-02 00:00 UTC),
    }
}

fn steam_app_stub(app_id: u32) -> SteamAppCache {
    SteamAppCache {
        app_id,
        detail: None,
        overall: None,
        recent: None,
        fetched_at: 1_800_000_000,
        reviews_fetched_at: 1_800_000_000,
    }
}

const NOW_EPOCH: i64 = 1_900_000_000;

// ---------------------------------------------------------------------------
// per-lambda drivers — each `capture` call cites the real caller it mirrors
// ---------------------------------------------------------------------------

/// public-api (`crates/public-api/src/lib.rs`) — the unauthenticated internet-facing lambda.
async fn drive_public(rig: &Rig) -> MethodOps {
    let (s, cap) = (&rig.store, &rig.cap);
    let gid = game_id("gk1", "mn");
    let now = datetime!(2026-07-03 00:00 UTC);
    rig.seed.put_game(&game(1, true)).await.unwrap();
    rig.seed.create_link(&link("ptok")).await.unwrap();
    rig.seed
        .put_steam_app(&steam_app_stub(570), SteamAppPutGuard::Absent)
        .await
        .unwrap();

    let mut m = MethodOps::new();
    capture(cap, &mut m, "list_listable_games", async {
        s.list_listable_games().await.unwrap();
    })
    .await;
    capture(cap, &mut m, "get_game", async {
        s.get_game(&gid).await.unwrap().unwrap();
    })
    .await;
    capture(cap, &mut m, "batch_get_games", async {
        s.batch_get_games(std::slice::from_ref(&gid)).await.unwrap();
    })
    .await;
    capture(cap, &mut m, "get_link", async {
        s.get_link("ptok").await.unwrap().unwrap();
    })
    .await;
    // the claim transact: game update + link update + claim put (lib.rs:686)
    capture(cap, &mut m, "claim_game", async {
        s.claim_game("ptok", &gid, "pc1", now).await.unwrap();
    })
    .await;
    capture(cap, &mut m, "claims_for_link", async {
        s.claims_for_link("ptok").await.unwrap();
    })
    .await;
    capture(cap, &mut m, "set_link_thanks", async {
        s.set_link_thanks("ptok", "ty!", now).await.unwrap();
    })
    .await;
    capture(cap, &mut m, "get_steam_app", async {
        s.get_steam_app(570).await.unwrap();
    })
    .await;
    capture(cap, &mut m, "batch_get_steam_genres_tags", async {
        s.batch_get_steam_genres_tags(&[570]).await.unwrap();
    })
    .await;
    // #86 OIDC nonce mint + single-use take
    capture(cap, &mut m, "put_oidc_state", async {
        s.put_oidc_state("nonce1", "ctx", NOW_EPOCH).await.unwrap();
    })
    .await;
    capture(cap, &mut m, "take_oidc_state", async {
        s.take_oidc_state("nonce1", NOW_EPOCH - 60).await.unwrap();
    })
    .await;
    // cached_owned_or_fetch's two dynamo legs (public-api lib.rs:500)
    capture(cap, &mut m, "get_steam_owned", async {
        s.get_steam_owned("76561198000000001").await.unwrap();
    })
    .await;
    capture(cap, &mut m, "put_steam_owned", async {
        s.put_steam_owned("76561198000000001", &[570], NOW_EPOCH)
            .await
            .unwrap();
    })
    .await;
    m
}

/// admin-api (`crates/admin-api/src/lib.rs`) — session-authenticated, owns SESSION#.
async fn drive_admin(rig: &Rig) -> MethodOps {
    let (s, cap) = (&rig.store, &rig.cap);
    let (gid2, gid3) = (game_id("gk2", "mn"), game_id("gk3", "mn"));
    let now = datetime!(2026-07-03 01:00 UTC);
    rig.seed.put_game(&game(2, true)).await.unwrap();
    rig.seed.put_game(&game(3, true)).await.unwrap();
    rig.seed
        .put_steam_app(&steam_app_stub(620), SteamAppPutGuard::Absent)
        .await
        .unwrap();

    let mut m = MethodOps::new();
    capture(cap, &mut m, "create_session", async {
        s.create_session("sess-a", NOW_EPOCH).await.unwrap();
    })
    .await;
    capture(cap, &mut m, "get_session", async {
        s.get_session("sess-a").await.unwrap().unwrap();
    })
    .await;
    capture(cap, &mut m, "delete_session", async {
        s.delete_session("sess-a").await.unwrap();
    })
    .await;
    capture(cap, &mut m, "create_link", async {
        s.create_link(&link("atok2")).await.unwrap();
    })
    .await;
    capture(cap, &mut m, "get_link", async {
        s.get_link("atok2").await.unwrap().unwrap();
    })
    .await;
    capture(cap, &mut m, "update_link_meta", async {
        let mut l = link("atok2");
        l.label = "renamed".into();
        s.update_link_meta(&l).await.unwrap();
    })
    .await;
    // both legs (SET and REMOVE) captured separately so each asserts its own traffic
    capture(cap, &mut m, "set_link_gift_note", async {
        s.set_link_gift_note("atok2", Some("hi")).await.unwrap();
    })
    .await;
    capture(cap, &mut m, "set_link_gift_note", async {
        s.set_link_gift_note("atok2", None).await.unwrap();
    })
    .await;
    capture(cap, &mut m, "claims_for_link", async {
        s.claims_for_link("atok2").await.unwrap();
    })
    .await;
    capture(cap, &mut m, "list_links", async {
        s.list_links().await.unwrap();
    })
    .await;
    capture(cap, &mut m, "list_all_games", async {
        s.list_all_games().await.unwrap();
    })
    .await;
    capture(cap, &mut m, "get_game", async {
        s.get_game(&gid2).await.unwrap().unwrap();
    })
    .await;
    // both legs captured separately so each asserts its own traffic
    capture(cap, &mut m, "set_game_hidden", async {
        s.set_game_hidden(&gid2, true).await.unwrap();
    })
    .await;
    capture(cap, &mut m, "set_game_hidden", async {
        s.set_game_hidden(&gid2, false).await.unwrap();
    })
    .await;
    capture(cap, &mut m, "set_game_steam_appid_admin", async {
        s.set_game_steam_appid_admin(&gid2, Some(620))
            .await
            .unwrap();
    })
    .await;
    // ben's self-claim: game update + claim put transact, no link item
    capture(cap, &mut m, "claim_game_self", async {
        s.claim_game_self(&gid3, "ac1", now).await.unwrap();
    })
    .await;
    capture(cap, &mut m, "batch_get_steam_apps", async {
        s.batch_get_steam_apps(&[620]).await.unwrap();
    })
    .await;
    capture(cap, &mut m, "get_steam_app", async {
        s.get_steam_app(620).await.unwrap();
    })
    .await;
    capture(cap, &mut m, "put_steam_identity", async {
        s.put_steam_identity("76561198000000002").await.unwrap();
    })
    .await;
    capture(cap, &mut m, "get_steam_identity", async {
        s.get_steam_identity().await.unwrap();
    })
    .await;
    capture(cap, &mut m, "delete_steam_identity", async {
        s.delete_steam_identity().await.unwrap();
    })
    .await;
    // cached_owned_or_fetch's two dynamo legs (admin-api lib.rs:1088)
    capture(cap, &mut m, "get_steam_owned", async {
        s.get_steam_owned("76561198000000002").await.unwrap();
    })
    .await;
    capture(cap, &mut m, "put_steam_owned", async {
        s.put_steam_owned("76561198000000002", &[620], NOW_EPOCH)
            .await
            .unwrap();
    })
    .await;
    capture(cap, &mut m, "get_sync_state", async {
        s.get_sync_state().await.unwrap();
    })
    .await;
    capture(cap, &mut m, "get_sync_run", async {
        s.get_sync_run().await.unwrap();
    })
    .await;
    m
}

/// fulfillment (`crates/fulfillment/src/lib.rs`) — internal invoke-only; owns SYNC#.
async fn drive_fulfillment(rig: &Rig) -> MethodOps {
    let (s, cap) = (&rig.store, &rig.cap);
    let now = datetime!(2026-07-03 02:00 UTC);
    let gid = |n: u32| game_id(&format!("gk{n}"), "mn");
    for n in 4..=10 {
        rig.seed.put_game(&game(n, true)).await.unwrap();
    }
    for tok in ["ftok", "ftok2", "ftok3"] {
        rig.seed.create_link(&link(tok)).await.unwrap();
    }
    // pending claims are minted by the API lambdas; seed them uncaptured
    rig.seed
        .claim_game("ftok", &gid(4), "fc1", now)
        .await
        .unwrap();
    rig.seed
        .claim_game("ftok2", &gid(5), "fc2", now)
        .await
        .unwrap();
    rig.seed
        .claim_game("ftok3", &gid(6), "fc3", now)
        .await
        .unwrap();
    rig.seed.claim_game_self(&gid(7), "sc1", now).await.unwrap();
    rig.seed.claim_game_self(&gid(8), "sc2", now).await.unwrap();
    rig.seed.claim_game_self(&gid(9), "sc3", now).await.unwrap();
    rig.seed
        .put_steam_app(&steam_app_stub(700), SteamAppPutGuard::Absent)
        .await
        .unwrap();

    let mut m = MethodOps::new();
    capture(cap, &mut m, "get_sync_state", async {
        s.get_sync_state().await.unwrap();
    })
    .await;
    capture(cap, &mut m, "put_sync_state", async {
        s.put_sync_state(&SyncState {
            last_run_epoch: NOW_EPOCH,
            ok: true,
            cookie_ok: true,
            games_written: 1,
            message: "ok".into(),
            private_pinged: false,
        })
        .await
        .unwrap();
    })
    .await;
    capture(cap, &mut m, "begin_sync_run", async {
        s.begin_sync_run(NOW_EPOCH).await.unwrap();
    })
    .await;
    capture(cap, &mut m, "end_sync_run", async {
        s.end_sync_run().await.unwrap();
    })
    .await;
    capture(cap, &mut m, "list_pending_claims", async {
        s.list_pending_claims().await.unwrap();
    })
    .await;
    capture(cap, &mut m, "record_choice_intent", async {
        s.record_choice_intent("ftok", "fc1", vec!["tpk1".into()])
            .await
            .unwrap();
    })
    .await;
    capture(cap, &mut m, "fulfill_claim", async {
        s.fulfill_claim("ftok", "fc1", &gid(4), "https://gift.example/x")
            .await
            .unwrap();
    })
    .await;
    capture(cap, &mut m, "fulfill_self_claim", async {
        s.fulfill_self_claim("sc1", &gid(7), "AAAAA-BBBBB-CCCCC")
            .await
            .unwrap();
    })
    .await;
    capture(cap, &mut m, "compensate_claim", async {
        s.compensate_claim("ftok2", "fc2", &gid(5)).await.unwrap();
    })
    .await;
    capture(cap, &mut m, "compensate_self_claim", async {
        s.compensate_self_claim("sc2", &gid(8)).await.unwrap();
    })
    .await;
    capture(cap, &mut m, "fail_claim_dead_key", async {
        s.fail_claim_dead_key("ftok3", "fc3", &gid(6), "dead key")
            .await
            .unwrap();
    })
    .await;
    capture(cap, &mut m, "fail_self_claim_dead_key", async {
        s.fail_self_claim_dead_key("sc3", &gid(9), "dead key")
            .await
            .unwrap();
    })
    .await;
    capture(cap, &mut m, "upsert_game_from_sync", async {
        let mut fresh = game(10, true);
        fresh.title = "Game 10 (renamed)".into();
        s.upsert_game_from_sync(fresh).await.unwrap();
    })
    .await;
    // first-insert branch (#142 item 3): gk11 is never seeded, so merge_sync takes the
    // None arm → bare PutItem guarded only by attribute_not_exists(pk) stamping version 1.
    // Assert THIS leg captured that put. Discriminator (#145, post-#134): the existing-arm
    // guarded put gates on `version`, while the first-insert condition names only pk — so
    // a PutItem whose attributes lack `appid_source` AND whose condition never binds
    // :seen identifies the first-insert shape. (Pre-#134 the guarded put named
    // appid_source in BOTH its condition arms — `#asrc = :asrc` and
    // attribute_not_exists(#asrc) — an invariant stronger than first written up.)
    // #145 fixture pin: the absence-of-appid_source discriminator is sound ONLY while the
    // fixture carries no appid pair (game_item writes top-level appid_source when Some) —
    // fail HERE with a pinpoint message rather than confusingly at the shape assert.
    assert!(
        game(11, true).appid_source.is_none(),
        "#145: game(11) grew an appid_source — pick a new discriminator for the first-insert leg"
    );
    let first_insert = capture(cap, &mut m, "upsert_game_from_sync", async {
        s.upsert_game_from_sync(game(11, true)).await.unwrap();
    })
    .await;
    assert!(
        first_insert.iter().any(|o| o.action == "dynamodb:PutItem"
            && o.attributes.contains("pk")
            && !o.attributes.contains("appid_source")),
        "first-insert leg did not capture the attribute_not_exists(pk) PutItem shape"
    );
    capture(cap, &mut m, "set_game_steam_appid_if_unclaimed", async {
        s.set_game_steam_appid_if_unclaimed(&gid(10), 620, AppidSource::Title)
            .await
            .unwrap();
    })
    .await;
    capture(cap, &mut m, "set_game_owned_by_ben", async {
        s.set_game_owned_by_ben(&gid(10), true).await.unwrap();
    })
    .await;
    capture(cap, &mut m, "auto_hide_game", async {
        s.auto_hide_game(&gid(10)).await.unwrap();
    })
    .await;
    capture(cap, &mut m, "get_game", async {
        s.get_game(&gid(4)).await.unwrap().unwrap();
    })
    .await;
    capture(cap, &mut m, "list_all_games", async {
        s.list_all_games().await.unwrap();
    })
    .await;
    capture(cap, &mut m, "get_steam_app", async {
        s.get_steam_app(700).await.unwrap();
    })
    .await;
    capture(cap, &mut m, "get_steam_app_versioned", async {
        s.get_steam_app_versioned(700).await.unwrap();
    })
    .await;
    capture(cap, &mut m, "put_steam_app", async {
        s.put_steam_app(&steam_app_stub(701), SteamAppPutGuard::Absent)
            .await
            .unwrap();
    })
    .await;
    capture(cap, &mut m, "batch_get_steam_apps", async {
        s.batch_get_steam_apps(&[700]).await.unwrap();
    })
    .await;
    capture(cap, &mut m, "get_steam_identity", async {
        s.get_steam_identity().await.unwrap();
    })
    .await;
    capture(cap, &mut m, "get_steam_owned", async {
        s.get_steam_owned("76561198000000003").await.unwrap();
    })
    .await;
    capture(cap, &mut m, "put_steam_owned", async {
        s.put_steam_owned("76561198000000003", &[700], NOW_EPOCH)
            .await
            .unwrap();
    })
    .await;
    m
}

// ---------------------------------------------------------------------------
// policy generation — the corpus is the only input, so the allowlists cannot
// drift from what the code actually sends
// ---------------------------------------------------------------------------

fn action_union(methods: &MethodOps) -> BTreeSet<String> {
    methods
        .values()
        .flatten()
        .map(|op| op.action.clone())
        .collect()
}

fn leading_key_union(methods: &MethodOps) -> BTreeSet<String> {
    methods
        .values()
        .flatten()
        .flat_map(|op| op.leading_keys.iter().cloned())
        .collect()
}

/// Actions that address items by key — the set a LeadingKeys deny must cover.
/// (Scan is unconditionally absent from every policy this file emits.)
fn key_addressed(actions: &BTreeSet<String>) -> Vec<Value> {
    actions
        .iter()
        .cloned()
        .chain(std::iter::once("dynamodb:ConditionCheckItem".to_string()))
        .filter(|a| a != "dynamodb:Scan")
        .collect::<BTreeSet<_>>()
        .into_iter()
        .map(Value::String)
        .collect()
}

/// The traffic-vs-deny cross-assert: a deny prefix that appears in the lambda's OWN
/// captured traffic means regeneration would emit a policy whose Deny 403s a real path —
/// fail the build here instead. (OMBB's #141 review: this guarded admin/fulfillment but
/// not public's SESSION#/SYNC# deny; every deny now routes through it.)
fn assert_denies_unused(methods: &MethodOps, prefixes: &[&str]) {
    let leading = leading_key_union(methods);
    for p in prefixes {
        let stripped = p.trim_end_matches('*');
        assert!(
            !leading.contains(stripped),
            "corpus shows a captured request into denied partition {p} — the deny would break a real path"
        );
    }
}

fn deny_statement(sid: &str, actions: &BTreeSet<String>, prefixes: &[&str]) -> Value {
    serde_json::json!({
        "Sid": sid,
        "Effect": "Deny",
        "Action": key_addressed(actions),
        "Resource": ["${table_arn}"],
        "Condition": {
            "ForAnyValue:StringLike": {
                "dynamodb:LeadingKeys": prefixes
            }
        }
    })
}

fn policy_json(statements: Vec<Value>) -> String {
    let doc = serde_json::json!({
        "Version": "2012-10-17",
        "Statement": statements,
    });
    serde_json::to_string_pretty(&doc).unwrap() + "\n"
}

/// admin-api / fulfillment: their captured action set (plus ConditionCheckItem, kept for
/// parity with the certified #84 public policy — TransactWriteItems authorization is
/// documented as per-element underlying ops, but we hold the defensive line everywhere
/// until live traffic proves otherwise), and a Deny on the partitions the lambda has no
/// business in.
fn backend_policy(methods: &MethodOps, deny_sid: &str, deny_prefixes: &[&str]) -> String {
    let mut actions = action_union(methods);
    actions.insert("dynamodb:ConditionCheckItem".into());
    assert_denies_unused(methods, deny_prefixes);
    let allow = serde_json::json!({
        "Sid": "DataPlane",
        "Effect": "Allow",
        "Action": actions.iter().cloned().map(Value::String).collect::<Vec<_>>(),
        "Resource": ["${table_arn}", "${table_arn}/index/*"],
    });
    policy_json(vec![
        allow,
        deny_statement(deny_sid, &actions, deny_prefixes),
    ])
}

/// public-api: #84's shape (no Scan, Deny SESSION#/SYNC#) with UpdateItem moved out of the
/// broad statement into per-prefix scoped-writer statements whose dynamodb:Attributes
/// allowlists are the captured traffic, verbatim.
fn public_policy(methods: &MethodOps) -> String {
    assert_denies_unused(methods, &["SESSION#*", "SYNC#*"]);
    let mut actions = action_union(methods);
    assert!(
        !actions.contains("dynamodb:Scan"),
        "public-api captured a Scan — #84's no-Scan premise is broken, do not ship"
    );
    actions.insert("dynamodb:ConditionCheckItem".into());
    actions.remove("dynamodb:UpdateItem");

    // group public UpdateItem traffic by leading key → per-prefix attribute allowlists
    let mut update_groups: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for op in methods.values().flatten() {
        if op.action != "dynamodb:UpdateItem" {
            continue;
        }
        assert_eq!(
            op.return_values, None,
            "public UpdateItem sets ReturnValues — the scoped statements pin it, revisit"
        );
        for lk in &op.leading_keys {
            update_groups
                .entry(lk.clone())
                .or_default()
                .extend(op.attributes.iter().cloned());
        }
    }
    assert_eq!(
        update_groups.keys().cloned().collect::<Vec<_>>(),
        vec!["GAME#".to_string(), "LINK#".to_string()],
        "public UpdateItem traffic outside GAME#/LINK# — extend the scoped statements deliberately"
    );

    let mut statements = vec![serde_json::json!({
        "Sid": "DataPlaneNoScanNoUpdate",
        "Effect": "Allow",
        "Action": actions.iter().cloned().map(Value::String).collect::<Vec<_>>(),
        "Resource": ["${table_arn}", "${table_arn}/index/*"],
    })];
    for (prefix, attrs) in &update_groups {
        let sid = format!(
            "ScopedUpdate{}",
            prefix.trim_end_matches('#').to_ascii_uppercase()
        );
        statements.push(serde_json::json!({
            "Sid": sid,
            "Effect": "Allow",
            "Action": ["dynamodb:UpdateItem"],
            "Resource": ["${table_arn}"],
            "Condition": {
                "ForAllValues:StringLike": { "dynamodb:LeadingKeys": [format!("{prefix}*")] },
                "ForAllValues:StringEquals": {
                    "dynamodb:Attributes": attrs.iter().cloned().collect::<Vec<_>>()
                },
                // the code never sets ReturnValues (asserted above); pinning it closes the
                // "UpdateItem + ReturnValues=ALL_OLD as a full-item read" channel
                "StringEqualsIfExists": {
                    "dynamodb:ReturnValues": ["NONE", "UPDATED_OLD", "UPDATED_NEW"]
                }
            }
        }));
    }
    let mut all_actions = actions.clone();
    all_actions.insert("dynamodb:UpdateItem".into());
    statements.push(deny_statement(
        "DenySessionAndSyncItems",
        &all_actions,
        &["SESSION#*", "SYNC#*"],
    ));
    policy_json(statements)
}

// ---------------------------------------------------------------------------
// the test
// ---------------------------------------------------------------------------

#[tokio::test]
async fn iam_corpus_and_policies_match_code() {
    let Some(rig) = rig_or_skip("iam-capture").await else {
        return;
    };

    let mut corpus: BTreeMap<String, MethodOps> = BTreeMap::new();
    corpus.insert("public-api".into(), drive_public(&rig).await);
    corpus.insert("admin-api".into(), drive_admin(&rig).await);
    corpus.insert("fulfillment".into(), drive_fulfillment(&rig).await);

    // partition-hygiene facts the deny statements rest on, asserted from real traffic
    let ful_leading = leading_key_union(&corpus["fulfillment"]);
    assert!(!ful_leading.contains("SESSION#") && !ful_leading.contains("OIDCSTATE#"));
    let adm_leading = leading_key_union(&corpus["admin-api"]);
    assert!(!adm_leading.contains("OIDCSTATE#"));

    let files: BTreeMap<&str, String> = BTreeMap::from([
        (
            "iam-request-corpus.json",
            serde_json::to_string_pretty(&corpus).unwrap() + "\n",
        ),
        (
            "policies/dynamo-rw-public.json.tpl",
            public_policy(&corpus["public-api"]),
        ),
        (
            "policies/dynamo-rw-admin.json.tpl",
            backend_policy(&corpus["admin-api"], "DenyOidcItems", &["OIDCSTATE#*"]),
        ),
        (
            "policies/dynamo-rw-fulfillment.json.tpl",
            backend_policy(
                &corpus["fulfillment"],
                "DenySessionAndOidcItems",
                &["SESSION#*", "OIDCSTATE#*"],
            ),
        ),
    ]);

    let tf = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../terraform");
    let write_mode = std::env::var("IAM_CORPUS_WRITE").is_ok();
    for (rel, content) in &files {
        let path = tf.join(rel);
        if write_mode {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, content).unwrap();
            eprintln!("wrote {}", path.display());
        } else {
            let committed = std::fs::read_to_string(&path).unwrap_or_else(|_| {
                panic!(
                    "{rel} missing — run `IAM_CORPUS_WRITE=1 cargo test -p dynamo --test iam_capture`"
                )
            });
            assert!(
                committed == *content,
                "{rel} is stale: the code's dynamo request surface changed. Re-run with \
                 IAM_CORPUS_WRITE=1, review the diff like the IAM change it is, and commit."
            );
        }
    }
}
