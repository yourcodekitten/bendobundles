//! Key builders + item (de)serialization. The item shapes here are the storage contract.
use aws_sdk_dynamodb::types::AttributeValue;
use domain::{Claim, ClaimState, Game, Link};
use std::collections::HashMap;
use time::OffsetDateTime;

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

pub(crate) fn s(v: impl Into<String>) -> AttributeValue {
    AttributeValue::S(v.into())
}

/// Encode a timestamp as a numeric (epoch **seconds**) AttributeValue. Use this everywhere a time
/// enters a top-level attribute or a condition compare, so expiry math is numeric — never a
/// fractional-second-width-sensitive, offset-sensitive lexicographic string compare. (The claim's
/// `gsi2sk` is the one deliberate exception — ordering only; see `claim_item`.)
pub(crate) fn epoch_s(t: OffsetDateTime) -> AttributeValue {
    AttributeValue::N(t.unix_timestamp().to_string())
}

/// Build the (pk, sk) primary-key AttributeValues for a single-item get/update. The key *names*
/// are always the literals `"pk"`/`"sk"`; this just kills the repeated `AttributeValue::S(..)` at
/// call sites.
pub(crate) fn key_pair(
    pk: impl Into<String>,
    sk: impl Into<String>,
) -> (AttributeValue, AttributeValue) {
    (s(pk), s(sk))
}

pub fn game_item(g: &Game) -> HashMap<String, AttributeValue> {
    let mut item = HashMap::from([
        ("pk".into(), s(game_pk(&g.id))),
        ("sk".into(), s("META")),
        (
            "body".into(),
            s(serde_json::to_string(g).expect("game serializes")),
        ),
    ]);
    item.insert("status".into(), s(g.status.as_wire()));
    // Top-level `appid_source` mirrors the body so the mapper's PutItem condition can guard
    // against a concurrent admin Manual override. Only written when Some — attribute_not_exists
    // then correctly matches unmapped/legacy items (None → no attribute, condition fires).
    if let Some(src) = g.appid_source {
        item.insert("appid_source".into(), s(src.as_wire()));
    }
    // Top-level `hidden_source` mirrors the body so auto-hide's PutItem condition can guard
    // against racing an admin toggle. Only written when Some — attribute_not_exists then
    // correctly matches legacy items (never admin-touched → auto-hide eligible).
    if let Some(src) = g.hidden_source {
        item.insert("hidden_source".into(), s(src.as_wire()));
    }
    // Top-level `claim_id` mirrors the body so fulfill's flip can condition on ownership
    // (`claim_id = :cid`). Omitted when None so compensate's re-list transparently clears it.
    if let Some(cid) = &g.claim_id {
        item.insert("claim_id".into(), s(cid));
    }
    if g.is_listable() {
        item.insert("gsi1pk".into(), s("LISTABLE"));
        item.insert(
            "gsi1sk".into(),
            s(format!("{}#{}", g.title.to_lowercase(), g.id)),
        );
    }
    item
}

/// Top-level `claims_used` (N) is the **authoritative counter** — updated atomically via `ADD`
/// in `claim_game`'s transaction and enforced by that transaction's condition expression.
/// `body.claims_used` may go stale under concurrent claims; `Store::get_link` overrides it on
/// read from this top-level attribute so callers always see the enforcer's truth.
/// Serialize a Link for the `body` attribute — WITHOUT the gift note. The note
/// lives ONLY in the top-level `gift_note` attribute (`set_link_gift_note` is
/// its one writer; `link_from_item` overrides from it on every read). Keeping
/// it out of body entirely means "clear the note" leaves no copy at rest —
/// a body blob written while a note was set would otherwise retain the text
/// verbatim until the next body write (OMBB, #69 review). Every body writer
/// (create, `update_link_meta`, `claim_game`) must use this.
/// The thanks pair follows the same rule for the same reason (the friend's words
/// deserve the same no-copy-at-rest guarantee as ben's).
pub fn link_body(l: &Link) -> String {
    let noteless = Link {
        gift_note: None,
        thank_note: None,
        thanked_at: None,
        ..l.clone()
    };
    serde_json::to_string(&noteless).expect("link serializes")
}

pub fn link_item(l: &Link) -> HashMap<String, AttributeValue> {
    let mut item = HashMap::from([
        ("pk".into(), s(link_pk(&l.token))),
        ("sk".into(), s("META")),
        ("body".into(), s(link_body(l))),
        (
            "claims_allowed".into(),
            AttributeValue::N(l.claims_allowed.to_string()),
        ),
        (
            "claims_used".into(),
            AttributeValue::N(l.claims_used.to_string()),
        ),
        ("revoked".into(), AttributeValue::Bool(l.revoked)),
    ]);
    // expires_at is numeric (epoch seconds), so claim_game's expiry gate is a numeric compare.
    // Omitted when None (never-expires); update_link_meta REMOVEs it to match.
    if let Some(exp) = l.expires_at {
        item.insert("expires_at".into(), epoch_s(exp));
    }
    // Top-level `gift_note` is authoritative, like the enforcer attrs: the note is
    // editable post-creation via `set_link_gift_note`'s single-attribute SET/REMOVE,
    // so it must not live only in body where claim_game's `SET body` (serialized from
    // a pre-transaction read) would silently revert an edit landing in the window.
    // Omitted when None; `link_from_item` treats absence as no-note.
    if let Some(n) = &l.gift_note {
        item.insert("gift_note".into(), s(n));
    }
    // Same contract for the thanks pair (`set_link_thanks` is the one live writer —
    // links are created un-thanked — but a roundtrip through link_item must not
    // silently drop the fields).
    if let Some(n) = &l.thank_note {
        item.insert("thank_note".into(), s(n));
    }
    // Numeric like expires_at (`epoch_s`'s blanket rule for top-level times), even
    // though this one is display-only today.
    if let Some(at) = l.thanked_at {
        item.insert("thanked_at".into(), epoch_s(at));
    }
    item
}

pub fn claim_item(c: &Claim) -> HashMap<String, AttributeValue> {
    let mut item = HashMap::from([
        ("pk".into(), s(link_pk(&c.link_token))),
        ("sk".into(), s(claim_sk(&c.id))),
        (
            "body".into(),
            s(serde_json::to_string(c).expect("claim serializes")),
        ),
    ]);
    if c.state == ClaimState::Pending {
        item.insert("gsi2pk".into(), s("PENDINGCLAIM"));
        // gsi2sk stays RFC3339: it only ORDERS pending claims in the index, it is never an
        // expiry/enforcer compare — so the string form is fine here (unlike link expires_at).
        let ts = c
            .created_at
            .format(&time::format_description::well_known::Rfc3339)
            .expect("rfc3339");
        item.insert("gsi2sk".into(), s(ts));
    }
    item
}

/// Build the full item for a `SyncState` record. pk="SYNC#STATE", sk="META", body=json.
/// Use `Store::put_sync_state` / `Store::get_sync_state` — do not write SYNC#STATE items directly.
pub fn sync_state_item(state: &crate::SyncState) -> HashMap<String, AttributeValue> {
    HashMap::from([
        ("pk".into(), s("SYNC#STATE")),
        ("sk".into(), s("META")),
        (
            "body".into(),
            s(serde_json::to_string(state).expect("SyncState serializes")),
        ),
    ])
}

pub fn session_pk(token: &str) -> String {
    format!("SESSION#{token}")
}

/// Build the item for an admin session. pk="SESSION#<token>", sk="META", top-level
/// `expires_epoch` N + `ttl` N (same value). `ttl` is the DynamoDB TTL attribute — terraform
/// will enable it in plan 4. Until then, expiry enforcement is the caller's job (compare epoch).
pub fn session_item(token: &str, expires_epoch: i64) -> HashMap<String, AttributeValue> {
    let epoch_str = expires_epoch.to_string();
    HashMap::from([
        ("pk".into(), s(session_pk(token))),
        ("sk".into(), s("META")),
        ("expires_epoch".into(), AttributeValue::N(epoch_str.clone())),
        ("ttl".into(), AttributeValue::N(epoch_str)),
    ])
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

/// Seconds in seven days — the TTL window for STEAMOWN cache entries.
pub const STEAM_OWNED_TTL_SECS: i64 = 7 * 24 * 3600;

pub fn steam_app_pk(app_id: u32) -> String {
    format!("STEAMAPP#{app_id}")
}

/// Build the full item for a STEAMAPP enrichment cache entry.
/// pk="STEAMAPP#<app_id>", sk="META", body=JSON of [`crate::SteamAppCache`].
/// Use `Store::put_steam_app` / `Store::get_steam_app` — do not write STEAMAPP# items directly.
pub fn steam_app_item(
    cache: &crate::SteamAppCache,
) -> std::collections::HashMap<String, AttributeValue> {
    std::collections::HashMap::from([
        ("pk".into(), s(steam_app_pk(cache.app_id))),
        ("sk".into(), s("META")),
        (
            "body".into(),
            s(serde_json::to_string(cache).expect("SteamAppCache serializes")),
        ),
    ])
}

/// Build the full item for the CONFIG#STEAM identity record.
/// pk="CONFIG#STEAM", sk="META", body=JSON-serialized steamid string.
/// Use `Store::put_steam_identity` / `Store::get_steam_identity` — do not write CONFIG#STEAM items directly.
pub fn steam_identity_item(steamid: &str) -> HashMap<String, AttributeValue> {
    HashMap::from([
        ("pk".into(), s("CONFIG#STEAM")),
        ("sk".into(), s("META")),
        (
            "body".into(),
            s(serde_json::to_string(steamid).expect("steamid serializes")),
        ),
    ])
}

/// Build the full item for a STEAMOWN cache entry.
/// pk="STEAMOWN#<steamid>", sk="META", body=JSON of [`crate::SteamOwnedCache`],
/// `ttl` N = now_epoch + [`STEAM_OWNED_TTL_SECS`] (DynamoDB TTL attribute; terraform enables it
/// in plan 4 — until then, callers must check `fetched_at` manually).
pub fn steam_owned_item(
    steamid: &str,
    appids: &[u32],
    now_epoch: i64,
) -> HashMap<String, AttributeValue> {
    let ttl = now_epoch + STEAM_OWNED_TTL_SECS;
    let body = serde_json::to_string(&crate::SteamOwnedCache {
        appids: appids.to_vec(),
        fetched_at: now_epoch,
    })
    .expect("SteamOwnedCache serializes");
    let ttl_str = ttl.to_string();
    HashMap::from([
        ("pk".into(), s(format!("STEAMOWN#{steamid}"))),
        ("sk".into(), s("META")),
        ("body".into(), s(body)),
        ("ttl".into(), AttributeValue::N(ttl_str)),
    ])
}
