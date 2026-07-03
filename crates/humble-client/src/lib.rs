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
    /// Humble returned success=false with a reason that is not "already redeemed".
    /// Refusal reasons vary (non-giftable, gifting disabled, transient) — the exact
    /// already-redeemed phrasing is community-documented, unverified against the live API until
    /// the first real gifting; callers must treat RedeemRefused conservatively (park, don't
    /// assume the key survives or burned).
    #[error("humble refused the redeem: {0}")]
    RedeemRefused(String),
    #[error(
        "humble reported success but returned no gift key — outcome ambiguous, do not retry blindly"
    )]
    AmbiguousRedeem,
    #[error("humble returned status {0}")]
    Api(u16),
    #[error("network error talking to humble: {0}")]
    Network(#[from] reqwest::Error),
    #[error("could not parse humble response: {0}")]
    Parse(serde_json::Error),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Order {
    pub gamekey: String,
    pub bundle_name: String,
    pub keys: Vec<KeyEntry>,
    pub subproducts: Vec<Subproduct>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyEntry {
    pub machine_name: String,
    pub human_name: String,
    pub key_type: String,
    pub redeemed: bool,
    pub expired: bool,
    pub giftable: bool,
    pub keyindex: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Subproduct {
    pub machine_name: String,
    pub human_name: String,
    pub icon: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GiftUrl(pub String);

#[derive(serde::Deserialize)]
struct RedeemResponse {
    // No #[serde(default)] — a 200 body missing `success` must be a parse error, not silently
    // treated as failure.
    success: bool,
    #[serde(default)]
    giftkey: Option<String>,
    #[serde(default)]
    errormsg: Option<String>,
}

/// Decode a response body as JSON. On serde failure, detect HTML login interstitials
/// (humble sometimes serves a 200 with HTML when the session cookie is stale) and surface
/// them as Unauthorized rather than a confusing Parse error.
fn decode_body<T: serde::de::DeserializeOwned>(bytes: &[u8]) -> Result<T, HumbleError> {
    serde_json::from_slice::<T>(bytes).map_err(|e| {
        let first_nonws = bytes
            .iter()
            .find(|&&b| !b.is_ascii_whitespace())
            .copied()
            .unwrap_or(0);
        if first_nonws == b'<' {
            HumbleError::Unauthorized
        } else {
            HumbleError::Parse(e)
        }
    })
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
            200 => {
                let bytes = resp.bytes().await?;
                decode_body(&bytes)
            }
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
                        keyindex: t.keyindex,
                    }
                })
                .collect(),
            subproducts: wire
                .subproducts
                .into_iter()
                .map(|s| Subproduct {
                    machine_name: s.machine_name,
                    human_name: s.human_name,
                    icon: s.icon,
                })
                .collect(),
        })
    }

    pub async fn redeem_as_gift(
        &self,
        gamekey: &str,
        machine_name: &str,
        keyindex: u32,
    ) -> Result<GiftUrl, HumbleError> {
        let resp = self
            .http
            .post(format!("{}/humbler/redeemkey", self.base))
            .header("Cookie", format!("_simpleauth_sess={}", self.cookie.0))
            .header("X-Requested-By", "hb_android_app")
            // keyindex now passes the tpk's true index; we pass the position in the order's
            // key list. if humble actually selects by keytype=<machine_name>, this index is
            // redundant. VERIFY on the first real gifting of a non-first key in an order
            // (the read-only probe cannot test redeems by design) — tracked for the plan-2
            // live receipt.
            .form(&[
                ("keytype", machine_name),
                ("key", gamekey),
                ("keyindex", &keyindex.to_string()),
                ("gift", "true"),
            ])
            .send()
            .await?;
        match resp.status().as_u16() {
            200 => {
                let bytes = resp.bytes().await?;
                let body: RedeemResponse = decode_body(&bytes)?;
                match (body.success, body.giftkey) {
                    (true, Some(token)) => Ok(GiftUrl(format!(
                        "https://www.humblebundle.com/gift?key={token}"
                    ))),
                    // AmbiguousRedeem: possible API drift where humble claims success but hands
                    // back no key — the key MAY have already burned server-side. Callers must
                    // PARK and reconcile, never compensate: compensating would re-list a key that
                    // could be spent, double-gifting it. Distinct from AlreadyRedeemed on purpose.
                    (true, None) => Err(HumbleError::AmbiguousRedeem),
                    (false, _) => {
                        let msg = body
                            .errormsg
                            .unwrap_or_else(|| "no error message".to_string());
                        // Community-documented phrase: "This key has already been redeemed."
                        // Belt-and-suspenders: also catch if humble ever shortens it to
                        // "already redeemed" without "been".
                        let lower = msg.to_lowercase();
                        if lower.contains("already been redeemed")
                            || lower.contains("already redeemed")
                        {
                            Err(HumbleError::AlreadyRedeemed)
                        } else {
                            Err(HumbleError::RedeemRefused(msg))
                        }
                    }
                }
            }
            401 | 403 | 302 => Err(HumbleError::Unauthorized),
            429 => Err(HumbleError::RateLimited),
            s => Err(HumbleError::Api(s)),
        }
    }
}
