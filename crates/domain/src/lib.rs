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
    #[serde(default)]
    pub keyindex: u32,
    /// `true` = a claimable Humble Choice game that must be chosen (spends a monthly pick)
    /// before it has a redeemable key; `false` = a normal key-backed game.
    /// `#[serde(default)]` means every stored record written before this field existed
    /// deserializes to `false` (no migration needed).
    #[serde(default)]
    pub requires_choice: bool,
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

pub fn sync_status(redeemed: bool, expired: bool) -> GameStatus {
    if expired {
        GameStatus::Expired
    } else if redeemed {
        GameStatus::BenRedeemed
    } else {
        GameStatus::Available
    }
}

pub fn merge_sync(existing: Option<&Game>, fresh: Game) -> Option<Game> {
    match existing {
        None => Some(fresh),
        Some(existing_game) => {
            let merged = match existing_game.status {
                GameStatus::Pending | GameStatus::Gifted => {
                    // App owns the record: keep status, claim_id, hidden, giftable
                    // Refresh: title, bundle, artwork_url, keyindex, key_type, requires_choice
                    // from fresh (requires_choice is Humble-derived: it flips false once a
                    // choose→redeem lands a real key, and sync is the source of that truth)
                    Game {
                        id: existing_game.id.clone(),
                        title: fresh.title,
                        bundle: fresh.bundle,
                        gamekey: existing_game.gamekey.clone(),
                        machine_name: existing_game.machine_name.clone(),
                        key_type: fresh.key_type,
                        giftable: existing_game.giftable,
                        hidden: existing_game.hidden,
                        status: existing_game.status,
                        claim_id: existing_game.claim_id.clone(),
                        artwork_url: fresh.artwork_url,
                        keyindex: fresh.keyindex,
                        requires_choice: fresh.requires_choice,
                    }
                }
                GameStatus::Available | GameStatus::BenRedeemed | GameStatus::Expired => {
                    // Humble-owned: fresh wins entirely except hidden. No catch-all `_` —
                    // a future GameStatus variant must be consciously classified here,
                    // same as the no-`_` rule in fulfillment's gift_decision.
                    Game {
                        hidden: existing_game.hidden,
                        ..fresh
                    }
                }
            };

            if merged == *existing_game {
                None
            } else {
                Some(merged)
            }
        }
    }
}

pub fn match_artwork<'a>(
    human_name: &str,
    subproducts: &'a [(String, Option<String>)],
) -> Option<&'a str> {
    let human_lower = human_name.to_lowercase();

    // First try exact case-insensitive match
    for (name, icon) in subproducts {
        if name.to_lowercase() == human_lower {
            // Exact match found, return its icon (even if None)
            return icon.as_deref();
        }
    }

    // Then try prefix match (either direction, case-insensitive): prefer the longest
    // matching subproduct name so "Portal 2" beats "Portal" for key "Portal 2 Steam Key".
    let best = subproducts
        .iter()
        .filter(|(name, icon)| {
            let name_lower = name.to_lowercase();
            icon.is_some()
                && (name_lower.starts_with(&human_lower) || human_lower.starts_with(&name_lower))
        })
        .max_by_key(|(name, _)| name.len());
    if let Some((_, icon)) = best {
        return icon.as_deref();
    }

    None
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
            keyindex: 0,
            requires_choice: false,
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

    #[test]
    fn sync_status_derivation() {
        assert_eq!(sync_status(false, false), GameStatus::Available);
        assert_eq!(sync_status(true, false), GameStatus::BenRedeemed);
        assert_eq!(sync_status(false, true), GameStatus::Expired);
        assert_eq!(sync_status(true, true), GameStatus::Expired);
    }

    fn fresh_game() -> Game {
        Game {
            id: game_id("gk", "mn"),
            title: "New Title".into(),
            bundle: "B".into(),
            gamekey: "gk".into(),
            machine_name: "mn".into(),
            key_type: "steam".into(),
            giftable: true,
            hidden: false,
            status: GameStatus::Available,
            claim_id: None,
            artwork_url: Some("new.png".into()),
            keyindex: 4,
            requires_choice: false,
        }
    }

    #[test]
    fn merge_new_game_is_fresh() {
        assert_eq!(merge_sync(None, fresh_game()), Some(fresh_game()));
    }

    #[test]
    fn merge_preserves_hidden_on_humble_owned() {
        let mut existing = fresh_game();
        existing.hidden = true;
        existing.title = "Old Title".into();
        let merged = merge_sync(Some(&existing), fresh_game()).unwrap();
        assert!(merged.hidden);
        assert_eq!(merged.title, "New Title");
        assert_eq!(merged.status, GameStatus::Available);
    }

    #[test]
    fn merge_never_touches_app_owned_status() {
        let mut existing = fresh_game();
        existing.status = GameStatus::Gifted;
        existing.claim_id = Some("c1".into());
        existing.title = "Old Title".into();
        let mut fresh = fresh_game();
        fresh.status = GameStatus::BenRedeemed; // humble sees the gifted key as redeemed
        let merged = merge_sync(Some(&existing), fresh).unwrap();
        assert_eq!(merged.status, GameStatus::Gifted);
        assert_eq!(merged.claim_id.as_deref(), Some("c1"));
        assert_eq!(merged.title, "New Title"); // cosmetics refresh
    }

    #[test]
    fn merge_no_change_returns_none() {
        let g = fresh_game();
        assert_eq!(merge_sync(Some(&g), g.clone()), None);
    }

    #[test]
    fn requires_choice_defaults_false_on_old_records() {
        // A stored record written before the field existed: no `requires_choice` key at all.
        let json = serde_json::to_value(fresh_game()).unwrap();
        let mut stripped = json.clone();
        stripped.as_object_mut().unwrap().remove("requires_choice");
        assert!(stripped.get("requires_choice").is_none(), "field stripped");
        let g: Game = serde_json::from_value(stripped).unwrap();
        assert!(
            !g.requires_choice,
            "missing attribute must default to false"
        );
    }

    #[test]
    fn requires_choice_roundtrips_true() {
        let mut g = fresh_game();
        g.requires_choice = true;
        let json = serde_json::to_string(&g).unwrap();
        let back: Game = serde_json::from_str(&json).unwrap();
        assert!(back.requires_choice);
        assert_eq!(back, g);
    }

    #[test]
    fn artwork_matching() {
        let subs = vec![
            ("Stardew Valley".to_string(), Some("s.png".to_string())),
            ("Undertale".to_string(), None),
            ("BIT.TRIP".to_string(), Some("b.png".to_string())),
        ];
        assert_eq!(match_artwork("stardew valley", &subs), Some("s.png"));
        assert_eq!(match_artwork("Undertale", &subs), None); // matched but no icon
        assert_eq!(
            match_artwork("BIT.TRIP BEAT Steam Key", &subs),
            Some("b.png")
        ); // prefix
        assert_eq!(match_artwork("Nothing Alike", &subs), None);
    }

    #[test]
    fn artwork_longest_prefix_wins() {
        // "Portal" is a prefix of "Portal 2 Steam Key"; "Portal 2" is a longer prefix.
        // The longest matching subproduct name must win.
        let subs = vec![
            ("Portal".to_string(), Some("p.png".to_string())),
            ("Portal 2".to_string(), Some("p2.png".to_string())),
        ];
        assert_eq!(
            match_artwork("Portal 2 Steam Key", &subs),
            Some("p2.png"),
            "longest prefix (Portal 2) must beat shorter prefix (Portal)"
        );
    }
}
