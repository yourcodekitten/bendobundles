//! Fulfillment: the safety-critical heart of bendobundles.
//!
//! **Invariant:** a humble key burns exactly once, and a burned key's gift URL is never lost.
//!
//! The gift ladder decides every arm from a single humble outcome. Policy is split from side
//! effects on purpose: [`gift_decision`] is a *pure*, exhaustively-tested function that maps a
//! humble outcome to a [`Decision`]; [`handle`] executes that decision against the store + webhook.
//! Because the `HumbleError` match in `gift_decision` has NO catch-all `_` arm, a future error
//! variant is a compile error until someone consciously picks its decision — the invariant can't
//! silently rot.

/// Pure gate logic for the `heal_choice_pairs` operator sweep (spec Q5). Non-gated so the normal
/// test suite runs it; only the `delete_game` call in the bin is `heal`-feature-gated.
pub mod heal_pairs;
pub mod operator_message;

use crate::operator_message::{ErrorSummary, OperatorMessage, Part};
use domain::{AppidSource, Claim, Game, GameStatus};
use dynamo::{OwnedWrite, Store, StoreError, SyncBegin, SyncState, SyncWrite};
use humble_client::{
    GiftUrl, HumbleClient, HumbleError, KeyEntry, OfferedGame, Order, RevealedKey,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use time::OffsetDateTime;

/// A parked (`Pending`) claim younger than this is left alone — the live fulfillment call may
/// still be in flight, and reconciling it would race a redeem that is about to record its URL.
/// Only claims older than this are re-checked against humble's truth.
const RECONCILE_MIN_AGE: time::Duration = time::Duration::minutes(15);

/// Pacing between per-order humble fetches during sync — same jitter-free floor as the probe, to
/// stay under humble's bot-detection radar.
const SYNC_PACE: std::time::Duration = std::time::Duration::from_millis(300);

/// Politeness floor between EVERY Steam storefront call in the enrichment pass (Ben's be-nice
/// rule). The storefront endpoints are hit ONLY inside [`enrich_steam_apps`], never at request time.
/// This is the prod default for `Deps::steam_enrich_pace`.
pub const STEAM_ENRICH_PACE: std::time::Duration = std::time::Duration::from_millis(1500);

/// Per-sync cap on how many distinct appids the enrichment pass fetches. Everything past this is
/// deferred to the next sync — logged, never silently truncated.
const STEAM_ENRICH_MAX_APPS: usize = 75;

/// The enrichment pass stops STARTING new apps once fewer than this much of the lambda budget
/// remains, so `persist_sync` + `end_sync_run` always get to land after it.
const STEAM_ENRICH_DEADLINE_MARGIN: std::time::Duration = std::time::Duration::from_secs(180);

/// The fulfillment lambda's hard timeout — FALLBACK used by [`compute_enrich_deadline`] when no
/// lambda context deadline is available (local runs, tests that inject zero). In the lambda env the
/// real per-invoke remaining time is preferred; this const exists so a terraform-timeout change
/// doesn't silently mis-budget when the context deadline is absent.
const SYNC_LAMBDA_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(900);

/// Compute how long from now until the enrichment deadline, given the lambda context's per-invoke
/// deadline and the current wall-clock epoch (both in milliseconds).
///
/// - If `context_deadline_epoch_ms` is 0 (absent — local runs, tests), falls back to
///   `SYNC_LAMBDA_TIMEOUT - STEAM_ENRICH_DEADLINE_MARGIN`.
/// - Otherwise computes remaining time from the context deadline and subtracts the margin.
///   Saturating on both steps: if the deadline is already past, or if remaining ≤ margin,
///   returns `Duration::ZERO` (immediate deadline — skip the pass, protect bookkeeping).
pub fn compute_enrich_deadline(
    context_deadline_epoch_ms: u64,
    now_epoch_ms: u64,
) -> std::time::Duration {
    if context_deadline_epoch_ms == 0 {
        return SYNC_LAMBDA_TIMEOUT - STEAM_ENRICH_DEADLINE_MARGIN;
    }
    let remaining =
        std::time::Duration::from_millis(context_deadline_epoch_ms.saturating_sub(now_epoch_ms));
    remaining.saturating_sub(STEAM_ENRICH_DEADLINE_MARGIN)
}

/// appdetails refresh window (30 days) measured on `SteamAppCache::fetched_at`.
const STEAM_DETAIL_TTL_SECS: i64 = 30 * 24 * 60 * 60;

/// reviews+histogram refresh window (14 days) measured on `SteamAppCache::reviews_fetched_at`.
const STEAM_REVIEWS_TTL_SECS: i64 = 14 * 24 * 60 * 60;

/// How many pages of the Choice-months list walk the discovery pass enumerates. The walk is
/// ~26 pages for the full membership history (3 months/page) and self-terminates early with
/// `complete = true` once it runs out, so this is a ceiling, not a target. It has to reach back far
/// enough to catch an *old* month whose pick is still unspent (Humble keeps a choice redeemable
/// until it's spent), so it covers the whole history; the expensive per-month reads are still gated
/// to the handful of live months (`uses_choices && can_redeem_games`).
// Runaway guard only (era-stop is the normal terminal): ~120 months covers the full Choice era
// plus years of margin. Hitting it means era-stop never fired — itself a signal (it warns).
const CHOICE_DISCOVERY_MAX_PAGES: usize = 40;
// Whole-list-walk deadline: bounds the paginated GETs even if the server hands back cursors
// forever slowly. The per-month detail fan-out has its own deadline (Task 7).
const CHOICE_WALK_DEADLINE: std::time::Duration = std::time::Duration::from_secs(120);

/// How many of the newest months discovery probes DIRECTLY by constructed slug (current month + the
/// preceding N-1), independent of the subscription list. The `subscription_products_with_gamekeys`
/// list omits the 1-2 newest months (the current + just-billed one), which is exactly where an
/// unspent pick lives — so we build their slugs from the wall clock and read each membership page.
/// A small window with margin; each probe is one paced GET, deduped against the walk's slugs.
const CHOICE_DISCOVERY_RECENT_PROBE: usize = 4;

/// A parked claim that reconcile structurally CANNOT act on — a `game_id` with no
/// `gamekey:machine_name` split, or a machine_name that never appears in its order's keys on
/// humble — would otherwise be skipped silently on every pass, forever: the friend stays stuck on
/// "processing", the link slot stays consumed, and no operator ever hears about it. That violates
/// this crate's "stop loudly, never skip silently" principle, so once such a claim is older than
/// this threshold the skip turns loud: `warn!` plus a discord ping, once per claim per reconcile
/// pass (the same bounded cadence as the redeemed-but-unrecorded arm — sync runs on a schedule,
/// so ping volume is capped by that schedule). Younger than this, the skip stays log-only: the
/// mismatch may be a mid-deploy artifact or an order shape the very next sync corrects.
const RECONCILE_STUCK_ALERT_AGE: time::Duration = time::Duration::hours(24);

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum FulfillRequest {
    Gift {
        claim_id: String,
        link_token: String,
        game_id: String,
        gamekey: String,
        /// Bundle game: the key's tpk machine_name. Choice game (`requires_choice=true`): the
        /// OFFERED game's id, the identifier fed to `choosecontent` — there is no tpk yet.
        machine_name: String,
        /// Bundle game: the tpk's key index. Meaningless on a choice game (no key yet) — ignored on
        /// that path (the real keyindex is read off the post-choose order's new tpk).
        keyindex: u32,
        /// `true` ⇒ dispatch the two-write Choice orchestration (spend a monthly pick via
        /// `choosecontent`, THEN redeem the freshly-minted key). `#[serde(default)]` keeps every
        /// existing (bundle) Gift payload wire-valid — absent reads back `false`.
        #[serde(default)]
        requires_choice: bool,
    },
    /// Admin self-claim: reveal the key VALUE to Ben (no gift URL). Mirrors `Gift`'s field
    /// semantics: on `requires_choice=true`, `machine_name` is the OFFERED id and `keyindex` is
    /// ignored (read off the post-choose order).
    SelfClaim {
        claim_id: String,
        game_id: String,
        gamekey: String,
        /// Bundle game: the key's tpk machine_name.
        machine_name: String,
        /// Bundle game: the tpk's key index. Meaningless on a choice game.
        keyindex: u32,
        #[serde(default)]
        requires_choice: bool,
    },
    Sync,
    /// MANUAL-INVOKE-ONLY diagnostic since the cookie-paste teardown. Its only in-app sender was
    /// admin-api's paste-validate (removed with the paste flow); EventBridge fires `Sync`, which
    /// already self-heals + reports `cookie_ok` on cadence. Reach this by a hand-run
    /// `aws lambda invoke '{"op":"validate_cookie"}'` — kept as a break-glass probe, NOT a
    /// scheduled healthcheck. (A dedicated EventBridge validate schedule for a cheap mid-day heal
    /// is a tracked follow-up, deliberately out of this teardown.)
    ValidateCookie,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "result", rename_all = "snake_case")]
pub enum FulfillResponse {
    GiftUrl {
        url: String,
    },
    /// Self-claim success: the revealed key VALUE. Serialized only on the admin-api wire —
    /// never logged, never in a friend response.
    RevealedKey {
        key: String,
    },
    /// definitive: key was already redeemed; claim compensated; friend should pick another
    AlreadyRedeemed,
    /// Terminal dead key: the claim was failed (reason persisted), the friend's slot
    /// returned, the game retired as expired. public-api maps this to 410 with its own
    /// friend-honest message — the AlreadyRedeemed pattern, different words.
    KeyDead,
    /// ambiguous or refused: claim stays PENDING for reconcile; friend told "processing"
    Parked {
        reason: String,
    },
    /// Sync ran (or was skipped because another run holds the sync-run marker). Fieldless on
    /// purpose: sync is only ever invoked async (`Event`), whose return payload Lambda discards —
    /// the run's real results live in the persisted `SyncState`, not on the wire.
    SyncDone,
    CookieStatus {
        ok: bool,
    },
    Error {
        message: String,
    },
}

/// The pure gift-ladder decision. Compensate ONLY on definitive `AlreadyRedeemed`; park on
/// EVERYTHING ambiguous; `Unauthorized` is its own arm (park + flag cookie + ping). No `_` arm on
/// `HumbleError` — a new variant must be classified here before the crate compiles.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    /// Ok: gift URL exists — record it durably, then flip the game.
    Record,
    /// Definitively burned already — return the slot, re-list the game.
    Compensate,
    /// Dead session cookie — park, flag `cookie_ok=false`, ping ben.
    ParkCookieDead,
    /// Ambiguous or refused — park pending for reconcile; NEVER compensate blind.
    Park,
    /// The key is definitively DEAD (humble: expired, unredeemable forever). Terminal:
    /// fail the claim with its reason, return the slot, retire the game as Expired.
    /// NEVER park (retry cannot succeed) and NEVER compensate (re-listing would hand
    /// the next friend the same dead key).
    DeadKey,
}

/// The Err-arm classification shared by [`gift_decision`] and [`reveal_decision`]. Extracted so
/// the two decision functions can never drift — a new `HumbleError` variant is a compile error
/// in this one place, not two. No `_` catch-all arm.
fn gift_error_decision(err: &HumbleError) -> Decision {
    match err {
        // The ONE definitive "key is gone" signal from humble → safe to compensate.
        HumbleError::AlreadyRedeemed => Decision::Compensate,
        // Dead cookie: park + flag + ping (handled in the ParkCookieDead executor). Only the
        // 200-with-HTML login interstitial maps here now — the one redeem response shape
        // that positively identifies a stale session.
        HumbleError::Unauthorized => Decision::ParkCookieDead,
        // Auth/CSRF-layer rejection of the WRITE. The cookie may be perfectly healthy (live
        // 2026-07-04 capture: redeem 403 while sync read the whole library) — reads own the
        // cookie-health signal, so park WITHOUT flipping cookie_ok or pinging cookie-death.
        // (The Park executor still pings for this variant — a distinct, correctly-labeled
        // alert — because otherwise a persistent rejection loops silently: park → daily
        // reconcile compensates → re-list → re-claim → reject again, with no operator signal.)
        HumbleError::RedeemAuthRejected { .. } => Decision::Park,
        // Secure-area step-up never completed (bad password/TOTP, locked account, or humble
        // still gating). A gated redeem returns `login_required` BEFORE touching the key, so
        // the key is not burned — park, never compensate. The Park executor pings a distinct,
        // correctly-labeled alert so a persistent step-up failure doesn't loop silently.
        HumbleError::SecureAreaStepUpFailed { .. } => Decision::Park,
        // login() is the session self-heal path, never a redeem outcome — but the match is
        // exhaustive, so classify it: a login failure means no session, so park (never burn).
        HumbleError::LoginFailed { .. } => Decision::Park,
        // choose_content (the Choice pick-spend) is handled BEFORE the redeem in the Choice
        // orchestration, so this never actually reaches a redeem decision — but the match is
        // exhaustive. A ChooseFailed provably spent no pick, so park (never compensate).
        HumbleError::ChooseFailed { .. } => Decision::Park,
        // Everything else is ambiguous-or-refused. The key MAY have burned (or may not have);
        // only reconcile against humble truth can tell. Park — never compensate blind.
        HumbleError::RedeemRefused { .. } => Decision::Park,
        // Definitively dead server-side — terminal, not park: retrying a key humble
        // has expired loops forever (live receipt: claim 87b9a4d8, 21 silent days).
        HumbleError::KeyExpired { .. } => Decision::DeadKey,
        HumbleError::AmbiguousRedeem => Decision::Park,
        HumbleError::RateLimited => Decision::Park,
        HumbleError::Api(_) => Decision::Park,
        HumbleError::Network(_) => Decision::Park,
        HumbleError::Parse(_) => Decision::Park,
        // #160: only `order()` constructs this — a redeem-write never can — but the match is
        // exhaustive by design. Park is the only safe classification anyway: a missing
        // `tpkd_dict` means humble's key truth is UNREADABLE, and unreadable is the one thing
        // that must never be mistaken for the definite "key is gone" that Compensate requires.
        HumbleError::TpkdDictAbsent(_) => Decision::Park,
    }
}

/// Map a humble redeem outcome to a [`Decision`]. Pure: no I/O, no panics, exhaustive.
pub fn gift_decision(outcome: &Result<GiftUrl, HumbleError>) -> Decision {
    match outcome {
        Ok(_) => Decision::Record,
        Err(err) => gift_error_decision(err),
    }
}

/// Reveal ladder decision — [`gift_decision`] typed over the reveal outcome. IDENTICAL
/// classification (the two must never drift); only the Compensate arm's EXECUTION differs at the
/// call site (self-claim recovers the key instead of compensating — spec §4).
pub fn reveal_decision(outcome: &Result<RevealedKey, HumbleError>) -> Decision {
    match outcome {
        Ok(_) => Decision::Record,
        Err(err) => gift_error_decision(err),
    }
}

/// Map a Humble Choice `choosecontent` (pick-spend) outcome to a [`Decision`]. Pure: no I/O, no
/// panics, exhaustive — a sibling of [`gift_decision`] with NO catch-all `_` arm, so a new
/// `HumbleError` variant is a compile error until it's consciously classified.
///
/// The whole double-spend prevention rests on ONE property of this map: **no arm produces
/// `Compensate`.** A `choosecontent` outcome can NEVER prove a pick was NOT spent well enough to
/// justify returning the monthly slot here — the ambiguous outcomes (`Api`/`Network`/`Parse`) may
/// follow a pick humble already committed. Compensation for a choice claim happens ONLY in
/// reconcile, and ONLY where the order diff PROVES no pick was spent (§3 A/B1). So:
/// - `Ok(())` ⇒ pick spent ⇒ `Record` (read as "proceed to the re-read + redeem").
/// - `Unauthorized` ⇒ dead session (200-HTML interstitial, provably no spend) ⇒ `ParkCookieDead`.
/// - everything else ⇒ `Park`. A blind re-choose on the ambiguous ones IS the double-spend bug this
///   design exists to prevent, so they park and let reconcile's diff (not the error) decide.
pub fn choose_decision(outcome: &Result<(), HumbleError>) -> Decision {
    match outcome {
        // Pick spent — proceed to re-read the order and redeem the new key.
        Ok(()) => Decision::Record,
        Err(err) => match err {
            // A dead session answers `choosecontent` with the 200-with-HTML login interstitial
            // (decode_body → Unauthorized) BEFORE the handler runs — provably no pick spent. Same
            // dead-cookie treatment as the gift path.
            HumbleError::Unauthorized => Decision::ParkCookieDead,
            // Step-up gate never cleared: the choose handler runs BEHIND the gate, so no pick was
            // spent. Park (distinct ping in the executor), never compensate.
            HumbleError::SecureAreaStepUpFailed { .. } => Decision::Park,
            // `success=false` / auth-CSRF-layer reject: PROVABLY no pick spent THIS attempt. Still
            // park, never compensate: an earlier duplicate attempt may already have spent the pick
            // ("already chosen"), and only reconcile's order diff can tell. Parking also avoids a
            // silent daily loop on a genuine "no picks left".
            HumbleError::ChooseFailed { .. } => Decision::Park,
            // Rate-limited (429): almost certainly not spent, but unproven → park; reconcile decides.
            HumbleError::RateLimited => Decision::Park,
            // THE dangerous trio — an ambiguous status/transport/parse failure can follow a pick
            // humble already COMMITTED. Pick state is UNKNOWN, so park and let reconcile's diff
            // resolve it; a blind re-choose here would double-spend.
            HumbleError::Api(_) => Decision::Park,
            HumbleError::Network(_) => Decision::Park,
            HumbleError::Parse(_) => Decision::Park,
            // login() is the self-heal path, never a choose outcome — but the match is exhaustive:
            // no session ⇒ no choose ⇒ park.
            HumbleError::LoginFailed { .. } => Decision::Park,
            // Not constructible from a `choosecontent` call (these are redeem-write outcomes), but
            // classified for exhaustiveness. None of them may compensate a choice claim.
            HumbleError::AlreadyRedeemed => Decision::Park,
            HumbleError::RedeemAuthRejected { .. } => Decision::Park,
            HumbleError::RedeemRefused { .. } => Decision::Park,
            // choose_content never yields KeyExpired (it spends picks, it doesn't redeem
            // keys) -- classified conservatively as Park; reconcile's order diff decides.
            HumbleError::KeyExpired { .. } => Decision::Park,
            HumbleError::AmbiguousRedeem => Decision::Park,
            // #160: an order-read failure, never a `choosecontent` outcome — classified for
            // exhaustiveness. Park regardless: unreadable key truth resolves by reconcile's
            // diff on a later sync, never by acting on the gap.
            HumbleError::TpkdDictAbsent(_) => Decision::Park,
        },
    }
}

/// The outcome of diffing a post-`choosecontent` order against the pre-choose snapshot: which new
/// tpk (if any) is the key the pick just minted. Pure; produced by [`find_new_tpk`].
#[derive(Debug, PartialEq, Eq)]
pub enum TpkPick<'a> {
    /// Exactly one tpk to burn was identified (either a single new tpk, or an exact-title match
    /// among several) — safe to redeem.
    Unique(&'a KeyEntry),
    /// No new tpk appeared. Either the choose has not committed yet (eventual consistency / a crash
    /// mid-write) or it never spent a pick. NEVER re-choose — reconcile owns the resolution.
    None,
    /// More than one new tpk appeared and the exact title can't single one out (a concurrent
    /// sibling claim on the same month). A human must disambiguate — never guess which key to burn.
    Ambiguous,
}

/// Diff a freshly-read order against the pre-choose snapshot to find the tpk a `choosecontent` just
/// minted. Pure. `new = order.keys \ pre` (by `machine_name`):
/// - exactly one new tpk ⇒ `Unique` (the common happy path: one pick, one new key);
/// - zero new ⇒ `None`;
/// - two-or-more new ⇒ split by an EXACT case-insensitive `human_name == title` match (exactly one
///   match ⇒ `Unique`, else `Ambiguous`). Exact-only: when the output is "which real key to burn",
///   a prefix/fuzzy guess is unacceptable.
pub fn find_new_tpk<'a>(order: &'a Order, pre: &[String], title: &str) -> TpkPick<'a> {
    let pre_set: std::collections::HashSet<&str> = pre.iter().map(String::as_str).collect();
    let new: Vec<&KeyEntry> = order
        .keys
        .iter()
        .filter(|k| !pre_set.contains(k.machine_name.as_str()))
        .collect();
    match new.len() {
        0 => TpkPick::None,
        1 => TpkPick::Unique(new[0]),
        _ => {
            let exact: Vec<&KeyEntry> = new
                .iter()
                .copied()
                .filter(|k| k.human_name.eq_ignore_ascii_case(title))
                .collect();
            if exact.len() == 1 {
                TpkPick::Unique(exact[0])
            } else {
                TpkPick::Ambiguous
            }
        }
    }
}

/// What an SSM secret read actually FOUND.
///
/// `Option<String>` collapsed **four** states into one `None`: parameter absent (terraform's
/// `discord_webhook_enabled = false`), the `UNSET` placeholder, an empty value, and an SSM/KMS/IAM
/// **error**. The fourth is the dangerous one — transient external *weather* recorded as permanent
/// internal *intent*. And because config resolves once per container, a 200ms throttle does not
/// cost 200ms of notifications: it costs every notification that container handles.
///
/// NOTE: lives in the library, not the binary, because both the binary's `get_secret` and the
/// library's `Notify::resolve` need it.
#[derive(Clone, Debug)]
pub enum SecretRead {
    Resolved(String),
    /// Absent, `UNSET`, or empty — all deliberate.
    DeliberatelyOff,
    /// The read FAILED. Throttle, KMS grant, IAM, network. **Not a statement about intent.**
    ReadFailed,
}

/// How this process reaches the operator. Resolved ONCE at init, so a missing webhook is one loud
/// event per cold start instead of twenty silent no-ops a day.
///
/// A use-time `else { return; }` can never distinguish *deliberately off* from *someone dropped
/// the env var* — that collapse IS the defect. Three states keep them apart.
///
/// **INFALLIBLE BY DESIGN.** This runs in the Lambda init phase, and a cold start is *caused by* an
/// invocation, so init and request are the same instant. Halting here fails the order that woke the
/// container, and every order after it. Notification config is OBSERVABILITY, not a safety gate:
/// **fail LOUD, never CLOSED.** Do not change this to return `Result`.
#[derive(Clone, Debug)]
pub enum Notify {
    Webhook(String),
    /// Deliberately off. Silent by request; suppresses the alarm.
    Disabled,
    /// Misconfigured or unreadable. Behaves like `Disabled` at runtime, but is LOUD at init and
    /// distinct in logs — which is the entire point of separating the states.
    Unresolved,
}

impl Notify {
    /// *** `disabled` IS CONSUMED. It used to be a PASSENGER, and that was a real defect. ***
    ///
    /// Every arm here matched `_` on the flag, so `NOTIFY_DISABLED=1` did **nothing** whenever the
    /// SSM read succeeded — the one case an operator actually reaches for it in. The parameter was
    /// accepted, threaded from `main.rs`, named in the startup log line ("absent/UNSET param, **or
    /// NOTIFY_DISABLED=1**"), and relied on by this design's own gate ruling — *"NOTIFY_DISABLED=1
    /// is the one-env-var escape hatch"* — and consumed by nothing. **A flag the code reads, logs
    /// about, and never branches on.**
    ///
    /// That is precisely this change's subject at a third altitude: a value that rides through the
    /// whole path as a passenger and never reaches the decision. It shipped because the test named
    /// for the flag — `explicit_disable_flag_also_yields_disabled` — passed via the
    /// `DeliberatelyOff` arm and would have passed identically with the flag deleted. **A test can
    /// be named for the thing it does not test.**
    ///
    /// ORDER MATTERS AND IS DELIBERATE: the flag is checked FIRST, so it beats a resolved secret.
    /// `NOTIFY_DISABLED=1` is alarm SUPPRESSION — deliberate, operator-initiated silence — so it
    /// also silences `Unresolved`; someone who has asked for quiet should not be paged about the
    /// quiet they asked for. It is NOT a safety valve for fulfilment, which never depends on it.
    /// *** NO WILDCARD OVER EITHER INPUT. ALL SIX CELLS ARE WRITTEN OUT. ***
    ///
    /// My first fix for this was `(_, true) => Disabled` — which is **a wildcard bug fixed with
    /// another wildcard**. `_` is asking the compiler for permission to ignore an input and being
    /// granted it silently; that is exactly how the original defect got in. Enumerated, the
    /// compiler enforces the matrix for me: add a `SecretRead` variant and this **fails to compile**
    /// until someone decides what it means with the flag on and with it off.
    ///
    /// >>> A test asserts the cell you thought of. Exhaustiveness asserts the cells you didn't. <<<
    ///
    /// (`clippy::wildcard_enum_match_arm` would enforce this repo-wide. Not turned on in this PR —
    /// it is a workspace-wide lint change with its own blast radius, and mixing it in here would
    /// make a behaviour fix hostage to a lint sweep. Filed as a follow-up.)
    pub fn resolve(read: SecretRead, disabled: bool) -> Notify {
        match (read, disabled) {
            // Suppression beats a working webhook. *** THIS IS THE CELL THE BUG LIVED IN *** — and
            // it is the one a future reader will be most tempted to collapse back into a wildcard.
            (SecretRead::Resolved(_), true) => Notify::Disabled,
            (SecretRead::Resolved(u), false) => Notify::Webhook(u),
            (SecretRead::DeliberatelyOff, true) => Notify::Disabled,
            (SecretRead::DeliberatelyOff, false) => Notify::Disabled,
            // Suppression is the job: do not page someone about the quiet they asked for.
            (SecretRead::ReadFailed, true) => Notify::Disabled,
            // Weather, never intent.
            (SecretRead::ReadFailed, false) => Notify::Unresolved,
        }
    }
}

/// Everything `handle` needs to do its job. Constructed once by Task 5's lambda main.
pub struct Deps {
    pub store: Store,
    pub humble: HumbleClient,
    pub notify: Notify,
    pub http: reqwest::Client,
    /// SSM client + the humble-cookie parameter name, so the app can self-heal its own session:
    /// on a dead session it logs in (via `humble.login()`) and persists the fresh cookie here,
    /// replacing the human cookie-paste flow. `None` when self-login credentials aren't configured
    /// (then a dead session falls back to the old flag-and-ping behavior).
    pub session_store: Option<SessionStore>,
    /// Steam Web API client, used by the appid mapper pass. `None` when the Steam API key is not
    /// configured; `run_sync` skips the title-pass but still flows tier-1 tpk ids from the walk.
    pub steam: Option<Arc<steam_client::SteamClient>>,
    /// Kill switch for the Steam enrichment pass (Ben's be-nice rule): `STEAM_ENRICH_DISABLED=1`
    /// in the lambda env sets this `true` and [`enrich_steam_apps`] skips entirely. Read via config
    /// here (not raw env) so tests can toggle it without touching process state.
    pub steam_enrich_disabled: bool,
    /// Politeness floor between EVERY Steam storefront call in the enrichment pass. Prod uses
    /// [`STEAM_ENRICH_PACE`] (1.5s); tests inject `Duration::ZERO` so the paced pass runs instantly
    /// against real wiremock I/O (a virtual/paused clock would auto-advance into reqwest's request
    /// timeout while a real HTTP call is in flight).
    pub steam_enrich_pace: std::time::Duration,
    /// Per-invoke deadline for the enrichment pass — computed by the caller from the lambda
    /// context's `deadline` epoch-ms via [`compute_enrich_deadline`]. Tests inject `far_deadline()`
    /// so the deadline never fires during the run; prod sets it from the real lambda context.
    pub steam_enrich_deadline: tokio::time::Instant,
    /// Whole-pass deadline for choice discovery's per-month detail fan-out (spec A5 / OMBB's
    /// arithmetic rider). Bounds the ~77 membership reads in aggregate — the per-request timeout
    /// alone does not. Tests inject a short value to exercise the early-break; prod is 180s.
    pub choice_discovery_deadline: std::time::Duration,
}

/// Where a self-refreshed humble session is persisted, so the next cold start reads it back.
pub struct SessionStore {
    pub ssm: aws_sdk_ssm::Client,
    pub cookie_param: String,
}

/// Outcome of a session self-heal attempt. Split so callers can tell "this invoke can keep
/// working" (the in-memory session is live) apart from "the DURABLE cookie in SSM is healthy" —
/// after a persist failure those disagree, and persisting `cookie_ok=true` on the former would
/// report a healthy cookie while the next invoke reads the dead one back from SSM.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Heal {
    /// Login succeeded AND the fresh cookie is persisted to SSM: fully healthy.
    Persisted,
    /// Login succeeded (this invoke's in-memory session works) but the SSM persist failed —
    /// the durable cookie is still the dead one.
    Unpersisted,
    /// Self-login isn't configured, or the login itself failed.
    Failed,
}

impl Heal {
    /// This invoke holds a working session (durability aside) — safe to retry the failed read.
    fn usable(self) -> bool {
        !matches!(self, Heal::Failed)
    }
    /// The cookie in SSM is known-good — the most persisted `cookie_ok` is allowed to claim.
    fn durable(self) -> bool {
        matches!(self, Heal::Persisted)
    }
}

/// Try to self-heal a dead humble session: log in fresh and persist the new cookie to SSM. Returns
/// a [`Heal`] so callers can distinguish in-memory health from durable (SSM) health. A no-op
/// returning `Heal::Failed` when self-login isn't configured (no credentials / no session store) —
/// callers then keep the old dead-cookie behavior.
///
/// This path never touches a key: a login authenticates the SESSION, it does not redeem, so the
/// burns-once invariant is untouched. Failures are logged and surface as `Failed` (park, never burn).
async fn refresh_session(deps: &Deps) -> Heal {
    let Some(store) = deps.session_store.as_ref() else {
        return Heal::Failed;
    };
    let mut attempt = deps.humble.login().await;
    if let Err(HumbleError::LoginFailed { reason }) = &attempt {
        // A TOTP code may be single-use server-side (RFC 6238 recommends it): a concurrent
        // invoke's heal, or a step-up that just fired, can already have spent this 30s window's
        // code — making this failure a collision, not a credential problem. Retry ONCE in the
        // next window so humble's reuse behavior is moot. Cadence is ~1 heal/day, so the ≤31s
        // stall is cheap; a genuine credential failure just fails again and pings below.
        tracing::warn!(%reason, "self-login failed — retrying once after the TOTP window rolls");
        let elapsed = OffsetDateTime::now_utc().unix_timestamp().rem_euclid(30);
        tokio::time::sleep(std::time::Duration::from_secs((31 - elapsed) as u64)).await;
        attempt = deps.humble.login().await;
    }
    match attempt {
        Ok(new_session) => {
            // Persist so the next invoke's cold start reads a live session instead of re-logging in.
            match store
                .ssm
                .put_parameter()
                .name(&store.cookie_param)
                .value(&new_session)
                .r#type(aws_sdk_ssm::types::ParameterType::SecureString)
                // Pin the terraform-declared Advanced tier. An untiered overwrite would KEEP an
                // existing Advanced tier (AWS can't downgrade a param on overwrite), but pinning
                // also guarantees a >4 KB session lands even if the param were somehow still
                // Standard (fresh env).
                .tier(aws_sdk_ssm::types::ParameterTier::Advanced)
                .overwrite(true)
                .send()
                .await
            {
                Ok(_) => {
                    tracing::info!(
                        "session self-heal: logged in and persisted a fresh humble cookie"
                    );
                    // Ping ONCE per heal so a silently-dying session is still visible. Before
                    // self-login every dead cookie pinged; now a heal is otherwise invisible, and
                    // the operator would lose the early-warning trend (rate-limit / TOTP drift /
                    // an impending new-device challenge) until self-login finally hard-fails.
                    ping_msg(deps, &OperatorMessage::literal(SESSION_HEALED_MSG)).await;
                    Heal::Persisted
                }
                Err(e) => {
                    // The in-memory client already holds the new session (login swapped it in), so
                    // THIS invoke still works; only the persistence failed. But without the write,
                    // every invoke re-reads the dead cookie and re-logs-in (main rebuilds the
                    // client from SSM per invoke) — a silent "login every invoke" that feeds
                    // humble's bot-detection. Ping so it's not buried in CloudWatch.
                    tracing::warn!(error = %e, "session self-heal: logged in but persisting to SSM failed");
                    ping_msg(deps, &OperatorMessage::literal(SESSION_PERSIST_FAILED_MSG)).await;
                    Heal::Unpersisted
                }
            }
        }
        Err(e) => {
            // Surface the failure CLASS in the alert (TOTP drift vs captcha vs new-device each
            // have a different remediation) — otherwise callers ping only the generic
            // COOKIE_DEAD_MSG and the root cause lives buried in CloudWatch while the operator
            // flails blind. LoginFailed reasons carry statuses/labels, never secret values.
            tracing::warn!(error = ?e, "session self-heal: login failed");
            ping_msg(deps, &OperatorMessage::fmt(
    "humble self-login FAILED ({}) — session still dead; break-glass: update the humble-cookie SSM param directly (AWS console/CLI)",
    &[Part::Error(logged(&e, "humble self-login failed"))],
)).await;
            Heal::Failed
        }
    }
}

/// Dispatch a fulfillment request. Never panics; every arm returns a typed response.
pub async fn handle(deps: &Deps, req: FulfillRequest) -> FulfillResponse {
    match req {
        FulfillRequest::Gift {
            claim_id,
            link_token,
            game_id,
            gamekey,
            machine_name,
            keyindex,
            requires_choice,
        } => {
            tracing::info!(
                claim_id,
                game_id,
                machine_name,
                keyindex,
                requires_choice,
                "fulfillment: gift request"
            );
            if requires_choice {
                // Choice game: machine_name carries the OFFERED id; there's no tpk/keyindex yet.
                handle_gift_choice(
                    deps,
                    &claim_id,
                    &link_token,
                    &game_id,
                    &gamekey,
                    &machine_name,
                )
                .await
            } else {
                handle_gift(
                    deps,
                    &claim_id,
                    &link_token,
                    &game_id,
                    &gamekey,
                    &machine_name,
                    keyindex,
                )
                .await
            }
        }
        FulfillRequest::SelfClaim {
            claim_id,
            game_id,
            gamekey,
            machine_name,
            keyindex,
            requires_choice,
        } => {
            tracing::info!(
                claim_id,
                game_id,
                machine_name,
                keyindex,
                requires_choice,
                "fulfillment: self-claim request"
            );
            if requires_choice {
                handle_self_claim_choice(deps, &claim_id, &game_id, &gamekey, &machine_name).await
            } else {
                handle_self_claim(deps, &claim_id, &game_id, &gamekey, &machine_name, keyindex)
                    .await
            }
        }
        FulfillRequest::Sync => handle_sync(deps).await,
        FulfillRequest::ValidateCookie => handle_validate_cookie(deps).await,
    }
}

/// Self-claim choice wrapper — dispatches via [`handle_choice_claim`] with [`ClaimFlavor::SelfClaim`].
async fn handle_self_claim_choice(
    deps: &Deps,
    claim_id: &str,
    game_id: &str,
    gamekey: &str,
    offered_id: &str,
) -> FulfillResponse {
    handle_choice_claim(
        deps,
        claim_id,
        domain::SELF_LINK_TOKEN,
        game_id,
        gamekey,
        offered_id,
        ClaimFlavor::SelfClaim,
    )
    .await
}

/// #88 item 4: the pre-redeem gate — validate stored state BEFORE the irreversible Humble
/// write. The redeem coordinates ride the invoke payload from a `get_game` read taken in
/// public-api BEFORE the claim tx; a concurrent compensate/re-list/re-key can stale them,
/// and a redeem on stale coordinates burns a key nothing tracks. This re-reads claim and
/// game and refuses — parks, ZERO writes, reconcile owns recovery — unless:
///
/// - the claim exists and its state is `Pending` or `Fulfilled`. `Fulfilled` is the
///   stranded-game re-drive (write-1 landed, the flip didn't): Humble's redeem/reveal is
///   idempotent on re-POST — but only against the SAME coordinates, so the coordinate
///   check applies to re-drives too. `Compensated`/`Failed` are terminal: a redeem there
///   is exactly the burn this gate exists to stop.
/// - the claim's `game_id` matches the payload's (linkage).
/// - the CURRENT game record matches the payload's `gamekey`, and — where the payload
///   carries real key coordinates (non-choice paths) — `machine_name` + `keyindex` too.
///   `keyindex` is the coordinate that actually drifts: `game_id` derives from
///   gamekey:machine_name, but keyindex is refreshed from the wire by sync re-keys.
///   Choice paths pass `None`: their tpk coordinates are born fresh inside the choice
///   flow, and the offered id is validated downstream against the order itself.
///
/// TOCTOU honesty: the gate shrinks the stale window from claim-age (minutes/hours) to
/// read→redeem (ms). It cannot be zero without a lock Humble doesn't participate in.
async fn redeem_gate(
    deps: &Deps,
    claim_id: &str,
    link_token: &str,
    game_id: &str,
    gamekey: &str,
    key_coords: Option<(&str, u32)>,
) -> Result<(), FulfillResponse> {
    let claim = match deps.store.get_claim(link_token, claim_id).await {
        Ok(Some(c)) => c,
        // Missing claim: park, matching this handler's established fail-safe contract
        // (the get_game-missing path parks too). Zero spend either way; a park keeps the
        // "nothing seeded / precondition absent → park" invariant the suite pins.
        Ok(None) => {
            return Err(FulfillResponse::Parked {
                reason: "pre-redeem gate: claim not found".into(),
            });
        }
        Err(e) => {
            return Err(FulfillResponse::Parked {
                reason: format!("pre-redeem gate: claim read failed: {e}"),
            });
        }
    };
    if !matches!(
        claim.state,
        domain::ClaimState::Pending | domain::ClaimState::Fulfilled
    ) {
        return Err(FulfillResponse::Parked {
            reason: format!(
                "pre-redeem gate: claim state {:?} is terminal — refusing to touch a key",
                claim.state
            ),
        });
    }
    if claim.game_id != game_id {
        return Err(FulfillResponse::Parked {
            reason: "pre-redeem gate: claim/game linkage mismatch".into(),
        });
    }
    let game = match deps.store.get_game(game_id).await {
        Ok(Some(g)) => g,
        Ok(None) => {
            return Err(FulfillResponse::Parked {
                reason: "pre-redeem gate: game record missing".into(),
            });
        }
        Err(e) => {
            return Err(FulfillResponse::Parked {
                reason: format!("pre-redeem gate: game read failed: {e}"),
            });
        }
    };
    if game.gamekey != gamekey {
        return Err(FulfillResponse::Parked {
            reason: "pre-redeem gate: gamekey drift since claim".into(),
        });
    }
    if let Some((machine_name, keyindex)) = key_coords
        && (game.machine_name != machine_name || game.keyindex != keyindex)
    {
        return Err(FulfillResponse::Parked {
            reason: "pre-redeem gate: key coordinates drifted since claim".into(),
        });
    }
    Ok(())
}

/// The gift ladder's side-effecting half. Policy lives in [`gift_decision`]; this executes it.
async fn handle_gift(
    deps: &Deps,
    claim_id: &str,
    link_token: &str,
    game_id: &str,
    gamekey: &str,
    machine_name: &str,
    keyindex: u32,
) -> FulfillResponse {
    // #88: validate stored state before the irreversible Humble write.
    if let Err(resp) = redeem_gate(
        deps,
        claim_id,
        link_token,
        game_id,
        gamekey,
        Some((machine_name, keyindex)),
    )
    .await
    {
        return resp;
    }
    // The redeem rides the shared heal ladder: on a dead session (`Unauthorized`) with self-login
    // configured, heal IN-LINE and retry the redeem once — the friend gets their gift now instead
    // of parking until the next scheduled sync/validate. Burn-safety of retrying this WRITE is
    // argued on [`selfheal_once`] (Unauthorized precedes any key touch); every other failure keeps
    // its park/compensate semantics below. Composition with `redeem_as_gift`'s INTERNAL step-up
    // retry stays bounded: at most two outer attempts, each making at most two redeem POSTs, and
    // only ever after outcomes that prove the key untouched — no loop, no second burn window.
    // (A fresh self-login is born secure-area-elevated, so the healed retry normally needs no
    // step-up at all.)
    let (heal, outcome) = selfheal_once(deps, deps.session_store.is_some(), || {
        deps.humble.redeem_as_gift(gamekey, machine_name, keyindex)
    })
    .await;
    // Log the mapped outcome (never the gift URL/token). On a park, this names
    // which HumbleError variant drove it — pairs with humble-client's status log.
    if let Err(e) = &outcome {
        tracing::warn!(claim_id, game_id, error = ?e, "gift redeem did not return a URL");
    } else {
        tracing::info!(claim_id, game_id, "gift redeem returned a URL");
    }
    let decision = gift_decision(&outcome);
    // A heal ran mid-gift: record the DURABLE cookie truth now, the same bookkeeping the sync
    // walk does (`Persisted` ⇒ SSM holds a live cookie ⇒ cookie_ok=true; `Unpersisted` ⇒ the
    // durable cookie is still the dead one ⇒ false — the persist-failure ping already fired).
    // The ParkCookieDead arm below owns its own cookie_ok write, so skip it here rather than
    // double-write on that path.
    if let Some(h) = heal
        && decision != Decision::ParkCookieDead
    {
        set_cookie_ok(deps, h.durable()).await;
    }
    match decision {
        Decision::Record => match outcome {
            Ok(GiftUrl(url)) => {
                // URL durable BEFORE returning — the invariant.
                match deps
                    .store
                    .fulfill_claim(link_token, claim_id, game_id, &url)
                    .await
                {
                    Ok(()) => FulfillResponse::GiftUrl { url },
                    // fulfill lost to compensate = loud Corrupt; the URL exists but the game moved
                    // on. Surface as Error + ping — human decides. NEVER retry the redeem.
                    Err(e) => {
                        ping_msg(deps, &OperatorMessage::fmt(
    "fulfill after redeem failed for claim {}: {} — gift URL was generated but not recorded — recover it from humble\'s gift history page (purchases → the order → gift link)",
    &[Part::Id(claim_id), Part::Error(logged(&e, "fulfill after redeem failed"))],
))
                        .await;
                        FulfillResponse::Error {
                            message: "gift generated but recording failed — flagged for ben".into(),
                        }
                    }
                }
            }
            // gift_decision guarantees Record ⇒ Ok; unreachable, handled without panic.
            Err(_) => FulfillResponse::Error {
                message: "internal: record decision without a gift url".into(),
            },
        },
        // definitive from humble: the key was already gone. Compensate (slot returns, game re-lists;
        // the next sync corrects the game to ben-redeemed via merge policy).
        Decision::Compensate => match deps
            .store
            .compensate_claim(link_token, claim_id, game_id)
            .await
        {
            Ok(()) => FulfillResponse::AlreadyRedeemed,
            Err(e) => {
                ping_msg(
                    deps,
                    &OperatorMessage::fmt(
                        "compensate failed for claim {}: {}",
                        &[
                            Part::Id(claim_id),
                            Part::Error(logged(&e, "compensate failed")),
                        ],
                    ),
                )
                .await;
                FulfillResponse::Error {
                    message: "recording failed — flagged for ben".into(),
                }
            }
        },
        // dead cookie: park + flag cookie state + ping. Friend sees "processing".
        Decision::ParkCookieDead => {
            set_cookie_ok(deps, false).await;
            // With self-login configured, reaching this arm means the IN-LINE heal already ran
            // and could not produce a working session — either the login itself failed (its
            // failure-reason ping fired from `refresh_session`) or, pathologically, a successful
            // login's retry still came back `Unauthorized` (the heal-outcome ping fired either
            // way). So no scheduled run will magically fix this; the paste IS the break-glass,
            // and the message says so instead of promising a heal that already lost.
            let msg = if deps.session_store.is_some() {
                COOKIE_DEAD_SELFHEAL_MSG
            } else {
                COOKIE_DEAD_MSG
            };
            ping_msg(deps, &OperatorMessage::literal(msg)).await;
            FulfillResponse::Parked {
                reason: "humble session needs attention".into(),
            }
        }
        // EVERYTHING else is ambiguous-or-refused → PARK (never compensate blind). Reconcile
        // re-checks against humble truth (see `reconcile`).
        Decision::Park => {
            let detail = match &outcome {
                Err(HumbleError::RedeemRefused { .. }) => "refused",
                Err(HumbleError::AmbiguousRedeem) => "ambiguous",
                Err(HumbleError::RateLimited) => "rate-limited",
                Err(HumbleError::RedeemAuthRejected { .. }) => "redeem-auth-rejected",
                Err(HumbleError::SecureAreaStepUpFailed { .. }) => "secure-area-step-up-failed",
                _ => "transient",
            };
            // A step-up failure gets its own ping: like the auth-rejection case, a persistent
            // failure would otherwise loop silently (park → reconcile → re-list → re-claim →
            // fail). The reason string carries no secret (it names the failure class only).
            if let Err(HumbleError::SecureAreaStepUpFailed { reason }) = &outcome {
                ping_msg(deps, &OperatorMessage::fmt(
    "gift redeem for claim {} ({}) needed humble\'s secure-area step-up and it did not complete: {}. Check the humble password + TOTP seed in SSM (or the account may be locked / rate-limited). The key was NOT redeemed — the claim is parked and will re-list on reconcile.",
    &[Part::Id(claim_id), Part::Id(machine_name), Part::Id(reason)],
))
                .await;
            }
            // A redeem-auth rejection gets its own correctly-labeled ping: without one, a
            // persistent rejection is invisible (park → reconcile compensates → re-list →
            // re-claim → reject, daily, gifting nothing). Message carries claim id + machine
            // name only — never a key, cookie, or csrf value.
            if let Err(HumbleError::RedeemAuthRejected {
                status,
                csrf_minted,
            }) = &outcome
            {
                let csrf_note = if *csrf_minted {
                    "csrf capture FAILED (minted fallback used) — the preflight isn't yielding a cookie"
                } else {
                    "humble rejected its own captured csrf token — the write dance needs a look"
                };
                ping_msg(deps, &OperatorMessage::fmt(
    "gift redeem for claim {} ({}) was blocked at humble\'s auth layer (status {}). {}. The session cookie is fine (reads work) — refreshing the session won\'t help. The claim is parked; reconcile will re-list the key if unredeemed, so this repeats on the next claim until the write path is fixed.",
    &[Part::Id(claim_id), Part::Id(machine_name), Part::Id(&status.to_string()), Part::Id(csrf_note)],
))
                .await;
            }
            FulfillResponse::Parked {
                reason: format!("humble call inconclusive: park for reconcile ({detail})"),
            }
        }
        Decision::DeadKey => {
            let reason = match &outcome {
                // The persisted failure_reason is the same value the wire carried:
                // "msg" or "msg [code: c]" — one truth from wire to dynamo.
                Err(HumbleError::KeyExpired { msg, code }) => match code {
                    Some(c) => format!("{msg} [code: {c}]"),
                    None => msg.clone(),
                },
                // DeadKey is only produced from KeyExpired today; a future producer
                // must carry its own words. Never leaves this arm blank.
                _ => "dead key (unclassified producer)".to_string(),
            };
            match fail_dead_key_any(deps, link_token, claim_id, game_id, &reason).await {
                Ok(()) => {
                    tracing::warn!(claim_id, game_id, %reason, "dead key: claim terminally failed, slot returned, game retired");
                    ping_msg(deps, &OperatorMessage::fmt(
    "claim {} ({}) hit a DEAD key — humble says: \"{}\". The claim is failed (reason recorded on the claim), the game is retired as expired and will not re-list, and the slot was returned. Nothing retries this.",
    &[Part::Id(claim_id), Part::Id(game_id), Part::Id(&reason)],
))
                    .await;
                    FulfillResponse::KeyDead
                }
                Err(e) => {
                    // The store write failed — the claim is STILL pending; reconcile
                    // will re-detect the dead key next pass and retry this transition.
                    ping_msg(deps, &OperatorMessage::fmt(
    "dead-key fail-claim write for {} failed: {} — still pending, reconcile retries",
    &[Part::Id(claim_id), Part::Error(logged(&e, "dead-key fail-claim write failed"))],
)).await;
                    FulfillResponse::Parked {
                        reason: "dead key detected but recording failed — will retry".into(),
                    }
                }
            }
        }
    }
}

/// The pick-spend flavor: determines `is_gift` on the choose, the terminal write, and the
/// already-claimed-AND-redeemed recovery strategy.
#[derive(Clone, Copy, PartialEq, Eq)]
enum ClaimFlavor {
    Gift,
    SelfClaim,
}

/// The Humble Choice orchestration: a TWO-write one-shot that must spend a monthly pick exactly
/// once. Sibling of [`handle_gift`] / [`handle_self_claim`], dispatched when `requires_choice` is
/// set. The three flavor points that differ between Gift and SelfClaim are:
///  1. `is_gift` on the choose call — `true` for Gift, `false` for SelfClaim.
///  2. The already-claimed-AND-redeemed pre-check arm — Gift pings human; SelfClaim recovers.
///  3. The terminal — [`redeem_claimed_tpk`] for Gift, [`reveal_claimed_tpk`] for SelfClaim.
///
/// The whole design turns on ONE durable write ordering: the pre-choose snapshot
/// ([`Store::record_choice_intent`]) is made durable BEFORE `choosecontent` runs (step 3 before
/// step 4). That snapshot is the crash-recovery hinge — its presence/absence lets reconcile decide
/// whether a pick could have been spent WITHOUT ever re-choosing. Nothing on this path ever
/// compensates: a spent pick can't be un-spent, and the ambiguous choose failures may have
/// committed, so reconcile (reading the order diff, not the error) owns every uncertain outcome.
///
/// Entry state (like `handle_gift`): `claim_game` already created a durable `Pending` claim with
/// its reconcile marker, so a crash at ANY step below leaves a claim reconcile will finish — no
/// extra "park write" is ever needed; parking = returning without fulfilling.
async fn handle_choice_claim(
    deps: &Deps,
    claim_id: &str,
    link_token: &str,
    game_id: &str,
    gamekey: &str,
    offered_id: &str,
    flavor: ClaimFlavor,
) -> FulfillResponse {
    // #88: validate stored state before any Humble write. Choice coords (the tpk) are
    // born fresh inside this flow, so the gate checks the shared core + gamekey only.
    if let Err(resp) = redeem_gate(deps, claim_id, link_token, game_id, gamekey, None).await {
        return resp;
    }
    let selfheal = deps.session_store.is_some();
    // One self-login per invoke, total (mirrors run_sync's one-heal-per-run cap): every humble call
    // below passes `selfheal && !healed`, and any heal flips this.
    let mut healed = false;

    // The friend-facing title is needed for the pre-check (step 2) and find_new_tpk's disambiguation
    // (step 5). The Gift wire doesn't carry it, so read the game once — cheap, and a choice claim is
    // rare. (§1.1's blessed alternative: a single store read on this path.)
    let title = match deps.store.get_game(game_id).await {
        Ok(Some(g)) => g.title,
        _ => {
            tracing::warn!(
                claim_id,
                game_id,
                "choice: game missing at fulfillment — parking"
            );
            return parked_choice("game-missing");
        }
    };

    // ── Step 1: pre-read the month order (self-heal like handle_gift). ──────────────────────────
    let (heal, read) =
        selfheal_once(deps, selfheal && !healed, || deps.humble.order(gamekey)).await;
    if let Some(h) = heal {
        healed = true;
        if !matches!(read, Err(HumbleError::Unauthorized)) {
            set_cookie_ok(deps, h.durable()).await;
        }
    }
    let pre_order = match read {
        Ok(o) => o,
        Err(HumbleError::Unauthorized) => return choice_cookie_dead(deps).await,
        Err(e) => {
            // `gamekey` is explicit here because it USED to arrive only as a side effect of the
            // error's unredacted request URL (`/api/v1/order/{gamekey}`). #173 claimed it was
            // "already logged beside claim_id" — it was not: this site carried `claim_id` and not
            // `gamekey`, and `:3392` carries `gamekey` and not `claim_id`. Two paths, neither
            // carrying both. A correlator you can only get out of a leak was never a correlator.
            tracing::warn!(claim_id, gamekey = %gamekey, error = ?e, "choice pre-read order failed — parking (no spend)");
            return parked_choice("pre-read");
        }
    };

    // ── Step 2: best-effort pre-check — is this game already claimed on humble? ──────────────────
    // EXACT case-insensitive title vs a claimed tpk's human_name. A match means a prior crash (or a
    // stale sync) already spent this game's pick, so we must NOT choose again — resume from the
    // existing key (idempotent), or hand to a human if it's already redeemed.
    if let Some(existing) = pre_order
        .keys
        .iter()
        .find(|k| k.human_name.eq_ignore_ascii_case(&title))
    {
        // #35: the pick is already spent (this tpk exists). If it materialized EXTERNALLY (a
        // hand-choose on humble, or a title-collision with a pre-existing key) and the resume
        // terminal below then fails/parks, the claim is left `Pending` with no snapshot — which
        // reconcile reads as branch A ("no intent ⇒ choose never ran ⇒ pick NOT spent") and
        // COMPENSATES, stranding the already-spent pick + drifting the accounting. Record an intent
        // snapshot that EXCLUDES this tpk, so reconcile's `find_new_tpk` surfaces it as the new key
        // and routes to B2 (unredeemed ⇒ complete) / B3 (redeemed ⇒ recover), never A. Snapshotting
        // the order VERBATIM would leave the tpk inside `pre` ⇒ find_new_tpk finds nothing ⇒ B1
        // compensate: the identical misread — it MUST be excluded. Recording intent is NOT choosing;
        // the resume still makes zero `choosecontent` calls (asserted in tests).
        let pre_excluding: Vec<String> = pre_order
            .keys
            .iter()
            .map(|k| k.machine_name.clone())
            .filter(|mn| mn != &existing.machine_name)
            .collect();
        if let Err(e) = deps
            .store
            .record_choice_intent(link_token, claim_id, pre_excluding)
            .await
        {
            // Snapshot didn't land — park rather than resume (mirrors the step-3 hinge). Safe: no
            // pick is spent by us here, and the next sync re-reads the order and retries the resume.
            tracing::warn!(claim_id, error = ?e, "choice pre-check: resume intent snapshot failed to persist — parking (will retry next sync)");
            return parked_choice("resume-intent-write");
        }
        if existing.redeemed {
            return match flavor {
                ClaimFlavor::Gift => {
                    tracing::warn!(
                        claim_id,
                        "choice pre-check: game already claimed AND redeemed on humble — human recovery"
                    );
                    ping_msg(deps, &OperatorMessage::fmt(
    "choice claim {} ({}): this game\'s pick appears already claimed AND redeemed on humble — no re-choose was attempted. Recover the gift URL from humble\'s gift-history page; the claim is parked.",
    &[Part::Id(claim_id), Part::Id(&title)],
))
                    .await;
                    parked_choice("already-claimed-redeemed")
                }
                ClaimFlavor::SelfClaim => {
                    tracing::warn!(
                        claim_id,
                        "choice pre-check: game already claimed AND redeemed — recovering key for self-claim"
                    );
                    recover_already_redeemed_key(
                        deps,
                        claim_id,
                        game_id,
                        gamekey,
                        &existing.machine_name,
                    )
                    .await
                }
            };
        }
        tracing::info!(
            claim_id,
            "choice pre-check: pick already spent (tpk present, unredeemed) — resuming to terminal WITHOUT choosing"
        );
        // Resume: the pick was already spent; skip the choose entirely and run the terminal on the
        // key already sitting in the order. The intent snapshot (EXCLUDING this tpk) was recorded
        // above (#35) so a failed/parked resume reconciles to B2/B3, never branch-A compensate —
        // nothing NEW is chosen here.
        return claimed_tpk_terminal(
            deps,
            flavor,
            claim_id,
            link_token,
            game_id,
            gamekey,
            existing,
            selfheal && !healed,
        )
        .await;
    }

    // ── Step 3: persist the intent snapshot BEFORE the choose (the crash-recovery hinge). ───────
    let pre_tpks: Vec<String> = pre_order
        .keys
        .iter()
        .map(|k| k.machine_name.clone())
        .collect();
    if let Err(e) = deps
        .store
        .record_choice_intent(link_token, claim_id, pre_tpks.clone())
        .await
    {
        // Snapshot didn't land ⇒ do NOT choose. Reconcile will read `choice_pre_tpks == None` and
        // safely compensate (choose provably never ran).
        tracing::warn!(claim_id, error = ?e, "choice: intent snapshot failed to persist BEFORE choose — parking, NOT choosing");
        return parked_choice("intent-write");
    }

    // ── Step 4: HUMBLE WRITE 1 — spend the pick. ────────────────────────────────────────────────
    // Bind the chosen slice in-scope: `selfheal_once`'s Fn closure may call twice (heal-retry), so
    // the borrowed slice must outlive both calls.
    let chosen: [&str; 1] = [offered_id];
    let is_gift = matches!(flavor, ClaimFlavor::Gift);
    let (heal, choose_outcome) = selfheal_once(deps, selfheal && !healed, || {
        deps.humble.choose_content(gamekey, &chosen, is_gift)
    })
    .await;
    let decision = choose_decision(&choose_outcome);
    if let Some(h) = heal {
        healed = true;
        if decision != Decision::ParkCookieDead {
            set_cookie_ok(deps, h.durable()).await;
        }
    }
    match decision {
        // Pick spent — fall through to the re-read + redeem.
        Decision::Record => {}
        Decision::ParkCookieDead => return choice_cookie_dead(deps).await,
        // NEVER compensate at choose time (choose_decision has no Compensate arm). Park; reconcile
        // resolves from the order diff. Distinct pings for the loop-forever failure classes.
        Decision::Park | Decision::Compensate => {
            return choose_park(deps, claim_id, &title, &choose_outcome).await;
        }
        // Unreachable: choose_decision maps KeyExpired -> Park, never DeadKey (Task 1 3f) — a
        // choosecontent call never redeems a key. Never-panic fallback, exhaustiveness only.
        Decision::DeadKey => {
            tracing::error!(
                claim_id,
                "unreachable: choose decision matched DeadKey — classified conservatively as park"
            );
            return FulfillResponse::Parked {
                reason: "dead key decision on a choose outcome -- classified conservatively".into(),
            };
        }
    }

    // ── Step 5: re-read the order and find the newly-minted tpk. ────────────────────────────────
    let (heal, read) =
        selfheal_once(deps, selfheal && !healed, || deps.humble.order(gamekey)).await;
    if let Some(h) = heal {
        healed = true;
        if !matches!(read, Err(HumbleError::Unauthorized)) {
            set_cookie_ok(deps, h.durable()).await;
        }
    }
    let post_order = match read {
        Ok(o) => o,
        Err(HumbleError::Unauthorized) => return choice_cookie_dead(deps).await,
        Err(e) => {
            // Pick spent, key not yet burned, tpk unknown THIS invoke = the crash-between-writes
            // state. Park; reconcile finishes from the snapshot — and NEVER re-chooses.
            tracing::warn!(claim_id, gamekey = %gamekey, error = ?e, "choice re-read after choose failed — parking; reconcile finishes (no re-choose)");
            return parked_choice("re-read");
        }
    };
    let tpk = match find_new_tpk(&post_order, &pre_tpks, &title) {
        TpkPick::Unique(t) => t,
        TpkPick::None => {
            // Choose said ok but no new tpk yet (eventual consistency / drift). Park; reconcile
            // finishes when the key materializes. NEVER re-choose.
            tracing::warn!(
                claim_id,
                "choose committed but no new tpk in the re-read — parking; reconcile finishes (no re-choose)"
            );
            ping_msg(deps, &OperatorMessage::fmt(
    "choice claim {} ({}): the monthly pick was spent but the new key hasn\'t appeared in the order yet — parked, reconcile will finish it. No pick will be spent twice.",
    &[Part::Id(claim_id), Part::Id(&title)],
))
            .await;
            return parked_choice("no-tpk-yet");
        }
        TpkPick::Ambiguous => {
            tracing::warn!(
                claim_id,
                "ambiguous new tpks after choose — parking for human review"
            );
            ping_msg(deps, &OperatorMessage::fmt(
    "choice claim {} ({}): several new keys appeared after the choose and the title can\'t single one out (a concurrent sibling claim on this month?) — parked for review. No key was burned.",
    &[Part::Id(claim_id), Part::Id(&title)],
))
            .await;
            return parked_choice("ambiguous-tpk");
        }
    };

    // ── Steps 6 + 7: HUMBLE WRITE 2 — burn the tpk (gift or reveal), record the result (shared tail). ─
    claimed_tpk_terminal(
        deps,
        flavor,
        claim_id,
        link_token,
        game_id,
        gamekey,
        tpk,
        selfheal && !healed,
    )
    .await
}

/// Gift choice wrapper — thin entry point; behavior-identical to pre-refactor `handle_gift_choice`.
async fn handle_gift_choice(
    deps: &Deps,
    claim_id: &str,
    link_token: &str,
    game_id: &str,
    gamekey: &str,
    offered_id: &str,
) -> FulfillResponse {
    handle_choice_claim(
        deps,
        claim_id,
        link_token,
        game_id,
        gamekey,
        offered_id,
        ClaimFlavor::Gift,
    )
    .await
}

/// Flavor-dispatched terminal on a claimed tpk — called from the pre-check resume, the happy tail,
/// and (Task 8) reconcile B2. Gift → [`redeem_claimed_tpk`]; SelfClaim → [`reveal_claimed_tpk`].
#[allow(clippy::too_many_arguments)] // private dispatcher; params mirror the two terminals it fans into
async fn claimed_tpk_terminal(
    deps: &Deps,
    flavor: ClaimFlavor,
    claim_id: &str,
    link_token: &str,
    game_id: &str,
    gamekey: &str,
    tpk: &KeyEntry,
    allow_heal: bool,
) -> FulfillResponse {
    // Structural truth beats string-matching (spec §1 ladder rung 1): a tpk humble
    // already marks expired is dead — don't spend a redeem/reveal call to learn it.
    // Same trust sync already grants tpk.expired at listing time.
    if tpk.expired {
        let reason = format!(
            "tpk {} is marked expired on the order (structural is_expired)",
            tpk.machine_name
        );
        // Executor parity with the DeadKey arm: fail, ping, KeyDead.
        return match fail_dead_key_any(deps, link_token, claim_id, game_id, &reason).await {
            Ok(()) => {
                tracing::warn!(claim_id, game_id, %reason, "dead key (structural): claim terminally failed");
                ping_msg(deps, &OperatorMessage::fmt(
    "claim {} ({}) sits on a key humble marks expired ({}) — failed terminally without a redeem attempt. Reason recorded, slot returned, game retired. If this was a choice claim, its spent pick is stranded.",
    &[Part::Id(claim_id), Part::Id(game_id), Part::Id(&tpk.machine_name)],
))
                .await;
                FulfillResponse::KeyDead
            }
            Err(e) => {
                ping_msg(deps, &OperatorMessage::fmt(
    "dead-key (structural) fail-claim write for {} failed: {} — still pending, reconcile retries",
    &[Part::Id(claim_id), Part::Error(logged(&e, "dead-key (structural) fail-claim write failed"))],
)).await;
                FulfillResponse::Parked {
                    reason: "dead key detected but recording failed — will retry".into(),
                }
            }
        };
    }
    match flavor {
        ClaimFlavor::Gift => {
            redeem_claimed_tpk(
                deps, claim_id, link_token, game_id, gamekey, tpk, allow_heal,
            )
            .await
        }
        ClaimFlavor::SelfClaim => {
            reveal_claimed_tpk(deps, claim_id, game_id, gamekey, tpk, allow_heal).await
        }
    }
}

/// The shared "redeem a now-present choice tpk and record its gift URL" tail, called by BOTH the
/// happy path (step 6) AND reconcile's B2 branch — one body so those two can never drift. It burns
/// `tpk.machine_name` VERBATIM as the keytype (read off the post-choose order, never constructed)
/// via `redeem_as_gift(gamekey, machine_name, keyindex)`.
///
/// Classification reuses [`gift_decision`]; the executor mirrors `handle_gift` with ONE Choice
/// override: a `Compensate`/`AlreadyRedeemed` outcome does NOT compensate. The monthly pick is
/// already spent; re-listing the game would strand that pick (a re-claim just `ChooseFailed`-parks)
/// and could orphan a real gift URL from a crashed prior redeem — so it parks for human recovery,
/// the same shape as reconcile B3.
///
/// `allow_heal` caps the shared one-heal ladder: the happy path passes `selfheal && !healed`;
/// reconcile passes `false` (its order read just proved the session live this pass — a dead-session
/// redeem here simply leaves the claim Pending for the next sync, which heals via the listing).
async fn redeem_claimed_tpk(
    deps: &Deps,
    claim_id: &str,
    link_token: &str,
    game_id: &str,
    gamekey: &str,
    tpk: &KeyEntry,
    allow_heal: bool,
) -> FulfillResponse {
    let (heal, outcome) = selfheal_once(deps, allow_heal, || {
        deps.humble
            .redeem_as_gift(gamekey, &tpk.machine_name, tpk.keyindex)
    })
    .await;
    if let Err(e) = &outcome {
        tracing::warn!(claim_id, game_id, error = ?e, "choice gift redeem did not return a URL");
    } else {
        tracing::info!(claim_id, game_id, "choice gift redeem returned a URL");
    }
    let decision = gift_decision(&outcome);
    if let Some(h) = heal
        && decision != Decision::ParkCookieDead
    {
        set_cookie_ok(deps, h.durable()).await;
    }
    match decision {
        Decision::Record => match outcome {
            Ok(GiftUrl(url)) => {
                match deps
                    .store
                    .fulfill_claim(link_token, claim_id, game_id, &url)
                    .await
                {
                    Ok(()) => FulfillResponse::GiftUrl { url },
                    Err(e) => {
                        ping_msg(deps, &OperatorMessage::fmt(
    "fulfill after choice redeem failed for claim {}: {} — gift URL was generated but not recorded — recover it from humble\'s gift history page (purchases → the order → gift link)",
    &[Part::Id(claim_id), Part::Error(logged(&e, "fulfill after choice redeem failed"))],
))
                        .await;
                        FulfillResponse::Error {
                            message: "gift generated but recording failed — flagged for ben".into(),
                        }
                    }
                }
            }
            // gift_decision guarantees Record ⇒ Ok; unreachable, handled without panic.
            Err(_) => FulfillResponse::Error {
                message: "internal: record decision without a gift url".into(),
            },
        },
        // CHOICE OVERRIDE (§5.3): the pick is spent — NEVER compensate. Park for human recovery,
        // identical to reconcile B3 (spent-and-burned, URL unrecorded).
        Decision::Compensate => {
            tracing::warn!(
                claim_id,
                game_id,
                "choice redeem returned AlreadyRedeemed — pick already spent, NOT compensating; human recovery"
            );
            ping_msg(deps, &OperatorMessage::fmt(
    "choice claim {} redeem returned already-redeemed — the monthly pick was already spent, so this claim was NOT compensated (re-listing would strand the pick). Recover the gift URL from humble\'s gift-history page; claim parked.",
    &[Part::Id(claim_id)],
))
            .await;
            FulfillResponse::Parked {
                reason: "choice key already redeemed — parked for human recovery".into(),
            }
        }
        Decision::ParkCookieDead => choice_cookie_dead(deps).await,
        // Ambiguous/refused → park (never compensate blind); distinct pings for the loop-forever
        // classes, mirroring handle_gift.
        Decision::Park => {
            let detail = match &outcome {
                Err(HumbleError::RedeemRefused { .. }) => "refused",
                Err(HumbleError::AmbiguousRedeem) => "ambiguous",
                Err(HumbleError::RateLimited) => "rate-limited",
                Err(HumbleError::RedeemAuthRejected { .. }) => "redeem-auth-rejected",
                Err(HumbleError::SecureAreaStepUpFailed { .. }) => "secure-area-step-up-failed",
                _ => "transient",
            };
            if let Err(HumbleError::SecureAreaStepUpFailed { reason }) = &outcome {
                ping_msg(deps, &OperatorMessage::fmt(
    "choice gift redeem for claim {} ({}) needed humble\'s secure-area step-up and it did not complete: {}. The key was NOT redeemed — the claim is parked and reconcile will finish it.",
    &[Part::Id(claim_id), Part::Id(&tpk.machine_name), Part::Id(reason)],
))
                .await;
            }
            if let Err(HumbleError::RedeemAuthRejected {
                status,
                csrf_minted,
            }) = &outcome
            {
                let csrf_note = if *csrf_minted {
                    "csrf capture FAILED (minted fallback used) — the preflight isn't yielding a cookie"
                } else {
                    "humble rejected its own captured csrf token — the write dance needs a look"
                };
                ping_msg(deps, &OperatorMessage::fmt(
    "choice gift redeem for claim {} ({}) was blocked at humble\'s auth layer (status {}). {}. The session cookie is fine (reads work). The claim is parked; reconcile will finish it once the write path is fixed.",
    &[Part::Id(claim_id), Part::Id(&tpk.machine_name), Part::Id(&status.to_string()), Part::Id(csrf_note)],
))
                .await;
            }
            FulfillResponse::Parked {
                reason: format!("humble call inconclusive: park for reconcile ({detail})"),
            }
        }
        Decision::DeadKey => {
            let reason = match &outcome {
                Err(HumbleError::KeyExpired { msg, code }) => match code {
                    Some(c) => format!("{msg} [code: {c}]"),
                    None => msg.clone(),
                },
                _ => "dead key (unclassified producer)".to_string(),
            };
            match fail_dead_key_any(deps, link_token, claim_id, game_id, &reason).await {
                Ok(()) => {
                    tracing::warn!(claim_id, game_id, %reason, "dead key: claim terminally failed, slot returned, game retired");
                    ping_msg(deps, &OperatorMessage::fmt(
    "claim {} ({}) hit a DEAD key — humble says: \"{}\". The claim is failed (reason recorded on the claim), the game is retired as expired and will not re-list, and the slot was returned. Nothing retries this. The monthly pick was already spent and is stranded -- the dead key was the pick\'s product.",
    &[Part::Id(claim_id), Part::Id(game_id), Part::Id(&reason)],
))
                    .await;
                    FulfillResponse::KeyDead
                }
                Err(e) => {
                    ping_msg(deps, &OperatorMessage::fmt(
    "dead-key fail-claim write for {} failed: {} — still pending, reconcile retries",
    &[Part::Id(claim_id), Part::Error(logged(&e, "dead-key fail-claim write failed"))],
)).await;
                    FulfillResponse::Parked {
                        reason: "dead key detected but recording failed — will retry".into(),
                    }
                }
            }
        }
    }
}

/// The shared "reveal a now-present choice tpk and record its key value" tail — the self-claim
/// sibling of [`redeem_claimed_tpk`], called by the happy path and (Task 8) reconcile B2.
///
/// Classification reuses [`reveal_decision`]. ONE Choice override vs the plain self-claim path:
/// a `Compensate`/`AlreadyRedeemed` outcome RECOVERS via [`recover_already_redeemed_key`] instead
/// of compensating — the monthly pick is already spent, re-listing would strand it, and for a
/// self-claim the key value IS recoverable from the order's `redeemed_key_val`.
///
/// `allow_heal` caps the shared one-heal ladder (same semantics as [`redeem_claimed_tpk`]).
async fn reveal_claimed_tpk(
    deps: &Deps,
    claim_id: &str,
    game_id: &str,
    gamekey: &str,
    tpk: &KeyEntry,
    allow_heal: bool,
) -> FulfillResponse {
    let (heal, outcome) = selfheal_once(deps, allow_heal, || {
        deps.humble
            .reveal_key(gamekey, &tpk.machine_name, tpk.keyindex)
    })
    .await;
    if let Err(e) = &outcome {
        tracing::warn!(claim_id, game_id, error = ?e, "choice self-claim reveal did not return a key");
    } else {
        tracing::info!(claim_id, game_id, "choice self-claim reveal returned a key");
    }
    let decision = reveal_decision(&outcome);
    if let Some(h) = heal
        && decision != Decision::ParkCookieDead
    {
        set_cookie_ok(deps, h.durable()).await;
    }
    match decision {
        Decision::Record => match outcome {
            Ok(RevealedKey(key)) => record_revealed_key(deps, claim_id, game_id, key).await,
            // reveal_decision guarantees Record ⇒ Ok; unreachable, handled without panic.
            Err(_) => FulfillResponse::Error {
                message: "internal: record decision without a revealed key".into(),
            },
        },
        // CHOICE OVERRIDE (§5.3 self-claim variant): the pick is spent — NEVER compensate. For a
        // self-claim, "already redeemed" means the key already belongs to Ben; recover the value
        // from the order's redeemed_key_val rather than re-listing.
        Decision::Compensate => {
            tracing::warn!(
                claim_id,
                game_id,
                "choice self-claim reveal returned AlreadyRedeemed — recovering key from order (NOT compensating)"
            );
            recover_already_redeemed_key(deps, claim_id, game_id, gamekey, &tpk.machine_name).await
        }
        Decision::ParkCookieDead => choice_cookie_dead(deps).await,
        // Ambiguous/refused → park (never compensate blind); distinct pings for the loop-forever
        // classes, mirroring redeem_claimed_tpk.
        Decision::Park => {
            let detail = match &outcome {
                Err(HumbleError::RedeemRefused { .. }) => "refused",
                Err(HumbleError::AmbiguousRedeem) => "ambiguous",
                Err(HumbleError::RateLimited) => "rate-limited",
                Err(HumbleError::RedeemAuthRejected { .. }) => "redeem-auth-rejected",
                Err(HumbleError::SecureAreaStepUpFailed { .. }) => "secure-area-step-up-failed",
                _ => "transient",
            };
            if let Err(HumbleError::SecureAreaStepUpFailed { reason }) = &outcome {
                ping_msg(deps, &OperatorMessage::fmt(
    "choice self-claim reveal for claim {} ({}) needed humble\'s secure-area step-up and it did not complete: {}. The key was NOT revealed — the claim is parked and reconcile will finish it.",
    &[Part::Id(claim_id), Part::Id(&tpk.machine_name), Part::Id(reason)],
))
                .await;
            }
            if let Err(HumbleError::RedeemAuthRejected {
                status,
                csrf_minted,
            }) = &outcome
            {
                let csrf_note = if *csrf_minted {
                    "csrf capture FAILED (minted fallback used) — the preflight isn't yielding a cookie"
                } else {
                    "humble rejected its own captured csrf token — the write dance needs a look"
                };
                ping_msg(deps, &OperatorMessage::fmt(
    "choice self-claim reveal for claim {} ({}) was blocked at humble\'s auth layer (status {}). {}. The session cookie is fine (reads work). The claim is parked; reconcile will finish it once the write path is fixed.",
    &[Part::Id(claim_id), Part::Id(&tpk.machine_name), Part::Id(&status.to_string()), Part::Id(csrf_note)],
))
                .await;
            }
            FulfillResponse::Parked {
                reason: format!("humble call inconclusive: park for reconcile ({detail})"),
            }
        }
        Decision::DeadKey => {
            let reason = match &outcome {
                Err(HumbleError::KeyExpired { msg, code }) => match code {
                    Some(c) => format!("{msg} [code: {c}]"),
                    None => msg.clone(),
                },
                _ => "dead key (unclassified producer)".to_string(),
            };
            match fail_dead_key_any(deps, domain::SELF_LINK_TOKEN, claim_id, game_id, &reason).await
            {
                Ok(()) => {
                    tracing::warn!(claim_id, game_id, %reason, "dead key: claim terminally failed, slot returned, game retired");
                    ping_msg(deps, &OperatorMessage::fmt(
    "claim {} ({}) hit a DEAD key — humble says: \"{}\". The claim is failed (reason recorded on the claim), the game is retired as expired and will not re-list, and the slot was returned. Nothing retries this. If this was a choice claim, its spent pick is stranded.",
    &[Part::Id(claim_id), Part::Id(game_id), Part::Id(&reason)],
))
                    .await;
                    FulfillResponse::KeyDead
                }
                Err(e) => {
                    ping_msg(deps, &OperatorMessage::fmt(
    "dead-key fail-claim write for {} failed: {} — still pending, reconcile retries",
    &[Part::Id(claim_id), Part::Error(logged(&e, "dead-key fail-claim write failed"))],
)).await;
                    FulfillResponse::Parked {
                        reason: "dead key detected but recording failed — will retry".into(),
                    }
                }
            }
        }
    }
}

/// Park a choice claim after a dead-session signal on one of its order reads / the choose
/// interstitial: flag `cookie_ok=false`, ping, return Parked — the same treatment `handle_gift`'s
/// ParkCookieDead arm applies. No pick can have been spent on this path (an `Unauthorized` choose is
/// the pre-handler interstitial), so reconcile stays safe.
async fn choice_cookie_dead(deps: &Deps) -> FulfillResponse {
    set_cookie_ok(deps, false).await;
    let msg = if deps.session_store.is_some() {
        COOKIE_DEAD_SELFHEAL_MSG
    } else {
        COOKIE_DEAD_MSG
    };
    ping_msg(deps, &OperatorMessage::literal(msg)).await;
    FulfillResponse::Parked {
        reason: "humble session needs attention".into(),
    }
}

/// Park after a non-cookie-dead `choosecontent` failure. NEVER compensates (a pick may already be
/// spent; only reconcile's diff can tell). Pings distinctly for the failure classes that would
/// otherwise loop silently (step-up gate, a `success=false` refusal). The ambiguous
/// `Api`/`Network`/`Parse`/`RateLimited` outcomes stay quiet — reconcile resolves them next pass.
async fn choose_park(
    deps: &Deps,
    claim_id: &str,
    title: &str,
    outcome: &Result<(), HumbleError>,
) -> FulfillResponse {
    let detail = match outcome {
        Err(HumbleError::ChooseFailed { .. }) => "choose-refused",
        Err(HumbleError::SecureAreaStepUpFailed { .. }) => "secure-area-step-up-failed",
        Err(HumbleError::RateLimited) => "rate-limited",
        Err(HumbleError::Api(_)) => "ambiguous-api",
        Err(HumbleError::Network(_)) => "ambiguous-network",
        Err(HumbleError::Parse(_)) => "ambiguous-parse",
        _ => "transient",
    };
    if let Err(HumbleError::SecureAreaStepUpFailed { reason }) = outcome {
        ping_msg(deps, &OperatorMessage::fmt(
    "choice claim {} ({}): choosecontent needed humble\'s secure-area step-up and it did not complete: {}. No pick was spent — the claim is parked.",
    &[Part::Id(claim_id), Part::Id(title), Part::Id(reason)],
))
        .await;
    }
    if let Err(HumbleError::ChooseFailed { reason }) = outcome {
        ping_msg(deps, &OperatorMessage::fmt(
    "choice claim {} ({}): humble refused the pick (choosecontent success=false): {}. No pick was spent this attempt — the claim is parked (reconcile will compensate if the order confirms nothing was claimed).",
    &[Part::Id(claim_id), Part::Id(title), Part::Id(reason)],
))
        .await;
    }
    FulfillResponse::Parked {
        reason: format!("choice choose inconclusive: park for reconcile ({detail})"),
    }
}

/// A plain choice-claim park (no ping, no cookie flag) — the claim stays `Pending` and reconcile
/// owns its fate. Used for the pure-transient / pre-read / re-read / snapshot-write parks.
fn parked_choice(detail: &str) -> FulfillResponse {
    FulfillResponse::Parked {
        reason: format!("choice fulfillment inconclusive: park for reconcile ({detail})"),
    }
}

/// The self-claim ladder's side-effecting half — [`handle_gift`]'s reveal sibling. Same heal
/// composition; two policy differences (spec §4): Record writes `revealed_key` via
/// `fulfill_self_claim`, and AlreadyRedeemed RECOVERS (re-read order → record `redeemed_key_val`)
/// instead of compensating — for a self-claim, "already redeemed" means the key already belongs
/// to Ben and its value is recoverable; compensating would re-list a burned game and lose the key.
async fn handle_self_claim(
    deps: &Deps,
    claim_id: &str,
    game_id: &str,
    gamekey: &str,
    machine_name: &str,
    keyindex: u32,
) -> FulfillResponse {
    // #88: validate stored state before the irreversible Humble write.
    if let Err(resp) = redeem_gate(
        deps,
        claim_id,
        domain::SELF_LINK_TOKEN,
        game_id,
        gamekey,
        Some((machine_name, keyindex)),
    )
    .await
    {
        return resp;
    }
    let (heal, outcome) = selfheal_once(deps, deps.session_store.is_some(), || {
        deps.humble.reveal_key(gamekey, machine_name, keyindex)
    })
    .await;
    if let Err(e) = &outcome {
        tracing::warn!(claim_id, game_id, error = ?e, "self-claim reveal did not return a key");
    } else {
        tracing::info!(claim_id, game_id, "self-claim reveal returned a key");
    }
    let decision = reveal_decision(&outcome);
    if let Some(h) = heal
        && decision != Decision::ParkCookieDead
    {
        set_cookie_ok(deps, h.durable()).await;
    }
    match decision {
        Decision::Record => match outcome {
            Ok(RevealedKey(key)) => record_revealed_key(deps, claim_id, game_id, key).await,
            Err(_) => FulfillResponse::Error {
                message: "internal: record decision without a revealed key".into(),
            },
        },
        // Spec §4 recover-then-record: NOT compensate.
        Decision::Compensate => {
            recover_already_redeemed_key(deps, claim_id, game_id, gamekey, machine_name).await
        }
        Decision::ParkCookieDead => {
            set_cookie_ok(deps, false).await;
            let msg = if deps.session_store.is_some() {
                COOKIE_DEAD_SELFHEAL_MSG
            } else {
                COOKIE_DEAD_MSG
            };
            ping_msg(deps, &OperatorMessage::literal(msg)).await;
            FulfillResponse::Parked {
                reason: "humble session needs attention".into(),
            }
        }
        Decision::Park => {
            let detail = match &outcome {
                Err(HumbleError::RedeemRefused { .. }) => "refused",
                Err(HumbleError::AmbiguousRedeem) => "ambiguous",
                Err(HumbleError::RateLimited) => "rate-limited",
                Err(HumbleError::RedeemAuthRejected { .. }) => "redeem-auth-rejected",
                Err(HumbleError::SecureAreaStepUpFailed { .. }) => "secure-area-step-up-failed",
                _ => "transient",
            };
            if let Err(HumbleError::SecureAreaStepUpFailed { reason }) = &outcome {
                ping_msg(deps, &OperatorMessage::fmt(
    "self-claim reveal for claim {} ({}) needed humble\'s secure-area step-up and it did not complete: {}. The key was NOT revealed — the claim is parked and reconcile will finish it.",
    &[Part::Id(claim_id), Part::Id(machine_name), Part::Id(reason)],
))
                .await;
            }
            FulfillResponse::Parked {
                reason: format!("self-claim reveal inconclusive: park for reconcile ({detail})"),
            }
        }
        Decision::DeadKey => {
            let reason = match &outcome {
                Err(HumbleError::KeyExpired { msg, code }) => match code {
                    Some(c) => format!("{msg} [code: {c}]"),
                    None => msg.clone(),
                },
                _ => "dead key (unclassified producer)".to_string(),
            };
            match fail_dead_key_any(deps, domain::SELF_LINK_TOKEN, claim_id, game_id, &reason).await
            {
                Ok(()) => {
                    tracing::warn!(claim_id, game_id, %reason, "dead key: claim terminally failed, slot returned, game retired");
                    ping_msg(deps, &OperatorMessage::fmt(
    "claim {} ({}) hit a DEAD key — humble says: \"{}\". The claim is failed (reason recorded on the claim), the game is retired as expired and will not re-list, and the slot was returned. Nothing retries this.",
    &[Part::Id(claim_id), Part::Id(game_id), Part::Id(&reason)],
))
                    .await;
                    FulfillResponse::KeyDead
                }
                Err(e) => {
                    ping_msg(deps, &OperatorMessage::fmt(
    "dead-key fail-claim write for {} failed: {} — still pending, reconcile retries",
    &[Part::Id(claim_id), Part::Error(logged(&e, "dead-key fail-claim write failed"))],
)).await;
                    FulfillResponse::Parked {
                        reason: "dead key detected but recording failed — will retry".into(),
                    }
                }
            }
        }
    }
}

/// Durable-first record of a revealed key + the RevealedKey response. Shared by the happy path
/// and the recover path.
async fn record_revealed_key(
    deps: &Deps,
    claim_id: &str,
    game_id: &str,
    key: String,
) -> FulfillResponse {
    match deps.store.fulfill_self_claim(claim_id, game_id, &key).await {
        Ok(()) => FulfillResponse::RevealedKey { key },
        Err(e) => {
            // Key exists but recording failed — loud, human decides. NEVER retry the reveal.
            // The ping names the claim, NEVER the key value.
            ping_msg(deps, &OperatorMessage::fmt(
    "self-claim fulfill failed for claim {}: {} — the key was revealed but not recorded; it is still readable in humble\'s library keys page.",
    &[Part::Id(claim_id), Part::Error(logged(&e, "self-claim fulfill failed"))],
)).await;
            FulfillResponse::Error {
                message: "key revealed but recording failed — flagged for ben".into(),
            }
        }
    }
}

/// AlreadyRedeemed recovery (spec §4): the key's value sits in the order's
/// `keys[].redeemed_key_val`. Re-read, match the tpk by machine_name, record.
/// Fallback when no value is present (e.g. the key was actually gifted away — gift-redeems may
/// not set redeemed_key_val): PARK + ping, never guess, never compensate blind.
async fn recover_already_redeemed_key(
    deps: &Deps,
    claim_id: &str,
    game_id: &str,
    gamekey: &str,
    machine_name: &str,
) -> FulfillResponse {
    let order = match deps.humble.order(gamekey).await {
        Ok(o) => o,
        Err(e) => {
            tracing::warn!(claim_id, gamekey = %gamekey, error = ?e, "self-claim recover: order re-read failed — parking");
            return FulfillResponse::Parked {
                reason: "recover re-read failed: park for reconcile".into(),
            };
        }
    };
    let tpk = order.keys.iter().find(|k| k.machine_name == machine_name);
    match tpk.and_then(|k| k.redeemed_key_val.clone()) {
        Some(val) => {
            tracing::info!(
                claim_id,
                "self-claim recover: redeemed_key_val present — recording"
            );
            record_revealed_key(deps, claim_id, game_id, val).await
        }
        None => {
            ping_msg(deps, &OperatorMessage::fmt(
    "self-claim {} ({}): humble says already-redeemed but the order carries no key value — it may have been gifted out-of-band. Parked for review; nothing was compensated.",
    &[Part::Id(claim_id), Part::Id(machine_name)],
))
            .await;
            FulfillResponse::Parked {
                reason: "already-redeemed with no recoverable key value".into(),
            }
        }
    }
}

/// Dispatch [`compensate_claim`](Store::compensate_claim) or
/// [`compensate_self_claim`](Store::compensate_self_claim) based on the claim's link token.
/// SELF claims have no link-meta item; the gift path needs the token to locate the link record.
/// Used by every reconcile arm that proves no pick was spent (bundle not-redeemed, choice A, B1).
async fn compensate_any(deps: &Deps, claim: &domain::Claim) -> Result<(), StoreError> {
    if claim.link_token == domain::SELF_LINK_TOKEN {
        deps.store
            .compensate_self_claim(&claim.id, &claim.game_id)
            .await
    } else {
        deps.store
            .compensate_claim(&claim.link_token, &claim.id, &claim.game_id)
            .await
    }
}

/// Dead-key dispatch — [`compensate_any`]'s sibling: SELF claims use the self variant
/// (no link decrement), everything else the gift variant.
async fn fail_dead_key_any(
    deps: &Deps,
    link_token: &str,
    claim_id: &str,
    game_id: &str,
    reason: &str,
) -> Result<(), StoreError> {
    if link_token == domain::SELF_LINK_TOKEN {
        deps.store
            .fail_self_claim_dead_key(claim_id, game_id, reason)
            .await
    } else {
        deps.store
            .fail_claim_dead_key(link_token, claim_id, game_id, reason)
            .await
    }
}

/// The choice branch of [`reconcile`]. Called for an aged `Pending` claim whose game
/// `requires_choice`, with a fresh `order`. Decides PURELY from the intent snapshot + the order
/// diff — it must NEVER call `choose_content` on any branch, and must not even take the offered id
/// (there is no choose argument here by construction). Compensation happens ONLY where the diff
/// PROVES no pick was spent (A / B1).
async fn reconcile_choice_claim(deps: &Deps, claim: &Claim, game: &Game, order: &Order) {
    match &claim.choice_pre_tpks {
        // A. No snapshot ⇒ the intent write never landed ⇒ choose was NEVER attempted (write order
        // §2.3) ⇒ pick NOT spent ⇒ compensate (slot returns, game re-lists). Same shape as the
        // bundle "not redeemed → compensate" arm. SELF uses compensate_self_claim (no link-meta).
        None => {
            // Diff against the empty baseline — title-scoped, NOT find_new_tpk: an unrelated tpk
            // elsewhere in the order (a different game entirely) must not park this claim, but ANY
            // key that could plausibly BE this game's pick must stop a blind compensate.
            let hits: Vec<&KeyEntry> = order
                .keys
                .iter()
                .filter(|k| k.human_name.eq_ignore_ascii_case(&game.title))
                .collect();
            match hits.as_slice() {
                [] => {
                    // Verified-nothing FOR THIS GAME: zero title-matched tpks exist anywhere in the
                    // order, so zero keys anyone could have revealed for it ⇒ choose never ran ⇒ pick
                    // NOT spent ⇒ compensate (slot returns, game re-lists). Same shape as the bundle
                    // "not redeemed → compensate" arm. SELF uses compensate_self_claim (no link-meta).
                    tracing::info!(claim_id = %claim.id, "reconcile(choice): no intent snapshot, no title-matched key anywhere in the order — compensating (verified nothing for this game)");
                    let _ = compensate_any(deps, claim).await;
                    ping_msg(deps, &OperatorMessage::fmt(
    "reconcile compensated choice claim {} ({}) — no choose intent was ever recorded, so the monthly pick was NOT spent — slot returned, game re-listed.",
    &[Part::Id(&claim.id), Part::Id(&game.title)],
))
                    .await;
                }
                [tpk] => {
                    // Absence of a measurement is not a measurement of absence: with no snapshot the
                    // wire cannot date this tpk (nothing ordinal on TpkWire) — attribution to THIS
                    // claim is unfounded. Park; the human attributes. SELF included: auto-recovery is
                    // a Some-only privilege.
                    tracing::warn!(claim_id = %claim.id, machine_name = %tpk.machine_name, redeemed = tpk.redeemed, "reconcile(choice): no intent snapshot, but a title-matched key exists on humble — NOT auto-compensating, parking");
                    let msg = if tpk.redeemed {
                        OperatorMessage::fmt(
                            "choice claim {} ({}) has no intent snapshot, but humble shows a key for \
                             this title (`{}`) already revealed — a pick was spent outside the app's \
                             writes. Cannot attribute it to this claim (no snapshot = no \
                             arrival-order evidence). Left pending for review.",
                            &[
                                Part::Id(&claim.id),
                                Part::Id(&game.title),
                                Part::Id(&tpk.machine_name),
                            ],
                        )
                    } else {
                        OperatorMessage::fmt(
                            "choice claim {} ({}) has no intent snapshot, but humble shows an \
                             unredeemed key for this title (`{}`). Cannot attribute it to this \
                             claim. Left pending for review.",
                            &[
                                Part::Id(&claim.id),
                                Part::Id(&game.title),
                                Part::Id(&tpk.machine_name),
                            ],
                        )
                    };
                    ping_msg(deps, &msg).await;
                }
                _ => {
                    tracing::warn!(claim_id = %claim.id, "reconcile(choice): no intent snapshot, multiple title-matched keys on humble — NOT auto-compensating, parking ambiguous");
                    ping_msg(deps, &OperatorMessage::fmt(
    "choice claim {} ({}) has no intent snapshot and multiple keys on humble could match the title. Left pending for review.",
    &[Part::Id(&claim.id), Part::Id(&game.title)],
))
                    .await;
                }
            }
        }
        Some(pre) => match find_new_tpk(order, pre, &game.title) {
            // B1. Snapshot present but no new tpk (and no exact-title match) ⇒ NEVER compensate, on
            // ANY route. The re-choose backstop (humble refuses "already chosen") only covers a
            // second PICK — it does nothing for re-LISTING a key that was already revealed: a
            // `Some` snapshot can hide a pick spent out of band (redeemed straight on humble,
            // outside this app, between the snapshot write and this reconcile pass), and
            // compensating would re-list a game whose key is already gone. Park + ping instead —
            // stays Pending for a human to look at the order directly. SELF and gift both park
            // identically; no store write at all.
            TpkPick::None => {
                tracing::warn!(claim_id = %claim.id, "reconcile(choice): snapshot present, no new tpk — NOT auto-compensating (a snapshot can hide an out-of-band spend), parking");
                ping_msg(deps, &OperatorMessage::fmt(
    "choice claim {} ({}) has an intent snapshot but no new key on humble — NOT auto-compensating: a snapshot can hide a pick spent out of band. Left pending for review.",
    &[Part::Id(&claim.id), Part::Id(&game.title)],
))
                .await;
            }
            // B2. Unique new tpk, NOT redeemed ⇒ pick SPENT, key not yet burned (crash between the
            // two writes) ⇒ complete the claim FROM RECONCILE — never choosing.
            // Gift → redeem as gift URL; SELF → reveal and record key value.
            // allow_heal=false: this pass's order read just proved the session live; a dead-session
            // call here simply leaves the claim Pending for the next sync (which heals + retries).
            TpkPick::Unique(tpk) if !tpk.redeemed => {
                let flavor = if claim.link_token == domain::SELF_LINK_TOKEN {
                    ClaimFlavor::SelfClaim
                } else {
                    ClaimFlavor::Gift
                };
                tracing::info!(
                    claim_id = %claim.id,
                    is_self = claim.link_token == domain::SELF_LINK_TOKEN,
                    "reconcile(choice): pick spent, key present + unredeemed — completing from reconcile (NO choose)"
                );
                let resp = claimed_tpk_terminal(
                    deps,
                    flavor,
                    &claim.id,
                    &claim.link_token,
                    &claim.game_id,
                    &order.gamekey,
                    tpk,
                    false,
                )
                .await;
                match resp {
                    FulfillResponse::GiftUrl { .. } | FulfillResponse::RevealedKey { .. } => {
                        tracing::info!(claim_id = %claim.id, "reconcile(choice): completed a crash-between-writes claim");
                    }
                    FulfillResponse::KeyDead => {
                        tracing::info!(claim_id = %claim.id, "reconcile(choice): dead key -- claim terminally failed from reconcile")
                    }
                    other => {
                        // Any non-success just leaves the claim Pending for the next pass (the
                        // executor already pinged the loud classes / handled AlreadyRedeemed → B3).
                        tracing::warn!(claim_id = %claim.id, ?other, "reconcile(choice): terminal did not complete — claim stays pending for the next pass");
                    }
                }
            }
            // B3. Unique new tpk, ALREADY redeemed ⇒ pick spent AND key burned/revealed.
            // Gift: URL unrecorded — leave Pending + human-recover ping (NEVER a key value).
            // SELF: key value may be recoverable from the order's redeemed_key_val; attempt
            //       recover_already_redeemed_key so the claim can complete autonomously.
            TpkPick::Unique(tpk) => {
                if claim.link_token == domain::SELF_LINK_TOKEN {
                    tracing::warn!(
                        claim_id = %claim.id,
                        "reconcile(choice): self-claim key already redeemed — recovering key from order"
                    );
                    let resp = recover_already_redeemed_key(
                        deps,
                        &claim.id,
                        &claim.game_id,
                        &order.gamekey,
                        &tpk.machine_name,
                    )
                    .await;
                    if let FulfillResponse::RevealedKey { .. } = resp {
                        let gift_flag = if tpk.is_gift == Some(true) {
                            " (humble marks it a gift)"
                        } else {
                            ""
                        };
                        ping_msg(deps, &OperatorMessage::fmt(
    "reconcile recovered the already-revealed key for self claim {} ({}) from the order — claim completed autonomously; the key was redeemed out of band{}.",
    &[Part::Id(&claim.id), Part::Id(&game.title), Part::Id(gift_flag)],
))
                        .await;
                    }
                } else {
                    tracing::warn!(claim_id = %claim.id, "reconcile(choice): key present but already redeemed — human recovery (URL unrecorded)");
                    ping_msg(deps, &OperatorMessage::fmt(
    "choice claim {} ({}) shows its key already redeemed on humble but no gift URL was recorded — recover it manually from humble\'s gift-history page. Claim left pending.",
    &[Part::Id(&claim.id), Part::Id(&game.title)],
))
                    .await;
                }
            }
            // B4. Two-or-more new tpks the title can't split (concurrent sibling on this month) ⇒
            // leave Pending + a distinct ping. A human decides; the next pass retries once the
            // sibling fulfills. NEVER a key value in the ping.
            TpkPick::Ambiguous => {
                tracing::warn!(claim_id = %claim.id, "reconcile(choice): ambiguous new tpks — leaving pending, human decides");
                ping_msg(deps, &OperatorMessage::fmt(
    "choice claim {} ({}) has multiple new keys on humble that the title can\'t disambiguate (a concurrent claim on this month?) — left pending for review.",
    &[Part::Id(&claim.id), Part::Id(&game.title)],
))
                .await;
            }
        },
    }
}

/// Catalog sync entry point. Takes the sync-run marker FIRST — a conditional put that makes
/// concurrent walks impossible no matter how many sync invokes are queued (admin double-click,
/// EventBridge overlap, async-invoke retry) — then runs the walk and releases the marker.
/// Two concurrent walks would double the humble request rate and race `put_sync_state`.
async fn handle_sync(deps: &Deps) -> FulfillResponse {
    let now = OffsetDateTime::now_utc().unix_timestamp();
    match deps.store.begin_sync_run(now).await {
        Ok(SyncBegin::Started) => {}
        // A live run owns the walk — skip; the owner reports via SyncState. Also skip when the
        // marker is unreadable: running unserialized is worse than missing one scheduled run.
        Ok(SyncBegin::AlreadyRunning) | Err(_) => {
            tracing::info!("sync skipped: another run holds the sync-run marker");
            return FulfillResponse::SyncDone;
        }
    }
    run_sync(deps).await;
    // Best-effort release — a failed delete only delays the next sync until the marker goes
    // stale (SYNC_RUN_STALE_SECS); it cannot wedge the system.
    let _ = deps.store.end_sync_run().await;
    FulfillResponse::SyncDone
}

/// Run a humble call through the ONE heal-then-retry-once ladder: on `Unauthorized`, self-heal
/// (when `allow_heal`) and retry the call exactly once. The heal outcome rides ALONGSIDE the
/// result instead of being folded into it, so durability survives the error path too — a heal
/// whose retry then hits a transient error (a 429 right after the login's extra requests) must
/// not let a caller go on asserting a healthy durable cookie. `None` means no heal was attempted
/// (the call didn't hit `Unauthorized`, or the cap disallowed one).
///
/// Membership rule (why this ladder may carry a WRITE): an op belongs on this ladder iff its
/// `Unauthorized` outcome PROVES the op had no effect. Reads qualify trivially. The gift redeem —
/// the one write here — qualifies because humble rejects a dead-session redeem at the AUTH layer
/// before the key is touched: the only redeem outcome that maps to `Unauthorized` is the
/// 200-with-HTML login interstitial (`decode_body` in humble-client), which is the session check
/// answering instead of the redeem handler. So an `Unauthorized` redeem provably did not burn the
/// key, and the healed retry is the first attempt that can — the same reasoning as the step-up
/// retry inside `redeem_as_gift` ("a gated redeem returns `login_required` BEFORE touching the
/// key"). No other redeem failure may ride this ladder: `RedeemAuthRejected` is a CSRF-layer
/// rejection on a LIVE session (a heal fixes nothing), and `AmbiguousRedeem` / `RedeemRefused` /
/// network errors can follow a request that REACHED the redeem handler — retrying any of those
/// could burn a key twice. Because the ladder retries on `Unauthorized` and nothing else, that
/// boundary holds by construction; a login itself never touches keys (see [`refresh_session`]).
///
/// Every self-healing humble call shares this ladder — the listing, the reconcile pass, the order
/// walk, and the gift redeem — so their durability bookkeeping can't diverge again.
/// [`handle_validate_cookie`] deliberately stays OUT: its no-retry, report-from-the-heal shape
/// is documented there.
async fn selfheal_once<T, F, Fut>(
    deps: &Deps,
    allow_heal: bool,
    op: F,
) -> (Option<Heal>, Result<T, HumbleError>)
where
    F: Fn() -> Fut,
    Fut: std::future::Future<Output = Result<T, HumbleError>>,
{
    match op().await {
        Err(HumbleError::Unauthorized) if allow_heal => {
            let heal = refresh_session(deps).await;
            let result = if heal.usable() {
                op().await
            } else {
                Err(HumbleError::Unauthorized)
            };
            (Some(heal), result)
        }
        other => (None, other),
    }
}

/// Diff-stamp `owned_by_ben` across `games` from an owned-appid list. Shared by the fetch
/// and fresh-cache lanes of [`refresh_ben_ownership`] (#47).
async fn stamp_owned(deps: &Deps, games: &[Game], appids: Vec<u32>) {
    let appid_set: std::collections::HashSet<u32> = appids.into_iter().collect();
    let mut stamped = 0usize;
    let mut unstamped = 0usize;

    for game in games {
        let Some(steam_app_id) = game.steam_app_id else {
            continue; // no appid — nothing to compare against
        };
        let owned = appid_set.contains(&steam_app_id);
        if owned == game.owned_by_ben {
            continue; // no change — skip the conditional write
        }
        match deps.store.set_game_owned_by_ben(&game.id, owned).await {
            Ok(OwnedWrite::Written) => {
                if owned {
                    stamped += 1;
                } else {
                    unstamped += 1;
                }
            }
            // A claim is in flight — the claim path owns this game's state for now.
            // The next sync will re-diff and write once the claim lands.
            Ok(OwnedWrite::Contested) => {
                tracing::debug!(game_id = %game.id, "steam owned refresh: game in-flight claim — skipping stamp");
            }
            Ok(OwnedWrite::NotFound) => {} // game was deleted between list and stamp — safe no-op
            Err(e) => {
                tracing::warn!(game_id = %game.id, error = ?e, "steam owned refresh: set_game_owned_by_ben failed");
            }
        }
    }

    tracing::info!(stamped, unstamped, "steam owned refresh: stamps updated");
}

/// Ownership pass: stamp `owned_by_ben` on every game that has a `steam_app_id`, using Ben's
/// Steam library fetched via the Web API.
///
/// Called once per sync, AFTER `map_missing_appids` (which writes tier-1 and title-pass appids),
/// so the appid coverage is as complete as possible before we diff ownership. `games` is the
/// sync's shared catalog scan with the mapper pass's writes applied (#47) — this pass no longer
/// re-scans.
///
/// ## Behavior (spec §3, M1)
///
/// - **Identity absent** → skip silently. Ben hasn't connected Steam yet.
/// - **Fresh STEAMOWN cache (≤24h)** → diff-stamp from the cached appids, ZERO Steam calls
///   (#47 perf), and the episode marker RESETS — a fresh entry can only come from a successful
///   post-episode-start fetch, so it is evidence the library reads again. Stated tradeoff: a
///   privacy flip inside the freshness window is detected (and pinged) up to 24h late.
/// - **`Ok(Games)`** → `put_steam_owned` (refresh the 7-day cache) + diff-stamp: write
///   `set_game_owned_by_ben` only for games whose `owned_by_ben` value CHANGED.
///   Games with no `steam_app_id` are skipped (nothing to compare against).
/// - **`Ok(Private)`** → keep stamps frozen (no writes). Log at INFO. Ping Ben ONCE PER EPISODE
///   with a clear message so he knows why badges stopped updating.
///
///   **Ping dedupe (#47):** two conditions — a STEAMOWN entry must be present (written only by
///   successful fetches, so its presence means "Private is a CHANGE, not the initial state"),
///   AND the persisted `SyncState.private_pinged` episode marker must be false. Presence alone
///   re-pinged every sync for the whole 7-day cache TTL, because a `Private` response
///   deliberately never touches the STEAMOWN entry. A successful `Ok(Games)` fetch resets the
///   marker — that is what ends an episode and re-arms the ping.
///
/// - **`Err(_)`** → keep stamps frozen, log at WARN. No ping — a transient error is not
///   actionable by Ben and should not noise the channel.
///
/// ## Disconnect / frozen stamps (deliberate design)
///
/// When Ben's Steam identity is removed (Task 9's handler deletes `CONFIG#STEAM`), this pass
/// skips silently on the next sync (identity absent → early return). Prior `owned_by_ben` stamps
/// are frozen in place — they are NOT mass-cleared here. The admin UI hides the owned-badge
/// column entirely when no Steam identity is configured (Task 11 checks identity presence), so
/// stale frozen stamps are invisible. A fresh re-connection and the next successful
/// `Ok(Games)` fetch will recompute and correct every stamp via the normal diff path.
///
/// The alternative — an HTTP-path O(catalog) mass-clear in Task 9's handler — would require
/// iterating and conditionally writing every game on delete. That's expensive, racey, and
/// unnecessary given the UI hides the column anyway. Keeping it here keeps the logic in one place.
///
/// Returns the `private_pinged` episode marker to persist (#47): `already_pinged` carried
/// through on every lane that learns nothing about privacy (client/identity skips, transient
/// errors), set true when a Private response is seen (pinging only if `!already_pinged` and a
/// prior success exists), and reset to false by evidence of a successful non-private fetch —
/// the Games arm directly, or a FRESH cache (which only a successful post-episode-start fetch
/// can produce). That reset is what ends an episode and re-arms the ping.
async fn refresh_ben_ownership(deps: &Deps, games: &[Game], already_pinged: bool) -> bool {
    // No Steam client → skip. Configured by the app at startup; if absent the whole Steam
    // feature is disabled and no identity can be fetched.
    let Some(steam) = deps.steam.as_ref() else {
        return already_pinged;
    };

    // No identity → skip silently. Ben hasn't linked his Steam account yet.
    let steamid = match deps.store.get_steam_identity().await {
        Ok(Some(id)) => id,
        Ok(None) => return already_pinged,
        Err(e) => {
            tracing::warn!(error = ?e, "steam owned refresh: get_steam_identity failed — skipping");
            return already_pinged;
        }
    };

    // One cache read serves both the freshness skip and the Private-arm presence dedupe.
    let cached = deps.store.get_steam_owned(&steamid).await.ok().flatten();

    // Freshness skip (#47 perf): a ≤24h-old STEAMOWN entry (proxy- or sync-written) makes the
    // Steam round-trip redundant — diff-stamp straight off the cached appids. Stated tradeoff:
    // a privacy flip inside the freshness window is detected (and pinged) up to 24h late.
    if let Some((appids, fetched_at)) = &cached {
        let now = time::OffsetDateTime::now_utc().unix_timestamp();
        if now - fetched_at <= dynamo::schema::STEAM_OWNED_FRESH_SECS {
            tracing::info!("steam owned refresh: cache fresh — stamping without a steam call");
            stamp_owned(deps, games, appids.clone()).await;
            // A fresh cache IS privacy news (OMBB, #128 review): the Private arm is only
            // reachable when the cache is stale/absent, so at episode start the cache was
            // stale — any fresh entry seen later was written by a successful Games fetch
            // AFTER the episode started (sync's Games arm resets the marker itself; the
            // proxies' write-through is the path that can't). Fresh cache ⇒ episode over.
            // Carrying the marker here would strand it true under steady friend traffic
            // and silently swallow the NEXT private episode's ping.
            return false;
        }
    }

    match steam
        .get_owned_games(&steam_client::SteamId64(steamid.clone()))
        .await
    {
        Ok(steam_client::OwnedGames::Games(appids)) => {
            // Refresh the 7-day owned-games cache so admin-api reads don't stale-out.
            let now = time::OffsetDateTime::now_utc().unix_timestamp();
            if let Err(e) = deps.store.put_steam_owned(&steamid, &appids, now).await {
                tracing::warn!(error = ?e, "steam owned refresh: put_steam_owned failed — continuing with in-memory appids");
            }
            stamp_owned(deps, games, appids).await;
            false // successful non-private fetch ends any private episode — re-arm the ping
        }

        Ok(steam_client::OwnedGames::Private) => {
            // Stamps remain frozen. Do NOT touch owned_by_ben.
            tracing::info!("steam owned refresh skipped: ben library reads private");

            // Ping ONCE per episode: prior-success presence dedupe (as before) AND the
            // persisted episode marker — presence alone re-pinged every sync for the whole
            // 7-day cache TTL, because a Private response deliberately never overwrites the
            // last good cache entry (#47).
            let prev_success = cached.is_some();
            let should_ping = prev_success && !already_pinged;
            if should_ping {
                ping_msg(deps, &OperatorMessage::literal("your steam \'game details\' privacy or the key\'s account changed — owned badges are frozen until fixed"))
                .await;
            }
            already_pinged || should_ping
        }

        Err(e) => {
            // Transient error — keep stamps, log, no ping, carry the marker.
            tracing::warn!(error = ?e, "steam owned refresh: get_owned_games failed — keeping prior stamps");
            already_pinged
        }
    }
}

/// Top-N user tags stored per app. Display caps are client-side (char-budget fit), so
/// tuning what cards SHOW never needs a backfill — only widening storage does (#71).
pub const STEAM_TAG_STORE_CAP: usize = 10;
// Compile-time pin: widening the store cap past what GetItems requests would silently
// truncate every app's stored tags (review round 2) — raise both or neither.
const _: () = assert!(STEAM_TAG_STORE_CAP <= steam_client::REQUESTED_TAG_COUNT);

/// Resolve one app's stored tag list from the batch results. `tag_data` None = the
/// GetItems/GetTagList batch failed → preserve the previous blob's tags (a network hiccup
/// must never strip chips). Present-but-absent appid in a SUCCESSFUL batch = Steam hides
/// the app from the browse surface → store empty and let genre fallback take over (#71).
fn tags_for_app(
    app_id: u32,
    tag_data: Option<&TagBatch>,
    prev_detail: Option<&steam_client::SteamAppDetail>,
) -> Vec<String> {
    match tag_data {
        Some((items, names)) => match items.get(&app_id) {
            Some(item) => item
                .tagids
                .iter()
                .filter_map(|id| names.get(id).cloned())
                .take(STEAM_TAG_STORE_CAP)
                .collect(),
            None => Vec::new(),
        },
        None => prev_detail.map(|d| d.tags.clone()).unwrap_or_default(),
    }
}

/// GetItems results + the tagid→name map, fetched together (both-or-nothing).
type TagBatch = (
    std::collections::HashMap<u32, steam_client::StoreItemTags>,
    std::collections::HashMap<u32, String>,
);

/// One batched GetItems+GetTagList fetch with the shape-drift plausibility guard.
/// `None` = failed or implausible → callers preserve existing tags. Both-or-nothing:
/// resolving tag names with a partial map would silently store a truncated tag list (#71).
async fn fetch_tag_batch(
    steam: &steam_client::SteamClient,
    ids: &[u32],
    ctx: &str,
) -> Option<TagBatch> {
    let (items_res, names_res) = tokio::join!(steam.get_store_items(ids), steam.get_tag_list());
    match (items_res, names_res) {
        // Plausibility guard (shape-drift): a non-empty request answered with ZERO items
        // is indistinguishable from a Valve field rename parsing as a "successful empty
        // response" — treating it as success would progressively wipe every app's tags,
        // invisibly behind the genre fallback. Real batches mix ordinary apps; all-gated
        // is not a plausible answer (#71).
        (Ok(items), Ok(names)) if items.is_empty() => {
            tracing::warn!(
                requested = ids.len(),
                names = names.len(),
                "{ctx}: GetItems answered a non-empty request with zero items — treating as batch failure (shape drift?)"
            );
            None
        }
        (Ok(items), Ok(names)) => Some((items, names)),
        (items_res, names_res) => {
            tracing::warn!(
                items_err = items_res.is_err(),
                names_err = names_res.is_err(),
                "{ctx}: tag batch failed — preserving existing tags"
            );
            None
        }
    }
}

/// appid → ids of every game mapped to it — the auto-hide sweep's input, built the same
/// way for enrichment and backfill so the sweep's coverage can't drift between them.
fn games_by_appid(games: &[domain::Game]) -> std::collections::HashMap<u32, Vec<String>> {
    let mut map: std::collections::HashMap<u32, Vec<String>> = std::collections::HashMap::new();
    for g in games {
        if let Some(id) = g.steam_app_id {
            map.entry(id).or_default().push(g.id.clone());
        }
    }
    map
}

/// Collect `app_id` into the sweep set iff the existing cache's detail carries an
/// auto-hide descriptor — the one definition of "cached descriptors say adult" shared
/// by every decide-pass collection site.
fn collect_adult(
    set: &mut std::collections::BTreeSet<u32>,
    app_id: u32,
    cache: Option<&dynamo::SteamAppCache>,
) {
    if has_adult_descriptors(cache.and_then(|c| c.detail.as_ref())) {
        set.insert(app_id);
    }
}

/// Freshly-PERSISTED descriptors supersede the decide-pass collection: a refetch that
/// cleared the adult descriptors must not be hidden by the very pass that proved them
/// gone — and only a successful put counts, so the sweep never acts on truth that was
/// never persisted (review round 2). Delisted is NOT a clear (no fresh descriptor
/// truth); callers only invoke this after a Found-refetch landed in the store.
fn supersede_adult(
    set: &mut std::collections::BTreeSet<u32>,
    app_id: u32,
    persisted_detail: Option<&steam_client::SteamAppDetail>,
) {
    if has_adult_descriptors(persisted_detail) {
        set.insert(app_id);
    } else {
        set.remove(&app_id);
    }
}

/// True when a detail carries any auto-hide descriptor ({3,4} — adult sexual content).
fn has_adult_descriptors(detail: Option<&steam_client::SteamAppDetail>) -> bool {
    detail.is_some_and(|d| {
        d.content_descriptor_ids
            .iter()
            .any(|id| domain::ADULT_HIDE_DESCRIPTOR_IDS.contains(id))
    })
}

/// One sweep over every appid known to carry adult descriptors: auto-hide each mapped
/// game Ben hasn't decided on. Shared verbatim by enrichment and backfill so the two
/// paths can never drift. Dynamo-only (no storefront calls), idempotent, safe to run
/// after a 429 abort. Returns how many games were newly hidden (#71).
async fn auto_hide_adult_games(
    store: &dynamo::Store,
    adult_appids: &std::collections::BTreeSet<u32>,
    games_by_appid: &std::collections::HashMap<u32, Vec<String>>,
) -> u32 {
    let mut hidden = 0u32;
    for app_id in adult_appids {
        for gid in games_by_appid.get(app_id).map(Vec::as_slice).unwrap_or(&[]) {
            match store.auto_hide_game(gid).await {
                Ok(dynamo::AutoHideWrite::Written) => {
                    hidden += 1;
                    tracing::info!(app_id, game_id = %gid, "auto-hide: hid adult game");
                }
                Ok(outcome) => {
                    tracing::debug!(app_id, game_id = %gid, ?outcome, "auto-hide: left alone");
                }
                Err(e) => {
                    tracing::warn!(app_id, game_id = %gid, error = ?e, "auto-hide: write failed");
                }
            }
        }
    }
    hidden
}

// ── #75: guarded STEAMAPP# persistence ───────────────────────────────────────

/// One pass's freshly fetched Steam halves for a single app — what the writer
/// wants to persist, independent of which snapshot it lands on (#75).
pub struct FetchedHalves {
    /// The pass clock: the `fetched_at`/`reviews_fetched_at` stamp for whichever
    /// halves are present.
    pub now: i64,
    /// Detail half, if fetched this pass.
    pub detail: Option<DetailFetch>,
    /// Reviews half (summary + recent histogram), if fetched this pass.
    pub reviews: Option<(steam_client::ReviewSummary, steam_client::RecentReviews)>,
}

/// Outcome of a detail fetch.
pub enum DetailFetch {
    Live(Box<steam_client::SteamAppDetail>),
    /// Steam says the app no longer exists: negative-cache stub. Stamps BOTH
    /// clocks (a dead app has no reviews to fetch).
    Delisted,
}

/// Newest-wins-per-half merge of this pass's fetched halves onto a store
/// snapshot. Pure — the single definition of the re-merge policy shared by
/// enrichment and backfill, unit-testable without staging a live race (#75).
///
/// Each half applies only if our stamp is >= the snapshot's (ties go to us: we
/// hold data fetched moments ago). A snapshot half NEWER than ours survives —
/// that's the concurrent writer's fresher fetch, not a loss.
/// Fetch-side frozen-at-death gate (#51): a stub snapshot (`detail: None`) freezes the reviews
/// fetch UNLESS this pass itself fetched Live detail — the relist exemption. At pass level this
/// gate only bites inside the JIT-stub concurrent-writer window: the decide gate never schedules
/// reviews for a stub it can already see, so a stub snapshot paired with scheduled reviews work
/// implies the stub landed between the decide read and this app's just-in-time read.
pub fn stub_freezes_reviews(
    snapshot: Option<&dynamo::SteamAppCache>,
    fetched: &Option<DetailFetch>,
) -> bool {
    snapshot.is_some_and(|c| c.detail.is_none()) && !matches!(fetched, Some(DetailFetch::Live(_)))
}

pub fn merge_fetched_halves(cache: &mut dynamo::SteamAppCache, ours: &FetchedHalves) {
    match &ours.detail {
        Some(DetailFetch::Live(d)) if ours.now >= cache.fetched_at => {
            cache.detail = Some((**d).clone());
            cache.fetched_at = ours.now;
        }
        Some(DetailFetch::Delisted) if ours.now >= cache.fetched_at => {
            cache.detail = None;
            cache.fetched_at = ours.now;
            // Both clocks, forward-only: never regress a fresher concurrent stamp.
            cache.reviews_fetched_at = cache.reviews_fetched_at.max(ours.now);
        }
        _ => {}
    }
    if let Some((overall, recent)) = &ours.reviews
        && ours.now >= cache.reviews_fetched_at
    {
        cache.overall = Some(overall.clone());
        cache.recent = Some(recent.clone());
        cache.reviews_fetched_at = ours.now;
    }
}

/// Result of [`persist_fetched_halves`].
pub enum PersistResult {
    /// The merged cache below is what's now in the store.
    Written {
        cache: Box<dynamo::SteamAppCache>,
        /// True = the first put lost a race and the retry (re-read + re-merge)
        /// landed.
        after_race: bool,
    },
    /// Two consecutive lost races — this app is skipped; the next pass retries.
    LostTwice,
}

fn snapshot_parts(
    app_id: u32,
    snapshot: Option<(dynamo::SteamAppCache, dynamo::SteamAppVersion)>,
) -> (dynamo::SteamAppCache, dynamo::SteamAppPutGuard) {
    match snapshot {
        None => (
            dynamo::SteamAppCache::empty(app_id),
            dynamo::SteamAppPutGuard::Absent,
        ),
        Some((c, v)) => (c, dynamo::SteamAppPutGuard::Unchanged(v)),
    }
}

/// The single home of the #75 write policy: guarded put; on a lost race,
/// re-read, re-merge (newest-wins per half — zero extra Steam calls, the data
/// is in hand), retry exactly once; a second loss yields the pass.
pub async fn persist_fetched_halves(
    store: &Store,
    app_id: u32,
    snapshot: Option<(dynamo::SteamAppCache, dynamo::SteamAppVersion)>,
    ours: &FetchedHalves,
) -> Result<PersistResult, StoreError> {
    let (mut cache, guard) = snapshot_parts(app_id, snapshot);
    merge_fetched_halves(&mut cache, ours);
    match store.put_steam_app(&cache, guard).await {
        Ok(()) => {
            return Ok(PersistResult::Written {
                cache: Box::new(cache),
                after_race: false,
            });
        }
        Err(dynamo::SteamAppPutError::Store(e)) => return Err(e),
        Err(dynamo::SteamAppPutError::LostRace) => {}
    }
    // Lost the race: someone wrote between our read and our put. Their write is
    // real data — re-read, merge ours onto THEIR item, retry once.
    let fresh = store.get_steam_app_versioned(app_id).await?;
    let (mut cache, guard) = snapshot_parts(app_id, fresh);
    merge_fetched_halves(&mut cache, ours);
    match store.put_steam_app(&cache, guard).await {
        Ok(()) => Ok(PersistResult::Written {
            cache: Box::new(cache),
            after_race: true,
        }),
        Err(dynamo::SteamAppPutError::Store(e)) => Err(e),
        Err(dynamo::SteamAppPutError::LostRace) => Ok(PersistResult::LostTwice),
    }
}

/// The budgeted, politely-paced Steam enrichment pass (spec §3). Runs in [`run_sync`] AFTER the
/// ownership pass, and hits the Steam storefront endpoints ONLY here — never at request time
/// (Ben's be-nice rule). Everything about it is throttled: `≥1.5s` between every storefront call,
/// a per-run cap of [`STEAM_ENRICH_MAX_APPS`] appids, a deadline guard so the sync's bookkeeping
/// always lands, and a hard abort on the first `429`.
///
/// `deadline` is the [`tokio::time::Instant`] past which no NEW app is started — the caller
/// computes it from the lambda budget (timeout − margin, anchored at the sync's start) so that
/// `persist_sync` + `end_sync_run` always run. Passed in (not read from a global clock) so tests
/// can drive the guard without a real 900s wait.
///
/// **Work list.** Every distinct `steam_app_id` across games whose STEAMAPP cache item is missing,
/// or whose `fetched_at` is older than 30d, or whose `reviews_fetched_at` is older than 14d. Sorted
/// ascending for a deterministic, resumable order. Capped at [`STEAM_ENRICH_MAX_APPS`]; the overflow
/// is counted into `deferred` and logged, never silently dropped.
///
/// **Fresh halves are preserved.** appdetails is refetched only when ITS clock is stale;
/// reviews+histogram only when THEIRS is. The two halves merge into the existing cache item, which
/// is written per-app as each one completes, so a mid-pass abort/timeout keeps the progress already
/// made.
///
/// **Negative cache.** A `Delisted` app writes a stub (`detail: None`) with BOTH clocks stamped, so
/// it's retried on the 30d window rather than every sync. A stub's reviews are FROZEN at
/// last-known (#51 family call, 2026-08-03): Steam does still serve reviews for delisted apps,
/// but a corpse's recent-histogram decays toward "no recent reviews" noise — refetching would
/// undersell the game exactly when the number matters, and it would bless an undocumented Steam
/// behavior. The 30d detail retry is the stub's ONLY heartbeat; a relist thaws the reviews
/// automatically on their next lapse.
///
/// **Error semantics.** A `429` (`RateLimited`) on appdetails aborts the pass on the spot; a `429`
/// on either reviews call banks any fetched detail half first (one persist), THEN aborts — what's
/// written stays, the rest waits for the next sync. Any other appdetails error logs and skips just
/// that app; any other reviews error keeps the fetched detail half, so only the reviews half
/// retries next sync (#51 item 2). The `SteamError` match names every variant (no `_` arm) — the
/// crate convention.
///
/// One summary log line per run: `steam enrichment: fetched=<n> fresh=<n> negative=<n>
/// lost_race=<n> aborted_429=<bool> auto_hidden=<n> tag_batch_failed=<bool>` (`fetched` = apps
/// whose cache item was written this run, `fresh` = of those, how many pulled live appdetails,
/// `negative` = delisted stubs, `lost_race` = apps whose guarded write hit a concurrent writer
/// at least once — re-merged and retried, or skipped on a second loss (#75), `auto_hidden` =
/// games newly hidden by the adult sweep, `tag_batch_failed` = the grep-able signal that
/// GetItems/GetTagList failed or answered implausibly — see #71), plus a `deferred` field.
pub async fn enrich_steam_apps(deps: &Deps, deadline: tokio::time::Instant) {
    // Kill switch (read via config, not raw env) → skip entirely.
    if deps.steam_enrich_disabled {
        tracing::info!("steam enrichment: disabled (STEAM_ENRICH_DISABLED) — skipping pass");
        return;
    }
    // No Steam client configured → the whole Steam feature is off; nothing to enrich.
    let Some(steam) = deps.steam.as_ref() else {
        return;
    };

    let games = match deps.store.list_all_games().await {
        Ok(g) => g,
        Err(e) => {
            tracing::warn!(error = ?e, "steam enrichment: list_all_games failed — skipping pass");
            return;
        }
    };

    let now = OffsetDateTime::now_utc().unix_timestamp();

    // Distinct appids across all games, sorted ascending (BTreeSet) for a deterministic order —
    // so a 429 abort or deadline stop resumes predictably on the next sync.
    let appids: std::collections::BTreeSet<u32> =
        games.iter().filter_map(|g| g.steam_app_id).collect();

    /// One appid's decided work: which half(s) are stale. (The decide-pass snapshot
    /// is deliberately NOT carried — the fetch loop re-reads just-in-time, #75.)
    struct Work {
        app_id: u32,
        need_detail: bool,
        need_reviews: bool,
    }

    // Every appid known to carry adult descriptors this pass — fed by BOTH the decide
    // loop (existing cache) and the fetch loop (fresh detail); swept once at the end.
    let mut adult_appids: std::collections::BTreeSet<u32> = std::collections::BTreeSet::new();

    // Decide the work list up front (cheap store reads only — no storefront calls yet).
    // ONE BatchGetItem drain instead of N serial GetItems (#47). Degradation shape change,
    // stated: a batch failure skips the whole pass (was: per-app skip) — enrichment is
    // best-effort and self-heals next sync either way. The fetch loop's versioned re-read
    // (#75 optimistic lock) is untouched.
    let appid_vec: Vec<u32> = appids.iter().copied().collect();
    let existing_caches = match deps.store.batch_get_steam_apps(&appid_vec).await {
        Ok(m) => m,
        Err(e) => {
            tracing::warn!(error = ?e, "steam enrichment: batch_get_steam_apps failed — skipping pass");
            return;
        }
    };
    let mut worklist: Vec<Work> = Vec::new();
    for app_id in appids {
        let existing = existing_caches.get(&app_id).cloned();
        // Adult appids are collected from the EXISTING cache too: a game newly mapped to
        // an already-fresh {3,4} cache (same adult game, second bundle) never enters the
        // worklist, and the sweep must still catch it. Cached descriptors don't need to
        // be fresh to be true; the sweep is idempotent (#71).
        collect_adult(&mut adult_appids, app_id, existing.as_ref());
        let (need_detail, need_reviews) = match &existing {
            // Missing item → fetch both halves.
            None => (true, true),
            Some(c) => (
                now - c.fetched_at >= STEAM_DETAIL_TTL_SECS,
                // Frozen-at-death (#51): a delisted stub (detail None) never schedules a
                // reviews refetch — pre-fix, a stub with a lapsed reviews clock but a fresh
                // detail clock re-entered the worklist every 14d window (the `delisted` flag
                // is only set by a same-pass detail fetch: the saw-tooth).
                now - c.reviews_fetched_at >= STEAM_REVIEWS_TTL_SECS && c.detail.is_some(),
            ),
        };
        if !need_detail && !need_reviews {
            continue; // both halves fresh — nothing to do
        }
        worklist.push(Work {
            app_id,
            need_detail,
            need_reviews,
        });
    }

    let deferred_by_cap = worklist.len().saturating_sub(STEAM_ENRICH_MAX_APPS);
    worklist.truncate(STEAM_ENRICH_MAX_APPS);

    // Community tags ride ONE batched keyless call pair per pass (GetItems chunks +
    // GetTagList), not per-app storefront calls. Both-or-nothing: resolving tag names
    // with a partial map would silently store a truncated tag list.
    // DECIDED DEVIATION from the spec's "GetItems 429 aborts the pass": ANY tag-batch
    // failure (429 included) logs, preserves existing tags, and lets the pass continue —
    // a keyless tag endpoint hiccup must not starve appdetails/reviews refreshes (#71).
    let detail_ids: Vec<u32> = worklist
        .iter()
        .filter(|w| w.need_detail)
        .map(|w| w.app_id)
        .collect();
    // Deadline guard mirrors the fetch loop's: a spent budget means ZERO storefront calls,
    // tag batch included — existing tags are preserved (tag_data None), not wiped.
    let mut tag_batch_failed = false;
    let tag_data = if detail_ids.is_empty() || tokio::time::Instant::now() >= deadline {
        None
    } else {
        let data = fetch_tag_batch(steam, &detail_ids, "steam enrichment").await;
        tag_batch_failed = data.is_none();
        data
    };

    // appid → game ids, for the auto-hide sweep (games is already in memory).
    let games_by_appid = games_by_appid(&games);

    let mut fetched = 0u32;
    let mut fresh = 0u32;
    let mut negative = 0u32;
    let mut lost_race = 0u32;
    let mut aborted_429 = false;
    let mut deferred_unstarted = 0usize;

    let mut it = worklist.into_iter();
    'apps: for work in it.by_ref() {
        // Deadline guard: never START a new app once the budget's nearly spent — this one and the
        // rest are deferred to the next sync.
        if tokio::time::Instant::now() >= deadline {
            deferred_unstarted += 1;
            break 'apps;
        }
        let Work {
            app_id,
            need_detail,
            need_reviews,
        } = work;
        // Just-in-time versioned read: the decide-pass snapshot can be minutes stale
        // by the time this item's turn comes (paced loop). The snapshot + token seed
        // the guarded merge write below — a concurrent writer inside the read→put
        // gap is now DETECTED (LostRace ⇒ re-merge + retry, #75), not silently
        // overwritten. The decide-pass snapshot only classified the work.
        let snapshot = match deps.store.get_steam_app_versioned(app_id).await {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!(app_id, error = ?e, "steam enrichment: re-read failed — skipping app");
                continue 'apps;
            }
        };
        let mut ours = FetchedHalves {
            now,
            detail: None,
            reviews: None,
        };
        let mut delisted = false;

        if need_detail {
            tokio::time::sleep(deps.steam_enrich_pace).await;
            match steam.get_app_details(app_id).await {
                Ok(steam_client::AppDetails::Found(d)) => {
                    let mut detail = *d;
                    detail.tags = tags_for_app(
                        app_id,
                        tag_data.as_ref(),
                        snapshot.as_ref().and_then(|(c, _)| c.detail.as_ref()),
                    );
                    ours.detail = Some(DetailFetch::Live(Box::new(detail)));
                    fresh += 1;
                }
                // Delisted: negative-cache stub. The merge stamps BOTH clocks so it's
                // retried on the 30d window (not every sync), and the reviews half is
                // FROZEN at last-known — see the frozen-at-death rule in the pass doc.
                Ok(steam_client::AppDetails::Delisted) => {
                    ours.detail = Some(DetailFetch::Delisted);
                    negative += 1;
                    delisted = true;
                }
                Err(steam_client::SteamError::RateLimited) => {
                    aborted_429 = true;
                    break 'apps;
                }
                Err(
                    e @ (steam_client::SteamError::Api(_)
                    | steam_client::SteamError::Network(_)
                    | steam_client::SteamError::Parse(_)
                    | steam_client::SteamError::KeyRejected
                    | steam_client::SteamError::NotFound
                    | steam_client::SteamError::OpenIdRejected(_)),
                ) => {
                    tracing::warn!(app_id, error = ?e, "steam enrichment: appdetails failed — skipping app");
                    continue 'apps;
                }
            }
        }

        // Frozen-at-death, fetch-level gate (#51): the just-in-time snapshot can reveal a stub
        // the decide pass didn't know about (concurrent writer), and `delisted` covers only a
        // same-pass discovery — both mean the reviews half stays frozen.
        let snapshot_is_stub =
            stub_freezes_reviews(snapshot.as_ref().map(|(c, _)| c), &ours.detail);
        // A reviews failure no longer throws away a fetched detail half (#51 item 2): the
        // block falls through to the persist below with `ours.reviews = None`, so the
        // appdetails politeness cost is banked and ONLY the reviews retry next sync.
        // A 429 still aborts the pass — after the persist.
        let mut abort_after_persist = false;
        if need_reviews && !delisted && !snapshot_is_stub {
            'reviews: {
                tokio::time::sleep(deps.steam_enrich_pace).await;
                let overall = match steam.get_review_summary(app_id).await {
                    Ok(s) => s,
                    Err(steam_client::SteamError::RateLimited) => {
                        aborted_429 = true;
                        abort_after_persist = true;
                        break 'reviews;
                    }
                    Err(
                        e @ (steam_client::SteamError::Api(_)
                        | steam_client::SteamError::Network(_)
                        | steam_client::SteamError::Parse(_)
                        | steam_client::SteamError::KeyRejected
                        | steam_client::SteamError::NotFound
                        | steam_client::SteamError::OpenIdRejected(_)),
                    ) => {
                        tracing::warn!(app_id, error = ?e, "steam enrichment: appreviews failed — keeping any fetched detail half");
                        break 'reviews;
                    }
                };
                tokio::time::sleep(deps.steam_enrich_pace).await;
                let recent = match steam.get_recent_reviews(app_id).await {
                    Ok(r) => r,
                    Err(steam_client::SteamError::RateLimited) => {
                        aborted_429 = true;
                        abort_after_persist = true;
                        break 'reviews;
                    }
                    Err(
                        e @ (steam_client::SteamError::Api(_)
                        | steam_client::SteamError::Network(_)
                        | steam_client::SteamError::Parse(_)
                        | steam_client::SteamError::KeyRejected
                        | steam_client::SteamError::NotFound
                        | steam_client::SteamError::OpenIdRejected(_)),
                    ) => {
                        tracing::warn!(app_id, error = ?e, "steam enrichment: histogram failed — keeping any fetched detail half");
                        break 'reviews;
                    }
                };
                ours.reviews = Some((overall, recent));
            }
        }

        // Nothing fetched for this app (reviews-only work that failed) — nothing to
        // persist; skip (or abort, post-429) exactly as before #51 item 2.
        if ours.detail.is_none() && ours.reviews.is_none() {
            if abort_after_persist {
                break 'apps;
            }
            continue 'apps;
        }

        // Merge write per-item: partial progress survives an abort/timeout later
        // in the pass. Guarded + re-merged per #75 — see persist_fetched_halves.
        let fresh_detail = matches!(ours.detail, Some(DetailFetch::Live(_)));
        match persist_fetched_halves(&deps.store, app_id, snapshot, &ours).await {
            Ok(PersistResult::Written { cache, after_race }) => {
                if after_race {
                    lost_race += 1;
                }
                fetched += 1;
                if fresh_detail {
                    supersede_adult(&mut adult_appids, app_id, cache.detail.as_ref());
                }
            }
            Ok(PersistResult::LostTwice) => {
                lost_race += 1;
                tracing::warn!(
                    app_id,
                    "steam enrichment: lost the STEAMAPP# race twice — skipping app, next sync retries"
                );
            }
            Err(e) => {
                tracing::warn!(app_id, error = ?e, "steam enrichment: put_steam_app failed — this app not persisted");
            }
        }
        // The reviews-429 abort, AFTER the detail half is banked (all persist outcomes —
        // a lost race or failed put doesn't un-rate-limit Steam).
        if abort_after_persist {
            break 'apps;
        }
    }
    // Whatever's left unstarted (deadline stop, or the tail after a 429 abort) is deferred too.
    deferred_unstarted += it.count();

    // The single adult sweep — deliberately AFTER a 429 abort too: it is dynamo-only, and
    // decide-loop-collected appids must still be swept even when the storefront walk died
    // early (#71).
    let auto_hidden = auto_hide_adult_games(&deps.store, &adult_appids, &games_by_appid).await;

    let deferred = deferred_by_cap + deferred_unstarted;
    tracing::info!(
        deferred,
        "steam enrichment: fetched={fetched} fresh={fresh} negative={negative} lost_race={lost_race} aborted_429={aborted_429} auto_hidden={auto_hidden} tag_batch_failed={tag_batch_failed}"
    );
}

/// Outcome of one [`backfill_steam_details`] run.
#[derive(Debug, Default)]
pub struct BackfillSummary {
    /// Live detail refetched and persisted.
    pub fetched: u32,
    /// Delisted stubs written.
    pub negative: u32,
    /// Skipped: `fetched_at` within the skip-fresh window (resume support).
    pub skipped: u32,
    /// Per-app failures (store read/write or storefront error) — logged, app skipped.
    pub failed: u32,
    /// True when a 429 aborted the run early. Rerun to resume; persisted progress survives.
    pub aborted_429: bool,
    /// Games auto-hidden by the adult-descriptor sweep during this run (#71).
    pub auto_hidden: u32,
    /// True when the GetItems/GetTagList batch failed or answered implausibly — refetched
    /// items kept their OLD tags but were stamped fresh, so the operator MUST rerun with
    /// SKIP_FRESH_SECS=0 or the catalog stays tagless until the 30-day window (#73 review).
    pub tag_batch_failed: bool,
    /// Apps whose STEAMAPP# write hit the #75 guard at least once this run.
    /// Most re-merge and land on the retry; an app that lost twice was NOT
    /// persisted this run (skipped, also counted in `failed` — next pass retries).
    pub lost_race: u32,
}

/// Run-once STEAMAPP# rebuild (issues #57, #61): refetch appdetails for EVERY catalog appid through
/// the current parse (id-allowlisted genres) and rewrite each item, preserving the reviews half
/// (`overall`, `recent`, `reviews_fetched_at`). Unlike [`enrich_steam_apps`] this ignores the
/// 30-day freshness window — refetching regardless is the point — but skips items whose
/// `fetched_at` is within `skip_fresh_secs`, so an aborted run resumes where it left off.
///
/// Takes `Store`/`SteamClient` directly rather than [`Deps`]: the caller is the feature-gated
/// `backfill_details` bin (human-run, never the lambda), which has no humble/webhook/session to
/// carry. Paced like the enrichment pass; a 429 aborts with `aborted_429` set.
pub async fn backfill_steam_details(
    store: &dynamo::Store,
    steam: &steam_client::SteamClient,
    pace: std::time::Duration,
    skip_fresh_secs: i64,
) -> Result<BackfillSummary, dynamo::StoreError> {
    let games = store.list_all_games().await?;
    // Distinct appids, ascending — deterministic order so an aborted run resumes predictably.
    let appids: std::collections::BTreeSet<u32> =
        games.iter().filter_map(|g| g.steam_app_id).collect();
    let total = appids.len();
    let now = OffsetDateTime::now_utc().unix_timestamp();
    let mut summary = BackfillSummary::default();

    let games_by_appid = games_by_appid(&games);
    let mut adult_appids: std::collections::BTreeSet<u32> = std::collections::BTreeSet::new();

    // Decide pass (store reads only): which appids get refetched, seeded with their
    // existing caches. Skip-fresh'd and failed-read items are accounted here. Adult
    // descriptors collect from EVERY existing cache — skip-fresh'd included — so a
    // resumed run never loses the sweep for items it doesn't refetch (#71).
    let mut worklist: Vec<u32> = Vec::new();
    for app_id in appids {
        let existing = match store.get_steam_app(app_id).await {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!(app_id, error = ?e, "backfill: get_steam_app failed — skipping app");
                summary.failed += 1;
                continue;
            }
        };
        collect_adult(&mut adult_appids, app_id, existing.as_ref());
        if let Some(c) = &existing
            && now - c.fetched_at < skip_fresh_secs
        {
            summary.skipped += 1;
            tracing::debug!(
                app_id,
                "backfill: fetched recently — skipped (resume window)"
            );
            continue;
        }
        worklist.push(app_id);
    }

    // ONE tag batch for exactly the ids being refetched: an all-skipped resumed run keeps
    // the zero-storefront-calls contract, and a post-429 resume doesn't re-burst the whole
    // catalog's tags to serve its tail (review round 1).
    let tag_data = if worklist.is_empty() {
        None
    } else {
        fetch_tag_batch(steam, &worklist, "backfill").await
    };
    summary.tag_batch_failed = !worklist.is_empty() && tag_data.is_none();

    for app_id in worklist {
        // Just-in-time versioned read (#75): snapshot + token seed the guarded
        // merge write; a concurrent writer is detected and re-merged, not
        // clobbered — the don't-run-during-cron rule is now politeness, not
        // load-bearing.
        let snapshot = match store.get_steam_app_versioned(app_id).await {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!(app_id, error = ?e, "backfill: re-read failed — skipping app");
                summary.failed += 1;
                continue;
            }
        };
        let mut ours = FetchedHalves {
            now,
            detail: None,
            reviews: None,
        };
        tokio::time::sleep(pace).await;
        match steam.get_app_details(app_id).await {
            Ok(steam_client::AppDetails::Found(d)) => {
                let mut detail = *d;
                detail.tags = tags_for_app(
                    app_id,
                    tag_data.as_ref(),
                    snapshot.as_ref().and_then(|(c, _)| c.detail.as_ref()),
                );
                ours.detail = Some(DetailFetch::Live(Box::new(detail)));
            }
            // Delisted: negative stub, BOTH clocks stamped by the merge — same
            // semantics as enrichment.
            Ok(steam_client::AppDetails::Delisted) => {
                ours.detail = Some(DetailFetch::Delisted);
            }
            Err(steam_client::SteamError::RateLimited) => {
                summary.aborted_429 = true;
                break;
            }
            Err(
                e @ (steam_client::SteamError::Api(_)
                | steam_client::SteamError::Network(_)
                | steam_client::SteamError::Parse(_)
                | steam_client::SteamError::KeyRejected
                | steam_client::SteamError::NotFound
                | steam_client::SteamError::OpenIdRejected(_)),
            ) => {
                tracing::warn!(app_id, error = ?e, "backfill: appdetails failed — skipping app");
                summary.failed += 1;
                continue;
            }
        }
        let fresh_detail = matches!(ours.detail, Some(DetailFetch::Live(_)));
        match persist_fetched_halves(store, app_id, snapshot, &ours).await {
            Ok(PersistResult::Written { cache, after_race }) => {
                if after_race {
                    summary.lost_race += 1;
                }
                // What was just written IS the outcome: Some detail = live refetch,
                // None = stub. (Post-merge, a concurrent writer's newer half may be
                // what landed — still the honest count.)
                if cache.detail.is_some() {
                    summary.fetched += 1;
                } else {
                    summary.negative += 1;
                }
                if fresh_detail {
                    supersede_adult(&mut adult_appids, app_id, cache.detail.as_ref());
                }
            }
            Ok(PersistResult::LostTwice) => {
                summary.lost_race += 1;
                summary.failed += 1;
                tracing::warn!(
                    app_id,
                    "backfill: lost the STEAMAPP# race twice — skipping app, next pass retries"
                );
                continue;
            }
            Err(e) => {
                tracing::warn!(app_id, error = ?e, "backfill: put_steam_app failed — this app not persisted");
                summary.failed += 1;
                continue;
            }
        }
        let done = summary.fetched + summary.negative + summary.skipped + summary.failed;
        tracing::info!(app_id, done, total, "backfill: item rewritten");
    }
    // Single sweep, shared with enrichment — runs even after a 429 abort (dynamo-only).
    summary.auto_hidden = auto_hide_adult_games(store, &adult_appids, &games_by_appid).await;
    tracing::info!(
        fetched = summary.fetched,
        negative = summary.negative,
        skipped = summary.skipped,
        failed = summary.failed,
        aborted_429 = summary.aborted_429,
        auto_hidden = summary.auto_hidden,
        tag_batch_failed = summary.tag_batch_failed,
        lost_race = summary.lost_race,
        "backfill: done"
    );
    Ok(summary)
}

/// Lazy title-pass: for every steam-type game with no `steam_app_id` (and not `Manual` source),
/// attempt a unique exact-title match against the Steam app list. Runs once per sync, AFTER the
/// order walk (tier-1 tpk ids are already written) so only genuinely unmapped games are touched.
///
/// Lazy: if no unmapped games exist, returns WITHOUT fetching the app list.
/// Resilient: 429 or any network/api failure from `get_app_list` logs a warning and skips the
/// pass — the sync NEVER fails because Steam is unreachable.
/// Returns the `(game_id, appid)` pairs actually WRITTEN, so the caller can apply them to its
/// shared in-memory scan — the ownership pass runs next and must see this pass's mappings
/// without a second full-catalog Scan (#47).
async fn map_missing_appids(deps: &Deps, games: &[Game]) -> Vec<(String, u32)> {
    let Some(steam) = deps.steam.as_ref() else {
        // No Steam client configured — skip the title pass but keep tier-1 ids already written.
        return Vec::new();
    };

    // Normalize: lowercase + trim + strip ™/® + collapse internal whitespace.
    // Stripping ™/® can leave a double space (e.g. "Cities: Skylines ™ II" →
    // "cities: skylines  ii"); split_whitespace().join(" ") collapses it.
    let normalize = |s: &str| -> String {
        s.to_lowercase()
            .replace(['™', '®'], "")
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
    };

    let manual_count = games
        .iter()
        .filter(|g| g.appid_source == Some(AppidSource::Manual))
        .count();

    // Candidates: steam key type, no appid, not Manual (a cleared override None/None participates).
    let to_map: Vec<&Game> = games
        .iter()
        .filter(|g| {
            g.key_type == "steam"
                && g.steam_app_id.is_none()
                && g.appid_source != Some(AppidSource::Manual)
        })
        .collect();

    let already_mapped = games.iter().filter(|g| g.steam_app_id.is_some()).count();

    if to_map.is_empty() {
        tracing::info!(
            mapped = already_mapped,
            unmapped = 0,
            manual = manual_count,
            "steam appid mapping: no unmapped games — skipping app list fetch"
        );
        return Vec::new();
    }

    // Fetch the app list — keyless endpoint, so no API key is sent.
    let app_list = match steam.get_app_list().await {
        Ok(list) => list,
        Err(steam_client::SteamError::RateLimited) => {
            tracing::warn!("steam appid mapping: 429 rate limited — skipping title pass this run");
            return Vec::new();
        }
        Err(
            e @ (steam_client::SteamError::Network(_)
            | steam_client::SteamError::Api(_)
            | steam_client::SteamError::Parse(_)
            | steam_client::SteamError::KeyRejected
            | steam_client::SteamError::NotFound
            | steam_client::SteamError::OpenIdRejected(_)),
        ) => {
            tracing::warn!(
                error = ?e,
                "steam appid mapping: network/api failure — skipping title pass this run"
            );
            return Vec::new();
        }
    };

    // Build name_lower → Vec<appid>. Duplicate names in Steam's list stay as duplicates — the
    // uniqueness check below skips any title that maps to more than one appid.
    let mut name_map: HashMap<String, Vec<u32>> = HashMap::new();
    for (appid, name) in &app_list {
        name_map.entry(normalize(name)).or_default().push(*appid);
    }

    let mut mapped = 0usize;
    let mut unmapped = 0usize;
    let mut written: Vec<(String, u32)> = Vec::new();

    for game in &to_map {
        let normalized = normalize(&game.title);
        match name_map.get(&normalized) {
            Some(ids) if ids.len() == 1 => {
                let appid = ids[0];
                match deps
                    .store
                    .set_game_steam_appid_if_unclaimed(&game.id, appid, AppidSource::Title)
                    .await
                {
                    Ok(dynamo::AppidWrite::Written) => {
                        mapped += 1;
                        written.push((game.id.clone(), appid));
                    }
                    Ok(_) => unmapped += 1, // NotFound / Skipped / Contested — leave unmapped
                    Err(e) => {
                        tracing::warn!(
                            game_id = %game.id,
                            error = ?e,
                            "steam appid mapping: write failed — leaving game unmapped"
                        );
                        unmapped += 1;
                    }
                }
            }
            _ => unmapped += 1, // No match or ambiguous (multiple Steam entries with same name)
        }
    }

    tracing::info!(
        "steam appid mapping: mapped={mapped} unmapped={unmapped} manual={manual_count}"
    );
    written
}

/// The sync walk. Runs [`pending_age_sweep`] first (the watchdog — see its doc for why it comes
/// before everything else), then [`reconcile`] (parked-claim recovery against humble truth), then
/// walks every order and upserts each key's `Game` via the guarded sync-upsert. Every exit path
/// persists a `SyncState` — the caller holds the run marker, so this must always report.
async fn run_sync(deps: &Deps) {
    tracing::info!("sync started (ensure session, reconcile, then order walk)");
    // Watchdog first (gate review B-4): needs no humble session — must run before
    // anything that can die. Do not add early returns above this line.
    pending_age_sweep(deps).await;
    // The private-library ping's episode marker (#47). Read once; every persist_sync lane
    // must carry it — a lane that never ran the ownership pass learned nothing and must not
    // reset the marker (that would re-arm the ping mid-episode).
    let prev_private_pinged = match deps.store.get_sync_state().await {
        Ok(Some(s)) => s.private_pinged,
        Ok(None) => false, // first sync ever — no episode to carry
        Err(e) => {
            // Fails toward one duplicate ping (marker re-armed), never toward silence.
            tracing::warn!(error = ?e, "sync: get_sync_state failed — private-ping marker read as false");
            false
        }
    };
    // Enrichment deadline is threaded from the caller via deps.steam_enrich_deadline. It was
    // computed from the lambda context's remaining time (minus the 180s margin) so `persist_sync`
    // + `end_sync_run` always have room to land — see compute_enrich_deadline.
    let enrich_deadline = deps.steam_enrich_deadline;
    // Acquire the library FIRST — this is the self-heal point (a dead session logs in + persists).
    // It MUST come before reconcile: reconcile reads humble per-order, so on a session that died
    // since the last run, running it first would Unauthorized-skip every claim and recover nothing.
    // Healing here means reconcile runs against a live session in the SAME sync.
    let (listing_heal, listing) = selfheal_once(deps, true, || deps.humble.gamekeys()).await;
    // ONE heal per sync run, total (listing + reconcile + walk). Uncapped, a single order URL
    // that persistently 403s with a live session — or an alternating die/heal pathology — would
    // turn one walk into up to N password+TOTP logins from the Lambda IP, exactly the
    // bot-detection profile self-login must avoid. A second `Unauthorized` after the run's heal
    // falls through to flag + ping + stop, as before self-login existed.
    let mut healed_this_run = listing_heal.is_some();
    // Persisted `cookie_ok` claims the DURABLE (SSM) cookie is valid. The LATEST heal's
    // durability is always the current SSM truth (`Persisted` is only returned after a
    // successful overwrite), so it replaces — never merely degrades — the running value.
    let mut cookie_ok = listing_heal.is_none_or(Heal::durable);
    let gamekeys = match listing {
        Ok(k) => k,
        // Dead AND self-login couldn't fix it (or isn't configured) → genuine attention needed.
        Err(HumbleError::Unauthorized) => {
            ping_msg(deps, &OperatorMessage::literal(COOKIE_DEAD_MSG)).await;
            persist_sync(
                deps,
                false,
                false,
                0,
                "humble session cookie is dead",
                prev_private_pinged,
            )
            .await;
            return;
        }
        Err(e) => {
            // Transient listing failure: reconcile doesn't need the listing, so parked-claim
            // recovery still runs this pass (it ran unconditionally before this ordering) —
            // a day's 429 on the listing shouldn't also cost a day of claim recovery.
            reconcile(deps, &mut healed_this_run, &mut cookie_ok).await;
            persist_sync(
                deps,
                false,
                cookie_ok,
                0,
                &format!("sync failed listing orders: {e}"),
                prev_private_pinged,
            )
            .await;
            return;
        }
    };

    // Reconcile parked claims against humble truth — now with a session known-good from the read above.
    reconcile(deps, &mut healed_this_run, &mut cookie_ok).await;

    let mut games_written = 0u32;
    let mut orders_failed = 0u32;
    // Built as the order walk reads orders; handed to choice discovery for order-authoritative
    // claimed-sets (spec D3) and the D2 gamekey ladder. Lives past the loop.
    let mut order_index = OrderIndex::default();
    // Order-walk truth for every key this pass actually FETCHED (#158 shelf-truth audit) — keyed
    // by (gamekey, machine_name) so it matches a row regardless of which id the D7 routing ladder
    // wrote its fresh copy under. A gamekey whose order read failed never inserts here: absence is
    // the rule, not a special case — see `shelf_truth_audit`.
    let mut truth: TruthMap = std::collections::HashMap::new();

    'orders: for gamekey in gamekeys {
        tokio::time::sleep(SYNC_PACE).await;
        // Session died mid-walk → the shared ladder heals (if the run's one heal is unspent) and
        // retries this order once. Without it, a mid-walk death would ping the dead-cookie
        // break-glass even when self-login is configured and would have healed it on the very
        // next run.
        let (heal, read) =
            selfheal_once(deps, !healed_this_run, || deps.humble.order(&gamekey)).await;
        if let Some(h) = heal {
            healed_this_run = true;
            cookie_ok = h.durable();
        }
        let order = match read {
            Ok(o) => o,
            // Still dead after the run's heal (or none possible) → flag + ping + stop early;
            // the manual SSM update IS the right break-glass once self-login itself has failed
            // (the failure reason was already pinged inside refresh_session).
            Err(HumbleError::Unauthorized) => {
                cookie_ok = false;
                ping_msg(deps, &OperatorMessage::literal(COOKIE_DEAD_MSG)).await;
                break 'orders;
            }
            Err(e) => {
                // #160 named "no warn" as half the silence: the count told an operator THAT an
                // order failed but never which one or why, so a systematic wire change (every
                // order arriving dict-less) read the same as one flaky fetch. The gamekey and
                // the error are the two facts that separate those.
                tracing::warn!(gamekey = %gamekey, error = ?e, "order read failed — skipping this gamekey (no truth recorded; audit will not act on it)");
                orders_failed += 1;
                continue;
            }
        };

        // Feed the choice-discovery index: this gamekey's claimed tpks (D3) + its product→gamekey
        // mapping (D2 rung 3). A failed read above `continue`d without inserting — that absence is
        // the "order silent" signal discovery keys off.
        order_index.tpks_by_gamekey.insert(
            order.gamekey.clone(),
            order.keys.iter().map(|k| k.machine_name.clone()).collect(),
        );
        if !order.product_machine_name.is_empty() {
            order_index
                .gamekey_by_product
                .insert(order.product_machine_name.clone(), order.gamekey.clone());
        }

        // domain::match_artwork wants (human_name, icon) pairs.
        let subs: Vec<(String, Option<String>)> = order
            .subproducts
            .iter()
            .map(|s| (s.human_name.clone(), s.icon.clone()))
            .collect();

        let mut order_failed = false;
        for key in &order.keys {
            // #158 shelf-truth audit: record this key's redeemed/expired truth BEFORE the D7
            // routing ladder below decides where the fresh write lands. A plain tpk's write always
            // targets game_id(gamekey, machine_name) — the audit will find that row already
            // corrected by the walk itself. A choice-suffixed tpk whose write D7 diverts onto its
            // offered sibling is the one case the walk's own write bypasses; the audit is what
            // catches that frozen tpk-named row.
            truth.insert(
                (order.gamekey.clone(), key.machine_name.clone()),
                TruthEntry {
                    redeemed: key.redeemed,
                    expired: key.expired,
                    key: key.clone(),
                },
            );
            // Spec D7: a choice-suffixed tpk may be the post-claim record of a game discovery
            // already surfaced under the OFFERED name — route onto that row so merge_sync flips it
            // (requires_choice true→false), instead of minting a sibling (15 live duplicate pairs
            // in prod, 2026-07-31 scan). Ladder, not set: exact base first (an offered name may
            // itself end _row), region-stripped second, FIRST hit wins. Id stability across syncs
            // rests on THIS routing re-deriving the id every pass, not on merge_sync preserving
            // machine_name (post-flip the row is Available → fresh wins → machine_name becomes the
            // tpk name; the id stays GK:offered only because routing keeps targeting it).
            let mut id = domain::game_id(&order.gamekey, &key.machine_name);
            if let Some((exact, stripped)) = domain::choice_tpk_bases(&key.machine_name) {
                for candidate in std::iter::once(exact).chain(stripped) {
                    let candidate_id = domain::game_id(&order.gamekey, &candidate);
                    match deps.store.get_game(&candidate_id).await {
                        Ok(Some(_)) => {
                            id = candidate_id; // offered row exists — route the fresh key onto it
                            break;
                        }
                        Ok(None) => {}
                        Err(e) => {
                            // Fail SAFE: on a read error mint under the tpk id (pre-D7 behavior)
                            // rather than dropping the key. A pair is loud and healable; a lost key
                            // is not.
                            tracing::warn!(error = ?e, candidate = %candidate_id, "D7 candidate lookup failed — minting under tpk id");
                            break;
                        }
                    }
                }
            }
            let game = Game {
                id,
                title: key.human_name.clone(),
                bundle: order.bundle_name.clone(),
                gamekey: order.gamekey.clone(),
                machine_name: key.machine_name.clone(),
                key_type: key.key_type.clone(),
                giftable: key.giftable,
                hidden: false,
                hidden_source: None,
                status: domain::sync_status(key.redeemed, key.expired),
                claim_id: None,
                artwork_url: domain::match_artwork(&key.human_name, &subs).map(str::to_string),
                keyindex: key.keyindex,
                // Sync walks order.keys — these all have a real redemption key already.
                // Choice discovery (which sets this true) is a separate ingest path.
                requires_choice: false,
                // Tier-1: flow the Steam App ID from the tpk wire data directly (78% coverage).
                steam_app_id: key.steam_app_id,
                appid_source: key.steam_app_id.map(|_| AppidSource::Humble),
                owned_by_ben: false,
            };
            match deps.store.upsert_game_from_sync(game).await {
                Ok(SyncWrite::Written) => games_written += 1,
                // Unchanged / SkippedInFlight (in-flight claim owns the game) — not a failure.
                Ok(_) => {}
                Err(_) => order_failed = true,
            }
        }
        if order_failed {
            orders_failed += 1;
        }
    }

    // ONE full-catalog Scan shared by the title pass, the ownership pass (#47), and the
    // shelf-truth audit (#158) — previously the first two each ran their own. Scanned AFTER
    // the order walk (its upserts must be visible). The title and ownership passes self-guard
    // when steam is absent; the audit's every-sync invariant must not inherit a stranger's
    // off-switch. The enrichment pass deliberately keeps its OWN scan: choice discovery writes
    // new games between these passes and enrichment must see them.
    let shared_scan: Option<Vec<Game>> = match deps.store.list_all_games().await {
        Ok(g) => Some(g),
        Err(e) => {
            tracing::warn!(error = ?e, "sync: list_all_games failed — skipping title + ownership passes");
            None
        }
    };

    // Title-pass: map any still-unmapped steam games by unique exact name match against the Steam
    // app list. Lazy — skips the GetAppList fetch if no unmapped games exist. 429/network errors
    // are logged and swallowed; the pass is best-effort and never blocks sync.
    let shared_scan = match shared_scan {
        Some(mut games) => {
            let written = map_missing_appids(deps, &games).await;
            // Apply this pass's mappings to the shared scan so the ownership pass diffs
            // against them without a re-Scan (it previously re-read the catalog).
            for (id, appid) in &written {
                if let Some(g) = games.iter_mut().find(|g| &g.id == id) {
                    g.steam_app_id = Some(*appid);
                    g.appid_source = Some(AppidSource::Title);
                }
            }
            Some(games)
        }
        None => None,
    };

    // Ownership pass: stamp owned_by_ben on every game with a steam_app_id. Runs AFTER the
    // mapper pass so appid coverage is as complete as possible before the diff. Failures are
    // logged and swallowed; the pass is best-effort and never blocks sync.
    let private_pinged = match &shared_scan {
        Some(games) => refresh_ben_ownership(deps, games, prev_private_pinged).await,
        None => prev_private_pinged, // pass skipped — carry the episode marker
    };

    // Choice-discovery ingest — surface each still-claimable OFFERED game as a `requires_choice=true`
    // catalog entry, so the gift-choice orchestration has something to run on. Runs AFTER the order
    // walk so a heal it triggers can't starve the walk, and it shares the run's one-heal budget via
    // `healed_this_run` / `cookie_ok`.
    games_written +=
        discover_choice_games(deps, &mut healed_this_run, &mut cookie_ok, &order_index).await;

    // Steam enrichment pass — budgeted, politely-paced storefront reads (appdetails + reviews +
    // histogram) into the STEAMAPP cache. Runs LAST (after choice discovery) so the 180s
    // deadline margin guards only the bookkeeping (`persist_sync` + `end_sync_run`) and choice
    // discovery's own network work is never squeezed into that margin. A 429 aborts the pass;
    // never fails the sync.
    enrich_steam_apps(deps, enrich_deadline).await;

    // Shelf-truth audit (#158): the every-sync backstop for the D7 frozen-sibling gap — no listable
    // row may keep referencing a key humble marks revealed or expired. Deliberately unconditional
    // (no steam gate), same reasoning as the shared scan above.
    // scan predates discover_choice_games' writes — harmless: offered names never exact-match tpk
    // names, and a stale-scan write CCFs into SkippedInFlight; next run re-reads.
    let (pulls, audit_rows_failed) = match &shared_scan {
        Some(scan) => shelf_truth_audit(deps, scan, &truth).await,
        None => (0, 0),
    };

    let msg = if cookie_ok {
        summary_line(games_written, orders_failed, pulls, audit_rows_failed)
    } else {
        // Covers both a hard-dead session and a heal whose SSM persist failed — either way the
        // DURABLE cookie can't be trusted; the pings that already fired carry the specifics.
        "humble session cookie is dead (or a refreshed one could not be persisted — see pings)"
            .to_string()
    };
    // ok = run completed with a live cookie AND no order-level failures.
    // cookie_ok tracks session health independently of order success rate.
    persist_sync(
        deps,
        cookie_ok && orders_failed == 0,
        cookie_ok,
        games_written,
        &msg,
        private_pinged,
    )
    .await;
    tracing::info!(games_written, orders_failed, cookie_ok, "sync finished");
}

/// Order-walk truth for one tpk, keyed by (gamekey, machine_name).
struct TruthEntry {
    redeemed: bool,
    expired: bool,
    key: KeyEntry,
}

type TruthMap = HashMap<(String, String), TruthEntry>;

/// The sync summary, extracted from an inline `format!` so it can be tested without a sync.
///
/// `audit_rows_failed` is NOT nested under `pulls` (#161). That nesting is the tempting shape —
/// both are audit numbers — and it is wrong in the one run that matters: rows that all FAIL to
/// write pull nothing, so `pulls` is 0 and a nested counter would go silent in exactly the sync
/// where every audit write broke. Pinned by
/// `summary_reports_audit_row_failures_even_when_nothing_was_pulled`.
fn summary_line(
    games_written: u32,
    orders_failed: u32,
    pulls: u32,
    audit_rows_failed: u32,
) -> String {
    let mut s = format!("sync ok: {games_written} written, {orders_failed} order(s) failed");
    if pulls > 0 {
        s.push_str(&format!(", {pulls} audit-pulled"));
    }
    if audit_rows_failed > 0 {
        s.push_str(&format!(", {audit_rows_failed} audit row(s) failed"));
    }
    s
}

/// The shelf-truth audit (#158): the order walk's own write only ever lands on the id its D7
/// routing ladder picks — for a plain tpk that's the same-id row (already corrected by the walk
/// itself, minutes before this runs), but for a choice-suffixed tpk whose offered sibling exists,
/// the routing ladder diverts the fresh write onto the OFFERED row and the tpk-named sibling is
/// never revisited. That frozen sibling can go on listing a key humble has since marked redeemed
/// (revealed outside the app) or expired, forever, unless something else corrects it. This pass is
/// that correction: every listable row whose (gamekey, machine_name) this pass's order walk
/// actually fetched, cross-checked against the walk's own truth.
///
/// Absence is the rule, not a special case: a row whose (gamekey, machine_name) tuple never
/// appears in `truth` — because its order read failed this pass, or the walk never saw that tuple
/// at all — is left completely untouched. The audit only ever acts on truth the walk itself
/// fetched; it never infers anything from a row's absence from the fetched set.
///
/// Returns `(rows pulled, rows whose write FAILED)` this pass, both for the sync summary.
///
/// The second number exists because it did not (#161): the per-row `Err` arm below was a
/// `tracing::warn!` and nothing more, while the order walk two hundred lines up has always
/// surfaced `orders_failed`. Same run, two loops, one of them counted — so a row that failed to
/// write on every single sync was invisible to anyone watching Discord, which is the only surface
/// Ben watches.
async fn shelf_truth_audit(deps: &Deps, scan: &[Game], truth: &TruthMap) -> (u32, u32) {
    struct Pulled {
        title: String,
        id: String,
        reason: &'static str,
        is_gift: Option<bool>,
    }
    let mut pulled: Vec<Pulled> = Vec::new();
    let mut rows_failed: u32 = 0;

    for g in scan.iter().filter(|g| g.is_listable()) {
        let Some(entry) = truth.get(&(g.gamekey.clone(), g.machine_name.clone())) else {
            // Never seen by this pass's order walk (order failed, or simply a different tuple) —
            // absence, not evidence. Leave it alone; a future successful fetch re-checks it.
            continue;
        };
        if !(entry.redeemed || entry.expired) {
            continue; // walk-fetched and still clean — nothing to correct.
        }

        // Build the fresh row from the EXISTING `g`, updating ONLY the truth fields — this is a
        // correction of an existing row, not the order-walk's own construction (which needs
        // order.bundle_name/subproducts that TruthEntry doesn't carry). merge_sync's Available arm
        // is fresh-wins on bundle/artwork_url/title (domain::merge_sync) — a guessed empty value
        // here would WIPE real data on every pulled row.
        let fresh = Game {
            status: domain::sync_status(entry.redeemed, entry.expired),
            giftable: entry.key.giftable,
            key_type: entry.key.key_type.clone(),
            keyindex: entry.key.keyindex,
            steam_app_id: entry.key.steam_app_id.or(g.steam_app_id),
            appid_source: entry
                .key
                .steam_app_id
                .map(|_| AppidSource::Humble)
                .or(g.appid_source),
            ..g.clone() // id (own id — NOT the D7 routing ladder: correcting an existing row, not
                        // minting), title, bundle, artwork_url, gamekey, machine_name, hidden,
                        // hidden_source, claim_id, requires_choice, owned_by_ben all carry from the
                        // row being corrected.
        };
        let reason = if entry.expired {
            "expired"
        } else {
            "revealed outside the app"
        };

        match deps.store.upsert_game_from_sync(fresh).await {
            Ok(SyncWrite::Written) => pulled.push(Pulled {
                title: g.title.clone(),
                id: g.id.clone(),
                reason,
                is_gift: entry.key.is_gift,
            }),
            // Set-driven, not retried: a concurrent claim/write CCFs into SkippedInFlight, and an
            // already-identical row is Unchanged — both self-correct on the next sync's fresh read.
            Ok(SyncWrite::SkippedInFlight) | Ok(SyncWrite::Unchanged) => {}
            Err(e) => {
                rows_failed += 1;
                tracing::warn!(
                    error = ?e,
                    game_id = %g.id,
                    "shelf audit: write failed — will retry next sync"
                );
            }
        }
    }

    if pulled.is_empty() {
        // Still return `rows_failed` — an all-failures pass pulls nothing, and returning a bare 0
        // here is precisely how the count would stay invisible in its worst run.
        return (0, rows_failed);
    }

    if pulled.len() > 3 {
        let ids: Vec<&str> = pulled.iter().map(|p| p.id.as_str()).collect();
        let titles: Vec<String> = pulled
            .iter()
            .map(|p| {
                if p.is_gift == Some(true) {
                    format!("{} (gift)", p.title)
                } else {
                    p.title.clone()
                }
            })
            .collect();
        tracing::warn!(
            ids = ?ids,
            "shelf audit: batch-pulled {} listed games whose keys are spent on humble",
            pulled.len()
        );
        ping_msg(
            deps,
            &OperatorMessage::fmt(
                "shelf audit: pulled {} listed games whose keys are spent on humble: {}",
                &[
                    Part::Id(&pulled.len().to_string()),
                    Part::Id(&titles.join(", ")),
                ],
            ),
        )
        .await;
    } else {
        for p in &pulled {
            tracing::warn!(
                game_id = %p.id,
                "shelf audit: pulled a listed game whose key is spent on humble"
            );
            // The conditional tail is a `&'static str` on BOTH branches, so it goes through
            // `Part::Text` rather than being appended to a runtime String — appending would have
            // meant a String constructor, which is the one door this type does not have.
            let gift_note = if p.is_gift == Some(true) {
                " (humble marks it a gift)"
            } else {
                ""
            };
            let text = OperatorMessage::fmt(
                "shelf audit: pulled {} ({}) — key {} on humble{}",
                &[
                    Part::Id(&p.title),
                    Part::Id(&p.id),
                    Part::Id(p.reason),
                    Part::Text(gift_note),
                ],
            );
            ping_msg(deps, &text).await;
        }
    }

    (pulled.len() as u32, rows_failed)
}

/// Choice-discovery ingest — the **sole intended writer** of `requires_choice = true` (see the
/// trust contract on [`domain::Game::requires_choice`]). A Humble Choice month grants picks that are
/// spent via `choosecontent`; until a pick is spent, the offered game has no redemption key and so
/// never appears in any `order.keys` walk. This pass is what surfaces those offered games into the
/// catalog as claimable entries.
///
/// Two-step, mirroring the read layer built in the choice-discovery client work:
/// 1. `choice_months` enumerates WHICH months exist (its `claimed_machine_names` is `None` — it
///    cannot see the picks, so it is *never* a source of `true`). We use it only for the month slugs.
/// 2. For each still-live month, the single-month `choice_month` read supplies the KNOWN claimed set,
///    and `ChoiceMonth::claimable_games` returns `offered − chosen`. Only this path may write `true`.
///
/// The offered game's `machine_name` is both the id fed to `choosecontent` and — per the id-agreement
/// obligation in the trust contract — the same `machine_name` the post-choose key record will carry,
/// so a later key-sync (which writes `requires_choice=false`) flips this entry via `merge_sync`
/// instead of duplicating it. Writes go through the guarded `upsert_game_from_sync`, never `put_game`.
///
/// Shares the run's one-heal budget (`healed` / `cookie_ok`) and runs LAST in [`run_sync`]. Returns
/// the count of newly-written offered games (folded into the sync's `games_written`).
/// A Humble Choice month's membership slug is deterministic: `<lowercase-month>-<year>` (e.g.
/// `june-2026`). The subscription list omits the 1-2 newest months, so discovery probes them by
/// building their slugs from `now`. Returns the current month and the preceding `count-1`, newest
/// first. `now` is injected so the construction is testable.
fn recent_month_slugs(now: OffsetDateTime, count: usize) -> Vec<String> {
    let mut year = now.year();
    let mut month = now.month() as u8; // time::Month: January = 1 ..= December = 12
    let mut slugs = Vec::with_capacity(count);
    for _ in 0..count {
        let name = match month {
            1 => "january",
            2 => "february",
            3 => "march",
            4 => "april",
            5 => "may",
            6 => "june",
            7 => "july",
            8 => "august",
            9 => "september",
            10 => "october",
            11 => "november",
            _ => "december",
        };
        slugs.push(format!("{name}-{year}"));
        if month == 1 {
            month = 12;
            year -= 1;
        } else {
            month -= 1;
        }
    }
    slugs
}

/// Everything choice-discovery needs from the order walk, built once as the walk reads orders.
/// `tpks_by_gamekey` is the ORDER-authoritative claimed record (spec D3 — the blob never alone
/// marks a game claimed); `gamekey_by_product` powers the D2 ladder rung 3 (order-product
/// machine_name → gamekey). A gamekey ABSENT from `tpks_by_gamekey` = its order read failed this
/// pass ("order silent") → discovery skips that month rather than guess.
#[derive(Default)]
pub(crate) struct OrderIndex {
    tpks_by_gamekey: std::collections::HashMap<String, Vec<String>>,
    gamekey_by_product: std::collections::HashMap<String, String>,
}

async fn discover_choice_games(
    deps: &Deps,
    healed: &mut bool,
    cookie_ok: &mut bool,
    orders: &OrderIndex,
) -> u32 {
    // Step 1: enumerate month slugs. A truncated walk (`complete == false`) simply means we discover
    // a prefix of months this pass — safe, because discovery only ADDS entries and never deletes on
    // absence, so a missed month just waits for the next run.
    let (heal, read) = selfheal_once(deps, !*healed, || {
        deps.humble
            .choice_months(CHOICE_DISCOVERY_MAX_PAGES, SYNC_PACE, CHOICE_WALK_DEADLINE)
    })
    .await;
    if let Some(h) = heal {
        *healed = true;
        *cookie_ok = h.durable();
    }
    let walk = match read {
        Ok(w) => w,
        // Dead after the run's heal (or none possible) → flag + ping, like the order walk. Discovery
        // is best-effort; the pings already fired carry the specifics.
        Err(HumbleError::Unauthorized) => {
            *cookie_ok = false;
            ping_msg(deps, &OperatorMessage::literal(COOKIE_DEAD_MSG)).await;
            return 0;
        }
        Err(e) => {
            tracing::warn!(error = ?e, "choice discovery: month enumeration failed — skipping this pass");
            return 0;
        }
    };
    if !walk.complete_for_choice() {
        tracing::warn!(
            max_pages = CHOICE_DISCOVERY_MAX_PAGES,
            "choice discovery: month walk truncated — discovered a prefix (additive; nothing deleted on absence)"
        );
    }

    let mut written = 0u32;
    // Targets = `(slug, is_probe)`: the newest months probed DIRECTLY (the list omits them, is_probe
    // = true), then every month the list DID enumerate (is_probe = false) — deduped, newest-first. We
    // read each via its membership page and do NOT pre-filter on the list's `can_redeem_games`
    // (unreliable for recent months); the page is the source of truth, gated on `detail.can_redeem_games`
    // below. Both tiers qualify — `choosecontent` works for pick-N and claim-all alike.
    let mut targets: Vec<(String, bool)> =
        recent_month_slugs(OffsetDateTime::now_utc(), CHOICE_DISCOVERY_RECENT_PROBE)
            .into_iter()
            .map(|s| (s, true))
            .collect();
    for m in &walk.months {
        if !targets.iter().any(|(s, _)| s == &m.product_url_path) {
            targets.push((m.product_url_path.clone(), false));
        }
    }
    // Ladder rung 2 (list): every enumerated month's slug → its list gamekey (Some entries only).
    let list_gamekey: std::collections::HashMap<&str, &str> = walk
        .months
        .iter()
        .filter_map(|m| Some((m.product_url_path.as_str(), m.gamekey.as_deref()?)))
        .collect();
    let started = tokio::time::Instant::now();
    let months_walked = targets.len() as u32;
    let mut months_processed = 0u32;
    let mut months_skipped = 0u32;
    let mut canary_unmatched_tpks = 0u32;
    for (slug, is_probe) in &targets {
        // Bound the per-month detail fan-out in aggregate (spec A5 / M6): the per-request timeout
        // caps one read, not ~77 of them. A partial pass is safe — discovery is additive, so a
        // month not reached this sync surfaces next sync.
        if started.elapsed() >= deps.choice_discovery_deadline {
            tracing::warn!(
                months_processed,
                "choice discovery: pass deadline — partial pass (additive, retried next sync)"
            );
            break;
        }
        tokio::time::sleep(SYNC_PACE).await;
        // A speculative probe NEVER spends the run's one heal: a not-yet-live month can 302 →
        // Unauthorized, which would both waste the heal and masquerade as a session death. Only a
        // list-enumerated month (a real month) may heal + treat Unauthorized as the cookie-dead signal.
        let allow_heal = !is_probe && !*healed;
        let (heal, read) = selfheal_once(deps, allow_heal, || deps.humble.choice_month(slug)).await;
        if let Some(h) = heal {
            *healed = true;
            *cookie_ok = h.durable();
        }
        let detail = match read {
            Ok(m) => m,
            // Probe hit a redirect/login page (a not-yet-live month) — skip, NOT a session death.
            Err(HumbleError::Unauthorized) if *is_probe => continue,
            Err(HumbleError::Unauthorized) => {
                *cookie_ok = false;
                ping_msg(deps, &OperatorMessage::literal(COOKIE_DEAD_MSG)).await;
                break;
            }
            Err(e) => {
                tracing::warn!(month = %slug, error = ?e, "choice discovery: month read failed — skipping");
                continue;
            }
        };
        // Gate on the membership PAGE's redeemability, not the list's — a month whose page can no
        // longer be redeemed carries no spendable pick, so skip it (no wasted writes on dead months).
        if !detail.can_redeem_games {
            months_skipped += 1;
            continue;
        }
        // Spec D2: the gamekey ladder — blob → list → order-side → loud skip. Never "".
        // Rung 3 derives the order-product machine_name from the slug ("july-2026" →
        // "july_2026_choice"). CONFIRMED against prod (2026-07-05 findings: the order endpoint's
        // product.machine_name IS "<month>_<year>_choice", e.g. "april_2026_choice"), the same
        // form the blob's productMachineName carries, so the two endpoints agree. Rung 3 is the
        // SOLE resolution for the active month. A rung-3 miss surfaces as the "gamekey_source =
        // none" skip below — a shape drift is a visible skip, never a silently-dark month.
        // Invariant: `slug` == the list's product_url_path key == the order-product construction
        // input — the same month identity across all three rungs.
        let slug_product = format!("{}_choice", slug.replace('-', "_"));
        let (month_gamekey, gamekey_source) = match (
            detail.gamekey.as_deref(),
            list_gamekey.get(slug.as_str()).copied(),
            orders
                .gamekey_by_product
                .get(&slug_product)
                .map(String::as_str),
        ) {
            (Some(g), _, _) => (g.to_string(), "blob"),
            (None, Some(g), _) => (g.to_string(), "list"),
            (None, None, Some(g)) => (g.to_string(), "order"),
            (None, None, None) => {
                months_skipped += 1;
                tracing::warn!(month = %slug, gamekey_source = "none",
                    choices_made_absent = detail.claimed_machine_names.is_none(),
                    "choice discovery: no gamekey from any rung — skipping (shape logged)");
                continue;
            }
        };
        if gamekey_source != "blob" {
            tracing::warn!(month = %slug, gamekey_source, "choice discovery: blob dropped gamekey — resolved via ladder");
        }
        // Spec D3: claimed is ORDER-authoritative; the blob's `contentChoicesMade` never alone marks
        // a game claimed. Order silent (its read failed this pass — the gamekey guarantees an order
        // exists) ⇒ skip the month LOUDLY and let the next sync retry: writing claimable rows on
        // missing evidence would mint ghosts that the additive/never-delete ingest keeps forever.
        let Some(order_tpks) = orders.tpks_by_gamekey.get(&month_gamekey) else {
            months_skipped += 1;
            tracing::warn!(month = %detail.product_url_path, gamekey = %month_gamekey,
                "choice discovery: order silent for month — skipping this pass (claimed-set unknowable, retried next sync)");
            continue;
        };
        months_processed += 1;
        // claimable = offered − (offered games a matching claimed tpk exists for). The matcher is
        // the prod-enumerated grammar (domain::choice_tpk_matches), NOT blob equality.
        let claimable: Vec<&OfferedGame> = detail
            .offered_games
            .iter()
            .filter(|o| {
                !order_tpks
                    .iter()
                    .any(|t| domain::choice_tpk_matches(t, &o.machine_name))
            })
            .collect();
        // Canary (spec D3): a `_choice_*` tpk matching NO offered name — expected for 1:N grants
        // (base + DLC tpks from one pick), so logged and counted, never month-fatal.
        let unmatched = order_tpks
            .iter()
            .filter(|t| domain::choice_tpk_bases(t).is_some())
            .filter(|t| {
                !detail
                    .offered_games
                    .iter()
                    .any(|o| domain::choice_tpk_matches(t, &o.machine_name))
            })
            .count();
        if unmatched > 0 {
            canary_unmatched_tpks += unmatched as u32;
            tracing::warn!(month = %detail.product_url_path, unmatched, "choice discovery: choice tpks matching no offered name (1:N grants or new grammar) — counted, not fatal");
        }
        // Per-month observability: which months surfaced how many claimable offered games. Turns
        // "why did this month write nothing?" from a guessing game into a log line.
        tracing::info!(
            month = %detail.product_url_path,
            gamekey = %month_gamekey,
            offered = detail.offered_games.len(),
            claimed_tpks = order_tpks.len(),
            claimable = claimable.len(),
            "choice discovery: month processed"
        );
        for offered in claimable {
            let game = Game {
                id: domain::game_id(&month_gamekey, &offered.machine_name),
                title: offered.title.clone(),
                bundle: detail.title.clone(),
                gamekey: month_gamekey.clone(),
                machine_name: offered.machine_name.clone(),
                // No key exists until the pick is spent, so the offered wire carries no key platform.
                // Placeholder; `merge_sync` refreshes `key_type` from the real key-sync `fresh` once a
                // pick lands (the id matches by the machine_name agreement above), so it self-corrects.
                key_type: "steam".to_string(),
                giftable: true,
                hidden: false,
                hidden_source: None,
                status: GameStatus::Available,
                claim_id: None,
                artwork_url: None,
                keyindex: 0,
                requires_choice: true,
                steam_app_id: None,
                appid_source: None,
                owned_by_ben: false,
            };
            match deps.store.upsert_game_from_sync(game).await {
                Ok(SyncWrite::Written) => written += 1,
                // Unchanged / SkippedInFlight (an in-flight claim owns the game) — not a failure.
                Ok(_) => {}
                Err(e) => {
                    tracing::warn!(machine_name = %offered.machine_name, error = ?e, "choice discovery: upsert failed")
                }
            }
        }
    }
    // One summary per pass (spec D5): turns "did discovery do anything?" into a single line.
    tracing::info!(
        months_walked,
        months_processed,
        months_skipped,
        stop_reason = ?walk.stop,
        canary_unmatched_tpks,
        written,
        "choice discovery: pass summary"
    );
    written
}

/// ── Pending-age sweep (spec §3, set-driven; placement per gate review B-4) ──────
/// The FIRST thing run_sync does, before any humble acquisition: the sweep needs
/// only dynamo + the discord webhook, so no session death, listing failure, or
/// reconcile regression can starve it. The invariant is on the SET: every claim
/// both pending and at least RECONCILE_STUCK_ALERT_AGE old pings, every sync, until
/// a terminal transition removes it from the GSI. Daily-by-cadence (sync schedule),
/// deliberately NOT deduplicated: a once-ever alert that scrolls away IS the
/// silent-loop bug this exists to kill (family review 2026-07-29). Placement is
/// PINNED by stale_pending_claim_pings_even_when_listing_is_dead — an early return
/// added above this call fails that test.
async fn pending_age_sweep(deps: &Deps) {
    let claims = match deps.store.list_pending_claims().await {
        Ok(c) => c,
        Err(_) => return, // can't read this pass — the next sync retries.
    };
    let now = OffsetDateTime::now_utc();
    for claim in &claims {
        let age = now - claim.created_at;
        if age >= RECONCILE_STUCK_ALERT_AGE {
            let days = age.whole_days();
            tracing::warn!(
                claim_id = %claim.id,
                game_id = %claim.game_id,
                age_days = days,
                "pending-age sweep: claim is still pending past the alert age"
            );
            ping_msg(deps, &OperatorMessage::fmt(
    "claim {} ({}) is STILL PENDING after ~{}d. Reconcile retries it every sync (or cannot reach it — see logs for this run). It will nag daily until it completes, compensates, or fails.",
    &[Part::Id(&claim.id), Part::Id(&claim.game_id), Part::Id(&days.to_string())],
))
            .await;
        }
    }
}

/// Reconcile parked (`Pending`) claims older than [`RECONCILE_MIN_AGE`] against humble's truth.
/// - key shows **redeemed** on humble → the gift WAS generated but we crashed before recording the
///   URL. This endpoint can't recover the URL → ping ben (claim id + game context, never a key
///   value) and leave the claim pending: loud, human-owned recovery via humble's gift-history page.
/// - key **not redeemed** → the redeem never landed → `compensate_claim` (slot + game return).
/// - transient humble fetch error → skip that claim; the next pass retries.
/// - claim is structurally unreconcilable (unsplittable `game_id`, or machine_name absent from
///   the order's keys) → skip, but LOUDLY once it's past [`RECONCILE_STUCK_ALERT_AGE`]: such a
///   skip repeats identically forever, so it warns + pings instead of leaking the slot in silence.
///   The skip itself is unchanged — reconcile still decides nothing for these claims.
/// - session dead mid-pass → self-heal via the shared ladder (respecting the caller's
///   once-per-run cap via `healed_this_run`); if still dead, stop the pass LOUDLY (warn +
///   `cookie_ok=false`) instead of silently skipping every remaining claim — the caller's order
///   walk hits the same dead session moments later and pings.
async fn reconcile(deps: &Deps, healed_this_run: &mut bool, cookie_ok: &mut bool) {
    let claims = match deps.store.list_pending_claims().await {
        Ok(c) => c,
        Err(_) => return, // can't read pending claims this pass — try again next time.
    };
    let now = OffsetDateTime::now_utc();
    for claim in claims {
        let age = now - claim.created_at;
        if age < RECONCILE_MIN_AGE {
            continue; // too fresh — a live redeem may still be recording.
        }
        // game_id is "gamekey:machine_name" (gamekey carries no colon).
        let Some((gamekey, machine_name)) = claim.game_id.split_once(':') else {
            alert_unreconcilable(
                deps,
                &claim,
                age,
                "its game_id has no `gamekey:machine_name` shape, so there is no order to check \
                 it against",
            )
            .await;
            continue;
        };
        let (heal, read) =
            selfheal_once(deps, !*healed_this_run, || deps.humble.order(gamekey)).await;
        if let Some(h) = heal {
            *healed_this_run = true;
            *cookie_ok = h.durable();
        }
        let order = match read {
            Ok(o) => o,
            Err(HumbleError::Unauthorized) => {
                // Dead and the run's heal is spent (or failed): every remaining claim would fail
                // identically — stop loudly rather than skip them one by one in silence.
                *cookie_ok = false;
                tracing::warn!(
                    "reconcile: session dead mid-pass — abandoning remaining parked claims until next sync"
                );
                return;
            }
            Err(_) => continue, // transient — skip this claim, reconcile again next pass.
        };
        // Choice claims reconcile by a DIFFERENT rule (never re-choose): the parked claim's
        // game_id offered-id never equals any tpk machine_name, so the bundle `find` below would
        // miss it forever and silently skip it every pass. Routing keys on the CLAIM's own
        // immutable `choice_pre_tpks` snapshot FIRST — `requires_choice` on the game row is
        // D7-mutable (a key-sync flips it false the moment a tpk appears), so a claim born choice
        // must still route choice even after the flip. `game.requires_choice` stays in the OR only
        // for legacy pre-snapshot choice claims (no snapshot was ever recorded for them). One extra
        // GetItem per parked claim; a transient game-read miss falls through to the bundle path
        // unchanged (that path needs no game read).
        if let Ok(Some(game)) = deps.store.get_game(&claim.game_id).await
            && (game.requires_choice || claim.choice_pre_tpks.is_some())
        {
            // reconcile may WRITE now (redeem/compensate) — pace it under the bot-detection floor.
            tokio::time::sleep(SYNC_PACE).await;
            reconcile_choice_claim(deps, &claim, &game, &order).await;
            continue;
        }
        let Some(key) = order.keys.iter().find(|k| k.machine_name == machine_name) else {
            let mut reason = format!(
                "machine_name `{machine_name}` is not among order `{gamekey}`'s keys on \
                 humble, so there is nothing to reconcile it against"
            );
            // Messaging-only probe (#158 task 8): the exact `find` above just missed, but the
            // order may still carry a choice-shaped tpk whose derived base equals this claim's
            // machine_name — an out-of-band-redeemed key humble is quietly holding under a name
            // the exact match can never see. Same grammar rung heal_pairs already uses. This ARMS
            // NOTHING — no routing change, no write — it only enriches the reason string.
            if let Some(hit) = order
                .keys
                .iter()
                .find(|k| domain::choice_tpk_matches(&k.machine_name, machine_name))
            {
                let mut flags = String::new();
                if hit.redeemed {
                    flags.push_str(", already revealed outside the app");
                }
                if hit.expired {
                    flags.push_str(", expired");
                }
                let tpk_machine_name = &hit.machine_name;
                reason.push_str(&format!(
                    "; NOTE: humble carries a key for this game under `{tpk_machine_name}`{flags}"
                ));
            }
            alert_unreconcilable(deps, &claim, age, &reason).await;
            continue;
        };
        if key.redeemed {
            if claim.link_token == domain::SELF_LINK_TOKEN {
                // SELF: the key value may be recoverable from the order's redeemed_key_val.
                // recover_already_redeemed_key re-reads the order, extracts the value, and
                // records it — completing the claim autonomously. NEVER a key value in logs.
                tracing::warn!(
                    claim_id = %claim.id,
                    "reconcile: self-claim parked shows redeemed on humble — recovering key from order"
                );
                let resp = recover_already_redeemed_key(
                    deps,
                    &claim.id,
                    &claim.game_id,
                    gamekey,
                    machine_name,
                )
                .await;
                if let FulfillResponse::RevealedKey { .. } = resp {
                    // The bundle path deliberately never reads the game row (see the routing
                    // comment above) — there is no `game` in scope here, so the title binding is
                    // the order's own key name.
                    let gift_flag = if key.is_gift == Some(true) {
                        " (humble marks it a gift)"
                    } else {
                        ""
                    };
                    ping_msg(deps, &OperatorMessage::fmt(
    "reconcile recovered the already-revealed key for self claim {} ({}) from the order — claim completed autonomously; the key was redeemed out of band{}.",
    &[Part::Id(&claim.id), Part::Id(&key.human_name), Part::Id(gift_flag)],
))
                    .await;
                }
            } else {
                tracing::warn!(claim_id = %claim.id, "reconcile: parked claim shows redeemed on humble but no URL recorded — human recovery");
                // Gift generated but URL unrecorded; leave pending (human-owned recovery). Message
                // carries claim id + human game context only — NEVER a key value.
                ping_msg(deps, &OperatorMessage::fmt(
    "parked claim {} ({} / {}) shows redeemed on humble but no gift URL was recorded — recover manually via humble\'s gift-history page",
    &[Part::Id(&claim.id), Part::Id(&order.bundle_name), Part::Id(&key.human_name)],
))
                .await;
            }
        } else if claim.link_token == domain::SELF_LINK_TOKEN {
            // SELF: the reveal never landed — attempt a late reveal (plan B1, allow_heal=false).
            // A race where the key was actually burned hits AlreadyRedeemed inside
            // reveal_claimed_tpk → recover_already_redeemed_key safely; no double-spend is possible.
            tracing::info!(claim_id = %claim.id, "reconcile: self-claim parked, not redeemed on humble — revealing (plan B1)");
            let _ = reveal_claimed_tpk(deps, &claim.id, &claim.game_id, gamekey, key, false).await;
        } else {
            // Gift: the redeem/reveal never landed on humble → return the slot and re-list the game.
            //
            // Risk bound (this arm's worst case is NOT a double-spend): the compensate arm assumes a
            // gifted key would show redeemed here (redeemed_key_val set). If humble does NOT set that
            // on a gift, a crash-after-gift claim reconciles as not-redeemed → compensate → re-list.
            // But the re-listed game can only be re-claimed and re-redeemed, and humble REFUSES to
            // re-redeem an already-burned key (→ AlreadyRedeemed → compensate). So no key is ever
            // double-spent; the residual is a RECOVERABLE lost gift URL (the first gift's URL wasn't
            // recorded) plus re-list churn. The ping below surfaces every compensate so that
            // recoverable case is caught. (Confirming whether a gift sets redeemed_key_val — which
            // would route the crash-after-gift case to the redeemed/URL-recovery branch instead — is
            // a non-urgent follow-up: the plan-2 live receipt.)
            tracing::info!(claim_id = %claim.id, "reconcile: parked gift claim not redeemed on humble — compensating (slot returns, game re-lists)");
            let _ = compensate_any(deps, &claim).await;
            // Ping every reconcile compensate. Self-login keeps the session alive 24/7, so this arm
            // runs autonomously on every sync — the dead-cookie stall that used to force a human to
            // look is gone. The ping restores that checkpoint: a compensate of a key that was in fact
            // gifted is a recoverable lost URL, and the operator sees it here to recover it from
            // humble's gift-history page.
            ping_msg(deps, &OperatorMessage::fmt(
    "reconcile compensated parked claim {} ({} / {}) as not-redeemed — slot returned, game re-listed. No key can be double-spent (humble refuses re-redeem of a burned key); but IF this key was actually gifted, its gift URL is lost — recover it from humble\'s gift-history page.",
    &[Part::Id(&claim.id), Part::Id(&order.bundle_name), Part::Id(&key.human_name)],
))
            .await;
        }
    }
}

/// A parked claim reconcile structurally can't act on repeats its silent skip on every pass — the
/// slot leaks and the friend stays stuck with zero operator signal. Past
/// [`RECONCILE_STUCK_ALERT_AGE`] that goes loud: `warn!` + one discord ping. Younger than that it
/// stays log-only (`debug!`) — the mismatch may be a transient deploy artifact the next sync fixes.
/// `reason` names the structural cause and MUST carry no key/cookie/URL secret (claim id + human
/// context only, same discipline as reconcile's other pings).
async fn alert_unreconcilable(
    deps: &Deps,
    claim: &domain::Claim,
    age: time::Duration,
    reason: &str,
) {
    if age < RECONCILE_STUCK_ALERT_AGE {
        tracing::debug!(
            claim_id = %claim.id,
            game_id = %claim.game_id,
            "reconcile: skipping an unreconcilable parked claim (still young — not yet alerting)"
        );
        return;
    }
    let hours = age.whole_hours();
    tracing::warn!(
        claim_id = %claim.id,
        game_id = %claim.game_id,
        age_hours = hours,
        "reconcile: parked claim is unreconcilable and STUCK — {reason}"
    );
    ping_msg(deps, &OperatorMessage::fmt(
    "parked claim {} (game_id {}) has been stuck ~{}h and reconcile cannot act on it: {}. Nothing self-heals this — the link slot stays consumed until someone looks. Fix the claim/game_id by hand (or compensate it) to free the slot.",
    &[Part::Id(&claim.id), Part::Id(&claim.game_id), Part::Id(&hours.to_string()), Part::Id(reason)],
))
    .await;
}

/// Validate the humble session by making a cheap authenticated call, self-healing a dead session
/// (log in + persist a fresh cookie) before reporting, and record the result in `SyncState.cookie_ok`.
/// With self-login configured this is what keeps the session alive with no human intervention.
///
/// Transient errors (rate-limited, API errors, network failures) do NOT update the persisted
/// cookie state — the cookie's validity is unknown, and writing `cookie_ok=false` on a 429
/// would be wrong. Only `Unauthorized` (after a self-login attempt) is a definitive dead signal.
async fn handle_validate_cookie(deps: &Deps) -> FulfillResponse {
    // Report health from the HEAL outcome, not a retry read. A successful login inside
    // refresh_session IS proof of a good session, so on a dead cookie we don't re-read (which could
    // hit a transient 429 right after the two extra login requests and leave cookie_ok stale-false
    // even though the session is now fine). But only a DURABLE heal may report healthy: after
    // login-ok-but-persist-failed, SSM still holds the dead cookie — and main rebuilds the client
    // from SSM per invoke — so persisting cookie_ok=true would disagree with the very cookie the
    // next invoke reads (a gift would park with a "cookie is DEAD" ping minutes after validate
    // said healthy). The persist-failure ping already fired inside refresh_session.
    let healthy: Option<bool> = match deps.humble.gamekeys().await {
        Ok(_) => Some(true),
        Err(HumbleError::Unauthorized) => Some(refresh_session(deps).await.durable()),
        // Transient (rate-limited / API / network): validity unknown — leave SyncState untouched.
        Err(_) => None,
    };
    tracing::info!(?healthy, "cookie validation (self-heal on dead)");
    match healthy {
        Some(ok) => {
            let mut st = deps
                .store
                .get_sync_state()
                .await
                .ok()
                .flatten()
                .unwrap_or_default();
            st.cookie_ok = ok;
            let _ = deps.store.put_sync_state(&st).await;
            FulfillResponse::CookieStatus { ok }
        }
        None => FulfillResponse::Error {
            message: "humble unreachable — cookie state unknown, try again".into(),
        },
    }
}

const COOKIE_DEAD_MSG: &str = "humble session cookie is DEAD and self-login could not heal it (not configured, or failed — \
     a failure pings separately with the reason) — break-glass: update the humble-cookie SSM \
     param directly (AWS console/CLI, SecureString overwrite).";
const COOKIE_DEAD_SELFHEAL_MSG: &str = "humble session died during a gift redeem and the in-line self-heal could not revive it — \
     claim parked for reconcile (the self-login ping just before this one has the details); \
     break-glass: update the humble-cookie SSM param directly (AWS console/CLI, SecureString overwrite).";
const SESSION_HEALED_MSG: &str = "humble session had died and self-login refreshed it automatically (no action needed). If these \
     recur often, the account may be trending toward a rate-limit or new-device challenge.";
const SESSION_PERSIST_FAILED_MSG: &str = "humble self-login worked but writing the refreshed cookie to SSM FAILED — the session is fine \
     this run, but every invoke will re-login until the write succeeds (check the fulfillment \
     ssm:PutParameter grant / SSM health).";

/// Flip ONLY `cookie_ok` on the persisted `SyncState`, leaving the rest of the run summary
/// (last_run_epoch / ok / games_written / message) intact. Used by the gift path (post-heal) and
/// the ParkCookieDead arm, which learn cookie health OUTSIDE a sync run and must not fabricate the
/// rest of the summary.
///
/// A transient `get_sync_state` error SKIPS the write rather than defaulting: collapsing an error
/// to `SyncState::default()` and writing it back would clobber the real last-run metadata (the
/// admin dashboard's last-run/games-written/message) to zeroes over a momentary DynamoDB blip. A
/// genuinely-absent state (`Ok(None)`) still seeds from default — that's the correct first-write.
async fn set_cookie_ok(deps: &Deps, cookie_ok: bool) {
    match deps.store.get_sync_state().await {
        Ok(existing) => {
            let mut st = existing.unwrap_or_default();
            st.cookie_ok = cookie_ok;
            let _ = deps.store.put_sync_state(&st).await;
        }
        Err(e) => {
            // Don't clobber real metadata on a read blip; the health signal isn't worth losing the
            // run summary. cookie_ok self-corrects on the next sync/validate.
            tracing::warn!(error = ?e, cookie_ok, "set_cookie_ok: get_sync_state failed — skipping the flag write to avoid clobbering the run summary");
        }
    }
}

/// Persist a sync-run summary. A run fully describes itself EXCEPT `private_pinged`, which is
/// episode state spanning runs — lanes where the ownership pass never ran must pass the carried
/// previous value, or a dead-cookie run would silently re-arm the private-library ping (#47).
async fn persist_sync(
    deps: &Deps,
    ok: bool,
    cookie_ok: bool,
    games_written: u32,
    message: &str,
    private_pinged: bool,
) {
    let st = SyncState {
        last_run_epoch: OffsetDateTime::now_utc().unix_timestamp(),
        ok,
        cookie_ok,
        games_written,
        message: message.to_string(),
        private_pinged,
    };
    let _ = deps.store.put_sync_state(&st).await;
}

/// POST one already-rendered body. Returns 1 on failure, 0 on success, so callers can COUNT
/// without being able to PROPAGATE.
///
/// Never returns `Err` — a dead webhook must not break fulfilment, and the `-> u32` keeps that a
/// structural guarantee rather than a convention every call site must honour. But infallible is
/// not the same as silent: the doc comment on `ping` licensed not *propagating* a failure, and
/// never licensed not *recording* one.
///
/// **The `Ok(r)` non-success arm below is the load-bearing one.** `reqwest`'s `Err` arm catches
/// TRANSPORT failure only; a 400/401/404/429 arrives as `Ok(response)` with a non-success status.
/// Without that arm, such a response is a silent success — a failure routed into the success path.
/// Discord rate-limits, so those are the notifications that vanish under exactly the load that
/// makes them matter. Pinned by `ping_treats_non_2xx_as_failure`; delete the arm and that test
/// goes red. (This paragraph said "`.error_for_status()` is the load-bearing call" until the #171
/// gate — there is no `error_for_status()` in this function and there never was in this shape. The
/// arm does that job. A doc naming a call the code does not make sends the next reader looking for
/// a guard that isn't there, and — worse — invites them to "restore" it on top of the one that is.)
async fn deliver(http: &reqwest::Client, url: &str, content: &str) -> u32 {
    // `allowed_mentions` is load-bearing, not boilerplate (#174). Without it Discord parses
    // `@everyone` / `@here` / `<@&role>` out of `content` — and THE CONTENT IS NOT OURS. `Part::Id`
    // is the RUNTIME door BY TYPE: `Part::Text` takes `&'static str`, `Part::Id` takes `&'a str`.
    // **So every argument that goes through `Part::Id` is a runtime value, and `allowed_mentions`
    // must cover ALL of them.** That is the whole justification for this field, it is checkable
    // from the type signature alone, and it cannot go stale. Everything below is colour.
    //
    // WHERE THOSE VALUES COME FROM — two classes, and the second is the one that bites:
    //   1. **Built here from Humble's wire data.** Titles and names (`title`, `game.title`,
    //      `key.human_name`, `order.bundle_name`, `tpk.machine_name`); `domain::game_id`, which is
    //      `format!("{gamekey}:{machine_name}")` — two wire values concatenated, no hash, no
    //      validation, so **an "id" is not automatically ours**; and error text carried verbatim
    //      (`HumbleError::KeyExpired{msg}` at :984 :1497 :1621 :1801 — "one truth from wire to
    //      dynamo", a good property and an untrusted one; `ChooseFailed{reason}` ← `body.errormsg`
    //      at :1686). Note the same NAME cuts both ways: `reason` is ours at :939 :1460 :1583 :1679
    //      :1779, where every one is `SecureAreaStepUpFailed` and its reasons are our own literals.
    //   2. **Never built here at all — DESERIALIZED.** `FulfillRequest` is `#[derive(Deserialize)]`
    //      (:113). `claim_id`, `game_id`, `gamekey`, `machine_name` are fields of the INVOKE
    //      PAYLOAD, destructured at :630+ and handed to every handler below. Grepping for where
    //      these are constructed finds `Uuid::new_v4()` in public-api/admin-api and that is
    //      **the wrong answer to the right question** — those are what the CALLERS put in the
    //      payload, not what this lambda receives.
    // >>> **WHAT MAKES CLASS 2 OURS IS AN IAM BOUNDARY, NOT A CONSTRUCTOR.** The
    // >>> `invoke_fulfillment` policy (`terraform/aws-lambda.tf:155`) is attached at exactly two
    // >>> places — `lambda_public_api` (:94) and `lambda_admin_api` (:137) — plus an EventBridge
    // >>> permission scoped to the sync rule, which sends `Sync` and carries no claim fields.
    // >>> **Widen who may invoke this function and these strings stop being ours**, with nothing
    // >>> in this crate changing to tell you. That is the sentence to keep.
    //     AND WHEN YOU GO CHECK THAT BOUNDARY, **LAMBDA HAS TWO GRANT MECHANISMS** (@oldmanbendobot
    //     again): the IDENTITY-based policy above, and RESOURCE-based `aws_lambda_permission`
    //     blocks — `eventbridge_sync` is one, and `aws-apigateway.tf:79/:87` are two more that
    //     happen to target the API lambdas rather than this one. Following `invoke_fulfillment`
    //     never traverses them, so **a future `aws_lambda_permission` on fulfillment would widen
    //     this exact boundary while the policy named above stays untouched and correct-looking.**
    //     Worse, `aws_lambda_function_url` is an invoke door with **no action string to grep at
    //     all** (verified absent workspace-wide, 2026-08-08, along with any wildcard action and any
    //     other grant naming the fulfillment ARN). *Enumerating one mechanism is not enumerating
    //     the door* — the same defect as everything else in this comment, one layer out.
    //
    // A NOTE ON METHOD, because it cost four passes (@oldmanbendobot found the last one). This
    // comment said "several", then 31, then 35, then 45 — four counts, and every miss was an
    // argument that READ like ours. Tracing each form to its construction site fixed three of them
    // and **structurally could not fix the fourth: a value that is never constructed here cannot be
    // found by looking for where it is constructed. It arrives.** So do not trust a number in this
    // comment as a safety property — trust the type invariant at the top, which covers all of them
    // without needing to be recounted.
    //
    // `operator_message` closes this trust boundary against DISCLOSURE — an error's text cannot
    // reach Discord, enforced by the type. This is the SAME boundary with a different verb: not
    // what the text reveals, but what the channel DOES with it. A type can keep a secret out of a
    // string; it cannot stop the receiver treating that string as a command. That half has to be
    // said here, at the one place the message becomes a request.
    //
    // `{"parse": []}` leaves the text intact and readable and denies it the power to notify.
    // Do not "simplify" this away: the message renders identically with and without it, so its
    // absence is invisible in every operator channel until the day it isn't.
    // Pinned by `operator_posts_never_carry_mention_permission`, which asserts on the payload we
    // send rather than on Discord's behaviour — Discord's behaviour is not ours to test, the field is.
    //
    // THE DOOR, NAMED HERE ON PURPOSE (#176 ②): `{"parse": []}` is a BLANKET deny, and the thing it
    // denies is not only the attack. The day someone adds "wake Ben when fulfilment is actually
    // broken", they will put `<@id>` in the message text and watch it **silently not ping** —
    // nothing errors, the mention just renders as inert text. That is a long-fuse trap whose victim
    // is the one message that most needs to arrive, and the debugging session it costs happens far
    // from this line. The shape that keeps both properties is an allow-list, not a blanket deny:
    //
    //     "allowed_mentions": { "parse": [], "users": [BEN_ID] }   // deny by default, one door
    //
    // It is deliberately NOT written that way today: no such page exists or is planned in this
    // repo, and there is no operator id in config. Plumbing an unused door is worse than naming
    // one. If you are here because your page didn't fire — this is why, and that line is the fix.
    // (Not hypothetical: Lilith hit the identical missing field in `claude-code-infra#61`, where a
    // blanket deny would have welded shut the "page Ben when the box is dying" remedy her #57 asks
    // for. Same bug, third language. #176 ① — `"flags": 4`, SUPPRESS_EMBEDS, so a title containing
    // a URL cannot make the channel fetch and render it — is a separate, deliberate, open call.)
    let body = serde_json::json!({
        "content": content,
        "allowed_mentions": { "parse": [] },
    });
    match http.post(url).json(&body).send().await {
        Ok(r) if r.status().is_success() => 0,
        // The status IS inspected. Without this arm a 400/401/404/429 is `Ok(response)` and
        // vanishes into the success path.
        Ok(r) => {
            tracing::error!(
                outcome = "discord_non_2xx",
                status = r.status().as_u16(),
                "operator notification rejected"
            );
            1
        }
        Err(e) => {
            // GATE BLOCKER (OMBB, #171): `without_url()` is load-bearing, not tidying.
            // `reqwest::Error`'s `Display` APPENDS the request URL, unredacted —
            // `reqwest-0.12.28/src/error.rs:267-269`, `write!(f, " for url ({url})")` — and the URL
            // IS attached on exactly the failures this arm catches (`async_impl/client.rs:3071`
            // for request errors, `:3048`/`:3056` for timeouts). A Discord webhook URL carries its
            // token in the path, and THIS CODEBASE ALREADY CLASSIFIES IT AS A CREDENTIAL
            // (`main.rs:109-112`: SecureString, "#81 … and never logs the value"). So the value was
            // decrypted under KMS, deliberately kept out of the read-side logs, and then printed in
            // full by the one line that reports a dead webhook — on every connection reset.
            //
            // PRE-EXISTING, NOT INTRODUCED HERE: `origin/main`'s `ping()` had
            // `eprintln!("discord ping failed (non-fatal): {e}")` — same `Display`, same leak. What
            // this PR did was PROMOTE it from unstructured stderr into a structured, indexed,
            // metric-filterable field. More durable and more greppable is worse, for a credential.
            //
            // `without_url()` (`error.rs:88`) drops the url and keeps the kind, which is the half
            // that has diagnostic value. Do not swap this back to `%e` for a "better message".
            tracing::error!(
                outcome = "discord_transport",
                error = %e.without_url(),
                "operator notification transport failure"
            );
            1
        }
    }
}

/// The prefix every operator message carries. Counted against the 2000 budget by `chunks`.
const PING_PREFIX: &str = "🐱 bendobundles: ";

/// Record an error on the CloudWatch side of the trust boundary and hand back the operator-safe
/// reference to it. **This is the only shape a call site should use.**
///
/// The join is the whole point: the full error (payload, chain, whatever it carries) goes to
/// CloudWatch, which is access-controlled; the operator channel, which is not, gets
/// `[kind req id]`. Both sides read the id from the SAME `ErrorSummary` via `log_fields`, in the
/// same call, so they cannot drift.
///
/// STATED LIMIT: `ErrorSummary::of` remains public, so this is the *convenient* door, not the
/// *only* one — a site that mints a summary without logging it produces an unjoinable id, and that
/// is caught by review, not by the type. Capturing emitted log records to test it would need a
/// dependency; the trade was made deliberately (zero new deps, 4GB box).
fn logged<E: std::error::Error>(e: &E, what: &'static str) -> ErrorSummary {
    let s = ErrorSummary::of(e);
    let (req_id, kind) = s.log_fields();
    tracing::error!(
        outcome = "operator_reported_error",
        req_id = %req_id,
        kind = kind,
        error = ?e,
        "{what}"
    );
    s
}

/// Post an `OperatorMessage`, chunked and bounded, recording each chunk's outcome OUT OF BAND.
///
/// Returns `()`. That is the structural guarantee — a dead webhook cannot break fulfilment, and no
/// call site can propagate. The failure record is a structured `tracing::error!`, which is a
/// CloudWatch metric-filter target: it survives the invocation and fires even if the next sync
/// never runs, which an in-band record cannot. **No dead-letter row** — there is no drainer, so a
/// durable queue would be storage with no consumer. **No retry** — a chunked send is not atomic
/// (`1 of 2 chunk(s) sent` is a real observed outcome), so a naive retry double-posts.
async fn ping_msg(deps: &Deps, msg: &OperatorMessage) {
    let Notify::Webhook(url) = &deps.notify else {
        return;
    };
    let chunks = msg.chunks(PING_PREFIX);
    // GATE MAJOR 2 (OMBB, #171): `of` is not decoration. The no-retry ruling three lines up rests
    // on "`1 of 2 chunk(s) sent` is a real observed outcome" — but a record carrying only
    // `chunk = 1` cannot express it. An operator reading `chunk=1` with no total cannot tell
    // 1-of-1 (the notification FAILED) from 1-of-5 (the notification is TORN, and four other posts
    // may have landed). That is the exact distinction the ruling is built on, and it was the one
    // field the record dropped. Bound before the loop so it is the real total, not a running count.
    let n = chunks.len();
    for (i, chunk) in chunks.into_iter().enumerate() {
        if deliver(&deps.http, url, &chunk).await == 1 {
            tracing::error!(
                outcome = "operator_notification_failed",
                chunk = i + 1,
                of = n,
                "operator notification failed"
            );
        }
    }
}

/// Test seam: drive the REAL `ping_msg` — `Notify` gate, chunking, delivery, failure record — from
/// an integration test.
///
/// `ping_msg` is private and stays private: it takes `&Deps`, and exposing it would make the
/// notification path callable from anywhere. This seam exists because the `NOTIFY_DISABLED` and
/// dead-webhook guarantees are only meaningful when tested through the gate a real caller hits —
/// a test that reimplements the gate proves the test's copy of it, not the code's.
#[doc(hidden)]
pub async fn ping_msg_for_test(deps: &Deps, text: &'static str) {
    ping_msg(deps, &OperatorMessage::literal(text)).await;
}

/// Test seam: drive the chunked delivery path against a wiremock server.
#[doc(hidden)]
pub async fn ping_chunks_for_test(url: &str, msg: &str) -> u32 {
    let m = operator_message::OperatorMessage::literal(Box::leak(msg.to_string().into_boxed_str()));
    let http = reqwest::Client::new();
    let mut failures = 0;
    for chunk in m.chunks(PING_PREFIX) {
        failures += deliver(&http, url, &chunk).await;
    }
    failures
}

// GATE MINOR m2 (OMBB, #171): `ping_content` and `ping_for_test` USED TO LIVE HERE, and both are
// deleted rather than repaired. `ping_content` was production-dead — its only non-test caller was
// `ping_for_test` — and it held a FOURTH hardcoded copy of `"🐱 bendobundles: "` while production
// read `PING_PREFIX`. Change the const and the seam plus its green unit test would have drifted
// silently, which is this arc's own defect class wearing a test's clothes. It also bypassed
// `chunks()` entirely, so the seam had no 2000-char bound — a test seam that is SAFER than
// production proves the wrong thing.
//
// `ping_chunks_for_test` above already has the identical signature and routes through the real
// `PING_PREFIX` + `chunks()`, so the two integration callers moved to it and the divergence is now
// impossible BY CONSTRUCTION rather than by remembering to update four literals. One seam, one
// prefix, one bound.

#[cfg(test)]
mod tests {
    use super::*;

    // -----------------------------------------------------------------------------------------
    // Notify / SecretRead — operator-truth (A). An unconfigured webhook used to return success
    // from every call site and every log: a prod deploy that lost its URL was indistinguishable
    // from one that notified. The states are separated ONCE at init instead of collapsed forever
    // at each use.
    // -----------------------------------------------------------------------------------------

    #[test]
    fn ssm_read_failure_is_unresolved_not_disabled() {
        // THE four-state fix. An SSM error is WEATHER — a throttle, a KMS grant, a network blip —
        // and must never be recorded as INTENT. `Option<String>` collapsed them.
        assert!(matches!(
            Notify::resolve(SecretRead::ReadFailed, false),
            Notify::Unresolved
        ));
    }

    #[test]
    fn deliberately_off_is_disabled_not_unresolved() {
        assert!(matches!(
            Notify::resolve(SecretRead::DeliberatelyOff, false),
            Notify::Disabled
        ));
    }

    // *** THE FLAG'S OWN TEST DID NOT TEST THE FLAG. ***
    // `explicit_disable_flag_also_yields_disabled` passed `(DeliberatelyOff, true)` — which is
    // Disabled because of the READ, not because of the flag, and would have passed identically
    // with the flag deleted from the signature. The matrix is 3 reads x 2 flag values = 6 cells;
    // four were covered and the two uncovered ones were exactly the two where the flag is the
    // ONLY thing that can decide the answer. A hole in a test matrix is not random: it sits where
    // the redundant cases were easy to write.
    #[test]
    fn disable_flag_overrides_a_resolved_secret() {
        // THE cell the bug lived in. Before the fix this returned Webhook — NOTIFY_DISABLED=1 was
        // inert in the only situation an operator ever sets it: notifications currently working.
        assert!(matches!(
            Notify::resolve(SecretRead::Resolved("https://x".into()), true),
            Notify::Disabled
        ));
    }

    #[test]
    fn disable_flag_suppresses_the_unresolved_alarm() {
        // Suppression is the whole job: someone who asked for quiet must not be paged about it.
        assert!(matches!(
            Notify::resolve(SecretRead::ReadFailed, true),
            Notify::Disabled
        ));
    }

    #[test]
    fn explicit_disable_flag_also_yields_disabled() {
        assert!(matches!(
            Notify::resolve(SecretRead::DeliberatelyOff, true),
            Notify::Disabled
        ));
    }

    #[test]
    fn resolved_value_is_webhook() {
        let r = SecretRead::Resolved("https://x".into());
        assert!(matches!(Notify::resolve(r, false), Notify::Webhook(_)));
    }

    #[test]
    fn resolve_never_halts_the_process() {
        // Pins the gate ruling. Config resolution runs in the Lambda INIT phase, and a cold start
        // is CAUSED by an invocation — so init and request are the same instant. If this ever
        // returns Result or panics, a monitoring misconfiguration takes fulfilment down with it.
        // Notification config is observability, not a safety gate: fail LOUD, never CLOSED.
        let _ = Notify::resolve(SecretRead::ReadFailed, false);
    }

    // -----------------------------------------------------------------------------------------
    // compute_enrich_deadline
    // -----------------------------------------------------------------------------------------

    #[test]
    fn compute_enrich_deadline_normal_remaining_subtracts_margin() {
        // context deadline = now + 900s → remaining = 900s → minus 180s margin = 720s.
        let now_ms: u64 = 1_000_000;
        let deadline_ms = now_ms + 900_000;
        let d = compute_enrich_deadline(deadline_ms, now_ms);
        assert_eq!(d, std::time::Duration::from_secs(720));
    }

    #[test]
    fn compute_enrich_deadline_zero_context_falls_back_to_const() {
        // context_deadline_epoch_ms == 0 means no lambda context; uses SYNC_LAMBDA_TIMEOUT -
        // STEAM_ENRICH_DEADLINE_MARGIN = 900s - 180s = 720s.
        let d = compute_enrich_deadline(0, 1_000_000);
        assert_eq!(d, SYNC_LAMBDA_TIMEOUT - STEAM_ENRICH_DEADLINE_MARGIN,);
    }

    #[test]
    fn compute_enrich_deadline_remaining_smaller_than_margin_is_zero() {
        // remaining = 60s < 180s margin → Duration::ZERO (pass must be skipped immediately).
        let now_ms: u64 = 1_000_000;
        let deadline_ms = now_ms + 60_000;
        let d = compute_enrich_deadline(deadline_ms, now_ms);
        assert_eq!(d, std::time::Duration::ZERO);
    }

    // -----------------------------------------------------------------------------------------
    // (#161) Audit per-row write failures. The order walk has always counted its failures
    // (`orders_failed`); the audit's per-row `Err` was a `tracing::warn!` and nothing else — so a
    // row failing to write on EVERY sync was invisible to anyone watching only Discord, forever.
    // The asymmetry was the defect: two loops in the same run, one counted, one not.
    // -----------------------------------------------------------------------------------------

    #[test]
    fn summary_reports_audit_row_failures_only_when_nonzero() {
        assert_eq!(
            summary_line(3, 0, 2, 0),
            "sync ok: 3 written, 0 order(s) failed, 2 audit-pulled"
        );
        assert_eq!(
            summary_line(3, 0, 2, 4),
            "sync ok: 3 written, 0 order(s) failed, 2 audit-pulled, 4 audit row(s) failed"
        );
    }

    #[test]
    fn summary_reports_audit_row_failures_even_when_nothing_was_pulled() {
        // THE CASE THAT MATTERS MOST, and the one an `if pulls > 0` wrapper would have swallowed:
        // rows that all FAIL to write pull nothing, so a failure count nested under the pulled
        // count would be silent in exactly the run where every write broke.
        assert_eq!(
            summary_line(3, 0, 0, 4),
            "sync ok: 3 written, 0 order(s) failed, 4 audit row(s) failed"
        );
        assert_eq!(
            summary_line(3, 0, 0, 0),
            "sync ok: 3 written, 0 order(s) failed"
        );
    }

    // Replaces `ping_content_is_prefixed_and_carries_message` (gate minor m2). The old test pinned
    // a hardcoded prefix literal inside a production-dead helper, so it stayed green while drifting
    // from `PING_PREFIX`. This one drives the SAME call production makes, and reads the const
    // rather than restating it — so it cannot pass while the two disagree.
    #[test]
    fn operator_body_is_prefixed_from_the_const_and_carries_the_message() {
        let m = OperatorMessage::literal("cookie is DEAD");
        let chunks = m.chunks(PING_PREFIX);
        assert_eq!(
            chunks.len(),
            1,
            "short message should be one chunk: {chunks:?}"
        );
        assert!(
            chunks[0].starts_with(PING_PREFIX),
            "prefix missing: {}",
            chunks[0]
        );
        assert!(
            chunks[0].contains("cookie is DEAD"),
            "message missing: {}",
            chunks[0]
        );
    }

    #[test]
    fn recent_month_slugs_are_newest_first_and_cross_year() {
        use time::macros::datetime;
        // 2026-07-06 → july-2026, newest first.
        let now = datetime!(2026-07-06 12:00 UTC);
        assert_eq!(
            recent_month_slugs(now, 4),
            vec!["july-2026", "june-2026", "may-2026", "april-2026"],
        );
        // Crosses into the prior year correctly.
        let jan = datetime!(2026-01-15 00:00 UTC);
        assert_eq!(
            recent_month_slugs(jan, 3),
            vec!["january-2026", "december-2025", "november-2025"],
        );
    }

    #[test]
    fn login_failure_parks_never_compensates() {
        // login() is the session self-heal path, not a redeem outcome — but if it ever reached the
        // gift ladder, it must PARK (no session ⇒ no redeem ⇒ never a burn).
        let outcome = Err(HumbleError::LoginFailed {
            reason: "/processlogin returned status 403 without a goto".into(),
        });
        assert_eq!(gift_decision(&outcome), Decision::Park);
    }

    #[test]
    fn secure_area_step_up_failure_parks_never_compensates() {
        // The key is not burned behind a step-up gate — this MUST park (reconcile re-lists it),
        // never compensate (which would only be safe on a definitive AlreadyRedeemed).
        let outcome = Err(HumbleError::SecureAreaStepUpFailed {
            reason: "humble /processlogin returned status 403 without a goto".into(),
        });
        assert_eq!(gift_decision(&outcome), Decision::Park);
    }

    #[test]
    fn gift_requires_choice_absent_defaults_false() {
        // Every pre-phase-3 (bundle) Gift payload omits requires_choice — it MUST deserialize to
        // false so those requests still dispatch to the bundle path, never the choice orchestration.
        let json = r#"{"op":"gift","claim_id":"c1","link_token":"tok","game_id":"gk:mn","gamekey":"gk","machine_name":"mn","keyindex":0}"#;
        let req: FulfillRequest = serde_json::from_str(json).unwrap();
        assert_eq!(
            req,
            FulfillRequest::Gift {
                claim_id: "c1".into(),
                link_token: "tok".into(),
                game_id: "gk:mn".into(),
                gamekey: "gk".into(),
                machine_name: "mn".into(),
                keyindex: 0,
                requires_choice: false,
            }
        );
    }

    #[test]
    fn choose_decision_ladder_never_compensates() {
        use humble_client::HumbleError as E;
        // Ok ⇒ Record (proceed). Unauthorized ⇒ ParkCookieDead. EVERYTHING else ⇒ Park.
        assert_eq!(choose_decision(&Ok(())), Decision::Record);
        assert_eq!(
            choose_decision(&Err(E::Unauthorized)),
            Decision::ParkCookieDead
        );
        let park_variants = [
            E::SecureAreaStepUpFailed { reason: "x".into() },
            E::ChooseFailed {
                reason: "already chosen".into(),
            },
            E::RateLimited,
            E::Api(500),
            E::LoginFailed { reason: "x".into() },
            E::AlreadyRedeemed,
            E::RedeemAuthRejected {
                status: 403,
                csrf_minted: false,
            },
            E::RedeemRefused {
                msg: "x".into(),
                code: None,
            },
            E::AmbiguousRedeem,
            // choose_content never yields KeyExpired (it spends picks, it doesn't redeem
            // keys) -- classified conservatively as Park; reconcile's order diff decides.
            E::KeyExpired {
                msg: "x".into(),
                code: None,
            },
        ];
        for v in park_variants {
            let d = choose_decision(&Err(v));
            assert_eq!(d, Decision::Park, "expected Park");
            assert_ne!(d, Decision::Compensate);
        }
        // The whole-map invariant: NO choose outcome — Ok or any Err — ever yields Compensate.
        // (Network/Parse are constructed only inside humble-client; the no-`_` match is the guard
        // that they, and any future variant, are classified — and never as Compensate.)
        assert_ne!(choose_decision(&Ok(())), Decision::Compensate);
    }

    #[test]
    fn find_new_tpk_diff_and_disambiguation() {
        use humble_client::{KeyEntry, Order};
        fn key(mn: &str, human: &str, redeemed: bool) -> KeyEntry {
            KeyEntry {
                machine_name: mn.into(),
                human_name: human.into(),
                key_type: "steam".into(),
                redeemed,
                expired: false,
                giftable: !redeemed,
                keyindex: 0,
                redeemed_key_val: None,
                steam_app_id: None,
                is_gift: None,
            }
        }
        fn order(keys: Vec<KeyEntry>) -> Order {
            Order {
                gamekey: "gk".into(),
                bundle_name: "May 2026 Humble Choice".into(),
                product_machine_name: "may_2026_choice".into(),
                keys,
                subproducts: vec![],
            }
        }
        // 0 new (order key already in pre) → None.
        let o = order(vec![key("old_choice_steam", "Old Game", false)]);
        assert_eq!(
            find_new_tpk(&o, &["old_choice_steam".into()], "New Game"),
            TpkPick::None
        );
        // 1 new → Unique regardless of title.
        let o = order(vec![
            key("old_choice_steam", "Old Game", false),
            key("octo_choice_steam", "Octopath Traveler II", false),
        ]);
        assert_eq!(
            find_new_tpk(&o, &["old_choice_steam".into()], "Octopath Traveler II"),
            TpkPick::Unique(&o.keys[1])
        );
        // 2 new, neither exact-title → Ambiguous.
        let o = order(vec![
            key("a_choice_steam", "Alpha", false),
            key("b_choice_steam", "Beta", false),
        ]);
        assert_eq!(find_new_tpk(&o, &[], "Gamma"), TpkPick::Ambiguous);
        // 2 new, exactly one exact case-insensitive title match → Unique (the split).
        let o = order(vec![
            key("a_choice_steam", "Alpha", false),
            key("b_choice_steam", "Beta", false),
        ]);
        assert_eq!(find_new_tpk(&o, &[], "beta"), TpkPick::Unique(&o.keys[1]));
    }

    #[test]
    fn request_response_serde_roundtrips() {
        let req = FulfillRequest::Gift {
            claim_id: "c1".into(),
            link_token: "tok".into(),
            game_id: "gk:mn".into(),
            gamekey: "gk".into(),
            machine_name: "mn".into(),
            keyindex: 3,
            requires_choice: true,
        };
        let json = serde_json::to_string(&req).unwrap();
        assert!(json.contains("\"op\":\"gift\""));
        assert!(json.contains("\"requires_choice\":true"));
        assert_eq!(serde_json::from_str::<FulfillRequest>(&json).unwrap(), req);

        let resp = FulfillResponse::Parked {
            reason: "processing".into(),
        };
        let json = serde_json::to_string(&resp).unwrap();
        assert!(json.contains("\"result\":\"parked\""));
        assert_eq!(
            serde_json::from_str::<FulfillResponse>(&json).unwrap(),
            resp
        );
    }

    // -----------------------------------------------------------------------------------------
    // #75: merge_fetched_halves — the newest-wins-per-half re-merge policy
    // -----------------------------------------------------------------------------------------

    fn test_detail(app_id: u32) -> steam_client::SteamAppDetail {
        steam_client::SteamAppDetail {
            app_id,
            name: "T".into(),
            developers: vec![],
            publishers: vec![],
            genres: vec![],
            release_date: None,
            short_description: "t".into(),
            header_image: None,
            video_hls_url: None,
            video_thumbnail: None,
            screenshots: vec![],
            tags: vec![],
            content_descriptor_ids: vec![],
            content_notes: None,
        }
    }

    fn test_review_summary() -> steam_client::ReviewSummary {
        steam_client::ReviewSummary {
            desc: "Positive".into(),
            total_positive: 10,
            total_negative: 1,
            total_reviews: 11,
        }
    }

    fn test_recent_reviews() -> steam_client::RecentReviews {
        steam_client::RecentReviews {
            percent_positive: 90,
            count: 11,
        }
    }

    fn halves(now: i64) -> FetchedHalves {
        FetchedHalves {
            now,
            detail: None,
            reviews: None,
        }
    }

    /// Frozen-at-death fetch gate (#51): the full truth table for `stub_freezes_reviews`,
    /// pinning the Live relist exemption — the one arm that keeps a relisted game's
    /// reviews reachable inside the JIT-stub concurrent-writer window. (The pass-level
    /// thaw is covered end-to-end by `enrich_relisted_stub_thaws_reviews_on_next_lapse`.)
    #[test]
    fn stub_gate_truth_table() {
        let stub = dynamo::SteamAppCache::empty(570);
        let mut live_cache = dynamo::SteamAppCache::empty(570);
        live_cache.detail = Some(test_detail(570));
        let fetched_live = Some(DetailFetch::Live(Box::new(test_detail(570))));

        // Stub snapshot: frozen unless THIS pass fetched Live (the relist exemption).
        assert!(!stub_freezes_reviews(Some(&stub), &fetched_live));
        assert!(stub_freezes_reviews(Some(&stub), &None));
        assert!(stub_freezes_reviews(
            Some(&stub),
            &Some(DetailFetch::Delisted)
        ));
        // Live snapshot or no snapshot at all: never frozen by this gate.
        assert!(!stub_freezes_reviews(Some(&live_cache), &None));
        assert!(!stub_freezes_reviews(None, &None));
    }

    /// #75 merge policy: our fresh detail applies over a staler snapshot half; the
    /// snapshot's untouched reviews half survives.
    #[test]
    fn merge_ours_newer_detail_applies() {
        let mut cache = dynamo::SteamAppCache::empty(570);
        cache.fetched_at = 100;
        cache.reviews_fetched_at = 900;
        let ours = FetchedHalves {
            detail: Some(DetailFetch::Live(Box::new(test_detail(570)))),
            ..halves(500)
        };
        merge_fetched_halves(&mut cache, &ours);
        assert!(cache.detail.is_some());
        assert_eq!(cache.fetched_at, 500);
        assert_eq!(cache.reviews_fetched_at, 900, "reviews half untouched");
    }

    /// #75 merge policy: a snapshot half NEWER than ours survives — the concurrent
    /// writer's fresher fetch wins, ours is dropped (correct, not a loss).
    #[test]
    fn merge_theirs_newer_detail_survives() {
        let mut cache = dynamo::SteamAppCache::empty(570);
        cache.detail = Some(test_detail(570));
        cache.fetched_at = 800;
        let ours = FetchedHalves {
            detail: Some(DetailFetch::Delisted),
            ..halves(500)
        };
        merge_fetched_halves(&mut cache, &ours);
        assert!(
            cache.detail.is_some(),
            "their live detail survives our stale stub"
        );
        assert_eq!(cache.fetched_at, 800);
        assert_eq!(
            cache.reviews_fetched_at, 0,
            "delisted reviews stamp only applies with the detail half"
        );
    }

    /// #75 merge policy, mirror direction: a NEWER concurrent Delisted verdict is
    /// not resurrected by our stale Live detail — the dead app stays dead.
    #[test]
    fn merge_theirs_newer_delisted_not_resurrected() {
        let mut cache = dynamo::SteamAppCache::empty(570);
        cache.detail = None; // concurrent writer's delisted stub…
        cache.fetched_at = 800; // …stamped fresher than our fetch
        cache.reviews_fetched_at = 800;
        let ours = FetchedHalves {
            detail: Some(DetailFetch::Live(Box::new(test_detail(570)))),
            ..halves(500)
        };
        merge_fetched_halves(&mut cache, &ours);
        assert!(
            cache.detail.is_none(),
            "our stale Live must not resurrect their newer Delisted"
        );
        assert_eq!(cache.fetched_at, 800);
    }

    /// #75 merge policy: equal stamps go to us — we hold data fetched moments ago.
    #[test]
    fn merge_equal_stamp_ours_wins() {
        let mut cache = dynamo::SteamAppCache::empty(570);
        cache.fetched_at = 500;
        let ours = FetchedHalves {
            detail: Some(DetailFetch::Live(Box::new(test_detail(570)))),
            ..halves(500)
        };
        merge_fetched_halves(&mut cache, &ours);
        assert!(cache.detail.is_some());
    }

    /// #75 merge policy: delisted stamps BOTH clocks (dead apps skip review fetches
    /// for the whole window) but never regresses a fresher concurrent reviews stamp.
    #[test]
    fn merge_delisted_stamps_both_clocks_forward_only() {
        let mut cache = dynamo::SteamAppCache::empty(570);
        cache.fetched_at = 100;
        cache.reviews_fetched_at = 100;
        let ours = FetchedHalves {
            detail: Some(DetailFetch::Delisted),
            ..halves(500)
        };
        merge_fetched_halves(&mut cache, &ours);
        assert!(cache.detail.is_none());
        assert_eq!(cache.fetched_at, 500);
        assert_eq!(cache.reviews_fetched_at, 500);

        let mut cache2 = dynamo::SteamAppCache::empty(571);
        cache2.fetched_at = 100;
        cache2.reviews_fetched_at = 800; // concurrent writer's fresher reviews
        merge_fetched_halves(
            &mut cache2,
            &FetchedHalves {
                detail: Some(DetailFetch::Delisted),
                ..halves(500)
            },
        );
        assert_eq!(cache2.reviews_fetched_at, 800, "never stamps backward");
    }

    /// #75 merge policy: the reviews half applies independently of the detail half.
    #[test]
    fn merge_reviews_half_independent() {
        let mut cache = dynamo::SteamAppCache::empty(570);
        cache.detail = Some(test_detail(570));
        cache.fetched_at = 800;
        cache.reviews_fetched_at = 100;
        let ours = FetchedHalves {
            reviews: Some((test_review_summary(), test_recent_reviews())),
            ..halves(500)
        };
        merge_fetched_halves(&mut cache, &ours);
        assert!(cache.overall.is_some());
        assert!(cache.recent.is_some());
        assert_eq!(cache.reviews_fetched_at, 500);
        assert_eq!(cache.fetched_at, 800, "detail half untouched");
    }
}
