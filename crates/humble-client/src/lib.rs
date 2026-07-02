//! Client for the community-documented unofficial Humble Bundle API.
//! No test touches the real API — see the probe binary for live verification.
mod model;

use model::{GamekeyEntry, OrderWire};

pub struct SessionCookie(String);

impl SessionCookie {
    pub fn new(value: String) -> Self {
        Self(value)
    }
}

impl std::fmt::Debug for SessionCookie {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "SessionCookie(REDACTED)")
    }
}

#[derive(Debug, thiserror::Error)]
pub enum HumbleError {
    #[error("session cookie rejected — needs a fresh paste")]
    Unauthorized,
    #[error("humble rate-limited us")]
    RateLimited,
    #[error("key already redeemed on humble")]
    AlreadyRedeemed,
    #[error(
        "humble reported success but returned no gift key — outcome ambiguous, do not retry blindly"
    )]
    AmbiguousRedeem,
    #[error("humble returned status {0}")]
    Api(u16),
    #[error("network error talking to humble: {0}")]
    Network(#[from] reqwest::Error),
    #[error("could not parse humble response: {0}")]
    Parse(#[from] serde_json::Error),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Order {
    pub gamekey: String,
    pub bundle_name: String,
    pub keys: Vec<KeyEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyEntry {
    pub machine_name: String,
    pub human_name: String,
    pub key_type: String,
    pub redeemed: bool,
    pub expired: bool,
    pub giftable: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GiftUrl(pub String);

#[derive(serde::Deserialize)]
struct RedeemResponse {
    #[serde(default)]
    success: bool,
    #[serde(default)]
    giftkey: Option<String>,
}

pub struct HumbleClient {
    http: reqwest::Client,
    base: String,
    cookie: SessionCookie,
}

impl HumbleClient {
    pub fn new(base_url: &str, cookie: SessionCookie) -> Result<Self, HumbleError> {
        let http = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none()) // a 302-to-login must surface, not follow
            .build()?;
        Ok(Self {
            http,
            base: base_url.trim_end_matches('/').to_string(),
            cookie,
        })
    }

    async fn get_json<T: serde::de::DeserializeOwned>(
        &self,
        path_q: &str,
    ) -> Result<T, HumbleError> {
        let resp = self
            .http
            .get(format!("{}{path_q}", self.base))
            .header("Cookie", format!("_simpleauth_sess={}", self.cookie.0))
            .header("X-Requested-By", "hb_android_app")
            .send()
            .await?;
        match resp.status().as_u16() {
            200 => Ok(resp.json::<T>().await?),
            401 | 403 | 302 => Err(HumbleError::Unauthorized),
            429 => Err(HumbleError::RateLimited),
            s => Err(HumbleError::Api(s)),
        }
    }

    pub async fn gamekeys(&self) -> Result<Vec<String>, HumbleError> {
        let entries: Vec<GamekeyEntry> = self.get_json("/api/v1/user/order").await?;
        Ok(entries.into_iter().map(|e| e.gamekey).collect())
    }

    pub async fn order(&self, gamekey: &str) -> Result<Order, HumbleError> {
        let wire: OrderWire = self
            .get_json(&format!("/api/v1/order/{gamekey}?all_tpkds=true"))
            .await?;
        Ok(Order {
            gamekey: wire.gamekey,
            bundle_name: wire.product.human_name,
            keys: wire
                .tpkd_dict
                .all_tpks
                .into_iter()
                .map(|t| {
                    let redeemed = t.redeemed_key_val.is_some();
                    let expired = t.is_expired;
                    KeyEntry {
                        giftable: !redeemed && !expired,
                        machine_name: t.machine_name,
                        human_name: t.human_name,
                        key_type: t.key_type,
                        redeemed,
                        expired,
                    }
                })
                .collect(),
        })
    }

    pub async fn redeem_as_gift(
        &self,
        gamekey: &str,
        machine_name: &str,
    ) -> Result<GiftUrl, HumbleError> {
        let resp = self
            .http
            .post(format!("{}/humbler/redeemkey", self.base))
            .header("Cookie", format!("_simpleauth_sess={}", self.cookie.0))
            .header("X-Requested-By", "hb_android_app")
            .form(&[
                ("keytype", machine_name),
                ("key", gamekey),
                ("keyindex", "0"),
                ("gift", "true"),
            ])
            .send()
            .await?;
        match resp.status().as_u16() {
            200 => {
                let body: RedeemResponse = resp.json().await?;
                match (body.success, body.giftkey) {
                    (true, Some(token)) => Ok(GiftUrl(format!(
                        "https://www.humblebundle.com/gift?key={token}"
                    ))),
                    // AmbiguousRedeem: possible API drift where humble claims success but hands
                    // back no key — the key MAY have already burned server-side. Callers must
                    // PARK and reconcile, never compensate: compensating would re-list a key that
                    // could be spent, double-gifting it. Distinct from AlreadyRedeemed on purpose.
                    (true, None) => Err(HumbleError::AmbiguousRedeem),
                    (false, _) => Err(HumbleError::AlreadyRedeemed),
                }
            }
            401 | 403 | 302 => Err(HumbleError::Unauthorized),
            429 => Err(HumbleError::RateLimited),
            s => Err(HumbleError::Api(s)),
        }
    }
}
