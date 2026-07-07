//! Steam Web API client — owned-games (privacy-pinned), persona, vanity.
use serde::Deserialize;

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

// ── Client ───────────────────────────────────────────────────────────────────

pub struct SteamClient {
    base_web_api: String,
    #[allow(dead_code)]
    base_store: String,
    http: reqwest::Client,
    key: SteamApiKey,
}

impl SteamClient {
    pub fn new(web_api_base: &str, store_base: &str, key: SteamApiKey) -> Result<Self, SteamError> {
        let http = reqwest::Client::builder()
            .build()
            .map_err(|e| SteamError::Network(e.to_string()))?;
        Ok(Self {
            base_web_api: web_api_base.to_string(),
            base_store: store_base.to_string(),
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
    SteamError::Network(e.to_string())
}
