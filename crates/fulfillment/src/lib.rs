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

use domain::Game;
use dynamo::{Store, SyncBegin, SyncState, SyncWrite};
use humble_client::{GiftUrl, HumbleClient, HumbleError};
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

/// A parked (`Pending`) claim younger than this is left alone — the live fulfillment call may
/// still be in flight, and reconciling it would race a redeem that is about to record its URL.
/// Only claims older than this are re-checked against humble's truth.
const RECONCILE_MIN_AGE: time::Duration = time::Duration::minutes(15);

/// Pacing between per-order humble fetches during sync — same jitter-free floor as the probe, to
/// stay under humble's bot-detection radar.
const SYNC_PACE: std::time::Duration = std::time::Duration::from_millis(300);

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum FulfillRequest {
    Gift {
        claim_id: String,
        link_token: String,
        game_id: String,
        gamekey: String,
        machine_name: String,
        keyindex: u32,
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

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "result", rename_all = "snake_case")]
pub enum FulfillResponse {
    GiftUrl {
        url: String,
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

/// Map a humble redeem outcome to a [`Decision`]. Pure: no I/O, no panics, exhaustive.
pub fn gift_decision(outcome: &Result<GiftUrl, HumbleError>) -> Decision {
    match outcome {
        Ok(_) => Decision::Record,
        Err(err) => match err {
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
            // Everything else is ambiguous-or-refused. The key MAY have burned (or may not have);
            // only reconcile against humble truth can tell. Park — never compensate blind.
            HumbleError::RedeemRefused(_) => Decision::Park,
            HumbleError::AmbiguousRedeem => Decision::Park,
            HumbleError::RateLimited => Decision::Park,
            HumbleError::Api(_) => Decision::Park,
            HumbleError::Network(_) => Decision::Park,
            HumbleError::Parse(_) => Decision::Park,
        },
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
        } => {
            tracing::info!(
                claim_id,
                game_id,
                machine_name,
                keyindex,
                "fulfillment: gift request"
            );
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
        FulfillRequest::Sync => handle_sync(deps).await,
        FulfillRequest::ValidateCookie => handle_validate_cookie(deps).await,
    }
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
    // The ParkCookieDead arm below owns its own always-false write, so skip it here rather
    // than double-write on that path.
    if let Some(h) = heal
        && decision != Decision::ParkCookieDead
    {
        let mut st = deps
            .store
            .get_sync_state()
            .await
            .ok()
            .flatten()
            .unwrap_or_default();
        st.cookie_ok = h.durable();
        let _ = deps.store.put_sync_state(&st).await;
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
            let mut st = deps
                .store
                .get_sync_state()
                .await
                .ok()
                .flatten()
                .unwrap_or_default();
            st.cookie_ok = false;
            let _ = deps.store.put_sync_state(&st).await;
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

/// Reconcile parked (`Pending`) claims older than [`RECONCILE_MIN_AGE`] against humble's truth.
/// - key shows **redeemed** on humble → the gift WAS generated but we crashed before recording the
///   URL. This endpoint can't recover the URL → ping ben (claim id + game context, never a key
///   value) and leave the claim pending: loud, human-owned recovery via humble's gift-history page.
/// - key **not redeemed** → the redeem never landed → `compensate_claim` (slot + game return).
/// - transient humble fetch error → skip that claim; the next pass retries.
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
        if now - claim.created_at < RECONCILE_MIN_AGE {
            continue; // too fresh — a live redeem may still be recording.
        }
        // game_id is "gamekey:machine_name" (gamekey carries no colon).
        let Some((gamekey, machine_name)) = claim.game_id.split_once(':') else {
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
        let Some(key) = order.keys.iter().find(|k| k.machine_name == machine_name) else {
            continue;
        };
        if key.redeemed {
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
        } else {
            // The redeem never landed on humble → return the slot and re-list the game.
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
            tracing::info!(claim_id = %claim.id, "reconcile: parked claim not redeemed on humble — compensating (slot returns, game re-lists)");
            let _ = deps
                .store
                .compensate_claim(&claim.link_token, &claim.id, &claim.game_id)
                .await;
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
    fn request_response_serde_roundtrips() {
        let req = FulfillRequest::Gift {
            claim_id: "c1".into(),
            link_token: "tok".into(),
            game_id: "gk:mn".into(),
            gamekey: "gk".into(),
            machine_name: "mn".into(),
            keyindex: 3,
        };
        let json = serde_json::to_string(&req).unwrap();
        assert!(json.contains("\"op\":\"gift\""));
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
