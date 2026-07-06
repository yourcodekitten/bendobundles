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

use domain::{Claim, Game, GameStatus};
use dynamo::{Store, StoreError, SyncBegin, SyncState, SyncWrite};
use humble_client::{GiftUrl, HumbleClient, HumbleError, KeyEntry, Order, RevealedKey};
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

/// A parked (`Pending`) claim younger than this is left alone — the live fulfillment call may
/// still be in flight, and reconciling it would race a redeem that is about to record its URL.
/// Only claims older than this are re-checked against humble's truth.
const RECONCILE_MIN_AGE: time::Duration = time::Duration::minutes(15);

/// Pacing between per-order humble fetches during sync — same jitter-free floor as the probe, to
/// stay under humble's bot-detection radar.
const SYNC_PACE: std::time::Duration = std::time::Duration::from_millis(300);

/// How many pages of the Choice-months list walk the discovery pass enumerates. The walk is
/// ~26 pages for the full membership history (3 months/page) and self-terminates early with
/// `complete = true` once it runs out, so this is a ceiling, not a target. It has to reach back far
/// enough to catch an *old* month whose pick is still unspent (Humble keeps a choice redeemable
/// until it's spent), so it covers the whole history; the expensive per-month reads are still gated
/// to the handful of live months (`uses_choices && can_redeem_games`).
const CHOICE_DISCOVERY_MAX_PAGES: usize = 26;

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
        HumbleError::RedeemRefused(_) => Decision::Park,
        HumbleError::AmbiguousRedeem => Decision::Park,
        HumbleError::RateLimited => Decision::Park,
        HumbleError::Api(_) => Decision::Park,
        HumbleError::Network(_) => Decision::Park,
        HumbleError::Parse(_) => Decision::Park,
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
            HumbleError::RedeemRefused(_) => Decision::Park,
            HumbleError::AmbiguousRedeem => Decision::Park,
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

/// Everything `handle` needs to do its job. Constructed once by Task 5's lambda main.
pub struct Deps {
    pub store: Store,
    pub humble: HumbleClient,
    pub webhook_url: Option<String>,
    pub http: reqwest::Client,
    /// SSM client + the humble-cookie parameter name, so the app can self-heal its own session:
    /// on a dead session it logs in (via `humble.login()`) and persists the fresh cookie here,
    /// replacing the human cookie-paste flow. `None` when self-login credentials aren't configured
    /// (then a dead session falls back to the old flag-and-ping behavior).
    pub session_store: Option<SessionStore>,
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
                    ping(deps, SESSION_HEALED_MSG).await;
                    Heal::Persisted
                }
                Err(e) => {
                    // The in-memory client already holds the new session (login swapped it in), so
                    // THIS invoke still works; only the persistence failed. But without the write,
                    // every invoke re-reads the dead cookie and re-logs-in (main rebuilds the
                    // client from SSM per invoke) — a silent "login every invoke" that feeds
                    // humble's bot-detection. Ping so it's not buried in CloudWatch.
                    tracing::warn!(error = %e, "session self-heal: logged in but persisting to SSM failed");
                    ping(deps, SESSION_PERSIST_FAILED_MSG).await;
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
            ping(deps, &format!("humble self-login FAILED ({e}) — session still dead; break-glass: update the humble-cookie SSM param directly (AWS console/CLI)")).await;
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
                        ping(
                            deps,
                            &format!(
                                "fulfill after redeem failed for claim {claim_id}: {e} — \
                                 gift URL was generated but not recorded — recover it from \
                                 humble's gift history page (purchases → the order → gift link)"
                            ),
                        )
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
                ping(
                    deps,
                    &format!("compensate failed for claim {claim_id}: {e}"),
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
            ping(deps, msg).await;
            FulfillResponse::Parked {
                reason: "humble session needs attention".into(),
            }
        }
        // EVERYTHING else is ambiguous-or-refused → PARK (never compensate blind). Reconcile
        // re-checks against humble truth (see `reconcile`).
        Decision::Park => {
            let detail = match &outcome {
                Err(HumbleError::RedeemRefused(_)) => "refused",
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
                ping(
                    deps,
                    &format!(
                        "gift redeem for claim {claim_id} ({machine_name}) needed humble's \
                         secure-area step-up and it did not complete: {reason}. Check the humble \
                         password + TOTP seed in SSM (or the account may be locked / rate-limited). \
                         The key was NOT redeemed — the claim is parked and will re-list on reconcile."
                    ),
                )
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
                ping(
                    deps,
                    &format!(
                        "gift redeem for claim {claim_id} ({machine_name}) was blocked at \
                         humble's auth layer (status {status}). {csrf_note}. The session cookie \
                         is fine (reads work) — refreshing the session won't help. The claim is \
                         parked; reconcile will re-list the key if unredeemed, so this repeats on \
                         the next claim until the write path is fixed."
                    ),
                )
                .await;
            }
            FulfillResponse::Parked {
                reason: format!("humble call inconclusive: park for reconcile ({detail})"),
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
            tracing::warn!(claim_id, error = ?e, "choice pre-read order failed — parking (no spend)");
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
        if existing.redeemed {
            return match flavor {
                ClaimFlavor::Gift => {
                    tracing::warn!(
                        claim_id,
                        "choice pre-check: game already claimed AND redeemed on humble — human recovery"
                    );
                    ping(
                        deps,
                        &format!(
                            "choice claim {claim_id} ({title}): this game's pick appears already \
                             claimed AND redeemed on humble — no re-choose was attempted. Recover \
                             the gift URL from humble's gift-history page; the claim is parked."
                        ),
                    )
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
        // key already sitting in the order. No intent snapshot needed — nothing new will be chosen.
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
            tracing::warn!(claim_id, error = ?e, "choice re-read after choose failed — parking; reconcile finishes (no re-choose)");
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
            ping(
                deps,
                &format!(
                    "choice claim {claim_id} ({title}): the monthly pick was spent but the new key \
                     hasn't appeared in the order yet — parked, reconcile will finish it. No pick \
                     will be spent twice."
                ),
            )
            .await;
            return parked_choice("no-tpk-yet");
        }
        TpkPick::Ambiguous => {
            tracing::warn!(
                claim_id,
                "ambiguous new tpks after choose — parking for human review"
            );
            ping(
                deps,
                &format!(
                    "choice claim {claim_id} ({title}): several new keys appeared after the choose \
                     and the title can't single one out (a concurrent sibling claim on this month?) \
                     — parked for review. No key was burned."
                ),
            )
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
                        ping(
                            deps,
                            &format!(
                                "fulfill after choice redeem failed for claim {claim_id}: {e} — \
                                 gift URL was generated but not recorded — recover it from humble's \
                                 gift history page (purchases → the order → gift link)"
                            ),
                        )
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
            ping(
                deps,
                &format!(
                    "choice claim {claim_id} redeem returned already-redeemed — the monthly pick was \
                     already spent, so this claim was NOT compensated (re-listing would strand the \
                     pick). Recover the gift URL from humble's gift-history page; claim parked."
                ),
            )
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
                Err(HumbleError::RedeemRefused(_)) => "refused",
                Err(HumbleError::AmbiguousRedeem) => "ambiguous",
                Err(HumbleError::RateLimited) => "rate-limited",
                Err(HumbleError::RedeemAuthRejected { .. }) => "redeem-auth-rejected",
                Err(HumbleError::SecureAreaStepUpFailed { .. }) => "secure-area-step-up-failed",
                _ => "transient",
            };
            if let Err(HumbleError::SecureAreaStepUpFailed { reason }) = &outcome {
                ping(
                    deps,
                    &format!(
                        "choice gift redeem for claim {claim_id} ({}) needed humble's secure-area \
                         step-up and it did not complete: {reason}. The key was NOT redeemed — the \
                         claim is parked and reconcile will finish it.",
                        tpk.machine_name
                    ),
                )
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
                ping(
                    deps,
                    &format!(
                        "choice gift redeem for claim {claim_id} ({}) was blocked at humble's auth \
                         layer (status {status}). {csrf_note}. The session cookie is fine (reads \
                         work). The claim is parked; reconcile will finish it once the write path is \
                         fixed.",
                        tpk.machine_name
                    ),
                )
                .await;
            }
            FulfillResponse::Parked {
                reason: format!("humble call inconclusive: park for reconcile ({detail})"),
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
                Err(HumbleError::RedeemRefused(_)) => "refused",
                Err(HumbleError::AmbiguousRedeem) => "ambiguous",
                Err(HumbleError::RateLimited) => "rate-limited",
                Err(HumbleError::RedeemAuthRejected { .. }) => "redeem-auth-rejected",
                Err(HumbleError::SecureAreaStepUpFailed { .. }) => "secure-area-step-up-failed",
                _ => "transient",
            };
            if let Err(HumbleError::SecureAreaStepUpFailed { reason }) = &outcome {
                ping(
                    deps,
                    &format!(
                        "choice self-claim reveal for claim {claim_id} ({}) needed humble's \
                         secure-area step-up and it did not complete: {reason}. The key was NOT \
                         revealed — the claim is parked and reconcile will finish it.",
                        tpk.machine_name
                    ),
                )
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
                ping(
                    deps,
                    &format!(
                        "choice self-claim reveal for claim {claim_id} ({}) was blocked at \
                         humble's auth layer (status {status}). {csrf_note}. The session cookie \
                         is fine (reads work). The claim is parked; reconcile will finish it once \
                         the write path is fixed.",
                        tpk.machine_name
                    ),
                )
                .await;
            }
            FulfillResponse::Parked {
                reason: format!("humble call inconclusive: park for reconcile ({detail})"),
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
    ping(deps, msg).await;
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
        ping(
            deps,
            &format!(
                "choice claim {claim_id} ({title}): choosecontent needed humble's secure-area \
                 step-up and it did not complete: {reason}. No pick was spent — the claim is parked."
            ),
        )
        .await;
    }
    if let Err(HumbleError::ChooseFailed { reason }) = outcome {
        ping(
            deps,
            &format!(
                "choice claim {claim_id} ({title}): humble refused the pick (choosecontent \
                 success=false): {reason}. No pick was spent this attempt — the claim is parked \
                 (reconcile will compensate if the order confirms nothing was claimed)."
            ),
        )
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
            ping(deps, msg).await;
            FulfillResponse::Parked {
                reason: "humble session needs attention".into(),
            }
        }
        Decision::Park => {
            let detail = match &outcome {
                Err(HumbleError::RedeemRefused(_)) => "refused",
                Err(HumbleError::AmbiguousRedeem) => "ambiguous",
                Err(HumbleError::RateLimited) => "rate-limited",
                Err(HumbleError::RedeemAuthRejected { .. }) => "redeem-auth-rejected",
                Err(HumbleError::SecureAreaStepUpFailed { .. }) => "secure-area-step-up-failed",
                _ => "transient",
            };
            if let Err(HumbleError::SecureAreaStepUpFailed { reason }) = &outcome {
                ping(
                    deps,
                    &format!(
                        "self-claim reveal for claim {claim_id} ({machine_name}) needed humble's \
                     secure-area step-up and it did not complete: {reason}. The key was NOT \
                     revealed — the claim is parked and reconcile will finish it."
                    ),
                )
                .await;
            }
            FulfillResponse::Parked {
                reason: format!("self-claim reveal inconclusive: park for reconcile ({detail})"),
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
            ping(deps, &format!(
                "self-claim fulfill failed for claim {claim_id}: {e} — the key was revealed but \
                 not recorded; it is still readable in humble's library keys page."
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
            tracing::warn!(claim_id, error = ?e, "self-claim recover: order re-read failed — parking");
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
            ping(
                deps,
                &format!(
                    "self-claim {claim_id} ({machine_name}): humble says already-redeemed but the \
                 order carries no key value — it may have been gifted out-of-band. Parked for \
                 review; nothing was compensated."
                ),
            )
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
            tracing::info!(claim_id = %claim.id, "reconcile(choice): no intent snapshot — choose never ran, compensating (no pick spent)");
            let _ = compensate_any(deps, claim).await;
            ping(
                deps,
                &format!(
                    "reconcile compensated choice claim {} ({}) — no choose intent was ever \
                     recorded, so the monthly pick was NOT spent — slot returned, game re-listed.",
                    claim.id, game.title
                ),
            )
            .await;
        }
        Some(pre) => match find_new_tpk(order, pre, &game.title) {
            // B1. Snapshot present but no new tpk (and no exact-title match) ⇒ the choose did not
            // commit ⇒ pick NOT spent ⇒ compensate. Hard backstop against a mis-decided ambiguous
            // choose: a re-list → re-claim → re-choose of the same game is REFUSED by humble
            // ("already chosen" → ChooseFailed → park), so no pick is ever double-spent — the
            // residual is churn + pings, never value. SELF uses compensate_self_claim (no link-meta).
            TpkPick::None => {
                tracing::info!(claim_id = %claim.id, "reconcile(choice): snapshot present, no new tpk — choose did not commit, compensating (no pick spent)");
                let _ = compensate_any(deps, claim).await;
                ping(
                    deps,
                    &format!(
                        "reconcile compensated choice claim {} ({}) — a choose intent was recorded \
                         but no new key ever appeared, so the pick was NOT spent — slot returned, \
                         game re-listed. (If humble later shows the pick spent, its re-choose \
                         refusal is the backstop — no double-spend.)",
                        claim.id, game.title
                    ),
                )
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
                    let _ = recover_already_redeemed_key(
                        deps,
                        &claim.id,
                        &claim.game_id,
                        &order.gamekey,
                        &tpk.machine_name,
                    )
                    .await;
                } else {
                    tracing::warn!(claim_id = %claim.id, "reconcile(choice): key present but already redeemed — human recovery (URL unrecorded)");
                    ping(
                        deps,
                        &format!(
                            "choice claim {} ({}) shows its key already redeemed on humble but no gift \
                             URL was recorded — recover it manually from humble's gift-history page. \
                             Claim left pending.",
                            claim.id, game.title
                        ),
                    )
                    .await;
                }
            }
            // B4. Two-or-more new tpks the title can't split (concurrent sibling on this month) ⇒
            // leave Pending + a distinct ping. A human decides; the next pass retries once the
            // sibling fulfills. NEVER a key value in the ping.
            TpkPick::Ambiguous => {
                tracing::warn!(claim_id = %claim.id, "reconcile(choice): ambiguous new tpks — leaving pending, human decides");
                ping(
                    deps,
                    &format!(
                        "choice claim {} ({}) has multiple new keys on humble that the title can't \
                         disambiguate (a concurrent claim on this month?) — left pending for review.",
                        claim.id, game.title
                    ),
                )
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

/// The sync walk. Runs [`reconcile`] first (parked-claim recovery against humble truth), then
/// walks every order and upserts each key's `Game` via the guarded sync-upsert. Every exit path
/// persists a `SyncState` — the caller holds the run marker, so this must always report.
async fn run_sync(deps: &Deps) {
    tracing::info!("sync started (ensure session, reconcile, then order walk)");
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
            ping(deps, COOKIE_DEAD_MSG).await;
            persist_sync(deps, false, false, 0, "humble session cookie is dead").await;
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
            )
            .await;
            return;
        }
    };

    // Reconcile parked claims against humble truth — now with a session known-good from the read above.
    reconcile(deps, &mut healed_this_run, &mut cookie_ok).await;

    let mut games_written = 0u32;
    let mut orders_failed = 0u32;

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
                ping(deps, COOKIE_DEAD_MSG).await;
                break 'orders;
            }
            Err(_) => {
                orders_failed += 1;
                continue;
            }
        };

        // domain::match_artwork wants (human_name, icon) pairs.
        let subs: Vec<(String, Option<String>)> = order
            .subproducts
            .iter()
            .map(|s| (s.human_name.clone(), s.icon.clone()))
            .collect();

        let mut order_failed = false;
        for key in &order.keys {
            let game = Game {
                id: domain::game_id(&order.gamekey, &key.machine_name),
                title: key.human_name.clone(),
                bundle: order.bundle_name.clone(),
                gamekey: order.gamekey.clone(),
                machine_name: key.machine_name.clone(),
                key_type: key.key_type.clone(),
                giftable: key.giftable,
                hidden: false,
                status: domain::sync_status(key.redeemed, key.expired),
                claim_id: None,
                artwork_url: domain::match_artwork(&key.human_name, &subs).map(str::to_string),
                keyindex: key.keyindex,
                // Sync walks order.keys — these all have a real redemption key already.
                // Choice discovery (which sets this true) is a separate ingest path.
                requires_choice: false,
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

    // Choice-discovery ingest — surface each still-claimable OFFERED game as a `requires_choice=true`
    // catalog entry, so the gift-choice orchestration has something to run on. Runs LAST (after the
    // order walk) so a heal it triggers can't starve the walk, and it shares the run's one-heal
    // budget via `healed_this_run` / `cookie_ok`.
    games_written += discover_choice_games(deps, &mut healed_this_run, &mut cookie_ok).await;

    let msg = if cookie_ok {
        format!("sync ok: {games_written} written, {orders_failed} order(s) failed")
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
    )
    .await;
    tracing::info!(games_written, orders_failed, cookie_ok, "sync finished");
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

async fn discover_choice_games(deps: &Deps, healed: &mut bool, cookie_ok: &mut bool) -> u32 {
    // Step 1: enumerate month slugs. A truncated walk (`complete == false`) simply means we discover
    // a prefix of months this pass — safe, because discovery only ADDS entries and never deletes on
    // absence, so a missed month just waits for the next run.
    let (heal, read) = selfheal_once(deps, !*healed, || {
        deps.humble.choice_months(CHOICE_DISCOVERY_MAX_PAGES)
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
            ping(deps, COOKIE_DEAD_MSG).await;
            return 0;
        }
        Err(e) => {
            tracing::warn!(error = ?e, "choice discovery: month enumeration failed — skipping this pass");
            return 0;
        }
    };
    if !walk.complete {
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
    for (slug, is_probe) in &targets {
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
                ping(deps, COOKIE_DEAD_MSG).await;
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
            continue;
        }
        // `choice_month` always populates the claimed set, so `claimable_games` is `Some`. A `None`
        // here would mean the claimed set is UNKNOWN (a `choice_months`-sourced month) — never true
        // on this path, but we skip rather than guess: the contract forbids writing `true` without a
        // known claimed set.
        let Some(claimable) = detail.claimable_games() else {
            tracing::warn!(month = %detail.product_url_path, "choice discovery: single-month read had no claimed set — skipping (never writes true without one)");
            continue;
        };
        // Per-month observability: which months surfaced how many claimable offered games, and the
        // offered/chosen split behind that number. Cheap, and it turns "why did this month write
        // nothing?" from a guessing game into a log line.
        tracing::info!(
            month = %detail.product_url_path,
            gamekey = %detail.gamekey,
            offered = detail.offered_games.len(),
            chosen = detail.claimed_machine_names.as_ref().map_or(0, Vec::len),
            claimable = claimable.len(),
            "choice discovery: month processed"
        );
        for offered in claimable {
            let game = Game {
                id: domain::game_id(&detail.gamekey, &offered.machine_name),
                title: offered.title.clone(),
                bundle: detail.title.clone(),
                gamekey: detail.gamekey.clone(),
                machine_name: offered.machine_name.clone(),
                // No key exists until the pick is spent, so the offered wire carries no key platform.
                // Placeholder; `merge_sync` refreshes `key_type` from the real key-sync `fresh` once a
                // pick lands (the id matches by the machine_name agreement above), so it self-corrects.
                key_type: "steam".to_string(),
                giftable: true,
                hidden: false,
                status: GameStatus::Available,
                claim_id: None,
                artwork_url: None,
                keyindex: 0,
                requires_choice: true,
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
    written
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
        // miss it forever and silently skip it every pass. One extra GetItem per parked claim keys
        // the branch off the durable `requires_choice` flag; a transient game-read miss falls
        // through to the bundle path unchanged (that path needs no game read).
        if let Ok(Some(game)) = deps.store.get_game(&claim.game_id).await
            && game.requires_choice
        {
            // reconcile may WRITE now (redeem/compensate) — pace it under the bot-detection floor.
            tokio::time::sleep(SYNC_PACE).await;
            reconcile_choice_claim(deps, &claim, &game, &order).await;
            continue;
        }
        let Some(key) = order.keys.iter().find(|k| k.machine_name == machine_name) else {
            alert_unreconcilable(
                deps,
                &claim,
                age,
                &format!(
                    "machine_name `{machine_name}` is not among order `{gamekey}`'s keys on \
                     humble, so there is nothing to reconcile it against"
                ),
            )
            .await;
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
                let _ = recover_already_redeemed_key(
                    deps,
                    &claim.id,
                    &claim.game_id,
                    gamekey,
                    machine_name,
                )
                .await;
            } else {
                tracing::warn!(claim_id = %claim.id, "reconcile: parked claim shows redeemed on humble but no URL recorded — human recovery");
                // Gift generated but URL unrecorded; leave pending (human-owned recovery). Message
                // carries claim id + human game context only — NEVER a key value.
                ping(
                    deps,
                    &format!(
                        "parked claim {} ({} / {}) shows redeemed on humble but no gift URL was \
                         recorded — recover manually via humble's gift-history page",
                        claim.id, order.bundle_name, key.human_name
                    ),
                )
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
            ping(
                deps,
                &format!(
                    "reconcile compensated parked claim {} ({} / {}) as not-redeemed — slot returned, \
                     game re-listed. No key can be double-spent (humble refuses re-redeem of a burned \
                     key); but IF this key was actually gifted, its gift URL is lost — recover it from \
                     humble's gift-history page.",
                    claim.id, order.bundle_name, key.human_name
                ),
            )
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
    ping(
        deps,
        &format!(
            "parked claim {} (game_id {}) has been stuck ~{hours}h and reconcile cannot act on \
             it: {reason}. Nothing self-heals this — the link slot stays consumed until someone \
             looks. Fix the claim/game_id by hand (or compensate it) to free the slot.",
            claim.id, claim.game_id
        ),
    )
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

/// Persist a sync-run summary, preserving nothing from prior runs (a run fully describes itself).
async fn persist_sync(deps: &Deps, ok: bool, cookie_ok: bool, games_written: u32, message: &str) {
    let st = SyncState {
        last_run_epoch: OffsetDateTime::now_utc().unix_timestamp(),
        ok,
        cookie_ok,
        games_written,
        message: message.to_string(),
    };
    let _ = deps.store.put_sync_state(&st).await;
}

/// Build the discord ping body. Pure so the message shape is unit-testable without a webhook.
/// Callers must never pass a cookie or gift URL into `msg`.
fn ping_content(msg: &str) -> String {
    format!("🐱 bendobundles: {msg}")
}

/// POST a discord webhook ping if a webhook is configured. Never fails the caller — a dead webhook
/// must not break fulfillment. `msg` must never contain cookie/URL secrets.
async fn ping(deps: &Deps, msg: &str) {
    let Some(url) = deps.webhook_url.as_deref() else {
        return;
    };
    let body = serde_json::json!({ "content": ping_content(msg) });
    if let Err(e) = deps.http.post(url).json(&body).send().await {
        eprintln!("discord ping failed (non-fatal): {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ping_content_is_prefixed_and_carries_message() {
        let c = ping_content("cookie is DEAD");
        assert!(c.starts_with("🐱 bendobundles: "));
        assert!(c.contains("cookie is DEAD"));
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
            E::RedeemRefused("x".into()),
            E::AmbiguousRedeem,
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
            }
        }
        fn order(keys: Vec<KeyEntry>) -> Order {
            Order {
                gamekey: "gk".into(),
                bundle_name: "May 2026 Humble Choice".into(),
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
}
