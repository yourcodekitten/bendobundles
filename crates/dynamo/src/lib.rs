//! DynamoDB storage. Single table; see schema.rs for the item contract.
pub mod schema;

use aws_sdk_dynamodb::Client;
use aws_sdk_dynamodb::types::{
    AttributeDefinition, BillingMode, GlobalSecondaryIndex, KeySchemaElement, KeyType, Projection,
    ProjectionType, ScalarAttributeType,
};
use domain::{Claim, ClaimState, Game, GameStatus, Link};
use schema::{claim_item, claim_sk, game_item, game_pk, link_item, link_pk, parse_body};
use time::OffsetDateTime;

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
        let out = self
            .client
            .get_item()
            .table_name(&self.table)
            .key(
                "pk",
                aws_sdk_dynamodb::types::AttributeValue::S(link_pk(token)),
            )
            .key(
                "sk",
                aws_sdk_dynamodb::types::AttributeValue::S("META".into()),
            )
            .send()
            .await?;
        out.item
            .map(|item| {
                let mut link: Link = parse_body(&item)?;
                // Top-level `claims_used` (N) is the authoritative counter — updated atomically
                // via ADD in claim_game's transaction. Override body's potentially stale value.
                if let Some(n) = item.get("claims_used").and_then(|v| v.as_n().ok())
                    && let Ok(v) = n.parse::<u32>()
                {
                    link.claims_used = v;
                }
                Ok(link)
            })
            .transpose()
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
            .key(
                "pk",
                aws_sdk_dynamodb::types::AttributeValue::S(link_pk(link_token)),
            )
            .key(
                "sk",
                aws_sdk_dynamodb::types::AttributeValue::S(claim_sk(claim_id)),
            )
            .send()
            .await?;
        out.item.map(|i| parse_body(&i)).transpose()
    }

    /// Single Query page (DynamoDB's 1 MB / page cap, no pagination). Fine at this app's scale —
    /// one person's giftable-game catalog is small — but do NOT assume completeness at larger
    /// scale: a bigger listable set would need `.into_paginator()` to be exhaustive.
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
            .key(
                "sk",
                aws_sdk_dynamodb::types::AttributeValue::S("META".into()),
            )
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
        };
        let now_ts = now
            .format(&time::format_description::well_known::Rfc3339)
            .expect("rfc3339");

        let av_s = |v: &str| aws_sdk_dynamodb::types::AttributeValue::S(v.to_string());

        let game_update = aws_sdk_dynamodb::types::Update::builder()
            .table_name(&self.table)
            .key("pk", av_s(&game_pk(game_id)))
            .key("sk", av_s("META"))
            .update_expression("SET body = :b, #st = :pending REMOVE gsi1pk, gsi1sk")
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
            .build()
            .expect("game_update");
        let link_update = aws_sdk_dynamodb::types::Update::builder()
            .table_name(&self.table)
            .key("pk", av_s(&link_pk(link_token)))
            .key("sk", av_s("META"))
            .update_expression("SET body = :b ADD claims_used :one")
            // expires_at > :now is a lexicographic string compare on RFC3339 timestamps; it is only
            // correct because every writer emits time's Rfc3339 in UTC (uniform offset). Mixed UTC
            // offsets would break the ordering — all writers must stay UTC.
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
            .expression_attribute_values(":now", av_s(&now_ts))
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
                // Capture debug string before borrowing sdk_err via as_service_error()
                let err_str = format!("{sdk_err:?}");
                // In aws-sdk-dynamodb 1.116.0 there is no as_transaction_canceled_exception();
                // pattern-match directly on the public enum variant instead.
                if let Some(
                    aws_sdk_dynamodb::operation::transact_write_items::TransactWriteItemsError::TransactionCanceledException(tce),
                ) = sdk_err.as_service_error()
                {
                    let reasons = tce.cancellation_reasons();
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
                let lost_condition = matches!(
                    sdk_err.as_service_error(),
                    Some(
                        aws_sdk_dynamodb::operation::put_item::PutItemError::ConditionalCheckFailedException(_)
                    )
                );
                if !lost_condition {
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

        // write 2: game flips pending → gifted. Conditional on status==pending so a retry after a
        // completed flip is an idempotent no-op (game already gifted).
        let mut game = self
            .get_game(game_id)
            .await?
            .ok_or(StoreError::Corrupt("fulfill: game missing"))?;
        game.status = GameStatus::Gifted;
        let game_res = self
            .client
            .put_item()
            .table_name(&self.table)
            .set_item(Some(game_item(&game)))
            .condition_expression("#st = :pending")
            .expression_attribute_names("#st", "status")
            .expression_attribute_values(
                ":pending",
                aws_sdk_dynamodb::types::AttributeValue::S("pending".into()),
            )
            .send()
            .await;
        match game_res {
            Ok(_) => Ok(()),
            Err(sdk_err) => {
                if matches!(
                    sdk_err.as_service_error(),
                    Some(aws_sdk_dynamodb::operation::put_item::PutItemError::ConditionalCheckFailedException(_))
                ) {
                    Ok(()) // game already flipped: idempotent no-op
                } else {
                    Err(StoreError::Aws(format!("{sdk_err:?}")))
                }
            }
        }
    }

    /// Idempotent + mutually exclusive with `fulfill_claim` (see that method for the shared
    /// pending-marker gate). A compensate that finds the marker already consumed returns Ok WITHOUT
    /// touching the game or link — retry-after-success must not double-decrement the link counter
    /// (that was the bug).
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

        // write 1: consume the pending marker. If it's already gone, a prior compensate (or a lost
        // fulfill) ran — return Ok and DO NOT touch game or link. Decrementing the link here on a
        // retry-after-success was the double-decrement bug.
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
                if matches!(
                    sdk_err.as_service_error(),
                    Some(aws_sdk_dynamodb::operation::put_item::PutItemError::ConditionalCheckFailedException(_))
                ) {
                    return Ok(()); // already compensated: no double side-effects
                }
                return Err(StoreError::Aws(format!("{sdk_err:?}")));
            }
        }

        // write 2: re-list the game (pending → available). Conditional on status==pending so we
        // never resurrect a game fulfill already flipped to gifted; ConditionalCheckFailed → no-op.
        let mut game = self
            .get_game(game_id)
            .await?
            .ok_or(StoreError::Corrupt("compensate: game missing"))?;
        game.status = GameStatus::Available;
        game.claim_id = None;
        let game_res = self
            .client
            .put_item()
            .table_name(&self.table)
            .set_item(Some(game_item(&game))) // game_item re-adds listable GSI attrs
            .condition_expression("#st = :pending")
            .expression_attribute_names("#st", "status")
            .expression_attribute_values(
                ":pending",
                aws_sdk_dynamodb::types::AttributeValue::S("pending".into()),
            )
            .send()
            .await;
        if let Err(sdk_err) = game_res
            && !matches!(
                sdk_err.as_service_error(),
                Some(aws_sdk_dynamodb::operation::put_item::PutItemError::ConditionalCheckFailedException(_))
            )
        {
            return Err(StoreError::Aws(format!("{sdk_err:?}")));
        }

        // write 3: atomically decrement claims_used. The condition prevents going below 0
        // (saturating semantics). ConditionalCheckFailed means counter already 0 — treat as success.
        let result = self
            .client
            .update_item()
            .table_name(&self.table)
            .key(
                "pk",
                aws_sdk_dynamodb::types::AttributeValue::S(link_pk(link_token)),
            )
            .key(
                "sk",
                aws_sdk_dynamodb::types::AttributeValue::S("META".into()),
            )
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
            .send()
            .await;
        match result {
            Ok(_) => Ok(()),
            Err(sdk_err) => {
                if let Some(
                    aws_sdk_dynamodb::operation::update_item::UpdateItemError::ConditionalCheckFailedException(
                        _,
                    ),
                ) = sdk_err.as_service_error()
                {
                    Ok(()) // counter already 0: saturating semantics, not an error
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
