//! Steam Web API client — owned-games (privacy-pinned), persona, vanity, OpenID.
use serde::{Deserialize, Serialize};

// ── Key newtype ──────────────────────────────────────────────────────────────

pub struct SteamApiKey(String);

impl SteamApiKey {
    pub fn new(v: String) -> Self {
        Self(v)
    }
    fn expose(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for SteamApiKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SteamApiKey(REDACTED)")
    }
}

// ── ID newtype ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SteamId64(pub String);

// ── Domain types ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SteamAppDetail {
    pub app_id: u32,
    pub name: String,
    pub developers: Vec<String>,
    pub publishers: Vec<String>,
    /// genres + allowlisted player-mode categories (Single-player, Multi-player,
    /// Co-op, PvP, MMO), deduped order-preserving. Store-feature categories
    /// (achievements, cloud, controller…) are filtered out by id at parse time.
    pub genres: Vec<String>,
    pub release_date: Option<String>,
    pub short_description: String,
    pub header_image: Option<String>,
    /// First movie's hls_h264 URL (movies are HLS/DASH-only now, no mp4/webm)
    pub video_hls_url: Option<String>,
    pub video_thumbnail: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum AppDetails {
    Found(Box<SteamAppDetail>),
    /// `success: false` from the API — app is delisted or never existed
    Delisted,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReviewSummary {
    pub desc: String,
    pub total_positive: u64,
    pub total_negative: u64,
    pub total_reviews: u64,
}

/// Computed from histogram `results.recent` buckets.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RecentReviews {
    pub percent_positive: u8,
    pub count: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OwnedGames {
    /// "game details" privacy hides the library: the response carries NO `game_count` at all.
    /// Distinct from an empty library (`game_count: 0`) — spec M4; do NOT infer privacy from
    /// GetPlayerSummaries' communityvisibilitystate (profile visibility is a different setting).
    Private,
    Games(Vec<u32>),
}

#[derive(Debug)]
pub struct Persona {
    pub name: String,
    pub avatar_url: Option<String>,
}

// ── Error ────────────────────────────────────────────────────────────────────

#[derive(Debug, thiserror::Error)]
pub enum SteamError {
    #[error("steam api http {0}")]
    Api(u16),
    #[error("network: {0}")]
    Network(String),
    #[error("parse: {0}")]
    Parse(String),
    #[error("rate limited")]
    RateLimited,
    #[error("bad api key")]
    KeyRejected,
    #[error("no such vanity/steamid")]
    NotFound,
    #[error("openid verification failed: {0}")]
    OpenIdRejected(String),
}

// ── Wire types ───────────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct OwnedWire {
    response: OwnedResp,
}
#[derive(Deserialize)]
struct OwnedResp {
    game_count: Option<u64>,
    #[serde(default)]
    games: Vec<OwnedGame>,
}
#[derive(Deserialize)]
struct OwnedGame {
    appid: u32,
}

#[derive(Deserialize)]
struct PlayerSummariesWire {
    response: PlayerSummariesResp,
}
#[derive(Deserialize)]
struct PlayerSummariesResp {
    players: Vec<PlayerWire>,
}
#[derive(Deserialize)]
struct PlayerWire {
    personaname: String,
    avatarfull: Option<String>,
}

#[derive(Deserialize)]
struct VanityWire {
    response: VanityResp,
}
#[derive(Deserialize)]
struct VanityResp {
    success: u8,
    steamid: Option<String>,
}

#[derive(Deserialize)]
struct AppListWire {
    response: AppListResp,
}
#[derive(Deserialize)]
struct AppListResp {
    #[serde(default)]
    apps: Vec<AppEntry>,
    /// Omitted (not `false`) on the final page.
    #[serde(default)]
    have_more_results: bool,
    #[serde(default)]
    last_appid: Option<u32>,
}
#[derive(Deserialize)]
struct AppEntry {
    appid: u32,
    name: String,
}

// ── Storefront wire types ─────────────────────────────────────────────────────

#[derive(Deserialize)]
struct AppDetailsEntry {
    success: bool,
    data: Option<AppDetailDataWire>,
}

#[derive(Deserialize)]
struct AppDetailDataWire {
    name: String,
    #[serde(default)]
    developers: Vec<String>,
    #[serde(default)]
    publishers: Vec<String>,
    #[serde(default)]
    genres: Vec<DescriptionWire>,
    #[serde(default)]
    categories: Vec<CategoryWire>,
    release_date: Option<ReleaseDateWire>,
    #[serde(default)]
    short_description: String,
    header_image: Option<String>,
    #[serde(default)]
    movies: Vec<MovieWire>,
}

#[derive(Deserialize)]
struct DescriptionWire {
    description: String,
}

/// `categories[].id` is a JSON number (unlike `genres[].id`, a string) and is Steam's
/// stable category identifier — the allowlist keys on it, not on the description text.
/// A missing id deserializes to 0 (allowlisted-nothing) rather than failing the parse.
#[derive(Deserialize)]
struct CategoryWire {
    #[serde(default)]
    id: u32,
    description: String,
}

/// Steam category ids that survive into `SteamAppDetail::genres`: the top-level player
/// modes only. 2 Single-player, 1 Multi-player, 9 Co-op, 49 PvP, 20 MMO. Mode *variants*
/// (Online Co-op 38, LAN Co-op 48, …) are dropped — Steam includes the parent category
/// alongside its variants, so coverage holds while the tag count stays flat (issue #57).
const ALLOWED_CATEGORY_IDS: [u32; 5] = [2, 1, 9, 49, 20];

#[derive(Deserialize)]
struct ReleaseDateWire {
    date: Option<String>,
}

#[derive(Deserialize)]
struct MovieWire {
    hls_h264: Option<String>,
    thumbnail: Option<String>,
}

#[derive(Deserialize)]
struct AppReviewsWire {
    query_summary: AppReviewsQuerySummary,
}

#[derive(Deserialize)]
struct AppReviewsQuerySummary {
    review_score_desc: String,
    total_positive: u64,
    total_negative: u64,
    total_reviews: u64,
}

#[derive(Deserialize)]
struct ReviewHistogramWire {
    results: ReviewHistogramResults,
}

#[derive(Deserialize)]
struct ReviewHistogramResults {
    #[serde(default)]
    recent: Vec<ReviewHistogramBucket>,
}

#[derive(Deserialize)]
struct ReviewHistogramBucket {
    recommendations_up: u64,
    recommendations_down: u64,
}

// ── Client ───────────────────────────────────────────────────────────────────

pub struct SteamClient {
    base_web_api: String,
    base_store: String,
    /// Base URL for Steam OpenID endpoints (prod: `https://steamcommunity.com`).
    base_openid: String,
    http: reqwest::Client,
    key: SteamApiKey,
}

impl SteamClient {
    /// Create a new client.
    ///
    /// * `web_api_base` — Steam Web API root (prod: `https://api.steampowered.com`)
    /// * `store_base`   — Steam store root (prod: `https://store.steampowered.com`)
    /// * `openid_base`  — Steam community root used for OpenID (prod: `https://steamcommunity.com`)
    /// * `key`          — Steam Web API key
    ///
    /// The HTTP client carries hard ceilings — 10s total-request timeout, 5s connect
    /// timeout — so a hung steamcommunity connection cannot pin a request handler open
    /// forever. This matters most for the unauthenticated OpenID return endpoint, which
    /// makes one outbound `check_authentication` call per hit: without a timeout, slow-drip
    /// connections become a trivial resource-exhaustion vector, and the spec's
    /// `steam_unreachable` contract could never fire.
    pub fn new(
        web_api_base: &str,
        store_base: &str,
        openid_base: &str,
        key: SteamApiKey,
    ) -> Result<Self, SteamError> {
        let http = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(10))
            .connect_timeout(std::time::Duration::from_secs(5))
            .build()
            .map_err(|e| SteamError::Network(e.to_string()))?;
        Ok(Self {
            base_web_api: web_api_base.to_string(),
            base_store: store_base.to_string(),
            base_openid: openid_base.to_string(),
            http,
            key,
        })
    }

    pub async fn get_owned_games(&self, steamid: &SteamId64) -> Result<OwnedGames, SteamError> {
        let url = format!("{}/IPlayerService/GetOwnedGames/v0001/", self.base_web_api);
        let resp = self
            .http
            .get(url)
            .query(&[
                ("key", self.key.expose()),
                ("steamid", &steamid.0),
                ("include_played_free_games", "1"),
                ("format", "json"),
            ])
            .send()
            .await
            .map_err(net)?;
        let wire: OwnedWire = keyed_json(resp).await?;
        match wire.response.game_count {
            None => Ok(OwnedGames::Private),
            Some(_) => Ok(OwnedGames::Games(
                wire.response.games.into_iter().map(|g| g.appid).collect(),
            )),
        }
    }

    pub async fn get_player_summary(&self, steamid: &SteamId64) -> Result<Persona, SteamError> {
        let url = format!("{}/ISteamUser/GetPlayerSummaries/v0002/", self.base_web_api);
        let resp = self
            .http
            .get(url)
            .query(&[("key", self.key.expose()), ("steamids", &steamid.0)])
            .send()
            .await
            .map_err(net)?;
        let wire: PlayerSummariesWire = keyed_json(resp).await?;
        wire.response
            .players
            .into_iter()
            .next()
            .map(|p| Persona {
                name: p.personaname,
                avatar_url: p.avatarfull,
            })
            .ok_or(SteamError::NotFound)
    }

    pub async fn resolve_vanity(&self, name: &str) -> Result<SteamId64, SteamError> {
        let url = format!("{}/ISteamUser/ResolveVanityURL/v0001/", self.base_web_api);
        let resp = self
            .http
            .get(url)
            .query(&[("key", self.key.expose()), ("vanityurl", name)])
            .send()
            .await
            .map_err(net)?;
        let wire: VanityWire = keyed_json(resp).await?;
        if wire.response.success == 1 {
            wire.response
                .steamid
                .map(SteamId64)
                .ok_or(SteamError::NotFound)
        } else {
            Err(SteamError::NotFound)
        }
    }

    /// Return all `(appid, name)` pairs from `IStoreService/GetAppList/v1` (#48 — Steam
    /// removed the keyless `ISteamApps/GetAppList`; the replacement is keyed and paginated).
    ///
    /// Pages via `have_more_results`/`last_appid` until exhausted. Duplicate names are
    /// returned verbatim; deduplication is the caller's (title-match mapper's) responsibility.
    ///
    /// Termination guards (tier-2 is best-effort — partial data beats a hung sync):
    /// a cursor that fails to strictly advance, or a missing cursor with
    /// `have_more_results:true`, ends the loop with what was collected, as does the
    /// page cap (full catalog is ~200k apps ≈ 5 pages at 50k; cap 50 is generous).
    pub async fn get_app_list(&self) -> Result<Vec<(u32, String)>, SteamError> {
        const PAGE_SIZE: &str = "50000";
        const MAX_PAGES: u32 = 50;
        let url = format!("{}/IStoreService/GetAppList/v1/", self.base_web_api);
        let mut out: Vec<(u32, String)> = Vec::new();
        let mut cursor: Option<u32> = None;
        for _ in 0..MAX_PAGES {
            let mut req = self
                .http
                .get(&url)
                .query(&[("key", self.key.expose()), ("max_results", PAGE_SIZE)]);
            if let Some(last) = cursor {
                req = req.query(&[("last_appid", last.to_string())]);
            }
            let resp = req.send().await.map_err(net)?;
            let wire: AppListWire = keyed_json(resp).await?;
            out.extend(wire.response.apps.into_iter().map(|a| (a.appid, a.name)));
            if !wire.response.have_more_results {
                break;
            }
            match wire.response.last_appid {
                Some(next) if cursor.is_none_or(|prev| next > prev) => cursor = Some(next),
                _ => break,
            }
        }
        Ok(out)
    }

    /// Fetch storefront detail for `app_id`. Returns `Delisted` when the response carries
    /// `success: false` (removed or private app) rather than erroring.
    pub async fn get_app_details(&self, app_id: u32) -> Result<AppDetails, SteamError> {
        let url = format!("{}/api/appdetails", self.base_store);
        let resp = self
            .http
            .get(url)
            .query(&[
                ("appids", app_id.to_string().as_str()),
                ("cc", "us"),
                ("l", "english"),
            ])
            .send()
            .await
            .map_err(net)?;
        let map: std::collections::HashMap<String, AppDetailsEntry> = keyed_json(resp).await?;
        let key = app_id.to_string();
        let entry = map
            .into_values()
            .next()
            .ok_or_else(|| SteamError::Parse(format!("missing key {key} in appdetails")))?;
        if !entry.success {
            return Ok(AppDetails::Delisted);
        }
        let data = entry
            .data
            .ok_or_else(|| SteamError::Parse("success:true but no data field".into()))?;
        let mut genres: Vec<String> = data.genres.into_iter().map(|g| g.description).collect();
        for cat in data.categories {
            if ALLOWED_CATEGORY_IDS.contains(&cat.id) && !genres.contains(&cat.description) {
                genres.push(cat.description);
            }
        }
        let first_movie = data.movies.into_iter().next();
        Ok(AppDetails::Found(Box::new(SteamAppDetail {
            app_id,
            name: data.name,
            developers: data.developers,
            publishers: data.publishers,
            genres,
            release_date: data.release_date.and_then(|r| r.date),
            short_description: data.short_description,
            header_image: data.header_image,
            video_hls_url: first_movie.as_ref().and_then(|m| m.hls_h264.clone()),
            video_thumbnail: first_movie.and_then(|m| m.thumbnail),
        })))
    }

    /// Fetch overall review summary for `app_id` from the store `/appreviews/` endpoint.
    pub async fn get_review_summary(&self, app_id: u32) -> Result<ReviewSummary, SteamError> {
        let url = format!("{}/appreviews/{}", self.base_store, app_id);
        let resp = self
            .http
            .get(url)
            .query(&[
                ("json", "1"),
                ("num_per_page", "0"),
                ("language", "english"),
                ("purchase_type", "all"),
            ])
            .send()
            .await
            .map_err(net)?;
        let wire: AppReviewsWire = keyed_json(resp).await?;
        let qs = wire.query_summary;
        Ok(ReviewSummary {
            desc: qs.review_score_desc,
            total_positive: qs.total_positive,
            total_negative: qs.total_negative,
            total_reviews: qs.total_reviews,
        })
    }

    /// Fetch recent review sentiment for `app_id` by summing histogram buckets.
    ///
    /// `percent_positive` and `count` are both 0 when there are no recent reviews.
    pub async fn get_recent_reviews(&self, app_id: u32) -> Result<RecentReviews, SteamError> {
        let url = format!("{}/appreviewhistogram/{}", self.base_store, app_id);
        let resp = self
            .http
            .get(url)
            .query(&[("l", "english")])
            .send()
            .await
            .map_err(net)?;
        let wire: ReviewHistogramWire = keyed_json(resp).await?;
        let up: u64 = wire
            .results
            .recent
            .iter()
            .map(|b| b.recommendations_up)
            .sum();
        let down: u64 = wire
            .results
            .recent
            .iter()
            .map(|b| b.recommendations_down)
            .sum();
        let total = up + down;
        let percent_positive = (100 * up + total / 2)
            .checked_div(total)
            .and_then(|n| u8::try_from(n).ok())
            .unwrap_or(0);
        Ok(RecentReviews {
            percent_positive,
            count: total,
        })
    }

    /// Verify a Steam OpenID assertion. Trust ladder (spec §2, ALL must hold):
    ///
    /// 1. `openid.return_to` in the params EXACTLY equals the URL we're handling (standard
    ///    OpenID rule — makes ctx tampering visible).
    /// 2. `openid.claimed_id` matches `https://steamcommunity.com/openid/id/<17-digit>`.
    /// 3. Steam's own `check_authentication` echo answers `is_valid:true`. The trust of the
    ///    returned `SteamId64` rests entirely on Steam's server re-validating the OpenID
    ///    signature over the SIGNED fields and enforcing single-use `response_nonce`
    ///    server-side (the replay defense). Do NOT trust `claimed_id` without this round-trip.
    ///
    /// Fail-closed behavior: missing or empty `openid.mode` is a no-op in the mode-rewrite
    /// loop, so the POST goes out without `mode=check_authentication`; Steam does not treat it
    /// as a check_authentication request, answers not-valid, and this function returns
    /// `OpenIdRejected`. Any missing required field falls through to rejection — there is no
    /// partially-trusted path.
    pub async fn verify_openid_assertion(
        &self,
        params: &[(String, String)],
        expected_return_to: &str,
    ) -> Result<SteamId64, SteamError> {
        // Reject duplicate security-relevant openid.* keys BEFORE any processing.
        // Our get() takes the FIRST occurrence while check_authentication echoes ALL pairs
        // to Steam. If Steam's parser resolves a duplicate key to a different occurrence,
        // an attacker can forge an identity: sign their own assertion (id Y), inject a second
        // claimed_id = X (victim) before it, have get() return X while Steam validates Y.
        // Rejecting any duplication upfront kills the class entirely.
        const DUP_GUARD: &[&str] = &[
            "openid.mode",
            "openid.claimed_id",
            "openid.identity",
            "openid.return_to",
            "openid.sig",
            "openid.signed",
            "openid.response_nonce",
            "openid.assoc_handle",
            "openid.ns",
        ];
        for key in DUP_GUARD {
            if params.iter().filter(|(k, _)| k == key).count() > 1 {
                return Err(SteamError::OpenIdRejected(
                    "duplicate openid parameter".into(),
                ));
            }
        }

        let get = |k: &str| {
            params
                .iter()
                .find(|(pk, _)| pk == k)
                .map(|(_, v)| v.as_str())
        };

        let return_to = get("openid.return_to").unwrap_or("");
        if return_to != expected_return_to {
            return Err(SteamError::OpenIdRejected("return_to mismatch".into()));
        }

        let claimed = get("openid.claimed_id").unwrap_or("");
        let id = claimed
            .strip_prefix("https://steamcommunity.com/openid/id/")
            .filter(|rest| rest.len() == 17 && rest.bytes().all(|b| b.is_ascii_digit()))
            .ok_or_else(|| SteamError::OpenIdRejected("claimed_id shape".into()))?;

        // claimed_id must be in the signed field set (names in openid.signed omit the
        // "openid." prefix per OpenID 2.0). If it isn't, Steam's check_authentication would
        // not recompute the signature over it — is_valid:true would then vouch for the
        // assertion WITHOUT vouching for the id we're about to return. Reject before HTTP.
        let signed = get("openid.signed").unwrap_or("");
        if !signed.split(',').any(|f| f == "claimed_id") {
            return Err(SteamError::OpenIdRejected("claimed_id not signed".into()));
        }

        // Echo the assertion back with mode=check_authentication (form-encoded).
        let mut form: Vec<(String, String)> = params.to_vec();
        for (k, v) in &mut form {
            if k == "openid.mode" {
                *v = "check_authentication".into();
            }
        }
        let resp = self
            .http
            .post(format!("{}/openid/login", self.base_openid))
            .form(&form)
            .send()
            .await
            .map_err(net)?;
        if resp.status().as_u16() != 200 {
            return Err(SteamError::Api(resp.status().as_u16()));
        }
        let body = resp.text().await.map_err(net)?;
        if body.lines().any(|l| l.trim() == "is_valid:true") {
            Ok(SteamId64(id.to_string()))
        } else {
            Err(SteamError::OpenIdRejected("is_valid:false".into()))
        }
    }
}

// ── OpenID redirect helper ────────────────────────────────────────────────────

/// Build the "Sign in through Steam" redirect URL. Pure; both surfaces use it via the return
/// endpoint. `realm` is the RP domain; `return_to` is the exact callback URL Steam will POST to.
///
/// Deliberate asymmetry: this helper hardcodes prod `https://steamcommunity.com/openid/login`
/// while `verify_openid_assertion` uses the injectable `self.base_openid`. Users always
/// authenticate against real Steam even when running from a dev origin — which is also why
/// this helper is not wiremock-pointable (its output is pinned by a pure string test instead).
pub fn steam_openid_redirect_url(realm: &str, return_to: &str) -> String {
    let q = [
        ("openid.ns", "http://specs.openid.net/auth/2.0"),
        ("openid.mode", "checkid_setup"),
        (
            "openid.claimed_id",
            "http://specs.openid.net/auth/2.0/identifier_select",
        ),
        (
            "openid.identity",
            "http://specs.openid.net/auth/2.0/identifier_select",
        ),
        ("openid.return_to", return_to),
        ("openid.realm", realm),
    ];
    let qs = q
        .iter()
        .map(|(k, v)| format!("{k}={}", urlencoding::encode(v)))
        .collect::<Vec<_>>()
        .join("&");
    format!("https://steamcommunity.com/openid/login?{qs}")
}

// ── Helpers ──────────────────────────────────────────────────────────────────

/// Shared keyed-endpoint status mapping: 429 → RateLimited, 401/403 → KeyRejected,
/// other non-2xx → Api(status), body → serde or Parse. The key never appears in any error string.
async fn keyed_json<T: serde::de::DeserializeOwned>(
    resp: reqwest::Response,
) -> Result<T, SteamError> {
    match resp.status().as_u16() {
        200 => resp
            .json::<T>()
            .await
            .map_err(|e| SteamError::Parse(e.to_string())),
        429 => Err(SteamError::RateLimited),
        401 | 403 => Err(SteamError::KeyRejected),
        s => Err(SteamError::Api(s)),
    }
}

fn net(e: reqwest::Error) -> SteamError {
    // Strip the request URL before stringifying: keyed endpoints embed ?key=... in the URL,
    // and reqwest::Error::Display can include the full URL → key leak into error strings.
    SteamError::Network(e.without_url().to_string())
}
