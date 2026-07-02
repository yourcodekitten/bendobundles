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
        (
            "body".into(),
            s(serde_json::to_string(g).expect("game serializes")),
        ),
    ]);
    item.insert(
        "status".into(),
        s(serde_json::to_value(g.status)
            .expect("status serializes")
            .as_str()
            .expect("status is a string")
            .to_string()),
    );
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
pub fn link_item(l: &Link) -> HashMap<String, AttributeValue> {
    let mut item = HashMap::from([
        ("pk".into(), s(link_pk(&l.token))),
        ("sk".into(), s("META")),
        (
            "body".into(),
            s(serde_json::to_string(l).expect("link serializes")),
        ),
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
    if let Some(exp) = l.expires_at {
        let ts = exp
            .format(&time::format_description::well_known::Rfc3339)
            .expect("rfc3339");
        item.insert("expires_at".into(), s(ts));
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
