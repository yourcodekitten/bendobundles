//! DynamoDB storage. Single table; see schema.rs for the item contract.
pub mod schema;

use aws_sdk_dynamodb::Client;
use aws_sdk_dynamodb::types::{
    AttributeDefinition, BillingMode, GlobalSecondaryIndex, KeySchemaElement, KeyType, Projection,
    ProjectionType, ScalarAttributeType,
};
use domain::{Claim, Game, Link};
use schema::{claim_item, claim_sk, game_item, game_pk, link_item, link_pk, parse_body};

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
