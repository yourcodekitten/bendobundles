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
