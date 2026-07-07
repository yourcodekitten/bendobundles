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

/// Source that produced a [`Game::steam_app_id`], used to decide which value wins in
/// [`merge_sync`]. Precedence (highest first): `Manual` > `Humble` > `Title`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AppidSource {
    /// Resolved by title-matching against the Steam app list — lowest-confidence tier.
    Title,
    /// Sourced directly from Humble's wire data (tpk `steam_app_id` field) — mid-confidence tier.
    Humble,
    /// Set by an admin override — highest-confidence tier; never overwritten by a sync walk.
    Manual,
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
    /// `true` = a Humble Choice game with **no redeemable key yet**: a monthly pick must be
    /// spent (choosecontent) before a key exists. `false` = a normal key-backed game.
    ///
    /// Trust contract (phase-3 orchestration reads this as law):
    /// - Only the Choice discovery ingest may write `true`, and only from a KNOWN claimed
    ///   set: humble-client's single-month read (`choice_month`, claimable = offered − chosen).
    ///   The `choice_months` list walk cannot see the picks (its claimed set is `None` =
    ///   unknown, and `ChoiceMonth::claimable_games` refuses to guess) — it must never be a
    ///   source of `true`. Every key-derived path (`fulfillment::run_sync` walking
    ///   `order.keys`) writes `false`, because presence in `order.keys` is itself proof a
    ///   redeemable key exists.
    /// - While `true`, there is no key to gift or redeem — any path that hands out a key
    ///   must gate on this flag (choose first, then redeem).
    /// - [`Game::is_listable`] deliberately does NOT consult this flag: choice games stay
    ///   listable/claimable, and the pick is spent at fulfillment time.
    /// - `#[serde(default)]`: records written before this field existed deserialize to
    ///   `false`, which is correct — every pre-existing record came from `order.keys`.
    ///
    /// As of this build nothing writes `true` yet; the discovery-wiring build is the sole
    /// intended writer.
    #[serde(default)]
    pub requires_choice: bool,

    /// Steam App ID for this game, when known. Set by one of three sources (see [`AppidSource`]).
    /// `None` for non-steam key types and any game whose appid has not yet been resolved.
    /// `#[serde(default)]`: records written before this field existed deserialize to `None`.
    #[serde(default)]
    pub steam_app_id: Option<u32>,

    /// Which source produced [`steam_app_id`](Self::steam_app_id). `None` iff `steam_app_id` is
    /// `None`. Determines merge precedence: `Manual` beats `Humble` beats `Title`.
    /// `#[serde(default)]`: records written before this field existed deserialize to `None`.
    #[serde(default)]
    pub appid_source: Option<AppidSource>,

    /// `true` if Ben has personally redeemed or owns this game on Steam, stamped by a dedicated
    /// ownership-sync pass (not by the order walk). `merge_sync` ALWAYS carries this from the
    /// existing record so the walk can never accidentally clear it.
    /// `#[serde(default)]`: records written before this field existed deserialize to `false`.
    #[serde(default)]
    pub owned_by_ben: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Link {
    pub token: String,
    pub label: String,
    pub claims_allowed: u32,
    pub claims_used: u32,
    pub revoked: bool,
    // `with = rfc3339::option` replaces serde's whole Deserialize impl, which DISABLES the
    // implicit missing-field-is-None behavior plain `Option` fields get — without `default`,
    // a stored record lacking the field fails the entire deserialize (and one bad link body
    // bricks a whole list read). `default` restores None-on-missing.
    #[serde(default, with = "time::serde::rfc3339::option")]
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
    /// Self-claim only: the revealed key VALUE, written durable-first exactly like `gift_url`.
    /// `default` keeps every pre-existing CLAIM item wire-valid.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revealed_key: Option<String>,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    /// Pre-choose snapshot of the month order's tpk `machine_name`s, taken and made durable BEFORE
    /// the `choosecontent` write (the crash-recovery hinge). Only ever set on a Humble Choice
    /// claim, by [`Store::record_choice_intent`](../dynamo). Its presence/absence is load-bearing
    /// for reconcile:
    /// - `None` ⇒ the intent write never landed ⇒ `choosecontent` was provably NEVER attempted ⇒
    ///   the monthly pick is NOT spent ⇒ reconcile may safely compensate.
    /// - `Some(pre)` ⇒ a choose MAY have run; reconcile decides purely from the order diff
    ///   (`order.keys \ pre`), never from the choose error and never by re-choosing.
    ///
    /// `#[serde(default)]`: every pre-existing stored claim (and every non-choice claim) reads back
    /// as `None`, which is correct — none of them ever recorded a choose intent.
    #[serde(default)]
    pub choice_pre_tpks: Option<Vec<String>>,
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

/// Reserved link_token partition for admin self-claims (`pk=LINK#SELF`). No Link META item ever
/// exists for it: intake/fulfill/compensate use the SELF-specific store writes, and the public
/// link fetch 404s it like any unknown token.
pub const SELF_LINK_TOKEN: &str = "SELF";

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

/// Merge rule for `steam_app_id` + `appid_source`: Manual admin override wins, then a fresh
/// Humble-sourced id beats a stale Title-sourced one, otherwise keep existing.
///
/// Precedence (highest wins):
/// 1. `existing.appid_source == Some(Manual)` → keep existing's pair unconditionally.
/// 2. `fresh.steam_app_id.is_some()` → take fresh's pair (Humble beats stale Title; new
///    Title beats None).
/// 3. else → keep existing's pair (fresh has no id; don't clear an existing one).
fn merge_appid(existing: &Game, fresh: &Game) -> (Option<u32>, Option<AppidSource>) {
    if existing.appid_source == Some(AppidSource::Manual) {
        // Admin override — untouchable
        (existing.steam_app_id, existing.appid_source)
    } else if fresh.steam_app_id.is_some() {
        // Fresh has an id: take it (Humble beats stale Title; new Title beats None)
        (fresh.steam_app_id, fresh.appid_source)
    } else {
        // Fresh has no id: preserve existing
        (existing.steam_app_id, existing.appid_source)
    }
}

pub fn merge_sync(existing: Option<&Game>, fresh: Game) -> Option<Game> {
    match existing {
        None => Some(fresh),
        Some(existing_game) => {
            let merged = match existing_game.status {
                GameStatus::Pending | GameStatus::Gifted => {
                    // App owns the record: keep status, claim_id, hidden, giftable, owned_by_ben.
                    // Refresh: title, bundle, artwork_url, keyindex, key_type, requires_choice
                    // from fresh. requires_choice is Humble-derived, so fresh always wins
                    // (both branches agree on this): a key-sync fresh carries `false` because
                    // presence in order.keys proves a key exists, so a chosen game flips false
                    // on its next sync — PROVIDED the discovery ingest derives the same
                    // game id (via `game_id()`: gamekey:machine_name) as the post-choose
                    // key record. That id agreement is an obligation on the discovery-wiring
                    // build; if the ids diverge, the stale `true` record lingers as a duplicate
                    // instead of flipping. A stale `true` must never survive a fresh `false`,
                    // nor the reverse.
                    let (steam_app_id, appid_source) = merge_appid(existing_game, &fresh);
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
                        steam_app_id,
                        appid_source,
                        owned_by_ben: existing_game.owned_by_ben,
                    }
                }
                GameStatus::Available | GameStatus::BenRedeemed | GameStatus::Expired => {
                    // Humble-owned: fresh wins entirely except hidden, owned_by_ben, and the
                    // appid pair (which follows its own precedence). No catch-all `_` —
                    // a future GameStatus variant must be consciously classified here,
                    // same as the no-`_` rule in fulfillment's gift_decision.
                    let (steam_app_id, appid_source) = merge_appid(existing_game, &fresh);
                    Game {
                        hidden: existing_game.hidden,
                        owned_by_ben: existing_game.owned_by_ben,
                        steam_app_id,
                        appid_source,
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
            steam_app_id: None,
            appid_source: None,
            owned_by_ben: false,
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
    fn link_expires_at_missing_field_is_none_not_error() {
        // A record written before the field existed (or hand-migrated without it): no
        // `expires_at` key at all. `time::serde::rfc3339::option` alone would make this a
        // hard deserialize error — `#[serde(default)]` must map it to None.
        let mut json = serde_json::to_value(link()).unwrap();
        json.as_object_mut().unwrap().remove("expires_at");
        assert!(json.get("expires_at").is_none(), "field stripped");
        let l: Link = serde_json::from_value(json).unwrap();
        assert_eq!(
            l.expires_at, None,
            "missing expires_at must default to None"
        );

        // present-and-null and present-and-set still roundtrip
        let none_link = link();
        let back: Link = serde_json::from_str(&serde_json::to_string(&none_link).unwrap()).unwrap();
        assert_eq!(back, none_link);
        let mut some_link = link();
        some_link.expires_at = Some(datetime!(2026-08-01 00:00 UTC));
        let back: Link = serde_json::from_str(&serde_json::to_string(&some_link).unwrap()).unwrap();
        assert_eq!(back, some_link);
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
            steam_app_id: None,
            appid_source: None,
            owned_by_ben: false,
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
    fn merge_flips_requires_choice_when_key_lands() {
        // A choice game got chosen: the next key-sync fresh carries requires_choice=false
        // (presence in order.keys proves a key exists). The stale `true` must not survive —
        // in either ownership branch.
        let mut existing = fresh_game();
        existing.requires_choice = true;
        let merged = merge_sync(Some(&existing), fresh_game()).unwrap();
        assert!(!merged.requires_choice, "humble-owned: fresh false wins");

        let mut existing = fresh_game();
        existing.requires_choice = true;
        existing.status = GameStatus::Pending;
        existing.claim_id = Some("c1".into());
        let merged = merge_sync(Some(&existing), fresh_game()).unwrap();
        assert!(!merged.requires_choice, "app-owned: fresh false wins");
        assert_eq!(merged.status, GameStatus::Pending, "status stays app-owned");
        assert_eq!(merged.claim_id.as_deref(), Some("c1"));
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
    fn claim_choice_pre_tpks_defaults_none_when_absent() {
        // A claim stored before choice_pre_tpks existed: the field is absent from the body JSON.
        // #[serde(default)] must read it back as None (never an error), so every legacy/bundle
        // claim round-trips — and reconcile reads None as "choose provably never ran".
        let claim = Claim {
            id: "c1".into(),
            link_token: "tok".into(),
            game_id: game_id("gk", "mn"),
            state: ClaimState::Pending,
            gift_url: None,
            created_at: datetime!(2026-07-02 00:00 UTC),
            choice_pre_tpks: None,
            revealed_key: None,
        };
        let mut json = serde_json::to_value(&claim).unwrap();
        json.as_object_mut().unwrap().remove("choice_pre_tpks");
        assert!(json.get("choice_pre_tpks").is_none(), "field stripped");
        let back: Claim = serde_json::from_value(json).unwrap();
        assert_eq!(back.choice_pre_tpks, None);
        assert_eq!(back, claim);
    }

    #[test]
    fn claim_choice_pre_tpks_roundtrips_some() {
        let claim = Claim {
            id: "c1".into(),
            link_token: "tok".into(),
            game_id: game_id("gk", "octopathtravelerii"),
            state: ClaimState::Pending,
            gift_url: None,
            created_at: datetime!(2026-07-02 00:00 UTC),
            choice_pre_tpks: Some(vec!["already_owned_choice_steam".into()]),
            revealed_key: None,
        };
        let json = serde_json::to_string(&claim).unwrap();
        let back: Claim = serde_json::from_str(&json).unwrap();
        assert_eq!(
            back.choice_pre_tpks.as_deref(),
            Some(&["already_owned_choice_steam".to_string()][..])
        );
        assert_eq!(back, claim);
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

    #[test]
    fn claim_without_revealed_key_field_still_deserializes() {
        // Every pre-existing CLAIM item in dynamo lacks the field — this pins backcompat.
        let old = r#"{"id":"c1","link_token":"t","game_id":"g","state":"pending","gift_url":null,"created_at":"2026-07-01T00:00:00Z","choice_pre_tpks":null}"#;
        let c: Claim = serde_json::from_str(old).expect("old claim must deserialize");
        assert_eq!(c.revealed_key, None);
    }

    #[test]
    fn self_link_token_is_self() {
        assert_eq!(SELF_LINK_TOKEN, "SELF");
    }

    // ── steam_app_id / appid_source / owned_by_ben field tests ────────────────

    #[test]
    fn steam_fields_default_on_old_records() {
        // Records written before these fields existed must deserialize cleanly with defaults.
        let mut json = serde_json::to_value(fresh_game()).unwrap();
        json.as_object_mut().unwrap().remove("steam_app_id");
        json.as_object_mut().unwrap().remove("appid_source");
        json.as_object_mut().unwrap().remove("owned_by_ben");
        assert!(json.get("steam_app_id").is_none(), "steam_app_id stripped");
        assert!(json.get("appid_source").is_none(), "appid_source stripped");
        assert!(json.get("owned_by_ben").is_none(), "owned_by_ben stripped");
        let g: Game = serde_json::from_value(json).unwrap();
        assert_eq!(g.steam_app_id, None);
        assert_eq!(g.appid_source, None);
        assert!(!g.owned_by_ben);
    }

    #[test]
    fn merge_appid_humble_fresh_beats_stale_title() {
        // existing {Some(111), Some(Title)} + fresh {Some(222), Some(Humble)} → fresh's pair wins
        let mut existing = fresh_game();
        existing.steam_app_id = Some(111);
        existing.appid_source = Some(AppidSource::Title);
        let mut fresh = fresh_game();
        fresh.steam_app_id = Some(222);
        fresh.appid_source = Some(AppidSource::Humble);
        let merged = merge_sync(Some(&existing), fresh).unwrap();
        assert_eq!(merged.steam_app_id, Some(222));
        assert_eq!(merged.appid_source, Some(AppidSource::Humble));
    }

    #[test]
    fn merge_appid_manual_wins_over_fresh_humble() {
        // existing {Some(111), Some(Manual)} + fresh {Some(222), Some(Humble)} → existing's pair wins
        // Force a title change so the merge returns Some (not a no-op).
        let mut existing = fresh_game();
        existing.steam_app_id = Some(111);
        existing.appid_source = Some(AppidSource::Manual);
        existing.title = "Old Title".into();
        let mut fresh = fresh_game(); // title = "New Title"
        fresh.steam_app_id = Some(222);
        fresh.appid_source = Some(AppidSource::Humble);
        let merged = merge_sync(Some(&existing), fresh).unwrap();
        assert_eq!(
            merged.steam_app_id,
            Some(111),
            "manual source: existing pair kept"
        );
        assert_eq!(merged.appid_source, Some(AppidSource::Manual));
    }

    #[test]
    fn merge_appid_app_owned_manual_wins_over_fresh_humble() {
        // Same manual-wins logic applies in the Pending/Gifted (app-owned) branch.
        // Force a title change so the merge returns Some (not a no-op).
        let mut existing = fresh_game();
        existing.status = GameStatus::Pending;
        existing.claim_id = Some("c1".into());
        existing.steam_app_id = Some(111);
        existing.appid_source = Some(AppidSource::Manual);
        existing.title = "Old Title".into();
        let mut fresh = fresh_game(); // title = "New Title"
        fresh.steam_app_id = Some(222);
        fresh.appid_source = Some(AppidSource::Humble);
        let merged = merge_sync(Some(&existing), fresh).unwrap();
        assert_eq!(
            merged.steam_app_id,
            Some(111),
            "manual wins in app-owned branch"
        );
        assert_eq!(merged.appid_source, Some(AppidSource::Manual));
        assert_eq!(merged.status, GameStatus::Pending);
    }

    #[test]
    fn merge_appid_app_owned_humble_fresh_beats_stale_title() {
        // Pending branch: fresh Humble id beats an existing Title id.
        let mut existing = fresh_game();
        existing.status = GameStatus::Pending;
        existing.claim_id = Some("c1".into());
        existing.steam_app_id = Some(111);
        existing.appid_source = Some(AppidSource::Title);
        let mut fresh = fresh_game();
        fresh.steam_app_id = Some(222);
        fresh.appid_source = Some(AppidSource::Humble);
        let merged = merge_sync(Some(&existing), fresh).unwrap();
        assert_eq!(merged.steam_app_id, Some(222));
        assert_eq!(merged.appid_source, Some(AppidSource::Humble));
        assert_eq!(merged.status, GameStatus::Pending);
    }

    #[test]
    fn merge_owned_by_ben_always_preserved() {
        // owned_by_ben is stamped by a separate sync pass; merge_sync must NEVER clobber it.
        // Force a title change so merge returns Some (otherwise returns None for no-op).
        let mut existing = fresh_game();
        existing.owned_by_ben = true;
        existing.title = "Old Title".into();
        let merged = merge_sync(Some(&existing), fresh_game()).unwrap();
        assert!(
            merged.owned_by_ben,
            "owned_by_ben must survive humble-owned merge"
        );
    }

    #[test]
    fn merge_owned_by_ben_app_owned_preserved() {
        // Same in the app-owned (Pending/Gifted) branch.
        // Force a title change so the merge returns Some (not a no-op).
        let mut existing = fresh_game();
        existing.status = GameStatus::Pending;
        existing.claim_id = Some("c1".into());
        existing.owned_by_ben = true;
        existing.title = "Old Title".into();
        let fresh = fresh_game(); // owned_by_ben = false (walk never sets it), title = "New Title"
        let merged = merge_sync(Some(&existing), fresh).unwrap();
        assert!(
            merged.owned_by_ben,
            "owned_by_ben preserved in app-owned branch"
        );
        assert_eq!(merged.status, GameStatus::Pending);
    }

    #[test]
    fn merge_appid_fresh_none_preserves_existing_pair_both_branches() {
        // Tier 3: fresh carries NO id — existing non-Manual pair must survive, or every
        // key-sync clobbers the mapper's work. (Deleting merge_appid's else-branch must
        // fail this test.) Force a title change so the merge returns Some (not a no-op).

        // Humble-owned (Available) branch, Title-sourced existing:
        let mut existing = fresh_game();
        existing.steam_app_id = Some(413150);
        existing.appid_source = Some(AppidSource::Title);
        existing.title = "Old Title".into();
        let merged = merge_sync(Some(&existing), fresh_game()).unwrap();
        assert_eq!(
            merged.steam_app_id,
            Some(413150),
            "fresh None: keep existing"
        );
        assert_eq!(merged.appid_source, Some(AppidSource::Title));

        // App-owned (Pending) branch, Humble-sourced existing:
        let mut existing = fresh_game();
        existing.status = GameStatus::Pending;
        existing.claim_id = Some("c1".into());
        existing.steam_app_id = Some(413150);
        existing.appid_source = Some(AppidSource::Humble);
        existing.title = "Old Title".into();
        let merged = merge_sync(Some(&existing), fresh_game()).unwrap();
        assert_eq!(
            merged.steam_app_id,
            Some(413150),
            "fresh None: keep existing"
        );
        assert_eq!(merged.appid_source, Some(AppidSource::Humble));
        assert_eq!(merged.status, GameStatus::Pending);
    }
}
