//! DynamoDB storage. Single table; see schema.rs for the item contract.
pub mod schema;

use aws_sdk_dynamodb::Client;
use aws_sdk_dynamodb::types::{
    AttributeDefinition, BillingMode, GlobalSecondaryIndex, KeySchemaElement, KeyType, Projection,
    ProjectionType, ScalarAttributeType,
};
use domain::{Claim, ClaimState, Game, GameStatus, Link};
use schema::{
    claim_item, claim_sk, game_item, game_pk, link_item, link_pk, parse_body, session_item,
    session_pk, steam_app_item, steam_app_pk, steam_identity_item, steam_owned_item,
    sync_state_item,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use time::OffsetDateTime;

/// Outcome of a guarded sync-upsert. The ONLY way catalog sync should write a game.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SyncWrite {
    /// Game was written (new or refreshed).
    Written,
    /// An optimistic-lock condition failed: a concurrent claim (or fulfill/compensate) changed the
    /// game's status while sync was preparing the write. Do NOT retry — the in-flight claim owns
    /// the game.
    SkippedInFlight,
    /// `domain::merge_sync` returned `None`: the stored game is already identical to what sync
    /// would write. No I/O was performed.
    Unchanged,
}

/// Outcome of a guarded steam-appid write. The ONLY safe way to write `steam_app_id` /
/// `appid_source` from the title-match or tier-1 mapper paths.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AppidWrite {
    /// Appid pair was written.
    Written,
    /// No game with that ID exists.
    NotFound,
    /// The game's current `appid_source` is `Manual` — the admin override is protected.
    Skipped,
    /// A concurrent claim holds the game `Pending` — caller should skip, not retry.
    Contested,
}

/// Outcome of a guarded hidden-flag write. The ONLY safe way to toggle `hidden` on a game.
/// Closes the admin-toggle vs claim race that an unguarded `put_game` would lose.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HiddenWrite {
    /// Hidden flag was written (game was found and the condition passed).
    Written,
    /// No game with that ID exists; caller should 404.
    NotFound,
    /// A concurrent claim holds a `claim_id` on the game — the conditional put's
    /// `attribute_not_exists(claim_id)` clause fired. Caller should 409 and retry later.
    Contested,
}

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

/// Outcome of a guarded owned-by-ben write. The ONLY safe way to toggle `owned_by_ben` on a game.
/// Closes the admin-toggle vs claim race that an unguarded `put_game` would lose.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OwnedWrite {
    /// Flag was written (game was found and the condition passed).
    Written,
    /// No game with that ID exists; caller should 404.
    NotFound,
    /// A concurrent claim holds the game `Pending` — the conditional put's status-lock fired.
    /// Caller should 409 and retry later.
    Contested,
}

/// Outcome of the friend's write-once thank-you write (`set_link_thanks`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SetThanksOutcome {
    /// Note + timestamp landed.
    Set,
    /// No link with that token exists; caller should 404.
    NotFound,
    /// A thank-you is already on the link — write-once, the first word stands.
    /// Caller should 409.
    AlreadyThanked,
    /// The link is revoked — the condition's `revoked = :f` guard fired. Covers the
    /// race where a revoke lands between the caller's liveness pre-check and this
    /// write; a dead link takes no mail even in that window (OMBB, #76 review).
    /// Caller should 409 with the revoked message.
    Revoked,
    /// The link is expired — the condition's expiry guard fired. Same storage-enforced
    /// liveness as `Revoked` (and the same clause `claim_game` uses), so the docstring's
    /// "liveness is storage-enforced" holds for BOTH halves. Caller should 409.
    Expired,
    /// The link has no claims — `claims_used >= :one` fired. The handler pre-checks
    /// this too, but `claims_used` is NOT monotonic (`compensate_claim` decrements it
    /// when fulfillment fails), so the pre-check's read can be stale for the whole
    /// claim→park→compensate span. Without this guard a thanks accepted in that span
    /// would permanently consume the friend's one note on a gift that was never
    /// delivered (#76 review pass 1). Caller should 409 "claim a game first".
    NoClaims,
}

/// Persisted summary of a catalog-sync run. Storage-shaped (lives in dynamo, not in domain).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct SyncState {
    pub last_run_epoch: i64,
    pub ok: bool,
    pub cookie_ok: bool,
    pub games_written: u32,
    pub message: String,
}

/// Cached result of a Steam-owned-games API fetch. Stored at STEAMOWN#<steamid>; TTL-evicted
/// after 7 days. Not part of the public domain model — storage-only.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SteamOwnedCache {
    pub appids: Vec<u32>,
    pub fetched_at: i64,
}

/// Cached enrichment data for a single Steam app, written at sync time.
/// Stored at pk="STEAMAPP#<app_id>", sk="META"; body is the full JSON of this struct.
/// `detail: None` is a negative-cache stub — the app was delisted or never existed when last
/// fetched, and the staleness windows still apply. Tasks 3-4 consume this type.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SteamAppCache {
    pub app_id: u32,
    /// `None` means the app was delisted (negative-cache stub).
    pub detail: Option<steam_client::SteamAppDetail>,
    pub overall: Option<steam_client::ReviewSummary>,
    pub recent: Option<steam_client::RecentReviews>,
    /// Unix epoch seconds: timestamp of the last `get_app_details` fetch (30-day refresh window).
    pub fetched_at: i64,
    /// Unix epoch seconds: timestamp of the last reviews+histogram fetch (14-day refresh window).
    pub reviews_fetched_at: i64,
}

impl SteamAppCache {
    /// A blank cache item for `app_id`: no halves fetched, both clocks at epoch — every
    /// staleness window sees it as maximally stale. The one definition of "blank" shared
    /// by the enrichment pass and the backfill bin, so they can never disagree on it.
    pub fn empty(app_id: u32) -> Self {
        Self {
            app_id,
            detail: None,
            overall: None,
            recent: None,
            fetched_at: 0,
            reviews_fetched_at: 0,
        }
    }
}

/// Opaque optimistic-lock token for a STEAMAPP# item — obtainable ONLY from
/// [`Store::get_steam_app_versioned`] (private field: a guard value cannot be
/// fabricated, it must come from a read). `None` inside = legacy item written
/// before the `version` attribute existed; the guarded put adopts it (#75).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SteamAppVersion(Option<i64>);

/// The caller's precondition for [`Store::put_steam_app`] — what the read that
/// seeded this write saw. Deliberately no unguarded variant: an unconditional
/// escape hatch is the next silent lost-update waiting for a caller (#75).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SteamAppPutGuard {
    /// The read returned `None`: create-only.
    Absent,
    /// The read returned an item carrying this token: write only if it still does.
    Unchanged(SteamAppVersion),
}

/// Errors from the guarded [`Store::put_steam_app`].
#[derive(Debug, thiserror::Error)]
pub enum SteamAppPutError {
    /// The item changed between the caller's read and this put — a concurrent
    /// writer won. Nothing is wrong with the payload; re-read, re-merge, retry.
    /// (`ClaimTxError::TxConflict` precedent: a timing race is not an AWS error.)
    #[error("STEAMAPP# item changed since read — lost the race, safe to re-merge")]
    LostRace,
    #[error(transparent)]
    Store(#[from] StoreError),
}

/// A sync-run marker older than this is dead: the fulfillment lambda's hard timeout is 900s, so
/// a run that began more than 900s (+ a skew margin) ago cannot still be executing. A stale
/// marker means a run crashed/timed out before reporting — it may be taken over.
pub const SYNC_RUN_STALE_SECS: i64 = 960;

/// True if a sync run that began at `started_epoch` could still be executing at `now_epoch`.
/// The single liveness definition shared by the fire-guard (admin-api) and the run mutex
/// (`begin_sync_run`) — they must never disagree on what "running" means.
pub fn sync_run_is_live(started_epoch: i64, now_epoch: i64) -> bool {
    now_epoch - started_epoch < SYNC_RUN_STALE_SECS
}

/// Outcome of [`Store::begin_sync_run`] — did this caller take ownership of the sync run?
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncBegin {
    /// Marker written — this caller owns the run and must `end_sync_run` when done.
    Started,
    /// A live marker exists — another run owns the walk; do not sync.
    AlreadyRunning,
}

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("dynamodb error: {0}")]
    Aws(String),
    #[error("corrupt item: {0}")]
    Corrupt(&'static str),
}

#[derive(Debug, thiserror::Error)]
pub enum ClaimTxError {
    #[error("game is not available")]
    GameUnavailable,
    #[error("link cannot claim (revoked/expired/exhausted)")]
    LinkNotClaimable,
    #[error("duplicate claim id")]
    DuplicateClaim,
    /// A concurrent transaction touched the same items — DynamoDB cancelled this one with
    /// `TransactionConflict` (or refused it outright with `TransactionInProgressException`).
    /// Transient by definition: nothing about THIS claim is invalid, it just lost a timing
    /// race. Callers should surface a retryable 409, never a 500.
    #[error("concurrent transaction conflict — safe to retry")]
    TxConflict,
    #[error(transparent)]
    Store(#[from] StoreError),
}

impl<E: std::fmt::Debug, R: std::fmt::Debug> From<aws_sdk_dynamodb::error::SdkError<E, R>>
    for StoreError
{
    fn from(e: aws_sdk_dynamodb::error::SdkError<E, R>) -> Self {
        StoreError::Aws(format!("{e:?}"))
    }
}

/// Single home for the "a PutItem's condition failed" test. A failed condition on a guarded put is
/// the designed idempotent-no-op / dedup signal (marker already consumed, item already exists,
/// ownership already moved) — everywhere that policy lives, it asks this one predicate.
fn is_ccf_put<R>(
    e: &aws_sdk_dynamodb::error::SdkError<aws_sdk_dynamodb::operation::put_item::PutItemError, R>,
) -> bool {
    matches!(
        e.as_service_error(),
        Some(
            aws_sdk_dynamodb::operation::put_item::PutItemError::ConditionalCheckFailedException(_)
        )
    )
}

/// `is_ccf_put`'s sibling for conditional UpdateItem calls — same policy, same
/// single home (see the doc above).
fn is_ccf_update<R>(
    e: &aws_sdk_dynamodb::error::SdkError<
        aws_sdk_dynamodb::operation::update_item::UpdateItemError,
        R,
    >,
) -> bool {
    matches!(
        e.as_service_error(),
        Some(
            aws_sdk_dynamodb::operation::update_item::UpdateItemError::ConditionalCheckFailedException(_)
        )
    )
}

/// Deserialize a LINK META item, overriding EVERY enforcer field from the authoritative
/// top-level attributes. The `body` blob is a convenience copy; the fields `claim_game`'s
/// condition expression actually enforces — `claims_used`, `claims_allowed`, `revoked`,
/// `expires_at` — live as top-level attributes (see `schema::link_item`) and are what
/// concurrent writers (claim's atomic ADD, compensate's decrement, `update_link_meta`'s
/// scoped SET/REMOVE) keep current. Reading any of them from `body` is a latent lost-update:
/// harmless while body and attrs move in lockstep, live the day any writer moves an attr
/// without rewriting body. So: body for immutable identity, top-level attrs for enforcement
/// AND for anything editable post-creation (`gift_note` — see its scoped writer). A future
/// editable field (label?) must follow the gift_note recipe, NOT ride in body alone: a
/// body-only editable field gets silently reverted by claim's `SET body` from a
/// pre-transaction read.
///
/// `expires_at` absence is authoritative too — `link_item` omits it and `update_link_meta`
/// REMOVEs it for never-expires — so the override is unconditional, not only-when-present.
fn link_from_item(
    item: &HashMap<String, aws_sdk_dynamodb::types::AttributeValue>,
) -> Result<Link, StoreError> {
    let mut link: Link = parse_body(item)?;
    let n_attr = |name: &str| item.get(name).and_then(|v| v.as_n().ok());
    if let Some(v) = n_attr("claims_used").and_then(|n| n.parse::<u32>().ok()) {
        link.claims_used = v;
    }
    if let Some(v) = n_attr("claims_allowed").and_then(|n| n.parse::<u32>().ok()) {
        link.claims_allowed = v;
    }
    if let Some(b) = item.get("revoked").and_then(|v| v.as_bool().ok()) {
        link.revoked = *b;
    }
    link.expires_at = match n_attr("expires_at") {
        None => None,
        Some(n) => {
            let secs = n
                .parse::<i64>()
                .map_err(|_| StoreError::Corrupt("link expires_at not numeric"))?;
            Some(
                OffsetDateTime::from_unix_timestamp(secs)
                    .map_err(|_| StoreError::Corrupt("link expires_at out of range"))?,
            )
        }
    };
    // gift_note is post-creation-editable via `set_link_gift_note`'s scoped write, so the
    // top-level attribute is authoritative and absence means no-note — unconditional
    // override, same rule as expires_at. (body never carries the note at all — see
    // `schema::link_body` — so this override is also the ONLY source, not just the winner.)
    link.gift_note = item
        .get("gift_note")
        .and_then(|v| v.as_s().ok())
        .map(String::from);
    // The thanks pair follows the full gift_note contract (top-level authoritative,
    // absent = never thanked, body never carries them) — same unconditional override.
    // thanked_at is numeric epoch seconds, like expires_at.
    link.thank_note = item
        .get("thank_note")
        .and_then(|v| v.as_s().ok())
        .map(String::from);
    link.thanked_at = match n_attr("thanked_at") {
        None => None,
        Some(n) => {
            let secs = n
                .parse::<i64>()
                .map_err(|_| StoreError::Corrupt("link thanked_at not numeric"))?;
            Some(
                OffsetDateTime::from_unix_timestamp(secs)
                    .map_err(|_| StoreError::Corrupt("link thanked_at out of range"))?,
            )
        }
    };
    Ok(link)
}

/// Map `claim_game`'s three-item transaction cancellation reasons to a `ClaimTxError`.
/// Reasons are positional: item 0 = GAME update, item 1 = LINK update, item 2 = CLAIM put.
///
/// A `ConditionalCheckFailed` is a definitive business answer (game taken / link dead /
/// duplicate), so it wins even in a mixed cancel where another item also reports
/// `TransactionConflict`. A cancel whose only failure codes are `TransactionConflict` is a
/// pure timing race — a concurrent transaction held the same items — and maps to the
/// retryable [`ClaimTxError::TxConflict`], not a 500-shaped `Store` error.
fn claim_cancellation_error(
    reasons: &[aws_sdk_dynamodb::types::CancellationReason],
) -> Option<ClaimTxError> {
    let code = |i: usize| reasons.get(i).and_then(|r| r.code());
    if code(0) == Some("ConditionalCheckFailed") {
        return Some(ClaimTxError::GameUnavailable);
    }
    if code(1) == Some("ConditionalCheckFailed") {
        return Some(ClaimTxError::LinkNotClaimable);
    }
    if code(2) == Some("ConditionalCheckFailed") {
        return Some(ClaimTxError::DuplicateClaim);
    }
    if reasons
        .iter()
        .any(|r| r.code() == Some("TransactionConflict"))
    {
        return Some(ClaimTxError::TxConflict);
    }
    None
}

/// Map `claim_game_self`'s two-item transaction cancellation reasons to a `ClaimTxError`.
/// Reasons are positional: item 0 = GAME update, item 1 = CLAIM put (no LINK item).
///
/// Same precedence rules as [`claim_cancellation_error`]: CCF wins over TransactionConflict in a
/// mixed cancel; a pure TransactionConflict cancel maps to the retryable [`ClaimTxError::TxConflict`].
fn self_claim_cancellation_error(
    reasons: &[aws_sdk_dynamodb::types::CancellationReason],
) -> Option<ClaimTxError> {
    let code = |i: usize| reasons.get(i).and_then(|r| r.code());
    if code(0) == Some("ConditionalCheckFailed") {
        return Some(ClaimTxError::GameUnavailable);
    }
    if code(1) == Some("ConditionalCheckFailed") {
        return Some(ClaimTxError::DuplicateClaim);
    }
    if reasons
        .iter()
        .any(|r| r.code() == Some("TransactionConflict"))
    {
        return Some(ClaimTxError::TxConflict);
    }
    None
}

#[derive(Clone)]
pub struct Store {
    client: Client,
    table: String,
}

impl Store {
    pub fn new(client: Client, table: String) -> Self {
        Self { client, table }
    }

    /// Unconditional full-item upsert of a game. NOT safe as a sync-upsert onto an in-flight
    /// (pending) game — it clobbers status/claim_id and re-adds/removes the listable GSI attrs
    /// wholesale. Plan 2's catalog sync MUST guard (skip or condition on status) before calling
    /// this on games that may be mid-claim.
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

    /// Batch-fetch game META items by id — one `BatchGetItem` per 100 ids (the
    /// DynamoDB batch cap), re-requesting unprocessed keys until drained.
    /// Missing ids are simply absent from the returned map; callers decide how
    /// to degrade. Avoids the N-serial-GetItem shape on hot read paths.
    pub async fn batch_get_games(
        &self,
        ids: &[String],
    ) -> Result<HashMap<String, Game>, StoreError> {
        use aws_sdk_dynamodb::types::KeysAndAttributes;
        let mut games = HashMap::with_capacity(ids.len());
        for chunk in ids.chunks(100) {
            let mut keys: Vec<HashMap<String, aws_sdk_dynamodb::types::AttributeValue>> = chunk
                .iter()
                .map(|id| {
                    let (pk, sk) = schema::key_pair(game_pk(id), "META");
                    HashMap::from([("pk".to_string(), pk), ("sk".to_string(), sk)])
                })
                .collect();
            while !keys.is_empty() {
                let ka = KeysAndAttributes::builder()
                    .set_keys(Some(keys))
                    .build()
                    .map_err(|e| StoreError::Aws(format!("{e:?}")))?;
                let resp = self
                    .client
                    .batch_get_item()
                    .request_items(&self.table, ka)
                    .send()
                    .await?;
                for item in resp
                    .responses()
                    .and_then(|tables| tables.get(&self.table))
                    .map(|items| items.as_slice())
                    .unwrap_or_default()
                {
                    let g: Game = parse_body(item)?;
                    games.insert(g.id.clone(), g);
                }
                keys = resp
                    .unprocessed_keys()
                    .and_then(|tables| tables.get(&self.table))
                    .map(|ka| ka.keys().to_vec())
                    .unwrap_or_default();
            }
        }
        Ok(games)
    }

    /// Create a fresh link. PutItem conditioned `attribute_not_exists(pk)` so it initializes the
    /// authoritative top-level `claims_used` counter exactly once — a legitimate write of the
    /// counter, the ONLY one that sets it. ConditionalCheckFailed → the token already exists
    /// (`Corrupt`): a re-create would clobber a live counter, so we refuse.
    pub async fn create_link(&self, l: &Link) -> Result<(), StoreError> {
        let res = self
            .client
            .put_item()
            .table_name(&self.table)
            .set_item(Some(link_item(l)))
            .condition_expression("attribute_not_exists(pk)")
            .send()
            .await;
        match res {
            Ok(_) => Ok(()),
            Err(sdk_err) => {
                if is_ccf_put(&sdk_err) {
                    Err(StoreError::Corrupt("link token already exists"))
                } else {
                    Err(StoreError::Aws(format!("{sdk_err:?}")))
                }
            }
        }
    }

    /// Update a link's mutable metadata (body, claims_allowed, revoked, expires_at) via a scoped
    /// UpdateItem that NEVER touches the top-level `claims_used` counter. The counter is only ever
    /// moved by `claim_game`'s atomic ADD and `compensate_claim`'s transactional decrement;
    /// `get_link` overrides body's (possibly stale) counter on read. A full-item put would clobber
    /// the enforcer's truth — hence this narrow SET/REMOVE. expires_at is written numerically
    /// (epoch seconds) when Some and REMOVEd when None, matching `link_item`.
    pub async fn update_link_meta(&self, l: &Link) -> Result<(), StoreError> {
        let (pk, sk) = schema::key_pair(link_pk(&l.token), "META");
        let expr = if l.expires_at.is_some() {
            "SET body = :b, claims_allowed = :ca, revoked = :r, expires_at = :exp"
        } else {
            "SET body = :b, claims_allowed = :ca, revoked = :r REMOVE expires_at"
        };
        let mut req = self
            .client
            .update_item()
            .table_name(&self.table)
            .key("pk", pk)
            .key("sk", sk)
            .update_expression(expr)
            .expression_attribute_values(":b", schema::s(schema::link_body(l)))
            .expression_attribute_values(
                ":ca",
                aws_sdk_dynamodb::types::AttributeValue::N(l.claims_allowed.to_string()),
            )
            .expression_attribute_values(
                ":r",
                aws_sdk_dynamodb::types::AttributeValue::Bool(l.revoked),
            );
        if let Some(exp) = l.expires_at {
            req = req.expression_attribute_values(":exp", schema::epoch_s(exp));
        }
        req.send().await?;
        Ok(())
    }

    /// Set/replace/clear a link's gift note via a single-attribute SET/REMOVE. Deliberately
    /// NOT a read-modify-write through `update_link_meta`: that shape carries the caller's
    /// possibly-stale `revoked`/`claims_allowed`/`expires_at` back into the write, so a note
    /// save racing a revoke would silently un-revoke the link. Touching ONLY `gift_note`
    /// (which reads override from the top-level attribute) makes the edit unable to disturb
    /// enforcement by construction — and it's one round-trip instead of two.
    /// Returns Ok(false) when no such link exists (the condition fails), Ok(true) on success.
    pub async fn set_link_gift_note(
        &self,
        token: &str,
        note: Option<&str>,
    ) -> Result<bool, StoreError> {
        let (pk, sk) = schema::key_pair(link_pk(token), "META");
        let req = self
            .client
            .update_item()
            .table_name(&self.table)
            .key("pk", pk)
            .key("sk", sk)
            .condition_expression("attribute_exists(pk)");
        let req = match note {
            Some(n) => req
                .update_expression("SET gift_note = :n")
                .expression_attribute_values(":n", schema::s(n)),
            None => req.update_expression("REMOVE gift_note"),
        };
        match req.send().await {
            Ok(_) => Ok(true),
            Err(sdk_err) if is_ccf_update(&sdk_err) => Ok(false),
            Err(sdk_err) => Err(StoreError::Aws(format!("{sdk_err:?}"))),
        }
    }

    /// Write-once thank-you from the friend — `set_link_gift_note`'s mirror, with one
    /// extra tooth: `attribute_not_exists(thank_note)` makes the first word stand
    /// forever, so two tabs (or a retry) can't overwrite what was already said. Same
    /// scoped single-update shape as the gift note, for the same reason: touching only
    /// the attrs this write owns means it cannot disturb enforcement (`revoked`,
    /// counters, `expires_at`) by construction. Note + timestamp land in ONE update,
    /// so `thanked_at` can never exist without its note.
    ///
    /// The condition storage-enforces the FULL guard ladder, not just write-once:
    /// `revoked = :f` (a revoke racing the handler's pre-check still refuses — OMBB,
    /// #76 review), the same expiry clause `claim_game` uses, and `claims_used >= :one`
    /// (claims are NOT monotonic — `compensate_claim` decrements on fulfillment
    /// failure, so the pre-check can be stale for the whole claim→park→compensate
    /// span). `revoked`/`claims_used` always exist as top-level attrs (`link_item`
    /// writes both at creation), so the compares never NULL-fail on a real item.
    ///
    /// A CCF is classified ATOMICALLY from the failed write's own
    /// `ReturnValuesOnConditionCheckFailure::AllOld` item — the exact item the
    /// condition was evaluated against. No follow-up read: a plain re-read is
    /// eventually consistent, and a stale replica that hadn't applied a concurrent
    /// tab's thanks write would misclassify AlreadyThanked as something else
    /// (#76 review pass 1 — moto can't see this, its reads are always strong).
    /// Classification order matches the pinned guard precedence: dead link first,
    /// then already-thanked, then no-claims.
    pub async fn set_link_thanks(
        &self,
        token: &str,
        note: &str,
        at: OffsetDateTime,
    ) -> Result<SetThanksOutcome, StoreError> {
        let (pk, sk) = schema::key_pair(link_pk(token), "META");
        let req = self
            .client
            .update_item()
            .table_name(&self.table)
            .key("pk", pk)
            .key("sk", sk)
            .condition_expression(
                "attribute_exists(pk) AND attribute_not_exists(thank_note) \
                 AND revoked = :f AND claims_used >= :one \
                 AND (attribute_not_exists(expires_at) OR expires_at > :now)",
            )
            .update_expression("SET thank_note = :n, thanked_at = :t")
            .expression_attribute_values(":n", schema::s(note))
            .expression_attribute_values(":f", aws_sdk_dynamodb::types::AttributeValue::Bool(false))
            .expression_attribute_values(
                ":one",
                aws_sdk_dynamodb::types::AttributeValue::N("1".into()),
            )
            .expression_attribute_values(":now", schema::epoch_s(at))
            .expression_attribute_values(":t", schema::epoch_s(at))
            .return_values_on_condition_check_failure(
                aws_sdk_dynamodb::types::ReturnValuesOnConditionCheckFailure::AllOld,
            );
        match req.send().await {
            Ok(_) => Ok(SetThanksOutcome::Set),
            Err(sdk_err) if is_ccf_update(&sdk_err) => {
                let item = match sdk_err.as_service_error() {
                    Some(
                        aws_sdk_dynamodb::operation::update_item::UpdateItemError::ConditionalCheckFailedException(ccf),
                    ) => ccf.item.clone(),
                    _ => unreachable!("guarded by is_ccf_update"),
                };
                // Item absent from the CCF payload ⇒ the key didn't exist at
                // evaluation time ⇒ the token never existed (links are never
                // deleted — the only delete_items in this crate are
                // sync-mutex/session/steam-identity; a link-deletion feature
                // would break this and must revisit here).
                let Some(item) = item.filter(|i| !i.is_empty()) else {
                    return Ok(SetThanksOutcome::NotFound);
                };
                let link = link_from_item(&item)?;
                // Liveness — BOTH halves — before already-sent, matching the
                // handler's ladder exactly (pass 2: expired-after-thanked here
                // answered a thanked+expired link differently than the handler
                // pre-check would, depending on which path caught it).
                if link.revoked {
                    Ok(SetThanksOutcome::Revoked)
                } else if link.expires_at.is_some_and(|exp| exp <= at) {
                    Ok(SetThanksOutcome::Expired)
                } else if link.thank_note.is_some() {
                    Ok(SetThanksOutcome::AlreadyThanked)
                } else if link.claims_used == 0 {
                    Ok(SetThanksOutcome::NoClaims)
                } else {
                    // Every conjunct accounted for above — reaching here means the
                    // condition and this classifier disagree. Fail loudly.
                    Err(StoreError::Corrupt("thanks CCF with no classifiable cause"))
                }
            }
            Err(sdk_err) => Err(StoreError::Aws(format!("{sdk_err:?}"))),
        }
    }

    pub async fn get_link(&self, token: &str) -> Result<Option<Link>, StoreError> {
        let (pk, sk) = schema::key_pair(link_pk(token), "META");
        let out = self
            .client
            .get_item()
            .table_name(&self.table)
            .key("pk", pk)
            .key("sk", sk)
            .send()
            .await?;
        // Enforcer fields (claims_used/claims_allowed/revoked/expires_at) come from the
        // authoritative top-level attributes, never the possibly-stale body — see link_from_item.
        out.item.map(|item| link_from_item(&item)).transpose()
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
        let (pk, sk) = schema::key_pair(link_pk(link_token), claim_sk(claim_id));
        let out = self
            .client
            .get_item()
            .table_name(&self.table)
            .key("pk", pk)
            .key("sk", sk)
            .send()
            .await?;
        out.item.map(|i| parse_body(&i)).transpose()
    }

    /// Every listable game, exhaustively: loops `last_evaluated_key` so a catalog past
    /// DynamoDB's 1 MB Query-page cap can't silently truncate the friend-facing list. (The
    /// old single-page read was "fine at this scale" — until it wasn't; same loop as
    /// `list_all_games` / `list_links`.)
    pub async fn list_listable_games(&self) -> Result<Vec<Game>, StoreError> {
        self.list_listable_games_paged(None).await
    }

    /// Pagination-exercising variant of [`list_listable_games`]. `page_limit` caps items per
    /// Query page (DynamoDB `Limit`) so tests can force multi-page reads deterministically
    /// without seeding a megabyte of data — it is a TEST SEAM, not a public paging API;
    /// production callers use `list_listable_games` (no limit, full 1 MB pages).
    #[doc(hidden)]
    pub async fn list_listable_games_paged(
        &self,
        page_limit: Option<i32>,
    ) -> Result<Vec<Game>, StoreError> {
        let mut games: Vec<Game> = Vec::new();
        let mut last_key: Option<HashMap<String, aws_sdk_dynamodb::types::AttributeValue>> = None;
        loop {
            let mut req = self
                .client
                .query()
                .table_name(&self.table)
                .index_name(schema::GSI_LISTABLE)
                .key_condition_expression("gsi1pk = :p")
                .expression_attribute_values(
                    ":p",
                    aws_sdk_dynamodb::types::AttributeValue::S("LISTABLE".into()),
                )
                .set_exclusive_start_key(last_key.take());
            if let Some(limit) = page_limit {
                req = req.limit(limit);
            }
            let out = req.send().await?;
            for item in out.items() {
                games.push(parse_body(item)?);
            }
            match out.last_evaluated_key() {
                None => break,
                Some(k) => last_key = Some(k.clone()),
            }
        }
        Ok(games)
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
        let (pk, sk) = schema::key_pair(pk, "META");
        let out = self
            .client
            .get_item()
            .table_name(&self.table)
            .key("pk", pk)
            .key("sk", sk)
            .send()
            .await?;
        out.item.map(|i| parse_body(&i)).transpose()
    }

    /// Atomic claim intake. Three-item transaction: GAME available→pending (removes listable GSI
    /// attrs), LINK counter increment (conditions: not revoked, not expired, not exhausted),
    /// CLAIM put (condition: attribute_not_exists = dedup).
    /// Cancellation reasons map positionally to the three writes.
    pub async fn claim_game(
        &self,
        link_token: &str,
        game_id: &str,
        claim_id: &str,
        now: OffsetDateTime,
    ) -> Result<(), ClaimTxError> {
        // read current bodies so the updated `body` JSON stays in sync with top-level attrs
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
            // A choice intent snapshot is recorded LATER (record_choice_intent), only on a Choice
            // claim and only after the pre-read — never at intake.
            choice_pre_tpks: None,
            revealed_key: None,
        };
        let av_s = |v: &str| aws_sdk_dynamodb::types::AttributeValue::S(v.to_string());

        let game_update = aws_sdk_dynamodb::types::Update::builder()
            .table_name(&self.table)
            .key("pk", av_s(&game_pk(game_id)))
            .key("sk", av_s("META"))
            // Also stamp the top-level `claim_id` so fulfill's flip can later assert ownership
            // (`claim_id = :cid`); compensate's re-list clears it (game_item omits None).
            .update_expression(
                "SET body = :b, #st = :pending, claim_id = :cid REMOVE gsi1pk, gsi1sk",
            )
            // Gate on the sparse listable marker too, not just status. `gsi1pk` exists iff
            // available ∧ giftable ∧ ¬hidden (schema::game_item). Requiring it here closes the
            // TOCTOU where a friend claims a game Ben just hid: hiding drops gsi1pk, so this
            // condition fails race-free even while status is momentarily still "available".
            .condition_expression("#st = :available AND attribute_exists(gsi1pk)")
            .expression_attribute_names("#st", "status")
            .expression_attribute_values(
                ":b",
                av_s(&serde_json::to_string(&pending).expect("game")),
            )
            .expression_attribute_values(":pending", av_s("pending"))
            .expression_attribute_values(":available", av_s("available"))
            .expression_attribute_values(":cid", av_s(claim_id))
            .build()
            .expect("game_update");
        let link_update = aws_sdk_dynamodb::types::Update::builder()
            .table_name(&self.table)
            .key("pk", av_s(&link_pk(link_token)))
            .key("sk", av_s("META"))
            .update_expression("SET body = :b ADD claims_used :one")
            // expires_at is numeric (epoch seconds via schema::epoch_s), so `expires_at > :now` is a
            // true numeric compare — immune to fractional-second width and non-UTC offset bugs that
            // a lexicographic RFC3339 string compare would suffer.
            .condition_expression(
                "revoked = :f AND claims_used < claims_allowed \
                 AND (attribute_not_exists(expires_at) OR expires_at > :now)",
            )
            .expression_attribute_values(":b", av_s(&schema::link_body(&bumped)))
            .expression_attribute_values(
                ":one",
                aws_sdk_dynamodb::types::AttributeValue::N("1".into()),
            )
            .expression_attribute_values(":f", aws_sdk_dynamodb::types::AttributeValue::Bool(false))
            .expression_attribute_values(":now", schema::epoch_s(now))
            .build()
            .expect("link_update");
        let claim_put = aws_sdk_dynamodb::types::Put::builder()
            .table_name(&self.table)
            .set_item(Some(schema::claim_item(&claim)))
            .condition_expression("attribute_not_exists(pk)")
            .build()
            .expect("claim_put");

        let result = self
            .client
            .transact_write_items()
            .transact_items(
                aws_sdk_dynamodb::types::TransactWriteItem::builder()
                    .update(game_update)
                    .build(),
            )
            .transact_items(
                aws_sdk_dynamodb::types::TransactWriteItem::builder()
                    .update(link_update)
                    .build(),
            )
            .transact_items(
                aws_sdk_dynamodb::types::TransactWriteItem::builder()
                    .put(claim_put)
                    .build(),
            )
            .send()
            .await;

        match result {
            Ok(_) => Ok(()),
            Err(sdk_err) => {
                use aws_sdk_dynamodb::operation::transact_write_items::TransactWriteItemsError as TwiErr;
                // Capture debug string before borrowing sdk_err via as_service_error()
                let err_str = format!("{sdk_err:?}");
                // In aws-sdk-dynamodb 1.116.0 there is no as_transaction_canceled_exception();
                // pattern-match directly on the public enum variants instead.
                match sdk_err.as_service_error() {
                    Some(TwiErr::TransactionCanceledException(tce)) => {
                        // Positional CCF mapping + TransactionConflict → TxConflict; see
                        // claim_cancellation_error for precedence rules.
                        if let Some(e) = claim_cancellation_error(tce.cancellation_reasons()) {
                            return Err(e);
                        }
                    }
                    // The transaction wasn't cancelled — it never ran, because an identical
                    // request is still in flight. Same disposition as a conflict cancel:
                    // transient, retryable, 409-shaped.
                    Some(TwiErr::TransactionInProgressException(_)) => {
                        return Err(ClaimTxError::TxConflict);
                    }
                    _ => {}
                }
                Err(ClaimTxError::Store(StoreError::Aws(err_str)))
            }
        }
    }

    /// Admin self-claim intake — the two-item sibling of [`claim_game`]. Differences, both
    /// deliberate (spec §3.1): NO LINK item (LINK#SELF has no META; there is no budget to
    /// enforce), and the GAME condition is `#st = :available` ALONE — not the gift path's
    /// `attribute_exists(gsi1pk)`. The sparse listable marker (available ∧ giftable ∧ ¬hidden)
    /// guards FRIEND claims against the hide-race; self-claim must accept exactly the
    /// non-giftable and hidden games that marker excludes. The status condition alone still
    /// makes gift-vs-self and self-vs-self races single-winner.
    pub async fn claim_game_self(
        &self,
        game_id: &str,
        claim_id: &str,
        now: OffsetDateTime,
    ) -> Result<(), ClaimTxError> {
        let game = self
            .get_game(game_id)
            .await?
            .ok_or(ClaimTxError::GameUnavailable)?;

        let mut pending = game.clone();
        pending.status = GameStatus::Pending;
        pending.claim_id = Some(claim_id.to_string());
        let claim = domain::Claim {
            id: claim_id.to_string(),
            link_token: domain::SELF_LINK_TOKEN.to_string(),
            game_id: game_id.to_string(),
            state: ClaimState::Pending,
            gift_url: None,
            revealed_key: None,
            created_at: now,
            choice_pre_tpks: None,
        };

        let av_s = |v: &str| aws_sdk_dynamodb::types::AttributeValue::S(v.to_string());
        let game_update = aws_sdk_dynamodb::types::Update::builder()
            .table_name(&self.table)
            .key("pk", av_s(&game_pk(game_id)))
            .key("sk", av_s("META"))
            .update_expression(
                "SET body = :b, #st = :pending, claim_id = :cid REMOVE gsi1pk, gsi1sk",
            )
            // Status-only condition — deliberately NO attribute_exists(gsi1pk). The sparse
            // listable marker guards friend claims against the hide-race; self-claim must accept
            // non-giftable and hidden games that the marker excludes. Status alone still
            // provides single-winner semantics for gift-vs-self and self-vs-self races.
            .condition_expression("#st = :available")
            .expression_attribute_names("#st", "status")
            .expression_attribute_values(
                ":b",
                av_s(&serde_json::to_string(&pending).expect("game")),
            )
            .expression_attribute_values(":pending", av_s("pending"))
            .expression_attribute_values(":available", av_s("available"))
            .expression_attribute_values(":cid", av_s(claim_id))
            .build()
            .expect("game_update");
        let claim_put = aws_sdk_dynamodb::types::Put::builder()
            .table_name(&self.table)
            .set_item(Some(claim_item(&claim)))
            .condition_expression("attribute_not_exists(pk)")
            .build()
            .expect("claim_put");

        let result = self
            .client
            .transact_write_items()
            .transact_items(
                aws_sdk_dynamodb::types::TransactWriteItem::builder()
                    .update(game_update)
                    .build(),
            )
            .transact_items(
                aws_sdk_dynamodb::types::TransactWriteItem::builder()
                    .put(claim_put)
                    .build(),
            )
            .send()
            .await;

        match result {
            Ok(_) => Ok(()),
            Err(sdk_err) => {
                use aws_sdk_dynamodb::operation::transact_write_items::TransactWriteItemsError as TwiErr;
                let err_str = format!("{sdk_err:?}");
                match sdk_err.as_service_error() {
                    Some(TwiErr::TransactionCanceledException(tce)) => {
                        if let Some(e) = self_claim_cancellation_error(tce.cancellation_reasons()) {
                            return Err(e);
                        }
                    }
                    Some(TwiErr::TransactionInProgressException(_)) => {
                        return Err(ClaimTxError::TxConflict);
                    }
                    _ => {}
                }
                Err(ClaimTxError::Store(StoreError::Aws(err_str)))
            }
        }
    }

    /// Spec invariant: gift URL becomes durable BEFORE the game flips to gifted.
    ///
    /// Idempotent + mutually exclusive with `compensate_claim`. Both race for the CLAIM's pending
    /// marker (`gsi2pk`, present iff `state == Pending`); a conditional put consumes it, so exactly
    /// one of the two wins. A fulfill that finds the marker already gone rechecks the claim: if it
    /// reads `Fulfilled`, that's an idempotent retry (Ok); if it reads anything else, compensate
    /// won and this is a LOUD unrecoverable-by-code error — the gift URL exists but the game was
    /// re-listed, needing manual/reconcile recovery.
    ///
    /// Write 2 (the game flip) additionally gates on `claim_id = :cid`, not just `status = pending`:
    /// the game must still be OWNED by this claim. If it isn't — already flipped, or the game was
    /// legitimately re-listed by compensate and re-claimed by a different claim — the flip is a
    /// no-op (Ok), so a stale fulfill can never hijack another claim's game or resurrect a gifted
    /// one. See `flip_game_from_pending`.
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

        // write 1: URL durable. Conditional on the pending marker so fulfill/compensate can't both
        // land. Overwriting with a Fulfilled claim drops gsi2pk (claim_item), consuming the marker.
        let put_res = self
            .client
            .put_item()
            .table_name(&self.table)
            .set_item(Some(claim_item(&claim)))
            .condition_expression("attribute_exists(gsi2pk)")
            .send()
            .await;
        match put_res {
            Ok(_) => {}
            Err(sdk_err) => {
                if !is_ccf_put(&sdk_err) {
                    return Err(StoreError::Aws(format!("{sdk_err:?}")));
                }
                let current = self
                    .get_claim(link_token, claim_id)
                    .await?
                    .ok_or(StoreError::Corrupt("fulfill: claim missing on recheck"))?;
                if current.state != ClaimState::Fulfilled {
                    return Err(StoreError::Corrupt(
                        "fulfill lost to compensate — gift URL needs manual/reconcile recovery",
                    ));
                }
                // idempotent retry: URL already durable. Fall through to re-attempt the
                // (idempotent) game flip — a transient write-2 failure on the prior attempt
                // may have left the game stranded in pending.
            }
        }

        // write 2: game flips pending → gifted, gated on ownership (claim_id = :cid).
        self.flip_game_from_pending(game_id, Some(claim_id), GameStatus::Gifted)
            .await
    }

    /// Self-claim fulfillment — [`fulfill_claim`]'s sibling (spec §3.2): write the revealed key
    /// to the CLAIM durable-FIRST (conditioned on the pending marker, same fulfill-vs-compensate
    /// mutual exclusion), then flip the GAME pending → ben_redeemed gated on claim ownership.
    pub async fn fulfill_self_claim(
        &self,
        claim_id: &str,
        game_id: &str,
        revealed_key: &str,
    ) -> Result<(), StoreError> {
        let mut claim = self
            .get_claim(domain::SELF_LINK_TOKEN, claim_id)
            .await?
            .ok_or(StoreError::Corrupt("fulfill_self: claim missing"))?;
        claim.state = ClaimState::Fulfilled;
        claim.revealed_key = Some(revealed_key.to_string());

        let put_res = self
            .client
            .put_item()
            .table_name(&self.table)
            .set_item(Some(claim_item(&claim)))
            .condition_expression("attribute_exists(gsi2pk)")
            .send()
            .await;
        match put_res {
            Ok(_) => {}
            Err(sdk_err) => {
                if !is_ccf_put(&sdk_err) {
                    return Err(StoreError::Aws(format!("{sdk_err:?}")));
                }
                let current = self
                    .get_claim(domain::SELF_LINK_TOKEN, claim_id)
                    .await?
                    .ok_or(StoreError::Corrupt(
                        "fulfill_self: claim missing on recheck",
                    ))?;
                if current.state != ClaimState::Fulfilled {
                    return Err(StoreError::Corrupt(
                        "fulfill_self lost to compensate — revealed key needs manual/reconcile recovery",
                    ));
                }
                // idempotent retry: key already durable; fall through to re-attempt the flip.
            }
        }
        self.flip_game_from_pending(game_id, Some(claim_id), GameStatus::BenRedeemed)
            .await
    }

    /// Flip a game out of `pending` to a terminal `new_status` via a guarded full-item put. The
    /// condition is always `status = pending`; when `claim_id` is `Some`, it ALSO requires the
    /// game still carries that top-level `claim_id`, so the caller only touches a game it still
    /// owns. A failed condition (already flipped, or ownership moved after a re-list+re-claim) is
    /// the designed idempotent no-op → `Ok(())`. (compensate's re-list runs inside its own
    /// TransactWriteItems and so does NOT use this helper.)
    async fn flip_game_from_pending(
        &self,
        game_id: &str,
        claim_id: Option<&str>,
        new_status: GameStatus,
    ) -> Result<(), StoreError> {
        let mut game = self
            .get_game(game_id)
            .await?
            .ok_or(StoreError::Corrupt("flip: game missing"))?;
        game.status = new_status;
        let mut req = self
            .client
            .put_item()
            .table_name(&self.table)
            .set_item(Some(game_item(&game)))
            .expression_attribute_names("#st", "status")
            .expression_attribute_values(":pending", schema::s("pending"));
        let cond = if let Some(cid) = claim_id {
            req = req.expression_attribute_values(":cid", schema::s(cid));
            "#st = :pending AND claim_id = :cid"
        } else {
            "#st = :pending"
        };
        let res = req.condition_expression(cond).send().await;
        match res {
            Ok(_) => Ok(()),
            Err(sdk_err) => {
                if is_ccf_put(&sdk_err) {
                    Ok(()) // already flipped / ownership moved: idempotent no-op
                } else {
                    Err(StoreError::Aws(format!("{sdk_err:?}")))
                }
            }
        }
    }

    /// Record a Humble Choice claim's pre-choose intent snapshot (the order's tpk `machine_name`s
    /// captured BEFORE the `choosecontent` write) so a crash between the two Choice writes is
    /// recoverable. This is the crash-recovery hinge: it MUST become durable before `choose_content`
    /// runs, so that reconcile can read the snapshot's presence to decide whether a pick could have
    /// been spent (see [`domain::Claim::choice_pre_tpks`]).
    ///
    /// A conditional put gated on `attribute_exists(gsi2pk)` — the pending marker, the SAME gate
    /// `fulfill_claim` / `compensate_claim` race for. It writes the same `Pending` claim back (only
    /// `choice_pre_tpks` changes), so the marker survives and `state` stays `Pending`. A failed
    /// condition means the claim is no longer pending (already fulfilled/compensated) — recording an
    /// intent on it is meaningless and a sign the caller raced its own reconcile, so surface it loud
    /// (`Corrupt`) and let the caller park rather than proceed to a choose on a settled claim.
    pub async fn record_choice_intent(
        &self,
        link_token: &str,
        claim_id: &str,
        pre_tpks: Vec<String>,
    ) -> Result<(), StoreError> {
        let mut claim = self
            .get_claim(link_token, claim_id)
            .await?
            .ok_or(StoreError::Corrupt("record_choice_intent: claim missing"))?;
        claim.choice_pre_tpks = Some(pre_tpks);
        let res = self
            .client
            .put_item()
            .table_name(&self.table)
            .set_item(Some(claim_item(&claim)))
            .condition_expression("attribute_exists(gsi2pk)")
            .send()
            .await;
        match res {
            Ok(_) => Ok(()),
            Err(sdk_err) => {
                if is_ccf_put(&sdk_err) {
                    Err(StoreError::Corrupt(
                        "record_choice_intent: claim no longer pending — refusing to choose",
                    ))
                } else {
                    Err(StoreError::Aws(format!("{sdk_err:?}")))
                }
            }
        }
    }

    /// Idempotent + mutually exclusive with `fulfill_claim` (see that method for the shared
    /// pending-marker gate).
    ///
    /// All three effects — mark the CLAIM `Compensated` (consuming its `gsi2pk` marker), re-list
    /// the GAME (`pending → available`, clearing `claim_id`), and decrement the LINK counter —
    /// happen in ONE `TransactWriteItems`, all-or-nothing. This is what kills the leak class: a
    /// transient failure after the marker was consumed used to short-circuit later retries into an
    /// early `Ok`, stranding the link slot forever. Now a transient failure rolls the whole
    /// transaction back, so a retry re-runs all three from scratch.
    ///
    /// On `TransactionCanceledException` we only special-case item 0 (the CLAIM put) failing its
    /// `attribute_exists(gsi2pk)` condition — i.e. the marker was already consumed by someone:
    /// re-read the claim and
    /// - `Compensated` → an idempotent retry landing after a prior full success → `Ok(())`;
    /// - `Fulfilled`   → fulfill won the mutual-exclusion race and now OWNS the game's fate. This
    ///   is the DESIGNED exclusion, not an error: fulfill's own (idempotent) retry completes the
    ///   flip, so compensate must be a no-op → `Ok(())`.
    ///
    /// Any other cancellation pattern is a genuine anomaly (a live claim whose game isn't pending,
    /// or whose link counter is already 0) → `StoreError::Aws` with the debug string, loud.
    pub async fn compensate_claim(
        &self,
        link_token: &str,
        claim_id: &str,
        game_id: &str,
    ) -> Result<(), StoreError> {
        // Read current claim + game BEFORE building the transaction: we need the game struct to
        // construct the re-listed item (status Available, claim_id cleared) from real fields.
        let mut claim = self
            .get_claim(link_token, claim_id)
            .await?
            .ok_or(StoreError::Corrupt("compensate: claim missing"))?;
        claim.state = ClaimState::Compensated;

        let mut game = self
            .get_game(game_id)
            .await?
            .ok_or(StoreError::Corrupt("compensate: game missing"))?;
        game.status = GameStatus::Available;
        game.claim_id = None; // game_item omits top-level claim_id → the re-listed game is unowned

        // item 0: CLAIM put — consume the pending marker (dedup / mutual-exclusion gate).
        let claim_put = aws_sdk_dynamodb::types::Put::builder()
            .table_name(&self.table)
            .set_item(Some(claim_item(&claim)))
            .condition_expression("attribute_exists(gsi2pk)")
            .build()
            .expect("claim_put");
        // item 1: GAME put — re-list (game_item re-adds the listable GSI attrs); never resurrect a
        // game fulfill already flipped to gifted.
        let game_put = aws_sdk_dynamodb::types::Put::builder()
            .table_name(&self.table)
            .set_item(Some(game_item(&game)))
            .condition_expression("#st = :pending")
            .expression_attribute_names("#st", "status")
            .expression_attribute_values(":pending", schema::s("pending"))
            .build()
            .expect("game_put");
        // item 2: LINK decrement — atomic ADD, guarded ≥ 1 so it can't go negative.
        let link_update = aws_sdk_dynamodb::types::Update::builder()
            .table_name(&self.table)
            .key("pk", schema::s(link_pk(link_token)))
            .key("sk", schema::s("META"))
            .update_expression("ADD claims_used :neg_one")
            .condition_expression("claims_used >= :one")
            .expression_attribute_values(
                ":neg_one",
                aws_sdk_dynamodb::types::AttributeValue::N("-1".into()),
            )
            .expression_attribute_values(
                ":one",
                aws_sdk_dynamodb::types::AttributeValue::N("1".into()),
            )
            .build()
            .expect("link_update");

        let result = self
            .client
            .transact_write_items()
            .transact_items(
                aws_sdk_dynamodb::types::TransactWriteItem::builder()
                    .put(claim_put)
                    .build(),
            )
            .transact_items(
                aws_sdk_dynamodb::types::TransactWriteItem::builder()
                    .put(game_put)
                    .build(),
            )
            .transact_items(
                aws_sdk_dynamodb::types::TransactWriteItem::builder()
                    .update(link_update)
                    .build(),
            )
            .send()
            .await;

        match result {
            Ok(_) => Ok(()),
            Err(sdk_err) => {
                let err_str = format!("{sdk_err:?}");
                if let Some(
                    aws_sdk_dynamodb::operation::transact_write_items::TransactWriteItemsError::TransactionCanceledException(tce),
                ) = sdk_err.as_service_error()
                {
                    let item0_ccf = tce
                        .cancellation_reasons()
                        .first()
                        .and_then(|r| r.code())
                        .is_some_and(|c| c == "ConditionalCheckFailed");
                    if item0_ccf {
                        // Marker already consumed — someone else finished this claim's fate.
                        let current = self
                            .get_claim(link_token, claim_id)
                            .await?
                            .ok_or(StoreError::Corrupt("compensate: claim missing on recheck"))?;
                        match current.state {
                            // idempotent retry after a prior full success.
                            ClaimState::Compensated => return Ok(()),
                            // fulfill won the race and owns the game; its retry completes the flip.
                            ClaimState::Fulfilled => return Ok(()),
                            // marker gone but still Pending is impossible-by-construction → fall
                            // through to the loud error below.
                            ClaimState::Pending => {}
                        }
                    }
                }
                Err(StoreError::Aws(err_str))
            }
        }
    }

    /// Self-claim compensation — [`compensate_claim`]'s two-item sibling (spec §3.3): CLAIM →
    /// compensated (conditioned on the pending marker), GAME re-listed (conditioned
    /// `#st = :pending`), and NO link decrement — LINK#SELF has no META item; the gift variant's
    /// `claims_used >= 1` guard against it would cancel the whole transaction, wedging every
    /// self-claim compensation permanently (the review-B1 finding this method exists to fix).
    pub async fn compensate_self_claim(
        &self,
        claim_id: &str,
        game_id: &str,
    ) -> Result<(), StoreError> {
        let mut claim = self
            .get_claim(domain::SELF_LINK_TOKEN, claim_id)
            .await?
            .ok_or(StoreError::Corrupt("compensate_self: claim missing"))?;
        claim.state = ClaimState::Compensated;

        let mut game = self
            .get_game(game_id)
            .await?
            .ok_or(StoreError::Corrupt("compensate_self: game missing"))?;
        game.status = GameStatus::Available;
        game.claim_id = None;

        let claim_put = aws_sdk_dynamodb::types::Put::builder()
            .table_name(&self.table)
            .set_item(Some(claim_item(&claim)))
            .condition_expression("attribute_exists(gsi2pk)")
            .build()
            .expect("claim_put");
        let game_put = aws_sdk_dynamodb::types::Put::builder()
            .table_name(&self.table)
            .set_item(Some(game_item(&game)))
            .condition_expression("#st = :pending")
            .expression_attribute_names("#st", "status")
            .expression_attribute_values(":pending", schema::s("pending"))
            .build()
            .expect("game_put");

        let result = self
            .client
            .transact_write_items()
            .transact_items(
                aws_sdk_dynamodb::types::TransactWriteItem::builder()
                    .put(claim_put)
                    .build(),
            )
            .transact_items(
                aws_sdk_dynamodb::types::TransactWriteItem::builder()
                    .put(game_put)
                    .build(),
            )
            .send()
            .await;
        match result {
            Ok(_) => Ok(()),
            Err(sdk_err) => {
                let err_str = format!("{sdk_err:?}");
                if let Some(
                    aws_sdk_dynamodb::operation::transact_write_items::TransactWriteItemsError::TransactionCanceledException(tce),
                ) = sdk_err.as_service_error()
                {
                    let item0_ccf = tce
                        .cancellation_reasons()
                        .first()
                        .and_then(|r| r.code())
                        .is_some_and(|c| c == "ConditionalCheckFailed");
                    if item0_ccf {
                        // Marker already consumed — someone else finished this claim's fate.
                        let current = self
                            .get_claim(domain::SELF_LINK_TOKEN, claim_id)
                            .await?
                            .ok_or(StoreError::Corrupt(
                                "compensate_self: claim missing on recheck",
                            ))?;
                        match current.state {
                            // idempotent retry after a prior full success.
                            ClaimState::Compensated => return Ok(()),
                            // fulfill won the race and owns the game; its retry completes the flip.
                            ClaimState::Fulfilled => return Ok(()),
                            // marker gone but still Pending is impossible-by-construction → fall
                            // through to the loud error below.
                            ClaimState::Pending => {}
                        }
                    }
                }
                Err(StoreError::Aws(err_str))
            }
        }
    }

    /// Toggle a game's `hidden` flag with a guarded conditional write.
    ///
    /// Race handling:
    /// - If the game is already `Pending` at read time, return `Contested` immediately — the
    ///   claim is in flight and any hide attempt would race its fulfill/compensate.
    /// - Otherwise use an optimistic lock on status (`#st = :expected`): a claim that lands
    ///   between our read and the put flips the game to `Pending`, which CCFs the condition
    ///   and safely returns `Contested`.
    ///
    /// The old `attribute_not_exists(claim_id)` guard permanently blocked gifted games (which
    /// retain `claim_id` after `fulfill_claim`) from ever being hidden. The status-only lock
    /// is the correct gate: `Gifted` games have a stable status string and no competing writer.
    pub async fn set_game_hidden(
        &self,
        game_id: &str,
        hidden: bool,
    ) -> Result<HiddenWrite, StoreError> {
        let Some(mut game) = self.get_game(game_id).await? else {
            return Ok(HiddenWrite::NotFound);
        };

        // Pending means a claim is actively in flight — return Contested immediately.
        // A claim landing AFTER this read will flip status to Pending → the put condition
        // below (status must equal the read value) will CCF → Contested.
        if game.status == GameStatus::Pending {
            return Ok(HiddenWrite::Contested);
        }

        game.hidden = hidden;
        // Every admin toggle — hide OR unhide — stamps Admin: from this moment the
        // auto-hide sweep defers to Ben on this record forever (#71).
        game.hidden_source = Some(domain::HiddenSource::Admin);

        // Optimistic lock: status must match what we read. Mirrors upsert_game_from_sync.
        let status_str = game.status.as_wire();

        let res = self
            .client
            .put_item()
            .table_name(&self.table)
            .set_item(Some(game_item(&game)))
            .expression_attribute_names("#st", "status")
            .expression_attribute_values(":expected", schema::s(status_str))
            .condition_expression("#st = :expected")
            .send()
            .await;

        match res {
            Ok(_) => Ok(HiddenWrite::Written),
            Err(sdk_err) => {
                if is_ccf_put(&sdk_err) {
                    Ok(HiddenWrite::Contested)
                } else {
                    Err(StoreError::Aws(format!("{sdk_err:?}")))
                }
            }
        }
    }

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

        let res = self
            .client
            .put_item()
            .table_name(&self.table)
            .set_item(Some(game_item(&game)))
            .expression_attribute_names("#st", "status")
            .expression_attribute_names("#hsrc", "hidden_source")
            .expression_attribute_values(":expected", schema::s(game.status.as_wire()))
            .expression_attribute_values(":admin", schema::s(domain::HiddenSource::Admin.as_wire()))
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

    /// Guarded appid write: the ONLY correct writer for the steam appid mapper. Guards against
    /// clobbering a `Manual` admin override and against racing a concurrent claim.
    ///
    /// Returns `AppidWrite::Skipped` when the stored `appid_source` is `Manual` — the admin
    /// override is never overwritten by a mapper pass. Returns `AppidWrite::Contested` when the
    /// game is `Pending` (a claim is in flight). Uses the same optimistic-lock-on-status pattern
    /// as `set_game_hidden` to close the read→write race.
    pub async fn set_game_steam_appid_if_unclaimed(
        &self,
        game_id: &str,
        appid: u32,
        source: domain::AppidSource,
    ) -> Result<AppidWrite, StoreError> {
        let Some(mut game) = self.get_game(game_id).await? else {
            return Ok(AppidWrite::NotFound);
        };

        // Manual guard — admin override is untouchable.
        if game.appid_source == Some(domain::AppidSource::Manual) {
            return Ok(AppidWrite::Skipped);
        }

        // Pending means a claim is actively in flight.
        if game.status == GameStatus::Pending {
            return Ok(AppidWrite::Contested);
        }

        game.steam_app_id = Some(appid);
        game.appid_source = Some(source);

        // Optimistic lock: status must match what we read. Mirrors set_game_hidden.
        // Additionally guard against a concurrent admin Manual override that landed
        // inside our read→write window: if appid_source is now Manual in DynamoDB,
        // we must NOT clobber it. attribute_not_exists allows the write on items that
        // predate the appid_source attribute (Title/Humble/None all still map).
        let status_str = game.status.as_wire();

        let res = self
            .client
            .put_item()
            .table_name(&self.table)
            .set_item(Some(game_item(&game)))
            .expression_attribute_names("#st", "status")
            .expression_attribute_names("#asrc", "appid_source")
            .expression_attribute_values(":expected", schema::s(status_str))
            .expression_attribute_values(
                ":manual",
                schema::s(domain::AppidSource::Manual.as_wire()),
            )
            .condition_expression(
                "#st = :expected AND (attribute_not_exists(#asrc) OR #asrc <> :manual)",
            )
            .send()
            .await;

        match res {
            Ok(_) => Ok(AppidWrite::Written),
            Err(sdk_err) => {
                if is_ccf_put(&sdk_err) {
                    Ok(AppidWrite::Contested)
                } else {
                    Err(StoreError::Aws(format!("{sdk_err:?}")))
                }
            }
        }
    }

    /// Guarded sync-upsert: the ONLY correct writer for catalog-sync. `put_game` remains unsafe
    /// for sync — it clobbers status/claim_id and resets the listable GSI attrs wholesale, without
    /// the optimistic lock that prevents mid-sync races with an in-flight claim.
    ///
    /// `domain::merge_sync` branch-c takes identity fields (`gamekey`, `machine_name`) from
    /// `fresh` — safe here because `fresh` is constructed from the same `gamekey:machine_name`
    /// that produced the lookup `id` (carry-note from task-2 review).
    pub async fn upsert_game_from_sync(&self, fresh: Game) -> Result<SyncWrite, StoreError> {
        let existing = self.get_game(&fresh.id).await?;
        let Some(merged) = domain::merge_sync(existing.as_ref(), fresh) else {
            return Ok(SyncWrite::Unchanged);
        };

        // Condition: if no existing record, guard with attribute_not_exists(pk) so a concurrent
        // first-insert doesn't clobber a claim that landed between our read and write.
        // If an existing record was found, optimistic-lock on its status string — a CCF means a
        // concurrent claim/compensate/fulfill changed status under us → SkippedInFlight.
        // When the existing game is owned (claim_id is Some), add an ownership clause to close a
        // TOCTOU where a compensate+reclaim lands inside sync's read→write window. Without it, the
        // put would stamp the stale claim_id and strand the live claim (whose fulfill gate checks
        // claim_id for ownership).
        // hidden_source must also be unchanged since the read: an admin toggle landing inside
        // this window would otherwise be silently reverted — INCLUDING the Admin stamp, which
        // erases Ben's permanent auto-hide immunity (#71). Sync yields; next run re-merges.
        let mut req = self
            .client
            .put_item()
            .table_name(&self.table)
            .set_item(Some(game_item(&merged)));

        let cond = match &existing {
            None => "attribute_not_exists(pk)".to_string(),
            Some(e) => {
                // Clause list, not a format! branch matrix: every guarded field appends
                // exactly one clause, so guard N+1 can't silently miss an arm.
                let mut clauses = vec!["#st = :expected".to_string()];
                req = req
                    .expression_attribute_names("#st", "status")
                    .expression_attribute_values(":expected", schema::s(e.status.as_wire()));
                // If the existing game is owned, add the ownership clause to the condition.
                if let Some(cid) = &e.claim_id {
                    req = req.expression_attribute_values(":cid", schema::s(cid.clone()));
                    clauses.push("claim_id = :cid".to_string());
                }
                // hidden_source must be unchanged since our read: an admin toggle landing
                // inside this window owns the record — sync yields (#71).
                req = req.expression_attribute_names("#hsrc", "hidden_source");
                match e.hidden_source {
                    None => clauses.push("attribute_not_exists(#hsrc)".to_string()),
                    Some(src) => {
                        req = req.expression_attribute_values(":hsrc", schema::s(src.as_wire()));
                        clauses.push("#hsrc = :hsrc".to_string());
                    }
                }
                clauses.join(" AND ")
            }
        };

        let res = req.condition_expression(cond).send().await;
        match res {
            Ok(_) => Ok(SyncWrite::Written),
            Err(sdk_err) => {
                if is_ccf_put(&sdk_err) {
                    Ok(SyncWrite::SkippedInFlight)
                } else {
                    Err(StoreError::Aws(format!("{sdk_err:?}")))
                }
            }
        }
    }

    /// Query the `pending-claims` GSI for all claims currently in `Pending` state, oldest first
    /// (ascending by gsi2sk, which is the RFC3339 `created_at`).
    ///
    /// Exhaustive: loops `last_evaluated_key` across Query pages. This list feeds reconcile's
    /// COMPLETENESS guarantee — a claim missing from a truncated page would be parked forever,
    /// invisibly — so unlike a cosmetic list, partial results here are corruption, not
    /// degradation. Ordering is preserved across pages (Query pages continue the gsi2sk sort).
    pub async fn list_pending_claims(&self) -> Result<Vec<Claim>, StoreError> {
        self.list_pending_claims_paged(None).await
    }

    /// Pagination-exercising variant of [`list_pending_claims`]. `page_limit` caps items per
    /// Query page (DynamoDB `Limit`) so tests can force multi-page reads deterministically —
    /// a TEST SEAM, not a public paging API; production callers use `list_pending_claims`.
    #[doc(hidden)]
    pub async fn list_pending_claims_paged(
        &self,
        page_limit: Option<i32>,
    ) -> Result<Vec<Claim>, StoreError> {
        let mut claims: Vec<Claim> = Vec::new();
        let mut last_key: Option<HashMap<String, aws_sdk_dynamodb::types::AttributeValue>> = None;
        loop {
            let mut req = self
                .client
                .query()
                .table_name(&self.table)
                .index_name(schema::GSI_PENDING)
                .key_condition_expression("gsi2pk = :p")
                .expression_attribute_values(
                    ":p",
                    aws_sdk_dynamodb::types::AttributeValue::S("PENDINGCLAIM".into()),
                )
                .scan_index_forward(true)
                .set_exclusive_start_key(last_key.take());
            if let Some(limit) = page_limit {
                req = req.limit(limit);
            }
            let out = req.send().await?;
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

    /// Full-catalog Scan over every GAME# item. Admin needs completeness: game IDs are scattered
    /// across arbitrary partition keys (`GAME#gamekey:machine_name`), so a Query is impossible
    /// without a dedicated GSI. A Scan with `FilterExpression = begins_with(pk, "GAME#") AND
    /// sk = "META"` is correct and acceptably fast at this catalog scale (single-digit MB at most).
    /// We loop on `last_evaluated_key` to exhaust every page — admin can never see a truncated list.
    /// At tens-of-thousands-of-games scale a `type` attribute + GSI would be preferred over a full
    /// table scan, but that day is not today.
    pub async fn list_all_games(&self) -> Result<Vec<Game>, StoreError> {
        let mut games: Vec<Game> = Vec::new();
        let mut last_key: Option<HashMap<String, aws_sdk_dynamodb::types::AttributeValue>> = None;
        loop {
            let out = self
                .client
                .scan()
                .table_name(&self.table)
                .filter_expression("begins_with(pk, :pfx) AND sk = :meta")
                .expression_attribute_values(
                    ":pfx",
                    aws_sdk_dynamodb::types::AttributeValue::S("GAME#".into()),
                )
                .expression_attribute_values(
                    ":meta",
                    aws_sdk_dynamodb::types::AttributeValue::S("META".into()),
                )
                .set_exclusive_start_key(last_key.take())
                .send()
                .await?;
            for item in out.items() {
                games.push(parse_body(item)?);
            }
            match out.last_evaluated_key() {
                None => break,
                Some(k) => last_key = Some(k.clone()),
            }
        }
        Ok(games)
    }

    /// Full Scan for all LINK# META items. Paginated via `last_evaluated_key` for completeness.
    /// The filter `begins_with(pk, "LINK#") AND sk = "META"` excludes CLAIM# sub-items (those
    /// have `sk = "CLAIM#<id>"`) and any other item types. Enforcer fields (claims_used,
    /// claims_allowed, revoked, expires_at) are overridden from the authoritative top-level
    /// attributes on each item — same logic as `get_link` (see `link_from_item`) — so the
    /// result always reflects the enforcer's truth rather than a potentially-stale `body`.
    pub async fn list_links(&self) -> Result<Vec<Link>, StoreError> {
        let mut links: Vec<Link> = Vec::new();
        let mut last_key: Option<HashMap<String, aws_sdk_dynamodb::types::AttributeValue>> = None;
        loop {
            let out = self
                .client
                .scan()
                .table_name(&self.table)
                .filter_expression("begins_with(pk, :pfx) AND sk = :meta")
                .expression_attribute_values(
                    ":pfx",
                    aws_sdk_dynamodb::types::AttributeValue::S("LINK#".into()),
                )
                .expression_attribute_values(
                    ":meta",
                    aws_sdk_dynamodb::types::AttributeValue::S("META".into()),
                )
                .set_exclusive_start_key(last_key.take())
                .send()
                .await?;
            for item in out.items() {
                // Same enforcer-field override as `get_link` — top-level attrs win over body.
                links.push(link_from_item(item)?);
            }
            match out.last_evaluated_key() {
                None => break,
                Some(k) => last_key = Some(k.clone()),
            }
        }
        Ok(links)
    }

    /// Persist a catalog-sync run summary. Unconditional upsert — only one SYNC#STATE item exists.
    pub async fn put_sync_state(&self, state: &SyncState) -> Result<(), StoreError> {
        self.client
            .put_item()
            .table_name(&self.table)
            .set_item(Some(sync_state_item(state)))
            .send()
            .await?;
        Ok(())
    }

    /// Retrieve the most recent catalog-sync run summary, or `None` if no run has been recorded.
    pub async fn get_sync_state(&self) -> Result<Option<SyncState>, StoreError> {
        self.get_meta("SYNC#STATE").await
    }

    /// Take the sync-run mutex: write the SYNC#RUN marker iff none exists or the existing one is
    /// stale (see [`SYNC_RUN_STALE_SECS`]). The marker lives outside the SyncState body JSON
    /// because a condition expression can't see inside a serialized string — `started_epoch` is a
    /// top-level N attribute exactly so this put can condition on it. The conditional put is what
    /// makes concurrent walks impossible no matter how many sync invokes get queued.
    pub async fn begin_sync_run(&self, now_epoch: i64) -> Result<SyncBegin, StoreError> {
        let (pk, sk) = schema::key_pair("SYNC#RUN", "META");
        let res = self
            .client
            .put_item()
            .table_name(&self.table)
            .item("pk", pk)
            .item("sk", sk)
            .item(
                "started_epoch",
                aws_sdk_dynamodb::types::AttributeValue::N(now_epoch.to_string()),
            )
            .condition_expression("attribute_not_exists(pk) OR started_epoch < :stale_before")
            .expression_attribute_values(
                ":stale_before",
                aws_sdk_dynamodb::types::AttributeValue::N(
                    (now_epoch - SYNC_RUN_STALE_SECS).to_string(),
                ),
            )
            .send()
            .await;
        match res {
            Ok(_) => Ok(SyncBegin::Started),
            Err(e) if is_ccf_put(&e) => Ok(SyncBegin::AlreadyRunning),
            Err(e) => Err(e.into()),
        }
    }

    /// Release the sync-run mutex. Idempotent; a failed delete only delays the next sync until
    /// the marker goes stale — it cannot wedge the system.
    pub async fn end_sync_run(&self) -> Result<(), StoreError> {
        let (pk, sk) = schema::key_pair("SYNC#RUN", "META");
        self.client
            .delete_item()
            .table_name(&self.table)
            .key("pk", pk)
            .key("sk", sk)
            .send()
            .await?;
        Ok(())
    }

    /// Read the sync-run marker's `started_epoch`, or `None` if no run marker exists. Liveness
    /// (vs. a crashed run's leftover marker) is the caller's judgment via [`sync_run_is_live`].
    pub async fn get_sync_run(&self) -> Result<Option<i64>, StoreError> {
        let (pk, sk) = schema::key_pair("SYNC#RUN", "META");
        let out = self
            .client
            .get_item()
            .table_name(&self.table)
            .key("pk", pk)
            .key("sk", sk)
            .send()
            .await?;
        out.item
            .map(|item| {
                item.get("started_epoch")
                    .and_then(|v| v.as_n().ok())
                    .and_then(|n| n.parse::<i64>().ok())
                    .ok_or(StoreError::Corrupt("sync run missing started_epoch"))
            })
            .transpose()
    }

    /// Persist an admin session. pk="SESSION#<token>", sk="META". Both `expires_epoch` and `ttl`
    /// are set to the same value; `ttl` is reserved for DynamoDB TTL (enabled in plan 4's
    /// terraform). Until then, callers are responsible for checking expiry against wall clock.
    pub async fn create_session(&self, token: &str, expires_epoch: i64) -> Result<(), StoreError> {
        self.client
            .put_item()
            .table_name(&self.table)
            .set_item(Some(session_item(token, expires_epoch)))
            .send()
            .await?;
        Ok(())
    }

    /// Look up an admin session by token, returning the stored `expires_epoch` (seconds since
    /// Unix epoch). Returns `None` if the session does not exist. Expiry enforcement (comparing
    /// against the current time) is the caller's responsibility.
    pub async fn get_session(&self, token: &str) -> Result<Option<i64>, StoreError> {
        let (pk, sk) = schema::key_pair(session_pk(token), "META");
        let out = self
            .client
            .get_item()
            .table_name(&self.table)
            .key("pk", pk)
            .key("sk", sk)
            .send()
            .await?;
        out.item
            .map(|item| {
                item.get("expires_epoch")
                    .and_then(|v| v.as_n().ok())
                    .and_then(|n| n.parse::<i64>().ok())
                    .ok_or(StoreError::Corrupt("session missing expires_epoch"))
            })
            .transpose()
    }

    /// Delete an admin session. Idempotent — silently succeeds if the token does not exist.
    pub async fn delete_session(&self, token: &str) -> Result<(), StoreError> {
        let (pk, sk) = schema::key_pair(session_pk(token), "META");
        self.client
            .delete_item()
            .table_name(&self.table)
            .key("pk", pk)
            .key("sk", sk)
            .send()
            .await?;
        Ok(())
    }

    /// Persist the Steam identity (SteamID string) under CONFIG#STEAM. Idempotent — overwrites any
    /// existing record. Use this as the single source of truth for Ben's Steam account ID.
    pub async fn put_steam_identity(&self, steamid: &str) -> Result<(), StoreError> {
        self.client
            .put_item()
            .table_name(&self.table)
            .set_item(Some(steam_identity_item(steamid)))
            .send()
            .await?;
        Ok(())
    }

    /// Retrieve the stored Steam identity string, or `None` if not yet configured.
    pub async fn get_steam_identity(&self) -> Result<Option<String>, StoreError> {
        self.get_meta("CONFIG#STEAM").await
    }

    /// Remove the Steam identity record. Idempotent — silently succeeds if absent.
    pub async fn delete_steam_identity(&self) -> Result<(), StoreError> {
        let (pk, sk) = schema::key_pair("CONFIG#STEAM", "META");
        self.client
            .delete_item()
            .table_name(&self.table)
            .key("pk", pk)
            .key("sk", sk)
            .send()
            .await?;
        Ok(())
    }

    /// Write (or refresh) the Steam-owned-games cache for the given Steam ID.
    /// pk="STEAMOWN#<steamid>", sk="META", ttl=now_epoch+7d. Idempotent — overwrites any existing
    /// entry. Callers must enforce staleness via `fetched_at` until DynamoDB TTL is enabled.
    pub async fn put_steam_owned(
        &self,
        steamid: &str,
        appids: &[u32],
        now_epoch: i64,
    ) -> Result<(), StoreError> {
        self.client
            .put_item()
            .table_name(&self.table)
            .set_item(Some(steam_owned_item(steamid, appids, now_epoch)))
            .send()
            .await?;
        Ok(())
    }

    /// Look up the Steam-owned-games cache. Returns `(appids, fetched_at)` or `None` if absent
    /// (never written or TTL-evicted). `fetched_at` is seconds since Unix epoch.
    pub async fn get_steam_owned(
        &self,
        steamid: &str,
    ) -> Result<Option<(Vec<u32>, i64)>, StoreError> {
        let pk = format!("STEAMOWN#{steamid}");
        let cache: Option<SteamOwnedCache> = self.get_meta(&pk).await?;
        Ok(cache.map(|c| (c.appids, c.fetched_at)))
    }

    /// Write (or refresh) a Steam app enrichment cache entry — guarded (#75).
    /// pk="STEAMAPP#<app_id>", sk="META", body=JSON of [`SteamAppCache`],
    /// version=N monotonic counter. Succeeds only if the item still matches the
    /// read in `guard`; otherwise [`SteamAppPutError::LostRace`] — re-read via
    /// [`Store::get_steam_app_versioned`], re-merge, retry. `detail: None` is a
    /// valid negative-cache stub.
    pub async fn put_steam_app(
        &self,
        cache: &SteamAppCache,
        guard: SteamAppPutGuard,
    ) -> Result<(), SteamAppPutError> {
        let req = self.client.put_item().table_name(&self.table);
        let req = match guard {
            SteamAppPutGuard::Absent => req
                .set_item(Some(steam_app_item(cache, 1)))
                .condition_expression("attribute_not_exists(pk)"),
            // Legacy item (pre-version): adopt at version 1. Cannot false-pass —
            // any concurrent new-code write stamps `version`, flipping this arm
            // to a CCF. (A vanished item also passes, which is create — correct.)
            SteamAppPutGuard::Unchanged(SteamAppVersion(None)) => req
                .set_item(Some(steam_app_item(cache, 1)))
                .condition_expression("attribute_not_exists(version)"),
            SteamAppPutGuard::Unchanged(SteamAppVersion(Some(v))) => req
                .set_item(Some(steam_app_item(cache, v + 1)))
                .condition_expression("version = :v")
                .expression_attribute_values(
                    ":v",
                    aws_sdk_dynamodb::types::AttributeValue::N(v.to_string()),
                ),
        };
        req.send().await.map_err(|e| {
            if is_ccf_put(&e) {
                SteamAppPutError::LostRace
            } else {
                SteamAppPutError::Store(e.into())
            }
        })?;
        Ok(())
    }

    /// Writer-side read of a STEAMAPP# item: parsed cache + the optimistic-lock
    /// token for a subsequent guarded [`Store::put_steam_app`]. Read-only paths
    /// (admin/public views) keep using [`Store::get_steam_app`].
    pub async fn get_steam_app_versioned(
        &self,
        app_id: u32,
    ) -> Result<Option<(SteamAppCache, SteamAppVersion)>, StoreError> {
        let (pk, sk) = schema::key_pair(&steam_app_pk(app_id), "META");
        let out = self
            .client
            .get_item()
            .table_name(&self.table)
            .key("pk", pk)
            .key("sk", sk)
            .send()
            .await?;
        let Some(item) = out.item else {
            return Ok(None);
        };
        let cache: SteamAppCache = parse_body(&item)?;
        let version = match item.get("version") {
            None => SteamAppVersion(None),
            Some(v) => SteamAppVersion(Some(
                v.as_n()
                    .ok()
                    .and_then(|s| s.parse::<i64>().ok())
                    .ok_or(StoreError::Corrupt("bad version attr"))?,
            )),
        };
        Ok(Some((cache, version)))
    }

    /// Look up a Steam app enrichment cache entry by app_id, or `None` if absent.
    /// `detail: None` in the returned struct means the app is a negative-cache stub (delisted).
    pub async fn get_steam_app(&self, app_id: u32) -> Result<Option<SteamAppCache>, StoreError> {
        self.get_meta(&steam_app_pk(app_id)).await
    }

    /// Batch-fetch Steam app cache entries by app_id — one `BatchGetItem` per 100
    /// ids (the DynamoDB batch cap), re-requesting unprocessed keys until drained.
    /// Missing appids are simply absent from the returned map; callers decide how
    /// to degrade. Avoids the N-serial-GetItem shape on hot read paths (the link
    /// view reads genres for the whole listable catalog per request).
    pub async fn batch_get_steam_apps(
        &self,
        app_ids: &[u32],
    ) -> Result<HashMap<u32, SteamAppCache>, StoreError> {
        use aws_sdk_dynamodb::types::KeysAndAttributes;
        let mut caches = HashMap::with_capacity(app_ids.len());
        for chunk in app_ids.chunks(100) {
            let mut keys: Vec<HashMap<String, aws_sdk_dynamodb::types::AttributeValue>> = chunk
                .iter()
                .map(|app_id| {
                    let (pk, sk) = schema::key_pair(steam_app_pk(*app_id), "META");
                    HashMap::from([("pk".to_string(), pk), ("sk".to_string(), sk)])
                })
                .collect();
            while !keys.is_empty() {
                let ka = KeysAndAttributes::builder()
                    .set_keys(Some(keys))
                    .build()
                    .map_err(|e| StoreError::Aws(format!("{e:?}")))?;
                let resp = self
                    .client
                    .batch_get_item()
                    .request_items(&self.table, ka)
                    .send()
                    .await?;
                for item in resp
                    .responses()
                    .and_then(|tables| tables.get(&self.table))
                    .map(|items| items.as_slice())
                    .unwrap_or_default()
                {
                    let c: SteamAppCache = parse_body(item)?;
                    caches.insert(c.app_id, c);
                }
                keys = resp
                    .unprocessed_keys()
                    .and_then(|tables| tables.get(&self.table))
                    .map(|ka| ka.keys().to_vec())
                    .unwrap_or_default();
            }
        }
        Ok(caches)
    }

    /// Return all cached Steam app_ids. Paged Scan filtered on `begins_with(pk, "STEAMAPP#")` —
    /// the same rationale as `list_all_games` (at ~700 items a Scan is fine). Does NOT include
    /// non-STEAMAPP items (games, links, etc.).
    pub async fn list_steam_app_ids(&self) -> Result<Vec<u32>, StoreError> {
        let mut ids: Vec<u32> = Vec::new();
        let mut last_key: Option<HashMap<String, aws_sdk_dynamodb::types::AttributeValue>> = None;
        loop {
            let out = self
                .client
                .scan()
                .table_name(&self.table)
                .filter_expression("begins_with(pk, :pfx) AND sk = :meta")
                .expression_attribute_values(
                    ":pfx",
                    aws_sdk_dynamodb::types::AttributeValue::S("STEAMAPP#".into()),
                )
                .expression_attribute_values(
                    ":meta",
                    aws_sdk_dynamodb::types::AttributeValue::S("META".into()),
                )
                .set_exclusive_start_key(last_key.take())
                .send()
                .await?;
            for item in out.items() {
                // Parse app_id from the pk attribute ("STEAMAPP#<id>") — avoids decoding the
                // full body JSON just to extract the id.
                let app_id = item
                    .get("pk")
                    .and_then(|v| v.as_s().ok())
                    .and_then(|s| s.strip_prefix("STEAMAPP#"))
                    .and_then(|n| n.parse::<u32>().ok())
                    .ok_or(StoreError::Corrupt("STEAMAPP item missing or bad pk"))?;
                ids.push(app_id);
            }
            match out.last_evaluated_key() {
                None => break,
                Some(k) => last_key = Some(k.clone()),
            }
        }
        Ok(ids)
    }

    /// Admin appid override: the ONLY writer allowed to bypass the `Manual` guard and clear
    /// `steam_app_id` to `None`. Called by the admin `POST /admin/api/games/:id/steam-app-id`
    /// endpoint.
    ///
    /// - `appid = Some(id)` → sets `steam_app_id = id, appid_source = Manual`.
    /// - `appid = None`     → clears both fields to `None`; auto-resolution reruns next sync.
    ///
    /// Uses the same optimistic-lock-on-status pattern as `set_game_hidden` — a concurrent
    /// claim that lands between our read and the put CCFs the condition → `Contested`.
    /// Returns `Contested` immediately if the game is already `Pending` at read time.
    pub async fn set_game_steam_appid_admin(
        &self,
        game_id: &str,
        appid: Option<u32>,
    ) -> Result<AppidWrite, StoreError> {
        let Some(mut game) = self.get_game(game_id).await? else {
            return Ok(AppidWrite::NotFound);
        };

        if game.status == GameStatus::Pending {
            return Ok(AppidWrite::Contested);
        }

        game.steam_app_id = appid;
        game.appid_source = appid.map(|_| domain::AppidSource::Manual);

        // Optimistic lock: status must match what we read. Mirrors set_game_hidden.
        let status_str = game.status.as_wire();

        let res = self
            .client
            .put_item()
            .table_name(&self.table)
            .set_item(Some(game_item(&game)))
            .expression_attribute_names("#st", "status")
            .expression_attribute_values(":expected", schema::s(status_str))
            .condition_expression("#st = :expected")
            .send()
            .await;

        match res {
            Ok(_) => Ok(AppidWrite::Written),
            Err(sdk_err) => {
                if is_ccf_put(&sdk_err) {
                    Ok(AppidWrite::Contested)
                } else {
                    Err(StoreError::Aws(format!("{sdk_err:?}")))
                }
            }
        }
    }

    /// Toggle a game's `owned_by_ben` flag with a guarded conditional write. Structural copy of
    /// `set_game_hidden` — uses the same optimistic-lock-on-status pattern to close the
    /// admin-toggle vs claim race.
    ///
    /// Returns `Contested` immediately if the game is `Pending` (a claim is in flight). A claim
    /// landing AFTER the initial read flips status to `Pending`, which CCFs the condition → `Contested`.
    pub async fn set_game_owned_by_ben(
        &self,
        game_id: &str,
        owned: bool,
    ) -> Result<OwnedWrite, StoreError> {
        let Some(mut game) = self.get_game(game_id).await? else {
            return Ok(OwnedWrite::NotFound);
        };

        // Pending means a claim is actively in flight — return Contested immediately.
        if game.status == GameStatus::Pending {
            return Ok(OwnedWrite::Contested);
        }

        game.owned_by_ben = owned;

        // Optimistic lock: status must match what we read. Mirrors set_game_hidden.
        let status_str = game.status.as_wire();

        let res = self
            .client
            .put_item()
            .table_name(&self.table)
            .set_item(Some(game_item(&game)))
            .expression_attribute_names("#st", "status")
            .expression_attribute_values(":expected", schema::s(status_str))
            .condition_expression("#st = :expected")
            .send()
            .await;

        match res {
            Ok(_) => Ok(OwnedWrite::Written),
            Err(sdk_err) => {
                if is_ccf_put(&sdk_err) {
                    Ok(OwnedWrite::Contested)
                } else {
                    Err(StoreError::Aws(format!("{sdk_err:?}")))
                }
            }
        }
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

#[cfg(test)]
mod tests {
    use super::*;
    use aws_sdk_dynamodb::types::CancellationReason;

    fn reason(code: Option<&str>) -> CancellationReason {
        let mut b = CancellationReason::builder();
        if let Some(c) = code {
            b = b.code(c);
        }
        b.build()
    }

    // dynamodb-local can't be coerced into producing a live TransactionConflict on demand, so
    // the conflict→TxConflict decision is unit-tested against the mapping function directly with
    // synthetic cancellation-reason vectors. (The positional CCF cases mirror what the live
    // integration tests already exercise end-to-end.)

    #[test]
    fn conflict_reason_maps_to_txconflict() {
        // A pure timing race: no CCF anywhere, a TransactionConflict on one item.
        let reasons = vec![
            reason(Some("TransactionConflict")),
            reason(None),
            reason(None),
        ];
        assert!(matches!(
            claim_cancellation_error(&reasons),
            Some(ClaimTxError::TxConflict)
        ));
    }

    #[test]
    fn conditional_check_beats_conflict() {
        // Mixed cancel: the game's CCF is a definitive business answer and must win over a
        // co-occurring TransactionConflict on the link — never degrade a real "taken" to a retry.
        let reasons = vec![
            reason(Some("ConditionalCheckFailed")),
            reason(Some("TransactionConflict")),
            reason(None),
        ];
        assert!(matches!(
            claim_cancellation_error(&reasons),
            Some(ClaimTxError::GameUnavailable)
        ));
    }

    #[test]
    fn positional_ccf_mapping() {
        assert!(matches!(
            claim_cancellation_error(&[
                reason(Some("ConditionalCheckFailed")),
                reason(None),
                reason(None)
            ]),
            Some(ClaimTxError::GameUnavailable)
        ));
        assert!(matches!(
            claim_cancellation_error(&[
                reason(None),
                reason(Some("ConditionalCheckFailed")),
                reason(None)
            ]),
            Some(ClaimTxError::LinkNotClaimable)
        ));
        assert!(matches!(
            claim_cancellation_error(&[
                reason(None),
                reason(None),
                reason(Some("ConditionalCheckFailed"))
            ]),
            Some(ClaimTxError::DuplicateClaim)
        ));
    }

    #[test]
    fn unclassifiable_cancel_is_none() {
        // No CCF, no conflict → caller falls through to the loud Store error.
        let reasons = vec![reason(None), reason(None), reason(None)];
        assert!(claim_cancellation_error(&reasons).is_none());
    }
}
