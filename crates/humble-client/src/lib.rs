//! Client for the community-documented unofficial Humble Bundle API.
//! No test touches the real API — see the probe binary for live verification.
mod model;

use hmac::{Hmac, Mac};
use model::{GamekeyEntry, OrderWire};
use sha1::Sha1;
use wreq_util::Emulation;

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

/// Credentials for humble's secure-area step-up. Humble gates key reveal/redeem/gift behind a
/// fresh-password re-auth that a session cookie alone can't pass: a redeem on a perfectly healthy
/// session returns `login_required` until that session is elevated by POSTing the account password
/// plus a current app-TOTP code to `/processlogin`. Held only in memory (read from SSM per-invoke,
/// same as the cookie); every field is redacted in `Debug` and never logged.
pub struct StepUpCredentials {
    username: String,
    password: String,
    /// The bare base32 TOTP seed (humble app authenticator: RFC 6238, SHA1, 6 digits, 30s step).
    totp_secret: String,
}

impl StepUpCredentials {
    pub fn new(username: String, password: String, totp_secret: String) -> Self {
        Self {
            username,
            password,
            totp_secret,
        }
    }
}

impl std::fmt::Debug for StepUpCredentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Redact everything — even the username is account-identifying and never needed in a log.
        write!(f, "StepUpCredentials(REDACTED)")
    }
}

/// Compute the current humble app-TOTP for a base32 seed (RFC 6238: SHA1, 6 digits, 30s step).
fn totp_now(secret_b32: &str) -> Result<String, String> {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|e| format!("system clock is before the unix epoch: {e}"))?
        .as_secs();
    totp_at(secret_b32, secs / 30)
}

/// TOTP for an explicit 30-second counter — the pure, testable core (drives the RFC 6238 vectors).
fn totp_at(secret_b32: &str, counter: u64) -> Result<String, String> {
    let key = decode_b32_secret(secret_b32)?;
    let mut mac =
        Hmac::<Sha1>::new_from_slice(&key).map_err(|_| "TOTP HMAC rejected the key".to_string())?;
    mac.update(&counter.to_be_bytes());
    let hs = mac.finalize().into_bytes();
    // Dynamic truncation, RFC 4226 §5.3: the low nibble of the last byte picks a 4-byte window.
    let offset = (hs[19] & 0x0f) as usize;
    let bin = (u32::from(hs[offset] & 0x7f) << 24)
        | (u32::from(hs[offset + 1]) << 16)
        | (u32::from(hs[offset + 2]) << 8)
        | u32::from(hs[offset + 3]);
    Ok(format!("{:06}", bin % 1_000_000))
}

/// Decode a base32 TOTP seed (RFC 4648), tolerant of humble's formatting — spaces, lowercase, and
/// trailing `=` padding are all normalized first. Errors carry only a symbol POSITION, never the
/// seed itself.
fn decode_b32_secret(seed: &str) -> Result<Vec<u8>, String> {
    let normalized = seed
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect::<String>()
        .trim_end_matches('=')
        .to_ascii_uppercase();
    data_encoding::BASE32_NOPAD
        .decode(normalized.as_bytes())
        .map_err(|e| {
            format!(
                "malformed base32 TOTP seed (bad symbol at position {})",
                e.position
            )
        })
}

/// Does a redeem response body carry humble's `login_required` gate marker? A gated (but healthy)
/// session returns this before the key is touched — the signal to step up and retry, not to fail.
fn is_login_required(bytes: &[u8]) -> bool {
    #[derive(serde::Deserialize)]
    struct GateProbe {
        #[serde(default)]
        error_id: Option<String>,
    }
    matches!(
        serde_json::from_slice::<GateProbe>(bytes),
        Ok(GateProbe { error_id: Some(e) }) if e == "login_required"
    )
}

/// Did `/processlogin` accept the step-up? Success is a `200` whose body carries a `goto` and no
/// `errors` (a rejection returns `{"errors":{"_all":[…]}}`). Verified live 2026-07-04.
fn processlogin_ok(bytes: &[u8]) -> bool {
    #[derive(serde::Deserialize)]
    struct ProcessLoginResp {
        #[serde(default)]
        goto: Option<String>,
        #[serde(default)]
        errors: Option<serde_json::Value>,
    }
    matches!(
        serde_json::from_slice::<ProcessLoginResp>(bytes),
        Ok(ProcessLoginResp {
            goto: Some(_),
            errors: None
        })
    )
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
    /// The redeem was gated behind humble's secure-area step-up and the step-up itself did not
    /// complete (bad password/TOTP, a locked account, a csrf-preflight miss, or humble still
    /// gating after a `200 {goto}` from `/processlogin`). The key was NOT burned — a gated redeem
    /// returns `login_required` before touching the key — so this always parks, never compensates.
    /// `reason` is a short human diagnosis and NEVER contains the password, the TOTP seed, or a
    /// computed code.
    #[error("secure-area step-up did not complete: {reason}")]
    SecureAreaStepUpFailed { reason: String },
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
    Network(#[from] wreq::Error),
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

/// The positive outcome of a single redeem attempt. Failures come back as `Err(HumbleError)`;
/// the two non-error shapes a caller must tell apart are a completed redeem and a redeem that was
/// gated behind humble's secure-area step-up.
enum RedeemStep {
    /// The redeem completed and humble handed back a gift URL.
    Done(GiftUrl),
    /// Humble gated the write behind a secure-area step-up (`login_required` / a `secureArea`
    /// redirect). The session is healthy and **the key was not burned** — retrying after a
    /// successful step-up is safe. Carries the observed HTTP status for logging only.
    StepUpNeeded { status: u16 },
}

/// Read one response header as an owned `String`, or `"-"` when absent/unprintable. Kept small
/// so the diagnostic log line on a redeem rejection stays flat and greppable.
fn header_str(headers: &wreq::header::HeaderMap, name: &str) -> String {
    headers
        .get(name)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("-")
        .to_string()
}

/// Read a redirect `Location` for logging, keeping only host+path — everything from the first
/// `?` or `#` is dropped. A 302 target is attacker-influenced-adjacent surface: if humble's
/// login redirect ever grows a `?return_to=` (or similar) echoing request data, an
/// allowlist-by-name still logs its value. Dropping the query/fragment makes that permanently
/// impossible while keeping the one bit we want — which path it redirects to. (OMBB, PR#14.)
fn location_str(headers: &wreq::header::HeaderMap) -> String {
    let raw = header_str(headers, "location");
    match raw.split(['?', '#']).next() {
        Some(base) if !base.is_empty() => base.to_string(),
        _ => raw,
    }
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
    http: wreq::Client,
    base: String,
    cookie: SessionCookie,
    /// Optional secure-area step-up credentials. `None` on the read-only / cookie-validate paths
    /// (they never gift); `Some` on the fulfillment lambda, which reads them from SSM per-invoke.
    /// Absent → a gated redeem parks exactly as it did before this module existed.
    step_up: Option<StepUpCredentials>,
}

impl HumbleClient {
    pub fn new(base_url: &str, cookie: SessionCookie) -> Result<Self, HumbleError> {
        // Emulate a real Chrome's TLS/JA3 + HTTP2 fingerprint. Humble sits behind Cloudflare,
        // whose WAF challenges non-browser TLS: the rustls fingerprint got the redeem POST a
        // Cloudflare interstitial (verified 2026-07-04), while a genuine Chrome handshake reaches
        // humble's app. This is the prod half of that fix — reqwest+rustls could not fake it.
        let http = wreq::Client::builder()
            .emulation(Emulation::Chrome137)
            .redirect(wreq::redirect::Policy::none()) // a 302-to-login must surface, not follow
            .build()?;
        Ok(Self {
            http,
            base: base_url.trim_end_matches('/').to_string(),
            cookie,
            step_up: None,
        })
    }

    /// Attach secure-area step-up credentials so a `login_required`-gated redeem can elevate the
    /// session and retry, instead of parking. Builder-style so read-only callers stay untouched.
    pub fn with_step_up(mut self, creds: StepUpCredentials) -> Self {
        self.step_up = Some(creds);
        self
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

    /// Redeem a key as a gift, transparently clearing humble's secure-area step-up when it gates
    /// the write. Flow: try once; if humble answers `login_required`/`secureArea` AND step-up
    /// credentials are configured, elevate the session via `/processlogin` and retry EXACTLY once.
    ///
    /// Safety (the burns-once invariant): a gated redeem returns `login_required` *before* touching
    /// the key, so the retry is the first and only attempt that can burn it — there is no
    /// double-redeem window. If the retry is still gated, or step-up fails, we return an error that
    /// [`crate`]'s caller parks on; the key is never assumed burned.
    pub async fn redeem_as_gift(
        &self,
        gamekey: &str,
        machine_name: &str,
        keyindex: u32,
    ) -> Result<GiftUrl, HumbleError> {
        match self.redeem_once(gamekey, machine_name, keyindex).await? {
            RedeemStep::Done(url) => Ok(url),
            RedeemStep::StepUpNeeded { status } => {
                if self.step_up.is_none() {
                    // No creds configured → surface the gate as an auth rejection, exactly as this
                    // client behaved before step-up existed: the caller parks (never dead-cookie,
                    // never compensate). Preserves the observed status for the diagnostic log.
                    tracing::warn!(
                        status,
                        "redeem gated behind secure-area step-up but no step-up credentials are configured — parking"
                    );
                    return Err(HumbleError::RedeemAuthRejected {
                        status,
                        csrf_minted: false,
                    });
                }
                self.secure_area_step_up().await?;
                match self.redeem_once(gamekey, machine_name, keyindex).await? {
                    RedeemStep::Done(url) => Ok(url),
                    // Still gated after `/processlogin` said OK. The key is NOT burned (still
                    // `login_required`), so this parks — a distinct, correctly-labeled failure so a
                    // persistent mismatch surfaces instead of masquerading as a plain 403.
                    RedeemStep::StepUpNeeded { status } => {
                        tracing::warn!(
                            status,
                            "redeem still gated after a successful step-up — parking (key not burned)"
                        );
                        Err(HumbleError::SecureAreaStepUpFailed {
                            reason: "redeem still returned login_required after /processlogin accepted the step-up".into(),
                        })
                    }
                }
            }
        }
    }

    /// One redeem attempt. Detects humble's secure-area gate (`login_required` body or a
    /// `secureArea` redirect) and reports it as [`RedeemStep::StepUpNeeded`] so the orchestrator can
    /// elevate and retry; every other outcome maps to a completed redeem or a typed error.
    async fn redeem_once(
        &self,
        gamekey: &str,
        machine_name: &str,
        keyindex: u32,
    ) -> Result<RedeemStep, HumbleError> {
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
                // A gated redeem on a HEALTHY session returns `login_required` (verified live
                // 2026-07-04) — catch it BEFORE the success parse so it drives a step-up, not a
                // Parse error, and above all NOT a burn: the key is untouched behind the gate.
                if is_login_required(&bytes) {
                    tracing::info!(
                        status,
                        "redeem gated: humble returned login_required — secure-area step-up needed"
                    );
                    return Ok(RedeemStep::StepUpNeeded { status });
                }
                let body: RedeemResponse = decode_body(&bytes)?;
                match (body.success, body.giftkey) {
                    (true, Some(token)) => Ok(RedeemStep::Done(GiftUrl(format!(
                        "https://www.humblebundle.com/gift?key={token}"
                    )))),
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
                let location = location_str(resp.headers());
                let cf_mitigated = header_str(resp.headers(), "cf-mitigated");
                let server = header_str(resp.headers(), "server");
                // Raw location (query intact, unlike the logged `location`) so we can spot humble's
                // `?reason=secureArea` step-up redirect — a gated-but-live session, NOT a rejection.
                let raw_location = header_str(resp.headers(), "location");
                let bytes = resp.bytes().await.unwrap_or_default();
                // Secure-area gate on a healthy session shows up here two ways: a 302 to
                // `/login?reason=secureArea`, or a 401/403 carrying a `login_required` JSON body.
                // Either is a step-up trigger, not a dead-cookie/CF block — the key is untouched.
                if raw_location.contains("secureArea") || is_login_required(&bytes) {
                    tracing::info!(
                        status,
                        location,
                        "redeem gated behind secure-area step-up (healthy session) — will step up and retry"
                    );
                    return Ok(RedeemStep::StepUpNeeded { status });
                }
                let body_preview = body_signature(&bytes);
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

    /// Elevate the session through humble's secure-area step-up: `POST /processlogin` with the
    /// account password + a current app-TOTP, guarded by the same double-submit CSRF pair as a
    /// redeem (csrf_cookie from a root `GET /`, replayed as cookie + `csrf-prevention-token`).
    /// Verified end-to-end live on 2026-07-04.
    ///
    /// On success we deliberately do NOT capture a rotated `_simpleauth_sess`: the live spike
    /// showed the SAME cookie redeems afterward and the elevation persists server-side for a window
    /// (a later run redeemed on the unchanged cookie even when its own re-step-up failed). So
    /// elevation is server-side on the existing session, not a new cookie. If humble ever changes
    /// that, the follow-up redeem simply re-gates → we park (no burn) — a safe failure mode.
    async fn secure_area_step_up(&self) -> Result<(), HumbleError> {
        let creds = self
            .step_up
            .as_ref()
            .ok_or_else(|| HumbleError::SecureAreaStepUpFailed {
                reason: "no step-up credentials configured".into(),
            })?;
        // Fresh csrf_cookie from root — same double-submit source the redeem uses.
        let csrf = self
            .csrf_token()
            .await
            .ok_or_else(|| HumbleError::SecureAreaStepUpFailed {
                reason: "csrf preflight yielded no csrf_cookie".into(),
            })?;
        let code = totp_now(&creds.totp_secret).map_err(|reason| {
            // `reason` names the failure class (e.g. malformed base32) with a position only — never
            // the seed or the derived code.
            HumbleError::SecureAreaStepUpFailed {
                reason: format!("TOTP: {reason}"),
            }
        })?;
        let resp = self
            .http
            .post(format!("{}/processlogin", self.base))
            .header(
                "Cookie",
                format!("{}; csrf_cookie={csrf}", self.session_cookie()),
            )
            .header("csrf-prevention-token", &csrf)
            .header("Origin", &self.base)
            .header("Referer", format!("{}/login", self.base))
            // Field shape captured from the live login (2026-07-04): the four auth fields plus the
            // three empties humble's SPA always sends. `goto` steers post-login to the keys area.
            .form(&[
                ("access_token", ""),
                ("access_token_provider_id", ""),
                ("username", creds.username.as_str()),
                ("password", creds.password.as_str()),
                ("code", code.as_str()),
                ("goto", "/home/keys"),
                ("qs", ""),
            ])
            .send()
            .await?;
        let status = resp.status().as_u16();
        // NEVER log the body — a /processlogin response can echo account/session state. The parsed
        // outcome bit + status is the entire safe signal.
        let bytes = resp.bytes().await.unwrap_or_default();
        let ok = status == 200 && processlogin_ok(&bytes);
        tracing::info!(status, ok, "secure-area step-up (/processlogin) response");
        if ok {
            Ok(())
        } else {
            Err(HumbleError::SecureAreaStepUpFailed {
                reason: format!("humble /processlogin returned status {status} without a goto"),
            })
        }
    }
}

#[cfg(test)]
mod signature_tests {
    use super::{body_signature, location_str};
    use wreq::header::HeaderMap;

    fn headers_with_location(value: &str) -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert("location", value.parse().unwrap());
        h
    }

    #[test]
    fn location_keeps_host_and_path_only() {
        // absolute URL: scheme+host+path survive, query + fragment do not.
        assert_eq!(
            location_str(&headers_with_location(
                "https://www.humblebundle.com/login?return_to=/secret&t=abc#frag"
            )),
            "https://www.humblebundle.com/login"
        );
        // relative redirect: path survives, query dropped.
        assert_eq!(
            location_str(&headers_with_location("/verify?email=leak@example.com")),
            "/verify"
        );
        // no query/fragment: unchanged.
        assert_eq!(location_str(&headers_with_location("/login")), "/login");
    }

    #[test]
    fn location_absent_is_dash() {
        assert_eq!(location_str(&HeaderMap::new()), "-");
    }

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

#[cfg(test)]
mod step_up_tests {
    use super::{StepUpCredentials, is_login_required, processlogin_ok, totp_at};

    // RFC 6238 Appendix B, SHA1 profile. Seed = ASCII "12345678901234567890" in base32. The 6-digit
    // TOTP is the low 6 digits of the published 8-digit value at each timestamp (counter = T / 30).
    const RFC6238_SHA1_SEED_B32: &str = "GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ";

    #[test]
    fn totp_matches_rfc6238_vectors() {
        // T=59 → counter 1 → 94287082 → "287082".
        assert_eq!(totp_at(RFC6238_SHA1_SEED_B32, 59 / 30).unwrap(), "287082");
        // T=1111111109 → counter 37037036 → 07081804 → "081804".
        assert_eq!(
            totp_at(RFC6238_SHA1_SEED_B32, 1111111109 / 30).unwrap(),
            "081804"
        );
        // T=1234567890 → counter 41152263 → 89005924 → "005924" (exercises leading-zero padding).
        assert_eq!(
            totp_at(RFC6238_SHA1_SEED_B32, 1234567890 / 30).unwrap(),
            "005924"
        );
    }

    #[test]
    fn totp_tolerates_lowercase_spaces_and_padding_in_the_seed() {
        // Same seed, humble-app formatting: lowercase, spaced into groups, trailing pad.
        let messy = "gezd gnbv gy3t qojq gezd gnbv gy3t qojq==";
        assert_eq!(totp_at(messy, 59 / 30).unwrap(), "287082");
    }

    #[test]
    fn totp_rejects_a_malformed_seed_without_echoing_it() {
        // '1' and '8' are not in the base32 alphabet.
        let err = totp_at("not-valid-base32-1888", 1).unwrap_err();
        assert!(err.contains("malformed base32"), "got: {err}");
        assert!(!err.contains("1888"), "error must not echo the seed: {err}");
    }

    #[test]
    fn login_required_body_is_detected_as_the_step_up_gate() {
        assert!(is_login_required(br#"{"error_id":"login_required"}"#));
        // A different error_id, or a normal redeem body, is NOT the gate.
        assert!(!is_login_required(br#"{"error_id":"rate_limited"}"#));
        assert!(!is_login_required(br#"{"success":true,"giftkey":"abc"}"#));
        assert!(!is_login_required(b"not json at all"));
    }

    #[test]
    fn processlogin_success_needs_a_goto_and_no_errors() {
        assert!(processlogin_ok(br#"{"goto":"/home/keys"}"#));
        // A rejection carries errors (the live 403 shape) — not success even if a goto tags along.
        assert!(!processlogin_ok(
            br#"{"errors":{"_all":["Invalid request."]}}"#
        ));
        assert!(!processlogin_ok(
            br#"{"goto":"/home/keys","errors":{"_all":["nope"]}}"#
        ));
        assert!(!processlogin_ok(b"{}"));
    }

    #[test]
    fn step_up_credentials_debug_is_fully_redacted() {
        let creds = StepUpCredentials::new(
            "person@example.com".into(),
            "hunter2".into(),
            "GEZDGNBV".into(),
        );
        let shown = format!("{creds:?}");
        assert_eq!(shown, "StepUpCredentials(REDACTED)");
        assert!(!shown.contains("hunter2"));
        assert!(!shown.contains("person@example.com"));
        assert!(!shown.contains("GEZDGNBV"));
    }
}
