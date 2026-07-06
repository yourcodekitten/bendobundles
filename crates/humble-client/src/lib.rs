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
    // A seed that normalizes away to nothing ("  ", "====") would base32-decode to an EMPTY key,
    // which HMAC-SHA1 happily accepts — yielding a syntactically valid but guaranteed-wrong TOTP
    // that we'd POST to the live account on every gated redeem. Fail here and name the real cause.
    if normalized.is_empty() {
        return Err("TOTP seed is empty after normalization".to_string());
    }
    let key = data_encoding::BASE32_NOPAD
        .decode(normalized.as_bytes())
        .map_err(|e| {
            format!(
                "malformed base32 TOTP seed (bad symbol at position {})",
                e.position
            )
        })?;
    if key.is_empty() {
        return Err("TOTP seed decoded to zero bytes".to_string());
    }
    Ok(key)
}

/// Extract a named cookie's value from a response's `set-cookie` headers (first non-empty match).
/// Used to capture `csrf_cookie` and `_simpleauth_sess` humble sets during the login bootstrap.
fn extract_set_cookie(headers: &wreq::header::HeaderMap, name: &str) -> Option<String> {
    let prefix = format!("{name}=");
    for sc in headers.get_all("set-cookie") {
        let Ok(s) = sc.to_str() else { continue };
        if let Some(rest) = s.trim_start().strip_prefix(&prefix) {
            let val = rest.split(';').next().unwrap_or("").trim();
            if !val.is_empty() {
                return Some(val.to_string());
            }
        }
    }
    None
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

/// The `/processlogin` auth form, shared by both callers (self-[`HumbleClient::login`] and the
/// secure-area step-up) so the field set can't drift between them: the four auth fields plus the
/// three empties humble's SPA always sends, with `goto` steering post-login to the keys area.
fn processlogin_form<'a>(
    creds: &'a StepUpCredentials,
    code: &'a str,
) -> [(&'static str, &'a str); 7] {
    [
        ("access_token", ""),
        ("access_token_provider_id", ""),
        ("username", creds.username.as_str()),
        ("password", creds.password.as_str()),
        ("code", code),
        ("goto", "/home/keys"),
        ("qs", ""),
    ]
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
    #[error("session cookie rejected — the humble session is dead")]
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
    /// A fresh self-login (`GET /` bootstrap → `/processlogin`) did not yield a usable session —
    /// bad credentials/TOTP, a login challenge (captcha / new-device), or no session cookie in the
    /// response. The caller keeps the prior session (if any) and surfaces this so a persistent
    /// failure is visible. `reason` NEVER contains the password, the TOTP seed, or a session value.
    #[error("humble self-login failed: {reason}")]
    LoginFailed { reason: String },
    /// A Humble Choice `choosecontent` write (the pick-spend that precedes the redeem) did not
    /// succeed — humble returned `success=false` (already chosen, no picks left, not offered) or a
    /// non-200. A choose only SPENDS a pick on `success=true`, so this variant always means the
    /// pick was NOT spent; the caller parks and does not proceed to the redeem. `reason` is
    /// humble's own refusal text or a status, never a secret value.
    #[error("humble choosecontent failed: {reason}")]
    ChooseFailed { reason: String },
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

#[derive(serde::Deserialize)]
struct ChooseResponse {
    // Same as RedeemResponse: a 200 body missing `success` is a parse error, not a silent failure.
    success: bool,
    #[serde(default)]
    errormsg: Option<String>,
    // humble also returns `force_refresh: true`; we don't act on it (the caller re-reads the order
    // to see the newly-claimed key), so it's intentionally not deserialized.
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

/// The positive outcome of a single `choosecontent` attempt — the choose analog of [`RedeemStep`].
/// A completed choose has SPENT a pick; a gated choose has not (the handler ran behind the gate).
enum ChooseStep {
    /// The choose completed (`success=true`) — a pick was spent.
    Done,
    /// Humble gated the choose behind a secure-area step-up. The session is healthy and **no pick
    /// was spent** — retrying after a successful step-up is safe. Status is for logging only.
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
    /// The session token, interior-mutable so a self-[`login`](Self::login) can swap in a freshly
    /// minted session in place without rebuilding the client (and losing its connection pool).
    /// Held as the raw `_simpleauth_sess` value; `RwLock` because reads far outnumber the rare swap.
    cookie: std::sync::RwLock<String>,
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
            cookie: std::sync::RwLock::new(cookie.0),
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
        // A poisoned lock only means a prior panic mid-swap; the value itself is still a valid
        // string, so recover it rather than propagating the panic into every request.
        let sess = self
            .cookie
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        format!("_simpleauth_sess={sess}")
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
        let captured = extract_set_cookie(resp.headers(), "csrf_cookie");
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

    /// Build a POST carrying humble's double-submit CSRF pair — `csrf_cookie` replayed as BOTH the
    /// cookie and the `csrf-prevention-token` header — plus the same-origin `Origin`/`Referer` a
    /// browser sends. Every write goes through this dance; the ONLY thing that varies is which
    /// session rides the Cookie header: writes authenticating with the CURRENT session (the redeem,
    /// the secure-area step-up) use [`csrf_write`](Self::csrf_write), while a fresh
    /// [`login`](Self::login) passes its bootstrap/anon session (or none) explicitly. One builder,
    /// so a humble-side change to the dance can't silently break just one caller.
    /// `referer_path` is appended to `base` (e.g. `/home/library`, `/login`).
    fn csrf_write_as(
        &self,
        url: String,
        csrf: &str,
        referer_path: &str,
        session_cookie: Option<&str>,
    ) -> wreq::RequestBuilder {
        let cookie = match session_cookie {
            Some(sess) => format!("{sess}; csrf_cookie={csrf}"),
            None => format!("csrf_cookie={csrf}"),
        };
        self.http
            .post(url)
            .header("Cookie", cookie)
            .header("csrf-prevention-token", csrf)
            .header("Origin", &self.base)
            .header("Referer", format!("{}{referer_path}", self.base))
    }

    /// [`csrf_write_as`](Self::csrf_write_as) with the client's CURRENT session — what every
    /// established-session write (redeem, step-up) uses.
    fn csrf_write(&self, url: String, csrf: &str, referer_path: &str) -> wreq::RequestBuilder {
        self.csrf_write_as(url, csrf, referer_path, Some(&self.session_cookie()))
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
                    // Gate positively identified, but step-up isn't configured. Do NOT reuse
                    // RedeemAuthRejected: its ping blames the CSRF dance ("humble rejected its own
                    // token, a session refresh won't help"), which misdirects the operator when the real
                    // fix is enabling step-up (set humble_username). Same Park decision, honest
                    // reason. (This is also NOT the pre-PR behavior — a 200 `login_required` body
                    // used to be a silent Parse-error park with no ping at all.)
                    tracing::warn!(
                        status,
                        "redeem gated behind secure-area step-up but step-up is not configured — parking (set humble_username to enable)"
                    );
                    return Err(HumbleError::SecureAreaStepUpFailed {
                        reason:
                            "secure-area step-up required but not configured (set humble_username)"
                                .into(),
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

    /// Spend a monthly Humble Choice pick: claim `chosen` game(s) from the Choice month identified
    /// by `gamekey`, transparently clearing humble's secure-area step-up when it gates the write
    /// (same as [`redeem_as_gift`]). This is the FIRST of the two Choice writes — it moves a game
    /// from "offered" to "claimed" and SPENDS a pick (real, one-shot value). It never touches a key;
    /// the redeemable key is minted by the subsequent `/humbler/redeemkey` (which `redeem_as_gift`
    /// already implements) with `keytype = <machine_name>_choice_steam` and `key = gamekey`.
    ///
    /// `is_gift = true` claims into a giftable form (redeem then with `gift=true` for a gift URL);
    /// `false` self-claims into the library. Returns `Ok(())` only on `success=true`.
    ///
    /// Flow (mirrors the redeem): try once; if humble answers `login_required` / `secureArea` AND
    /// step-up credentials are configured, elevate via `/processlogin` and retry EXACTLY once.
    ///
    /// SAFETY (pick-spend-once, mirrors burns-once): a gated choose returns `login_required` BEFORE
    /// the choose handler runs, so the retry is the first attempt that can spend a pick — no
    /// double-spend window. A pick is spent ONLY on `success=true`; a `Unauthorized` (dead session),
    /// `SecureAreaStepUpFailed` (gate, no/failed step-up), or `ChooseFailed` (`success=false` / a
    /// non-200) provably did NOT spend a pick. The one residual, like the redeem's `AmbiguousRedeem`:
    /// a lost response AFTER humble committed (a `Network` read error, or a 5xx post-commit) can
    /// leave a pick spent while this returns `Err` — the outcome is AMBIGUOUS, not provably clean.
    /// So the caller MUST reconcile against humble state (re-read the order's `all_tpks` /
    /// `contentChoicesMade`) before re-choosing; a blind retry could spend a second pick.
    pub async fn choose_content(
        &self,
        gamekey: &str,
        chosen: &[&str],
        is_gift: bool,
    ) -> Result<(), HumbleError> {
        // Guard the empty pick — a choosecontent with zero chosen_identifiers is a malformed write
        // (undefined server behavior); fail before the network call rather than POST it.
        if chosen.is_empty() {
            return Err(HumbleError::ChooseFailed {
                reason: "no games to choose (empty chosen set)".into(),
            });
        }
        match self.choose_once(gamekey, chosen, is_gift).await? {
            ChooseStep::Done => Ok(()),
            ChooseStep::StepUpNeeded { status } => {
                if self.step_up.is_none() {
                    tracing::warn!(
                        status,
                        "choose gated behind secure-area step-up but step-up is not configured — parking (set humble_username to enable)"
                    );
                    return Err(HumbleError::SecureAreaStepUpFailed {
                        reason:
                            "secure-area step-up required but not configured (set humble_username)"
                                .into(),
                    });
                }
                self.secure_area_step_up().await?;
                match self.choose_once(gamekey, chosen, is_gift).await? {
                    ChooseStep::Done => Ok(()),
                    // Still gated after `/processlogin` accepted the step-up. No pick spent (still
                    // `login_required` before the handler) — park with an honest reason.
                    ChooseStep::StepUpNeeded { status } => {
                        tracing::warn!(
                            status,
                            "choose still gated after a successful step-up — parking (no pick spent)"
                        );
                        Err(HumbleError::SecureAreaStepUpFailed {
                            reason: "choosecontent still returned login_required after /processlogin accepted the step-up".into(),
                        })
                    }
                }
            }
        }
    }

    /// One `choosecontent` attempt. Detects humble's secure-area gate (`login_required` body or a
    /// `secureArea` redirect) and reports it as [`ChooseStep::StepUpNeeded`] so the caller can
    /// elevate and retry — the exact shape as [`redeem_once`], since choose is the same
    /// browser-surface write that redeem is. `POST /humbler/choosecontent` — form: `gamekey`,
    /// `parent_identifier=initial`, `chosen_identifiers[]` (repeated per game), `is_gift`.
    async fn choose_once(
        &self,
        gamekey: &str,
        chosen: &[&str],
        is_gift: bool,
    ) -> Result<ChooseStep, HumbleError> {
        // Same csrf dance as the redeem write (double-submit pair, prefer humble's token, mint as a
        // tracked fallback so a systematic capture failure surfaces instead of a silent 403).
        let (csrf, csrf_minted) = match self.csrf_token().await {
            Some(t) => (t, false),
            None => {
                tracing::warn!(
                    "csrf capture failed — minting a double-submit fallback for choosecontent"
                );
                (uuid::Uuid::new_v4().simple().to_string(), true)
            }
        };
        // `chosen_identifiers[]` is an array field — repeat the key once per game.
        let mut form: Vec<(&str, String)> = vec![
            ("gamekey", gamekey.to_string()),
            ("parent_identifier", "initial".to_string()),
        ];
        form.extend(
            chosen
                .iter()
                .map(|m| ("chosen_identifiers[]", (*m).to_string())),
        );
        if is_gift {
            form.push(("is_gift", "true".to_string()));
        }
        // Referer is the membership surface (choice lives there). The double-submit cookie==header
        // pair is humble's actual CSRF check; the Referer path is browser-shaping — VERIFY the exact
        // path against the live receipt if a 403 ever appears here.
        let resp = self
            .csrf_write(
                format!("{}/humbler/choosecontent", self.base),
                &csrf,
                "/membership",
            )
            .form(&form)
            .send()
            .await?;
        let status = resp.status().as_u16();
        tracing::info!(
            status,
            gamekey,
            is_gift,
            n_chosen = chosen.len(),
            csrf_minted,
            "humble choosecontent POST response"
        );
        match status {
            200 => {
                let bytes = resp.bytes().await?;
                // A gated choose on a HEALTHY session returns `login_required` — catch it BEFORE the
                // success parse so it drives a step-up, NOT a dead-cookie alarm and NOT a pick-spend:
                // the handler never ran behind the gate. (Same live-verified shape as the redeem.)
                if is_login_required(&bytes) {
                    tracing::info!(
                        status,
                        "choose gated: humble returned login_required — secure-area step-up needed"
                    );
                    return Ok(ChooseStep::StepUpNeeded { status });
                }
                // A genuinely dead session returns 200-with-HTML → decode_body maps it to
                // Unauthorized (leading `<`). success=true spends the pick; success=false is a
                // semantic refusal (no picks left / already chosen / not offered).
                let body: ChooseResponse = decode_body(&bytes)?;
                if body.success {
                    Ok(ChooseStep::Done)
                } else {
                    let reason = body
                        .errormsg
                        .unwrap_or_else(|| "choosecontent returned success=false".to_string());
                    tracing::warn!(reason, "humble choosecontent refused (success=false)");
                    Err(HumbleError::ChooseFailed { reason })
                }
            }
            401 | 403 | 302 => {
                // A 302 to `?reason=secureArea` is the header-only form of the same step-up gate —
                // a live session that needs elevating, NOT a dead cookie. Everything else at this
                // layer is an auth/CSRF rejection: no pick spent, park (csrf_minted names a
                // systematic capture failure vs a rejection of humble's own token).
                let raw_location = header_str(resp.headers(), "location");
                if raw_location.contains("secureArea") {
                    tracing::info!(status, "choose gated: secureArea redirect — step-up needed");
                    return Ok(ChooseStep::StepUpNeeded { status });
                }
                Err(HumbleError::ChooseFailed {
                    reason: format!(
                        "choosecontent rejected at the auth/CSRF layer (status {status}, csrf_minted={csrf_minted})"
                    ),
                })
            }
            429 => Err(HumbleError::RateLimited),
            s => Err(HumbleError::ChooseFailed {
                reason: format!("choosecontent unexpected status {s}"),
            }),
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
        // Browser-shaped write via the shared CSRF-write builder (double-submit csrf pair +
        // same-origin Origin/Referer). NO X-Requested-By — the android-app header belongs to the
        // read API, not the browser-surface /humbler/ endpoints — so the form is added here.
        let resp = self
            .csrf_write(
                format!("{}/humbler/redeemkey", self.base),
                &csrf,
                "/home/library",
            )
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
                // The PR#14 body_preview diagnostic used to live here. What it found, in order:
                // the double-submit CSRF match was live-exonerated first (so DON'T re-suspect the
                // pair if 403s recur), then the mystery 403s pinned to a Cloudflare bot-challenge
                // (cf-mitigated: challenge, HTML body) on 2026-07-04. The wreq-based CF bypass
                // fixed it, and the preview was retired per the review's expiry rule.
                // No path in this crate logs RAW response-body bytes; the only body-derived TEXT
                // logged anywhere is humble's own parsed refusal `errormsg` in the success=false
                // arm above (reviewed as safe — `processlogin_ok` also derives a 1-bit `ok` from
                // the body, which leaks nothing). The header fields below stay: allowlisted by
                // NAME, non-sensitive by nature (content-type / query-stripped location /
                // cf-mitigated / server). A CHALLENGE-type CF recurrence names itself via
                // `cf-mitigated`; a BLOCK-type CF 403 carries no such header and is
                // header-ambiguous with a humble-app HTML refusal — if that pair ever needs
                // splitting again, restore the PR#14 preview from git history rather than
                // guessing. `set-cookie` is never logged.
                let content_type = header_str(resp.headers(), "content-type");
                let location = location_str(resp.headers());
                let cf_mitigated = header_str(resp.headers(), "cf-mitigated");
                let server = header_str(resp.headers(), "server");
                // Raw location (query intact, unlike the logged `location`) so we can spot humble's
                // `?reason=secureArea` step-up redirect — a gated-but-live session, NOT a rejection.
                let raw_location = header_str(resp.headers(), "location");
                // A mid-read failure still parks as a rejection (a dropped `login_required` body
                // yields login_required_body=false → RedeemAuthRejected, same as before) — the
                // match doesn't change that behavior, it keeps the failure VISIBLE: body_read_err
                // records why the read failed (timeout vs reset vs decode) so the log distinguishes
                // it from a genuinely empty body. The error text is wreq transport metadata, never
                // response-body content.
                let (login_required_body, body_read_err) = match resp.bytes().await {
                    Ok(b) => (is_login_required(&b), None),
                    Err(e) => (false, Some(e.to_string())),
                };
                // Secure-area gate on a healthy session shows up here two ways: a 302 to
                // `/login?reason=secureArea` (header-only, survives a body-read failure), or a
                // 401/403 carrying a `login_required` JSON body. Either is a step-up trigger, not a
                // dead-cookie/CF block — the key is untouched.
                if raw_location.contains("secureArea") || login_required_body {
                    tracing::info!(
                        status,
                        location,
                        "redeem gated behind secure-area step-up (healthy session) — will step up and retry"
                    );
                    return Ok(RedeemStep::StepUpNeeded { status });
                }
                let body_read_err = body_read_err.as_deref().unwrap_or("-");
                tracing::warn!(
                    status,
                    csrf_minted,
                    content_type,
                    location,
                    cf_mitigated,
                    server,
                    body_read_err,
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
        // Fresh csrf_cookie from root — same double-submit source the redeem uses, and the SAME
        // mint-on-miss fallback: `redeem_once` tolerates a missing csrf_cookie by minting a
        // matching pair (a pure double-submit check only cares that cookie == header), so a
        // transient preflight miss must not park step-up with a misleading "check the seed" ping
        // when a mint would likely have passed.
        let csrf = match self.csrf_token().await {
            Some(t) => t,
            None => {
                tracing::warn!(
                    "step-up csrf capture failed — minting a double-submit fallback (same as the redeem path)"
                );
                uuid::Uuid::new_v4().simple().to_string()
            }
        };
        let code = totp_now(&creds.totp_secret).map_err(|reason| {
            // `reason` names the failure class (e.g. malformed base32) with a position only — never
            // the seed or the derived code.
            HumbleError::SecureAreaStepUpFailed {
                reason: format!("TOTP: {reason}"),
            }
        })?;
        let resp = self
            .csrf_write(format!("{}/processlogin", self.base), &csrf, "/login")
            .form(&processlogin_form(creds, &code))
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

    /// Perform a fresh self-login and return a newly-minted authenticated `_simpleauth_sess`, so the
    /// app can manage its own humble session instead of a human pasting a cookie. Bootstraps an
    /// anonymous session + csrf via `GET /`, then POSTs `/processlogin` with the account password +
    /// a current app-TOTP. That POST is itself a password+2FA authentication, so the resulting
    /// session is born secure-area-elevated.
    ///
    /// This crate stays free of any storage/AWS concern: `login` only returns the new session value
    /// — the CALLER persists it (e.g. to SSM) and rebuilds the client with it. Verified live
    /// 2026-07-04: a cold login returns `200 {goto}` with no captcha/new-device friction and the
    /// minted session reads the authenticated order list.
    pub async fn login(&self) -> Result<String, HumbleError> {
        let creds = self
            .step_up
            .as_ref()
            .ok_or_else(|| HumbleError::LoginFailed {
                reason: "no credentials configured".into(),
            })?;
        // 1) Bootstrap with NO session cookie: humble sets an anonymous _simpleauth_sess + a
        //    csrf_cookie on the root GET. Both feed the login POST.
        let boot = self.http.get(format!("{}/", self.base)).send().await?;
        let csrf = extract_set_cookie(boot.headers(), "csrf_cookie").ok_or_else(|| {
            HumbleError::LoginFailed {
                reason: "bootstrap GET / offered no csrf_cookie".into(),
            }
        })?;
        let anon = extract_set_cookie(boot.headers(), "_simpleauth_sess");
        let _ = boot.bytes().await; // drain so the connection returns to the pool
        // 2) Authenticate that session. Double-submit csrf (cookie + header), same as a write.
        let code = totp_now(&creds.totp_secret).map_err(|reason| HumbleError::LoginFailed {
            reason: format!("TOTP: {reason}"),
        })?;
        // Same csrf dance as every other write, but riding the BOOTSTRAP session (not the
        // client's current, dead one) — csrf_write_as keeps the header shape shared.
        let boot_session = anon.as_ref().map(|a| format!("_simpleauth_sess={a}"));
        let resp = self
            .csrf_write_as(
                format!("{}/processlogin", self.base),
                &csrf,
                "/login",
                boot_session.as_deref(),
            )
            .form(&processlogin_form(creds, &code))
            .send()
            .await?;
        let status = resp.status().as_u16();
        // The authenticated session is humble's rotated cookie if it set a new one, else the anon
        // session (now authenticated). Capture the rotation BEFORE draining the body.
        let rotated = extract_set_cookie(resp.headers(), "_simpleauth_sess");
        // NEVER log the body — a /processlogin response can echo account/session state.
        let bytes = resp.bytes().await.unwrap_or_default();
        let ok = status == 200 && processlogin_ok(&bytes);
        tracing::info!(status, ok, "humble self-login (/processlogin) response");
        if !ok {
            return Err(HumbleError::LoginFailed {
                reason: format!("/processlogin returned status {status} without a goto"),
            });
        }
        let session = rotated.or(anon).ok_or_else(|| HumbleError::LoginFailed {
            reason: "login succeeded but no session cookie was captured".into(),
        })?;
        // Swap the freshly-authenticated session into the client IN PLACE so every subsequent call
        // uses it — the caller also gets it back to persist (e.g. to SSM) for the next cold start.
        *self
            .cookie
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = session.clone();
        Ok(session)
    }
}

#[cfg(test)]
mod location_tests {
    use super::location_str;
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
    fn totp_rejects_a_seed_that_normalizes_to_empty() {
        // Whitespace-only and pure-padding seeds decode to an empty key, which HMAC would ACCEPT —
        // producing a garbage-but-valid code we'd POST to the live account. These must error, not
        // silently compute.
        for seed in ["", "   ", "\t\n ", "===="] {
            let err = totp_at(seed, 1).unwrap_err();
            assert!(
                err.contains("empty"),
                "seed {seed:?} should error as empty: {err}"
            );
        }
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
    fn extract_set_cookie_pulls_the_named_value() {
        use super::extract_set_cookie;
        use wreq::header::HeaderMap;
        let mut h = HeaderMap::new();
        h.append(
            "set-cookie",
            "csrf_cookie=abc123; Path=/; HttpOnly".parse().unwrap(),
        );
        h.append(
            "set-cookie",
            "_simpleauth_sess=SESSVAL; Path=/; Secure; SameSite=Lax"
                .parse()
                .unwrap(),
        );
        assert_eq!(
            extract_set_cookie(&h, "csrf_cookie").as_deref(),
            Some("abc123")
        );
        assert_eq!(
            extract_set_cookie(&h, "_simpleauth_sess").as_deref(),
            Some("SESSVAL")
        );
        // absent + empty-valued cookies both yield None (never a bogus empty session)
        assert_eq!(extract_set_cookie(&h, "nope"), None);
        let mut e = HeaderMap::new();
        e.append("set-cookie", "_simpleauth_sess=; Path=/".parse().unwrap());
        assert_eq!(extract_set_cookie(&e, "_simpleauth_sess"), None);
        // prefix-collision safety: "csrf_cookie_x" must NOT match a query for "csrf_cookie"
        let mut c = HeaderMap::new();
        c.append(
            "set-cookie",
            "csrf_cookie_extra=zzz; Path=/".parse().unwrap(),
        );
        assert_eq!(extract_set_cookie(&c, "csrf_cookie"), None);
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
