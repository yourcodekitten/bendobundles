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
    /// The redeem WRITE was rejected at the auth/CSRF layer (401/403/302 on the POST). This is
    /// NOT proof the session cookie is dead — the live 2026-07-04 capture showed the redeem POST
    /// 403ing while the same cookie walked the full library on reads. Reads own the cookie-health
    /// signal; this variant must never trip the dead-cookie alarm.
    ///
    /// `csrf_minted` distinguishes a rejection of humble's own captured token from one where the
    /// preflight yielded no cookie and we fell back to minting — the latter means the CSRF
    /// capture itself is broken (or humble validates tokens server-side), a different repair.
    #[error(
        "humble rejected the redeem write at the auth/CSRF layer (status {status}, csrf_minted={csrf_minted})"
    )]
    RedeemAuthRejected { status: u16, csrf_minted: bool },
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

/// Read one response header as an owned `String`, or `"-"` when absent/unprintable. Kept small
/// so the diagnostic log line on a redeem rejection stays flat and greppable.
fn header_str(headers: &reqwest::header::HeaderMap, name: &str) -> String {
    headers
        .get(name)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("-")
        .to_string()
}

/// Collapse a response body into a single-line, length-bounded preview safe to log: control
/// characters (newlines, tabs, the trailing-tab humble sometimes emits) become spaces, and the
/// result is capped. Enough to tell a Cloudflare HTML challenge from a short humble-app JSON
/// refusal without dumping a whole page into CloudWatch.
fn body_signature(bytes: &[u8]) -> String {
    const MAX: usize = 300;
    let text = String::from_utf8_lossy(bytes);
    // Control chars → spaces, then collapse every whitespace run to one space, so the preview is
    // one clean line regardless of the source's formatting.
    let despaced: String = text
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    let collapsed = despaced.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.is_empty() {
        return "<empty>".to_string();
    }
    // Truncate and mark by CHARACTER count (never bytes — a mid-codepoint cut would panic).
    let out: String = collapsed.chars().take(MAX).collect();
    if collapsed.chars().count() > MAX {
        format!("{out}…")
    } else {
        out
    }
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

    /// The session cookie header value shared by every authenticated call — one builder so the
    /// read and write paths can never drift on how they authenticate.
    fn session_cookie(&self) -> String {
        format!("_simpleauth_sess={}", self.cookie.0)
    }

    async fn get_json<T: serde::de::DeserializeOwned>(
        &self,
        path_q: &str,
    ) -> Result<T, HumbleError> {
        let resp = self
            .http
            .get(format!("{}{path_q}", self.base))
            .header("Cookie", self.session_cookie())
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

    /// Fetch the value for humble's double-submit CSRF pair. A page GET (sent with the session
    /// cookie) makes humble set `csrf_cookie`; the redeem POST must replay that value as BOTH
    /// the `csrf_cookie` cookie and the `csrf-prevention-token` header. Returns `None` when the
    /// preflight fails or offers no cookie — the caller decides how loudly to treat that.
    async fn csrf_token(&self) -> Option<String> {
        let resp = match self
            .http
            .get(format!("{}/", self.base))
            .header("Cookie", self.session_cookie())
            .send()
            .await
        {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!(error = %e, "csrf preflight GET failed");
                return None;
            }
        };
        let mut captured = None;
        for sc in resp.headers().get_all("set-cookie") {
            let Ok(s) = sc.to_str() else { continue };
            if let Some(rest) = s.trim_start().strip_prefix("csrf_cookie=") {
                let val = rest.split(';').next().unwrap_or("").trim();
                if !val.is_empty() {
                    captured = Some(val.to_string());
                    break;
                }
            }
        }
        let status = resp.status().as_u16();
        // Drain the body so the connection returns to the pool — the redeem POST follows
        // immediately and shouldn't pay a fresh TCP+TLS handshake on the friend-facing path.
        let _ = resp.bytes().await;
        match captured {
            Some(v) => {
                tracing::info!("csrf preflight: captured csrf_cookie from humble");
                Some(v)
            }
            None => {
                tracing::warn!(status, "csrf preflight: no csrf_cookie offered");
                None
            }
        }
    }

    pub async fn redeem_as_gift(
        &self,
        gamekey: &str,
        machine_name: &str,
        keyindex: u32,
    ) -> Result<GiftUrl, HumbleError> {
        // Prefer humble's own token; mint only as a fallback. With a pure double-submit check
        // only the cookie/header MATCH matters and a wrong mint costs the 403 we'd get anyway —
        // but a minted pair is tracked (`csrf_minted`) so a systematic capture failure surfaces
        // as its own signal instead of masquerading as a generic auth rejection.
        let (csrf, csrf_minted) = match self.csrf_token().await {
            Some(t) => (t, false),
            None => {
                tracing::warn!(
                    "csrf capture failed — minting a double-submit fallback (a server-validated token check will reject this)"
                );
                (uuid::Uuid::new_v4().simple().to_string(), true)
            }
        };
        let resp = self
            .http
            .post(format!("{}/humbler/redeemkey", self.base))
            // Browser-shaped write, mirroring the proven redeemer flow (FailSpy's
            // humble-steam-key-redeemer): double-submit csrf_cookie + csrf-prevention-token
            // header, same-origin Origin/Referer, and NO X-Requested-By — the android-app header
            // belongs to the read API, not the browser-surface /humbler/ endpoints.
            .header(
                "Cookie",
                format!("{}; csrf_cookie={csrf}", self.session_cookie()),
            )
            .header("csrf-prevention-token", &csrf)
            .header("Origin", &self.base)
            .header("Referer", format!("{}/home/library", self.base))
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
        let status = resp.status().as_u16();
        // Diagnostic: this line named the original gift-redeem failure (live 403 on
        // 2026-07-04 → the missing-CSRF hypothesis this client now implements) and stays
        // as the primary live receipt for the redeem dance. Log the status + key
        // identifiers (never the cookie, the csrf token, or the gift token).
        tracing::info!(
            status,
            machine_name,
            keyindex,
            csrf_minted,
            "humble redeem POST response"
        );
        match status {
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
                        // The refusal text is humble's own — safe to log and the
                        // single most useful clue for a redeem that won't complete.
                        tracing::warn!(errormsg = %msg, "humble redeem refused (success=false)");
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
            401 | 403 | 302 => {
                // Auth/redirect rejection of the WRITE even though we now send the CSRF pair.
                // Typed separately from Unauthorized so callers never read this as cookie
                // death — a genuinely stale session surfaces as the 200-with-HTML interstitial
                // (decode_body → Unauthorized) or as read failures, both owned by the read paths.
                //
                // DIAGNOSTIC: the first live redeem on the CSRF fix (2026-07-04) captured
                // humble's OWN csrf_cookie and still 403'd — so double-submit *match* is not the
                // gate. The remaining suspects fail differently in the RESPONSE, not the status:
                // a Cloudflare bot-block returns an HTML challenge (content-type text/html, a
                // `cf-mitigated` header, body starting `<`); a humble-app CSRF/session refusal
                // returns short JSON or a `location` redirect to a login/verify path. These
                // fields name which one WITHOUT leaking anything — a 403/302 body is humble's
                // error/challenge page, never our cookie or a gift token.
                let content_type = header_str(resp.headers(), "content-type");
                let location = header_str(resp.headers(), "location");
                let cf_mitigated = header_str(resp.headers(), "cf-mitigated");
                let server = header_str(resp.headers(), "server");
                let body_preview = match resp.bytes().await {
                    Ok(b) => body_signature(&b),
                    Err(e) => format!("<body read failed: {e}>"),
                };
                tracing::warn!(
                    status,
                    csrf_minted,
                    content_type,
                    location,
                    cf_mitigated,
                    server,
                    body_preview,
                    "humble rejected the redeem write despite the CSRF pair — inspect the dance, do not blame the cookie"
                );
                Err(HumbleError::RedeemAuthRejected {
                    status,
                    csrf_minted,
                })
            }
            429 => Err(HumbleError::RateLimited),
            s => Err(HumbleError::Api(s)),
        }
    }
}

#[cfg(test)]
mod signature_tests {
    use super::body_signature;

    #[test]
    fn collapses_multiline_html_to_one_bounded_line() {
        let html =
            b"<!DOCTYPE html>\n<html>\n\t<body>Attention Required! Cloudflare</body>\n</html>";
        let sig = body_signature(html);
        assert!(
            !sig.contains('\n') && !sig.contains('\t'),
            "must be single-line: {sig:?}"
        );
        assert!(
            sig.starts_with("<!DOCTYPE html>"),
            "preview keeps the leading marker: {sig:?}"
        );
        assert!(sig.contains("Cloudflare"));
    }

    #[test]
    fn truncates_and_marks_long_bodies() {
        let long = vec![b'x'; 1000];
        let sig = body_signature(&long);
        assert!(
            sig.chars().count() <= 301,
            "capped near MAX (+ ellipsis): {}",
            sig.chars().count()
        );
        assert!(
            sig.ends_with('…'),
            "over-length bodies get an ellipsis: {sig:?}"
        );
    }

    #[test]
    fn empty_body_is_labeled() {
        assert_eq!(body_signature(b""), "<empty>");
        assert_eq!(body_signature(b"   \n\t  "), "<empty>");
    }

    #[test]
    fn short_json_passes_through_readable() {
        let sig = body_signature(br#"{"error":"csrf token invalid"}"#);
        assert_eq!(sig, r#"{"error":"csrf token invalid"}"#);
    }
}
