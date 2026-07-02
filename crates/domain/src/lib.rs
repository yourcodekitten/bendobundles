//! bendobundles domain types and state transitions. No I/O lives here.
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GameStatus {
    Available,
    Pending,
    Gifted,
    BenRedeemed,
    Expired,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClaimState {
    Pending,
    Fulfilled,
    Compensated,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Game {
    pub id: String,
    pub title: String,
    pub bundle: String,
    pub gamekey: String,
    pub machine_name: String,
    pub key_type: String,
    pub giftable: bool,
    pub hidden: bool,
    pub status: GameStatus,
    pub claim_id: Option<String>,
    pub artwork_url: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Link {
    pub token: String,
    pub label: String,
    pub claims_allowed: u32,
    pub claims_used: u32,
    pub revoked: bool,
    #[serde(with = "time::serde::rfc3339::option")]
    pub expires_at: Option<OffsetDateTime>,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Claim {
    pub id: String,
    pub link_token: String,
    pub game_id: String,
    pub state: ClaimState,
    pub gift_url: Option<String>,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ClaimRefusal {
    #[error("link revoked")]
    Revoked,
    #[error("link expired")]
    Expired,
    #[error("all claims used")]
    Exhausted,
}

impl Game {
    pub fn is_listable(&self) -> bool {
        self.status == GameStatus::Available && self.giftable && !self.hidden
    }
}

impl Link {
    pub fn can_claim(&self, now: OffsetDateTime) -> Result<(), ClaimRefusal> {
        if self.revoked {
            return Err(ClaimRefusal::Revoked);
        }
        if let Some(exp) = self.expires_at
            && exp <= now
        {
            return Err(ClaimRefusal::Expired);
        }
        if self.claims_used >= self.claims_allowed {
            return Err(ClaimRefusal::Exhausted);
        }
        Ok(())
    }
}

pub fn game_id(gamekey: &str, machine_name: &str) -> String {
    format!("{gamekey}:{machine_name}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::macros::datetime;

    fn link() -> Link {
        Link {
            token: "tok".into(),
            label: "dave".into(),
            claims_allowed: 2,
            claims_used: 0,
            revoked: false,
            expires_at: None,
            created_at: datetime!(2026-07-02 00:00 UTC),
        }
    }

    #[test]
    fn listable_iff_available_giftable_unhidden() {
        let mut g = Game {
            id: game_id("gk", "mn"),
            title: "T".into(),
            bundle: "B".into(),
            gamekey: "gk".into(),
            machine_name: "mn".into(),
            key_type: "steam".into(),
            giftable: true,
            hidden: false,
            status: GameStatus::Available,
            claim_id: None,
            artwork_url: None,
        };
        assert!(g.is_listable());
        g.hidden = true;
        assert!(!g.is_listable());
        g.hidden = false;
        g.status = GameStatus::Gifted;
        assert!(!g.is_listable());
        g.status = GameStatus::Available;
        g.giftable = false;
        assert!(!g.is_listable());
    }

    #[test]
    fn link_claim_gates() {
        let now = datetime!(2026-07-02 12:00 UTC);
        assert!(link().can_claim(now).is_ok());

        let mut l = link();
        l.revoked = true;
        assert_eq!(l.can_claim(now), Err(ClaimRefusal::Revoked));

        let mut l = link();
        l.expires_at = Some(datetime!(2026-07-01 00:00 UTC));
        assert_eq!(l.can_claim(now), Err(ClaimRefusal::Expired));

        let mut l = link();
        l.claims_used = 2;
        assert_eq!(l.can_claim(now), Err(ClaimRefusal::Exhausted));
    }

    #[test]
    fn game_id_shape() {
        assert_eq!(game_id("abc", "def_tpk"), "abc:def_tpk");
    }
}
