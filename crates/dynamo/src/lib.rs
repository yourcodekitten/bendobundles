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
    session_pk, sync_state_item,
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

/// Persisted summary of a catalog-sync run. Storage-shaped (lives in dynamo, not in domain).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct SyncState {
    pub last_run_epoch: i64,
    pub ok: bool,
    pub cookie_ok: bool,
    pub games_written: u32,
    pub message: String,
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

/// Deserialize a LINK META item, overriding EVERY enforcer field from the authoritative
/// top-level attributes. The `body` blob is a convenience copy; the fields `claim_game`'s
/// condition expression actually enforces — `claims_used`, `claims_allowed`, `revoked`,
/// `expires_at` — live as top-level attributes (see `schema::link_item`) and are what
/// concurrent writers (claim's atomic ADD, compensate's decrement, `update_link_meta`'s
/// scoped SET/REMOVE) keep current. Reading any of them from `body` is a latent lost-update:
/// harmless while body and attrs move in lockstep, live the day any writer moves an attr
/// without rewriting body. So: body for identity/cosmetics, top-level attrs for enforcement.
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
            .expression_attribute_values(
                ":b",
                schema::s(serde_json::to_string(l).expect("link serializes")),
            )
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
            .expression_attribute_values(":b", av_s(&serde_json::to_string(&bumped).expect("link")))
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

        // Optimistic lock: status must match what we read. Mirrors upsert_game_from_sync.
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
        let mut req = self
            .client
            .put_item()
            .table_name(&self.table)
            .set_item(Some(game_item(&merged)));

        let cond = match &existing {
            None => "attribute_not_exists(pk)".to_string(),
            Some(e) => {
                let status_str = serde_json::to_value(e.status)
                    .expect("status serializes")
                    .as_str()
                    .expect("status is a string")
                    .to_string();
                req = req
                    .expression_attribute_names("#st", "status")
                    .expression_attribute_values(":expected", schema::s(status_str));
                // If the existing game is owned, add the ownership clause to the condition.
                if let Some(cid) = &e.claim_id {
                    req = req.expression_attribute_values(":cid", schema::s(cid.clone()));
                    "#st = :expected AND claim_id = :cid".to_string()
                } else {
                    "#st = :expected".to_string()
                }
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
