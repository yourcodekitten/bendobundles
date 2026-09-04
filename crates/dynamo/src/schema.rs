//! Key builders + item (de)serialization. The item shapes here are the storage contract.
use aws_sdk_dynamodb::types::AttributeValue;
use domain::{Claim, ClaimState, Friend, Game, Link};
use std::collections::HashMap;
use time::OffsetDateTime;

pub const GSI_LISTABLE: &str = "listable";
pub const GSI_PENDING: &str = "pending-claims";
/// Sparse gsi3: pk-only (`gsi3pk`, no sort key), hit only by links that carry a
/// `friend_id` (`set_link_friend` / `link_item` are the only writers of `gsi3pk`).
/// `Store::list_links_for_friend` is the one reader.
pub const GSI_FRIEND_LINKS: &str = "friend-links";

pub fn game_pk(id: &str) -> String {
    format!("GAME#{id}")
}
pub fn link_pk(token: &str) -> String {
    format!("LINK#{token}")
}
pub fn claim_sk(claim_id: &str) -> String {
    format!("CLAIM#{claim_id}")
}
/// pk for a friend's own record: `FRIEND#<id>`. Also the exact `gsi3pk` value
/// `set_link_friend`/`link_item` stamp on an assigned link — `list_links_for_friend`
/// queries gsi3 for this same string, so a link's gsi3pk is literally a friend pk.
pub fn friend_pk(id: &str) -> String {
    format!("FRIEND#{id}")
}
/// pk for a shelf token's pointer record: `SHELF#<token>`. The pointer is the ONLY
/// path from a bearer token to a friend id — `get_friend_by_shelf_token` reads it
/// first, then resolves the friend by id. `create_friend`/`reissue_shelf_token`/
/// `revoke_shelf_token` are its only writers, always inside a transaction with the
/// friend record so a token and `Friend.shelf_token` can never drift apart.
pub fn shelf_pk(token: &str) -> String {
    format!("SHELF#{token}")
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

/// `version` is the monotonic counter value this write CARRIES (#134): callers pass the
/// next value (seen+1, or 1 for first-insert/legacy adoption). A required param — not an
/// Option — so no game writer can forget the stamp; a missed bump is a compile error.
pub fn game_item(g: &Game, version: i64) -> HashMap<String, AttributeValue> {
    let mut item = HashMap::from([
        ("pk".into(), s(game_pk(&g.id))),
        ("sk".into(), s("META")),
        ("version".into(), AttributeValue::N(version.to_string())),
        (
            "body".into(),
            s(serde_json::to_string(g).expect("game serializes")),
        ),
    ]);
    item.insert("status".into(), s(g.status.as_wire()));
    // Top-level `appid_source` mirrors the body. Historically the mapper's write condition
    // guarded on it; since #134 the version counter carries that protection and no condition
    // references this mirror — it stays for parity/diagnostics (top-level truth visible in
    // console scans without parsing body). Only written when Some.
    if let Some(src) = g.appid_source {
        item.insert("appid_source".into(), s(src.as_wire()));
    }
    // Top-level `hidden_source` mirrors the body. Historically auto-hide's write condition
    // guarded on it; since #134 the version counter carries that protection (auto-hide's
    // read-screen still consults the body value). Stays for parity/diagnostics. Only
    // written when Some.
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
    // Exhaustive destructure — a new Link field breaks compilation HERE until its body
    // fate is decided (2026-08-05 family review, lilith's type-level strip). Fields bound
    // `_` are the stripped set: authoritative ONLY in top-level attrs, and (for unlock_at,
    // whose serde skips None) absent from the body even as a null key.
    let Link {
        token,
        label,
        gift_note: _,
        thank_note: _,
        thanked_at: _,
        claims_allowed,
        claims_used,
        revoked,
        expires_at,
        unlock_at: _,
        // Stripped: lives ONLY in the top-level `curated_game_ids` attribute.
        // Body-carried curation dies by rollback: a pre-field binary's Link
        // deserialize drops the unknown field and its SET body write-back
        // erases it. Top-level is structurally out of that blast radius.
        curated_game_ids: _,
        // authoritative top-level only; ALSO a gsi3 key — body copy would desync the index
        friend_id: _,
        created_at,
    } = l;
    let stripped = Link {
        token: token.clone(),
        label: label.clone(),
        gift_note: None,
        thank_note: None,
        thanked_at: None,
        claims_allowed: *claims_allowed,
        claims_used: *claims_used,
        revoked: *revoked,
        expires_at: *expires_at,
        unlock_at: None,
        curated_game_ids: None,
        friend_id: None,
        created_at: *created_at,
    };
    serde_json::to_string(&stripped).expect("link serializes")
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
    // unlock_at: expires_at's storage (top-level epoch seconds — the claim transaction's
    // numeric compare) with the NOTES' body contract (stripped from body; top-level is the
    // ONLY source). Omitted when None; set_link_unlock/remove_link_unlock REMOVE to unseal
    // (absent = born open / unsealed — the single representation, never null).
    if let Some(unlock) = l.unlock_at {
        item.insert("unlock_at".into(), epoch_s(unlock));
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
    // Top-level, order-preserving L (a String Set would sort/dedupe — order is
    // ben's pick order and duplicates were already 422'd at create). Written
    // once here; no update expression anywhere names this attribute, which is
    // the rollback immunity the spec pins.
    if let Some(ids) = &l.curated_game_ids {
        item.insert(
            "curated_game_ids".into(),
            AttributeValue::L(ids.iter().map(s).collect()),
        );
    }
    // friend_id + gsi3pk move together, ALWAYS (Store::set_link_friend's post-creation
    // contract) — this is create-time's half of the same rule, so a link cut with a
    // friend already assigned indexes on gsi3 without a second write. Omitted when
    // None: gsi3 is sparse, and `link_from_item` treats absence as no-friend.
    if let Some(fid) = &l.friend_id {
        item.insert("friend_id".into(), s(fid));
        item.insert("gsi3pk".into(), s(friend_pk(fid)));
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

/// Session pk = `SESSION#<hex(sha256(token))>` (#88): the cookie value itself never
/// touches the table, so any read primitive over the table yields non-replayable
/// digests instead of bearer tokens. This is the ONE place a session key is built —
/// `create_session` (via `session_item`), `get_session`, and `delete_session` all route
/// through it, so hash-on-write and hash-on-lookup cannot drift. Encoding is pinned
/// lowercase hex (64 chars) by a raw-item test: changing it silently invalidates every
/// live session, which is exactly what happened ONCE, deliberately, at #88's deploy
/// (no dual-lookup migration — a shim keeping raw pks alive would keep them replayable
/// for as long as it existed).
pub fn session_pk(token: &str) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(token.as_bytes());
    let hex: String = digest.iter().map(|b| format!("{b:02x}")).collect();
    format!("SESSION#{hex}")
}

/// Build the item for an admin session. pk="SESSION#<token>", sk="META", top-level
/// `expires_epoch` N + `ttl` N (same value). `ttl` is the DynamoDB TTL attribute (enabled in
/// `terraform/aws-dynamodb.tf`). DynamoDB expiry is lazy (up to ~48h late), so callers still
/// compare `expires_epoch` against wall-clock — TTL just reclaims storage afterward.
pub fn session_item(token: &str, expires_epoch: i64) -> HashMap<String, AttributeValue> {
    let epoch_str = expires_epoch.to_string();
    HashMap::from([
        ("pk".into(), s(session_pk(token))),
        ("sk".into(), s("META")),
        ("expires_epoch".into(), AttributeValue::N(epoch_str.clone())),
        ("ttl".into(), AttributeValue::N(epoch_str)),
    ])
}

/// A pending OpenID login's CSRF state lives ~5 minutes — a redirect to Steam and back is seconds,
/// so this is generous. The nonce is single-use (`Store::take_oidc_state` deletes on read); the TTL
/// is only the janitor for logins the friend abandoned mid-flow.
pub const OIDC_STATE_TTL_SECS: i64 = 5 * 60;

pub fn oidc_state_pk(nonce: &str) -> String {
    format!("OIDCSTATE#{nonce}")
}

/// Build the item for a pending OpenID login's server-side CSRF state (#86). pk="OIDCSTATE#<nonce>",
/// sk="META", top-level `ctx` S (the already-validated return path — never sent to Valve) +
/// `expires_epoch` N + `ttl` N (same value, the DynamoDB TTL attribute).
pub fn oidc_state_item(
    nonce: &str,
    ctx: &str,
    expires_epoch: i64,
) -> HashMap<String, AttributeValue> {
    let epoch_str = expires_epoch.to_string();
    HashMap::from([
        ("pk".into(), s(oidc_state_pk(nonce))),
        ("sk".into(), s("META")),
        ("ctx".into(), s(ctx.to_string())),
        ("expires_epoch".into(), AttributeValue::N(epoch_str.clone())),
        ("ttl".into(), AttributeValue::N(epoch_str)),
    ])
}

/// Build the full item for a friend record: `pk = FRIEND#<id>`, `sk = "META"`, PLUS
/// `gsi1pk = "FRIEND"` / `gsi1sk = <name>` (the LISTABLE membership pattern reused with
/// a different pk value, so `list_friends` gets an ordered-by-name list for free off the
/// same index `game_item` already uses).
///
/// **Deliberately NO `body` attribute.** Every other item in this file carries a JSON
/// blob for exactly this reason: so a struct can gain fields without a migration. A
/// friend record is FOUR scalars total (id — carried by `pk`, stripped back out on read
/// — name, shelf_token, created_at); a body blob here would recreate the exact
/// top-level/body drift the `gift_note`/`friend_id` pattern exists to fight, for zero
/// benefit (there is nothing left for a blob to carry). `friend_from_item` reads the
/// three named attributes (plus `pk`) directly — no `parse_body` in this path.
pub fn friend_item(f: &Friend) -> HashMap<String, AttributeValue> {
    HashMap::from([
        ("pk".into(), s(friend_pk(&f.id))),
        ("sk".into(), s("META")),
        ("name".into(), s(f.name.clone())),
        ("shelf_token".into(), s(f.shelf_token.clone())),
        (
            "created_at".into(),
            s(f.created_at
                .format(&time::format_description::well_known::Rfc3339)
                .expect("rfc3339")),
        ),
        ("gsi1pk".into(), s("FRIEND")),
        ("gsi1sk".into(), s(f.name.clone())),
    ])
}

/// Build the item for a shelf-token pointer: `pk = SHELF#<token>`, `sk = "META"`,
/// `friend_id` (S). The only path from a bearer token to a friend id —
/// `Store::get_friend_by_shelf_token` reads this first, then resolves the friend by id.
/// `create_friend` (initial), `reissue_shelf_token` (delete old + put new), and
/// `revoke_shelf_token` (delete only) are its only writers, always transactionally
/// paired with the friend record's own `shelf_token` attribute so the two can never
/// drift: no pointer ever survives without a friend whose `shelf_token` matches it.
pub fn shelf_pointer_item(token: &str, friend_id: &str) -> HashMap<String, AttributeValue> {
    HashMap::from([
        ("pk".into(), s(shelf_pk(token))),
        ("sk".into(), s("META")),
        ("friend_id".into(), s(friend_id)),
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

/// How long a cached owned-games list is considered FRESH (servable without a Steam
/// round-trip) — distinct from [`STEAM_OWNED_TTL_SECS`], which is when DynamoDB deletes
/// the item entirely. One const shared by both owned proxies and the sync ownership
/// pass (#47) so the freshness rule can't drift per-surface.
pub const STEAM_OWNED_FRESH_SECS: i64 = 86400;

pub fn steam_app_pk(app_id: u32) -> String {
    format!("STEAMAPP#{app_id}")
}

/// Build the full item for a STEAMAPP enrichment cache entry.
/// pk="STEAMAPP#<app_id>", sk="META", body=JSON of [`crate::SteamAppCache`],
/// version=N (optimistic-lock counter — `Store::put_steam_app` computes it from
/// the guard).
/// INVARIANT (#75, recorded as a decision): `version` and `body` travel together,
/// always, atomically — this is the only builder of the item shape, the guarded
/// put is the only writer, and no UpdateItem ever touches STEAMAPP# items.
/// Use `Store::put_steam_app` / `Store::get_steam_app` — do not write STEAMAPP#
/// items directly.
pub fn steam_app_item(
    cache: &crate::SteamAppCache,
    version: i64,
) -> std::collections::HashMap<String, AttributeValue> {
    std::collections::HashMap::from([
        ("pk".into(), s(steam_app_pk(cache.app_id))),
        ("sk".into(), s("META")),
        (
            "body".into(),
            s(serde_json::to_string(cache).expect("SteamAppCache serializes")),
        ),
        ("version".into(), AttributeValue::N(version.to_string())),
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
/// `ttl` N = now_epoch + [`STEAM_OWNED_TTL_SECS`] (DynamoDB TTL attribute, enabled in
/// `terraform/aws-dynamodb.tf`; expiry is lazy so callers still check `fetched_at` for freshness
/// — TTL only reclaims stale rows afterward).
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
