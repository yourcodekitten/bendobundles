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

impl GameStatus {
    /// The exact wire/DDB string — see [`HiddenSource::as_wire`]; same contract, same
    /// pinning test. Every mirror write and condition value goes through this.
    pub const fn as_wire(self) -> &'static str {
        match self {
            GameStatus::Available => "available",
            GameStatus::Pending => "pending",
            GameStatus::Gifted => "gifted",
            GameStatus::BenRedeemed => "ben_redeemed",
            GameStatus::Expired => "expired",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClaimState {
    Pending,
    Fulfilled,
    Compensated,
    /// Terminal failure: the claim can never complete (today's one producer: a DEAD
    /// humble key — expired server-side). Generic on purpose: states are lifecycle,
    /// reasons are evidence — the *why* lives in [`Claim::failure_reason`], written in
    /// the same transaction (spec §2, family review 2026-07-29). The friend's slot is
    /// returned by that transaction; the game is retired, never re-listed.
    Failed,
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

impl AppidSource {
    /// The exact wire/DDB string — see [`HiddenSource::as_wire`]; same contract, same
    /// pinning test. The Manual-guard condition compares against this.
    pub const fn as_wire(self) -> &'static str {
        match self {
            AppidSource::Title => "title",
            AppidSource::Humble => "humble",
            AppidSource::Manual => "manual",
        }
    }
}

/// Who last decided a game's `hidden` flag. `Admin` is Ben's toggle and is FINAL: the
/// auto-hide sweep never overrides it in either direction — his unhide of an adult game
/// stays unhidden forever (#71 "never fights Ben"). `Sync` marks an automatic hide
/// (adult content descriptors) so admin can label it. `None` (legacy / never touched)
/// is auto-hide-eligible: every pre-existing unhidden game is untouched-by-Ben by
/// definition; his first toggle stamps `Admin` and immunizes the record.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HiddenSource {
    Admin,
    Sync,
}

impl HiddenSource {
    /// The exact wire/DDB string for this variant — the ONE source for every place that
    /// writes or compares the value (top-level mirror, condition-expression values).
    /// Must equal the serde output byte-for-byte or every condition against the mirror
    /// goes void; `hidden_source_serde_is_snake_case` pins the equality.
    pub const fn as_wire(self) -> &'static str {
        match self {
            HiddenSource::Admin => "admin",
            HiddenSource::Sync => "sync",
        }
    }
}

/// Content descriptor ids that auto-hide a game at sync/backfill time: 3 = adult-only
/// sexual content, 4 = gratuitous sexual content (Puss! carries both). NOT 1 (some
/// nudity — Witcher 3), NOT 5 (general mature — Rollerdrome), NOT 2 (violence).
/// Ben tightens/loosens by editing this list (#71) — AND its client twin: the admin 🔞
/// badge/mature-filter set lives in `web/src/tags.ts` (MATURE_DESCRIPTOR_IDS, {1,3,4}).
/// Invariant to keep by hand: this hide set stays a SUBSET of the badge set, or
/// auto-hidden rows stop badging and vanish from the mature=only filter.
pub const ADULT_HIDE_DESCRIPTOR_IDS: [u32; 2] = [3, 4];

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

    /// Provenance of [`hidden`](Self::hidden) — see [`HiddenSource`]. `None` iff no
    /// admin toggle or auto-hide has ever run on this record.
    /// `#[serde(default)]`: records written before this field existed deserialize to `None`.
    #[serde(default)]
    pub hidden_source: Option<HiddenSource>,
}

/// A friend — the person behind a shelf. The whole identity system: no auth,
/// no email. `shelf_token` is the bearer capability for `/s/{token}`; it is
/// cleared on revoke (no dead capability at rest) and replaced on reissue.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Friend {
    pub id: String,
    pub name: String,
    /// `""` means REVOKED: revoke REMOVEs the top-level attribute and the
    /// read side restores absence as empty (admin renders "no shelf link").
    /// A live token is always 64 hex.
    pub shelf_token: String,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Link {
    pub token: String,
    pub label: String,
    /// Ben's personal note to the friend, shown on their link page. Cosmetic
    /// (never consulted by claim enforcement), but editable after creation —
    /// so like the enforcer fields it is authoritative in a top-level dynamo
    /// attribute (written ONLY by `set_link_gift_note`'s scoped update) and
    /// overridden on read. The stored `body` blob NEVER carries it (writers
    /// serialize via `schema::link_body`, which strips it): the note lives in
    /// exactly one place, so clearing it leaves no copy at rest (OMBB, #69
    /// review). `#[serde(default)]`: records written before this field
    /// existed deserialize to `None`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gift_note: Option<String>,
    /// The friend's thank-you back to ben — `gift_note`'s return path. Same storage
    /// rules as the note it mirrors: authoritative ONLY in a top-level dynamo
    /// attribute (written by `set_link_thanks`' scoped conditional update, write-once),
    /// stripped from the stored `body` blob by `schema::link_body`, overridden on read
    /// by `link_from_item`. `#[serde(default)]`: every pre-existing record reads back
    /// as None (never thanked).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thank_note: Option<String>,
    /// When the thank-you landed. `Some` iff [`thank_note`](Self::thank_note) is `Some` —
    /// `set_link_thanks` writes both in one update. Same serde shape as `expires_at`
    /// (`default` restores None-on-missing under the `with` module) plus skip-on-None
    /// so absent stays absent on the wire.
    #[serde(
        default,
        with = "time::serde::rfc3339::option",
        skip_serializing_if = "Option::is_none"
    )]
    pub thanked_at: Option<OffsetDateTime>,
    pub claims_allowed: u32,
    pub claims_used: u32,
    pub revoked: bool,
    // `with = rfc3339::option` replaces serde's whole Deserialize impl, which DISABLES the
    // implicit missing-field-is-None behavior plain `Option` fields get — without `default`,
    // a stored record lacking the field fails the entire deserialize (and one bad link body
    // bricks a whole list read). `default` restores None-on-missing.
    #[serde(default, with = "time::serde::rfc3339::option")]
    pub expires_at: Option<OffsetDateTime>,
    /// The wrapped-gift unlock moment: while `unlock_at > now` the link is SEALED — the
    /// friend surface shows a countdown and the server withholds the payload; every claim
    /// path refuses with [`ClaimRefusal::Sealed`]. Absent = born open; a seal is
    /// CREATE-TIME-ONLY (spec 2026-08-05 §4: a link born open can never gain one, because
    /// the server can't know whether a friend already looked).
    /// Authoritative in a top-level numeric dynamo attribute like the enforcer fields;
    /// `schema::link_body` strips it from the body blob (the notes' one-place contract).
    /// Serde is thanked_at's combo — `default` restores None-on-missing under `with`, and
    /// skip-on-None keeps the stripped body free of even a null key (the body-strip test
    /// depends on absence, not null-ness).
    #[serde(
        default,
        with = "time::serde::rfc3339::option",
        skip_serializing_if = "Option::is_none"
    )]
    pub unlock_at: Option<OffsetDateTime>,
    /// The games ben picked when he wrapped this link — `None` = open shelf (the
    /// whole listable catalog, every pre-field record). CREATE-TIME-ONLY, like
    /// `unlock_at` (spec 2026-08-19 §1): no edit path exists or may be added
    /// without its own spec. ORDER IS MEANING: pick order = presentation order.
    /// Storage: top-level dynamo `L` attribute, NEVER the body blob — the claim
    /// gate reads this, making it an enforcement field (dynamo doctrine, its
    /// lib.rs "body for immutable identity, top-level attrs for enforcement"),
    /// and a body-carried copy would be erased by a pre-field binary's `SET
    /// body = :b` write-back on rollback. See the rollback pin in store_test.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub curated_game_ids: Option<Vec<String>>,
    /// The friend this link was cut for. Authoritative ONLY in a top-level
    /// dynamo attribute (scoped update via `set_link_friend`), stripped from
    /// the stored `body` blob, overridden on read — the `gift_note` pattern,
    /// MECHANICALLY REQUIRED here: the claim tx writes `SET body = :b` from a
    /// pre-transaction read, so a body-only field is silently reverted by the
    /// friend claiming — while gsi3pk survives, desyncing index from record
    /// (spec, family review; enforced by friend_id_survives_a_claim).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub friend_id: Option<String>,
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

    /// Why this claim terminally failed (humble's refusal text or matched code), written
    /// by the fail transaction in the same write that flips `state` to
    /// [`ClaimState::Failed`]. Durable on purpose: pings scroll away and log groups
    /// have retention; the claim record is the truth a future admin surface reads.
    /// `default` keeps every pre-existing CLAIM item wire-valid.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure_reason: Option<String>,
}

/// One whisper-log row: "the attic spoke in this SLOT, about this game" (the attic whispers,
/// 2026-08-28). The slot is an ISO week (`2026-W35`), not a date — the schedule speaks
/// America/New_York and a UTC date key would let a retry that crosses UTC midnight mint a fresh
/// key and double-send; the ISO week is stable across the whole weekend in either zone.
/// ⚠️ NAMED COUPLING: the slot grain equals the cadence grain (weekly). A sub-weekly schedule
/// must change the slot derivation in the same commit, or ticks collide silently.
///
/// `delivered` is the load-bearing bit: **an undelivered row is a failure receipt, never an
/// exclusion** — selection excludes only `delivered == true` rows, so a treasure whose whisper
/// failed to send stays eligible for the next slot instead of being silently burned.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WhisperRecord {
    pub slot: String,
    pub game_id: String,
    pub cycle: u32,
    pub delivered: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ClaimRefusal {
    #[error("link revoked")]
    Revoked,
    /// Wrapped gift, pre-unlock: `unlock_at > now`. Outranked only by Revoked; outranks
    /// Expired/Exhausted (a sealed link reports sealed whatever else is wrong with it).
    #[error("link sealed")]
    Sealed,
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
        // Sealed iff strictly before the unlock moment: `unlock_at == now` is OPEN,
        // matching expires_at's `<=`-dead edge (and the storage conditions' complement
        // pair — edit `> :now`, claim `<= :now`).
        if let Some(unlock) = self.unlock_at
            && unlock > now
        {
            return Err(ClaimRefusal::Sealed);
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

/// Choice-tpk machine-name grammar (spec D3): `<offered>[_row|_ww]_(choice|monthly)_<platform>`
/// where platform is one-or-more `[a-z0-9]`. Two era infixes carry the SAME claimed-tpk shape:
/// modern Humble Choice uses `_choice_` (enumerated from prod 2026-07-31, 175 rows); the Dec-2019
/// TRANSITION-era Choice — when the sub was mid-rebrand from "Humble Monthly" — uses `_monthly_`
/// (e.g. `ancestorslegacy_monthly_steam` is the Dec-2019 Choice claim of offered `ancestorslegacy`,
/// confirmed against real prod tpks 2026-08-03, #96). Excluding `_monthly_` was the #96 under-match.
/// Returns `Some((exact_base, region_stripped))` for a choice-shaped name — `exact_base`
/// keeps a `_row`/`_ww` region token (an offered name may itself end that way; D7's
/// candidate ladder tries exact first), `region_stripped` is `Some` only when a region
/// token existed. `None` = not choice-shaped (bundle keys, month-product slugs, bare names).
pub fn choice_tpk_bases(tpk_machine_name: &str) -> Option<(String, Option<String>)> {
    // Try the modern `_choice_` infix first, then the Dec-2019 transition-era `_monthly_` (#96).
    // Note `_monthly_` requires a trailing platform, so a month-PRODUCT slug like
    // `november_2019_monthly` (no `_<platform>` suffix) still returns None — only game tpks match.
    let (base, platform) = tpk_machine_name
        .rsplit_once("_choice_")
        .or_else(|| tpk_machine_name.rsplit_once("_monthly_"))?;
    if base.is_empty()
        || platform.is_empty()
        || !platform
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
    {
        return None;
    }
    let region_stripped = ["_row", "_ww"]
        .iter()
        .find_map(|r| base.strip_suffix(r))
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    Some((base.to_string(), region_stripped))
}

/// Spec D3's grammar rung: does this tpk record the claim of this offered game?
/// Bare equality is defensive cover for a claim-all mint that drops the suffix.
pub fn choice_tpk_matches(tpk_machine_name: &str, offered_machine_name: &str) -> bool {
    if tpk_machine_name == offered_machine_name {
        return true;
    }
    match choice_tpk_bases(tpk_machine_name) {
        Some((exact, stripped)) => {
            exact == offered_machine_name || stripped.as_deref() == Some(offered_machine_name)
        }
        None => false,
    }
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

/// Merge rule for `steam_app_id` + `appid_source`. Precedence is **Manual > Humble > Title**, with
/// fresh preferred on equal source (a sync refresh). The one non-obvious rung is that Humble is
/// authoritative over Title: a fresh Title-sourced id must NOT overwrite an existing Humble one.
///
/// Precedence (highest wins):
/// 1. `existing == Manual` → keep existing's pair unconditionally (admin override, untouchable).
/// 2. `existing == Humble` AND `fresh == Title` → keep existing (never downgrade an authoritative
///    Humble id to a guessed Title one — #47). Latent today (the sync walk only ever produces
///    Humble/None), but the invariant is now mechanical, not conventional.
/// 3. `fresh.steam_app_id.is_some()` → take fresh's pair (refresh; a Humble id upgrades a Title one;
///    a new id fills a None).
/// 4. else → keep existing's pair (fresh has no id; don't clear an existing one).
fn merge_appid(existing: &Game, fresh: &Game) -> (Option<u32>, Option<AppidSource>) {
    if existing.appid_source == Some(AppidSource::Manual) {
        // Admin override — untouchable
        (existing.steam_app_id, existing.appid_source)
    } else if existing.appid_source == Some(AppidSource::Humble)
        && fresh.appid_source == Some(AppidSource::Title)
    {
        // Humble outranks Title — never downgrade an authoritative id to a guessed one (#47).
        (existing.steam_app_id, existing.appid_source)
    } else if fresh.steam_app_id.is_some() {
        // Fresh has an id: take it (Humble upgrades a stale Title; new id fills a None)
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
                        hidden_source: existing_game.hidden_source,
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
                        hidden_source: existing_game.hidden_source,
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
            gift_note: None,
            thank_note: None,
            thanked_at: None,
            claims_allowed: 2,
            claims_used: 0,
            revoked: false,
            expires_at: None,
            unlock_at: None,
            curated_game_ids: None,
            friend_id: None,
            created_at: datetime!(2026-07-02 00:00 UTC),
        }
    }

    #[test]
    fn friend_serde_round_trips() {
        let f = Friend {
            id: "f1".into(),
            name: "sarah".into(),
            shelf_token: "ab".repeat(32),
            created_at: time::macros::datetime!(2026-09-04 12:00 UTC),
        };
        let s = serde_json::to_string(&f).unwrap();
        let back: Friend = serde_json::from_str(&s).unwrap();
        assert_eq!(back.id, f.id);
        assert_eq!(back.shelf_token, f.shelf_token);
        assert_eq!(back.name, f.name);
        // created_at is the ONLY field with custom serde (rfc3339) — the round-trip is meaningless
        // without asserting it (M3).
        assert_eq!(back.created_at, f.created_at);
    }

    #[test]
    fn link_friend_id_defaults_none_on_missing() {
        // A pre-field stored record must deserialize (the zero-migration guarantee).
        let json = serde_json::to_string(&link()).unwrap(); // `fn link()` — the existing fixture above
        let stripped: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert!(stripped.get("friend_id").is_none(), "None must not serialize");
        let back: Link = serde_json::from_str(&json).unwrap();
        assert_eq!(back.friend_id, None);
    }

    #[test]
    fn can_claim_sealed_before_unlock() {
        let mut l = link();
        let now = datetime!(2026-07-02 12:00 UTC);
        l.unlock_at = Some(now + time::Duration::seconds(1));
        assert_eq!(l.can_claim(now), Err(ClaimRefusal::Sealed));
    }

    #[test]
    fn can_claim_open_at_exact_unlock_instant() {
        // unlock_at == now ⇒ OPEN (strict >, matching expires_at's <=-dead edge).
        let mut l = link();
        let now = datetime!(2026-07-02 12:00 UTC);
        l.unlock_at = Some(now);
        assert!(l.can_claim(now).is_ok());
    }

    #[test]
    fn can_claim_revoked_outranks_sealed() {
        let mut l = link();
        let now = datetime!(2026-07-02 12:00 UTC);
        l.revoked = true;
        l.unlock_at = Some(now + time::Duration::hours(1));
        assert_eq!(l.can_claim(now), Err(ClaimRefusal::Revoked));
    }

    #[test]
    fn can_claim_sealed_outranks_expired_and_exhausted() {
        // Unreachable via admin validation (unlock must precede expiry) but the ordering
        // is still pinned: a sealed link reports sealed, whatever else is wrong with it.
        let mut l = link();
        let now = datetime!(2026-07-02 12:00 UTC);
        l.unlock_at = Some(now + time::Duration::hours(1));
        l.expires_at = Some(now - time::Duration::hours(1));
        l.claims_used = l.claims_allowed;
        assert_eq!(l.can_claim(now), Err(ClaimRefusal::Sealed));
    }

    #[test]
    fn link_unlock_at_missing_absent_and_skipped() {
        // Pre-feature record: no unlock_at key at all → None (the `default` half).
        let mut json = serde_json::to_value(link()).unwrap();
        json.as_object_mut().unwrap().remove("unlock_at");
        let l: Link = serde_json::from_value(json).unwrap();
        assert_eq!(l.unlock_at, None, "missing unlock_at must default to None");

        // None must serialize to NO key at all (the skip half — gate B1: the stripped
        // body blob depends on absence, not null-ness).
        let s = serde_json::to_string(&link()).unwrap();
        assert!(
            !s.contains("unlock_at"),
            "unlock_at: None must not serialize even a null key: {s}"
        );

        // Set value round-trips.
        let mut l2 = link();
        l2.unlock_at = Some(datetime!(2026-12-25 05:00 UTC));
        let back: Link = serde_json::from_str(&serde_json::to_string(&l2).unwrap()).unwrap();
        assert_eq!(back, l2);
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
            hidden_source: None,
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
    fn link_thanks_fields_missing_is_none_and_roundtrips() {
        // Every stored link predates the thanks feature: JSON without either field
        // must deserialize (thank_note=None, thanked_at=None), not error. A fresh
        // serialization already omits both keys (skip_serializing_if, asserted
        // below), so it IS the legacy shape — no key-stripping needed.
        let json = serde_json::to_value(link()).unwrap();
        assert!(json.get("thank_note").is_none());
        assert!(json.get("thanked_at").is_none());
        let l: Link = serde_json::from_value(json).unwrap();
        assert_eq!(l.thank_note, None);
        assert_eq!(l.thanked_at, None);

        // Set values roundtrip.
        let mut thanked = link();
        thanked.thank_note = Some("omg thank you!!".into());
        thanked.thanked_at = Some(datetime!(2026-07-15 12:00 UTC));
        let back: Link = serde_json::from_str(&serde_json::to_string(&thanked).unwrap()).unwrap();
        assert_eq!(back, thanked);
    }

    #[test]
    fn game_id_shape() {
        assert_eq!(game_id("abc", "def_tpk"), "abc:def_tpk");
    }

    #[test]
    fn choice_tpk_bases_grammar() {
        // plain platform suffix
        assert_eq!(
            choice_tpk_bases("wingspan_choice_steam"),
            Some(("wingspan".into(), None))
        );
        // region token before _choice
        assert_eq!(
            choice_tpk_bases("mylittleuniverse_row_choice_steam"),
            Some((
                "mylittleuniverse_row".into(),
                Some("mylittleuniverse".into())
            ))
        );
        assert_eq!(
            choice_tpk_bases("beholder2_ww_choice_steam"),
            Some(("beholder2_ww".into(), Some("beholder2".into())))
        );
        // platform is open: gog / origin / battlenet / future
        assert_eq!(
            choice_tpk_bases("diabloiv_choice_battlenet"),
            Some(("diabloiv".into(), None))
        );
        assert_eq!(
            choice_tpk_bases("somegame_choice_gog"),
            Some(("somegame".into(), None))
        );
        // multi-word machine names keep their own underscores
        assert_eq!(
            choice_tpk_bases("citizensleeper2_starwardvector_choice_steam"),
            Some(("citizensleeper2_starwardvector".into(), None))
        );
        // Dec-2019 transition-era Choice: `_monthly_` infix, SAME shape as `_choice_` (#96).
        // Real prod tpks (2026-08-03): offered `ancestorslegacy` was claimed as
        // `ancestorslegacy_monthly_steam`; `shadowofthetombraider` as `..._row_monthly_steam`.
        assert_eq!(
            choice_tpk_bases("ancestorslegacy_monthly_steam"),
            Some(("ancestorslegacy".into(), None))
        );
        assert_eq!(
            choice_tpk_bases("shadowofthetombraider_row_monthly_steam"),
            Some((
                "shadowofthetombraider_row".into(),
                Some("shadowofthetombraider".into())
            ))
        );
        // NOT choice-shaped: bare names, empty platform, and a month-PRODUCT slug (trailing
        // `_monthly` with NO `_<platform>` suffix — the era-stop's object, not a game tpk).
        assert_eq!(choice_tpk_bases("november_2019_monthly"), None);
        assert_eq!(choice_tpk_bases("wingspan"), None);
        assert_eq!(choice_tpk_bases("wingspan_choice_"), None);
        assert_eq!(choice_tpk_bases("wingspan_monthly_"), None);
        // fires-anyway for the platform CHARSET rung (M11 minor): uppercase must NOT parse.
        assert_eq!(choice_tpk_bases("wingspan_choice_Steam"), None);
    }

    #[test]
    fn choice_tpk_matches_is_the_grammar_rung() {
        // strip-grammar equality (the _row pair that killed starts_with)
        assert!(choice_tpk_matches(
            "mylittleuniverse_row_choice_steam",
            "mylittleuniverse"
        ));
        // exact-base match: an offered name that itself ends _row
        assert!(choice_tpk_matches(
            "mylittleuniverse_row_choice_steam",
            "mylittleuniverse_row"
        ));
        assert!(choice_tpk_matches("wingspan_choice_steam", "wingspan"));
        // bare equality (defensive: claim-all mints may drop the suffix)
        assert!(choice_tpk_matches("wingspan", "wingspan"));
        // Dec-2019 transition-era `_monthly_` tpk matches its offered game, same as `_choice_` (#96).
        assert!(choice_tpk_matches(
            "ancestorslegacy_monthly_steam",
            "ancestorslegacy"
        ));
        // region-stripped monthly match: offered bare, claimed carries `_row`.
        assert!(choice_tpk_matches(
            "shadowofthetombraider_row_monthly_steam",
            "shadowofthetombraider"
        ));
        // non-matches: different game, prefix-hazard neighbor (must still hold under the new infix)
        assert!(!choice_tpk_matches("wingspan_choice_steam", "wing"));
        assert!(!choice_tpk_matches(
            "atomicheart_row_choice_steam",
            "atomic"
        ));
        assert!(!choice_tpk_matches(
            "ancestorslegacy_monthly_steam",
            "ancestors"
        ));
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
            hidden_source: None,
        }
    }

    #[test]
    fn merge_new_game_is_fresh() {
        assert_eq!(merge_sync(None, fresh_game()), Some(fresh_game()));
    }

    #[test]
    fn merge_appid_precedence_manual_over_humble_over_title() {
        let with = |id: Option<u32>, src: Option<AppidSource>| Game {
            steam_app_id: id,
            appid_source: src,
            ..fresh_game()
        };
        // #47 regression: a fresh Title id must NOT downgrade an authoritative Humble id.
        assert_eq!(
            merge_appid(
                &with(Some(100), Some(AppidSource::Humble)),
                &with(Some(200), Some(AppidSource::Title)),
            ),
            (Some(100), Some(AppidSource::Humble)),
            "Humble outranks a fresh Title — never downgrade"
        );
        // ...but a fresh Humble id DOES upgrade a stale Title id.
        assert_eq!(
            merge_appid(
                &with(Some(100), Some(AppidSource::Title)),
                &with(Some(200), Some(AppidSource::Humble)),
            ),
            (Some(200), Some(AppidSource::Humble)),
            "Humble upgrades a stale Title"
        );
        // Equal source (a normal refresh) takes fresh.
        assert_eq!(
            merge_appid(
                &with(Some(100), Some(AppidSource::Humble)),
                &with(Some(200), Some(AppidSource::Humble)),
            ),
            (Some(200), Some(AppidSource::Humble)),
            "equal source refreshes to fresh"
        );
        // Manual is untouchable, even by a fresh Humble id.
        assert_eq!(
            merge_appid(
                &with(Some(100), Some(AppidSource::Manual)),
                &with(Some(200), Some(AppidSource::Humble)),
            ),
            (Some(100), Some(AppidSource::Manual)),
            "Manual override is untouchable"
        );
    }

    #[test]
    fn hidden_source_serde_is_snake_case() {
        assert_eq!(
            serde_json::to_string(&HiddenSource::Sync).unwrap(),
            r#""sync""#
        );
        assert_eq!(
            serde_json::to_string(&HiddenSource::Admin).unwrap(),
            r#""admin""#
        );
        // as_wire must track serde exactly — conditions compare against it (#71).
        for src in [HiddenSource::Admin, HiddenSource::Sync] {
            assert_eq!(
                serde_json::to_value(src).unwrap().as_str().unwrap(),
                src.as_wire()
            );
        }
    }

    #[test]
    fn wire_strings_track_serde_for_all_mirrored_enums() {
        // Every enum mirrored to a top-level DDB attribute keeps as_wire == serde output,
        // or condition expressions comparing against the mirror go silently void.
        for s in [
            GameStatus::Available,
            GameStatus::Pending,
            GameStatus::Gifted,
            GameStatus::BenRedeemed,
            GameStatus::Expired,
        ] {
            assert_eq!(
                serde_json::to_value(s).unwrap().as_str().unwrap(),
                s.as_wire()
            );
        }
        for s in [AppidSource::Title, AppidSource::Humble, AppidSource::Manual] {
            assert_eq!(
                serde_json::to_value(s).unwrap().as_str().unwrap(),
                s.as_wire()
            );
        }
    }

    #[test]
    fn game_blob_backcompat_hidden_source_defaults_none() {
        // Serialize a pre-field game shape by stripping the key from a current one.
        let mut v = serde_json::to_value(fresh_game()).unwrap();
        v.as_object_mut().unwrap().remove("hidden_source");
        let g: Game = serde_json::from_value(v).unwrap();
        assert_eq!(g.hidden_source, None);
    }

    #[test]
    fn merge_sync_carries_hidden_source_both_branches() {
        // Humble-owned branch (Available). fresh MUST differ on a refreshed field
        // (title) — if hidden/hidden_source were the only diffs, a correct merge
        // carries both from existing, merged == existing, and merge_sync returns
        // None (the no-op contract). The assertions below then prove carry-over
        // happened on a merge that actually wrote.
        let mut existing = fresh_game();
        existing.hidden = true;
        existing.hidden_source = Some(HiddenSource::Sync);
        let mut fresh = fresh_game();
        fresh.title = "renamed".into();
        let merged = merge_sync(Some(&existing), fresh).expect("title differs → Some");
        assert_eq!(merged.hidden_source, Some(HiddenSource::Sync));
        assert!(merged.hidden);

        // App-owned branch (Gifted)
        let mut existing = fresh_game();
        existing.status = GameStatus::Gifted;
        existing.hidden_source = Some(HiddenSource::Admin);
        let mut fresh = fresh_game();
        fresh.title = "renamed".into();
        let merged = merge_sync(Some(&existing), fresh).expect("title differs → Some");
        assert_eq!(merged.hidden_source, Some(HiddenSource::Admin));
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
            failure_reason: None,
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
            failure_reason: None,
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

    #[test]
    fn claim_state_failed_wire_value_and_reason_roundtrip() {
        // Wire value is load-bearing for web + admin rendering: exactly "failed".
        assert_eq!(
            serde_json::to_string(&ClaimState::Failed).unwrap(),
            "\"failed\""
        );
        // failure_reason must be absent-tolerant (every pre-existing claim item) and
        // round-trip when present.
        let json = r#"{"id":"c1","link_token":"t","game_id":"g:m","state":"failed",
            "gift_url":null,"created_at":"2026-07-09T21:38:28Z",
            "failure_reason":"This key has expired and can no longer be redeemed."}"#;
        let c: Claim = serde_json::from_str(json).unwrap();
        assert_eq!(c.state, ClaimState::Failed);
        assert_eq!(
            c.failure_reason.as_deref(),
            Some("This key has expired and can no longer be redeemed.")
        );
        // Absent field ⇒ None (pre-existing items stay wire-valid).
        let json_old = r#"{"id":"c1","link_token":"t","game_id":"g:m","state":"pending",
            "gift_url":null,"created_at":"2026-07-09T21:38:28Z"}"#;
        let c_old: Claim = serde_json::from_str(json_old).unwrap();
        assert_eq!(c_old.failure_reason, None);
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
