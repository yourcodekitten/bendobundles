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
    #[serde(default)]
    pub subproducts: Vec<SubproductWire>,
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
    #[serde(default)]
    pub keyindex: u32,
}

#[derive(Deserialize)]
pub(crate) struct SubproductWire {
    pub machine_name: String,
    pub human_name: String,
    #[serde(default)]
    pub icon: Option<String>,
}

// ── Humble Choice: the `webpack-monthly-product-data` blob embedded in `/membership/<month>` ──────

#[derive(Deserialize)]
pub(crate) struct MembershipBlob {
    #[serde(rename = "contentChoiceOptions")]
    pub content_choice_options: ContentChoiceOptions,
}

#[derive(Deserialize)]
pub(crate) struct ContentChoiceOptions {
    pub gamekey: String,
    pub title: String,
    #[serde(rename = "productUrlPath")]
    pub product_url_path: String,
    #[serde(rename = "productMachineName")]
    pub product_machine_name: String,
    #[serde(default, rename = "usesChoices")]
    pub uses_choices: bool,
    #[serde(default, rename = "isActiveContent")]
    pub is_active_content: bool,
    #[serde(default, rename = "canRedeemGames")]
    pub can_redeem_games: bool,
    #[serde(rename = "contentChoiceData")]
    pub content_choice_data: ContentChoiceData,
}

#[derive(Deserialize)]
pub(crate) struct ContentChoiceData {
    pub initial: ContentChoiceInitial,
}

#[derive(Deserialize)]
pub(crate) struct ContentChoiceInitial {
    #[serde(default)]
    pub total_choices: u32,
    /// machine_name → offered game. A claim-all month (`uses_choices=false`) still lists every
    /// game here; `total_choices` just isn't a limiting budget for that tier.
    #[serde(default)]
    pub content_choices: std::collections::HashMap<String, ContentChoiceGame>,
}

#[derive(Deserialize)]
pub(crate) struct ContentChoiceGame {
    #[serde(default)]
    pub title: String,
}

// ── Humble Choice: the paginated month list ──────────────────────────────────────────────────────
// GET /api/v1/subscriptions/humble_monthly/subscription_products_with_gamekeys/<cursor>
// Cursor is an opaque token in the URL PATH (not a query param); each response hands back the next
// page's cursor. 3 months/page, newest-first; terminate when `cursor` is absent/empty.

#[derive(Deserialize)]
pub(crate) struct SubProductsPage {
    #[serde(default)]
    pub cursor: Option<String>,
    #[serde(default)]
    pub products: Vec<SubProductWire>,
}

#[derive(Deserialize)]
pub(crate) struct SubProductWire {
    pub gamekey: String,
    #[serde(default)]
    pub title: String,
    #[serde(rename = "productUrlPath")]
    pub product_url_path: String,
    #[serde(rename = "productMachineName")]
    pub product_machine_name: String,
    #[serde(default, rename = "usesChoices")]
    pub uses_choices: bool,
    #[serde(default, rename = "isActiveContent")]
    pub is_active_content: bool,
    #[serde(default, rename = "canRedeemGames")]
    pub can_redeem_games: bool,
    #[serde(default, rename = "contentChoiceData")]
    pub content_choice_data: SubContentChoiceData,
}

/// The subscription endpoint nests offered games under `contentChoiceData.game_data` (a
/// machine_name→game map) — distinct from the membership blob's `contentChoiceData.initial`.
#[derive(Deserialize, Default)]
pub(crate) struct SubContentChoiceData {
    #[serde(default)]
    pub game_data: std::collections::HashMap<String, ContentChoiceGame>,
}
