# 🔴 RETRACTED — Surfacing Engine (Phase 1) Implementation Plan — DO NOT EXECUTE

> # 🔴 DO NOT EXECUTE THIS PLAN. IT WAS NEVER RUN, AND ITS PREMISE IS FALSE.
>
> **Measured against production on 2026-08-17, after this plan was written and reviewed:**
> `thank-you notes ever left: 0` (from **24 real claims**) · `claims 2026-07 -> 24, 2026-08 -> 0` ·
> `18 links across 1114 games`. **Flow is approximately zero.**
>
> Task 4 builds `unread thanks` as the flagship reason. **It has no production path** — its four
> tests (null arm, fire arm, fires-once, falsifiability) would pass forever on fixtures against a
> predicate that can never fire. The engine as a whole would run a backfill and then be **correct and
> silent forever**, which its own criterion ① would score as success.
>
> ⇒ See the **VERDICT FIRST** section of
> `docs/superpowers/specs/2026-08-17-surfacing-engine-design.md`, and criterion **⑥ (fire-rate
> floor)**, which was added because of this and is the thing that would have caught it.
>
> **This file is kept as the design that would have been right had the rows been there** — the
> mechanics (fires-once markers, per-reason first-run policy, an explicit null verdict, evidence that
> re-derives) are sound and reusable. **The reasons are not.** Anyone reviving this must re-run the
> reachability and flow measurements first; a plan that looks executable is a trap when its premise
> has been retracted.


> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build a pure reason engine that decides — from store state alone — whether the app has anything worth telling Ben today, and record what it *would* have said, delivering nothing to anyone.

**Architecture:** A new `surfacing` crate holds a **pure** core: `evaluate(snapshot, seen_markers, now, thresholds) -> Outcome`. It performs no I/O, touches no AWS, and is therefore exhaustively testable. `Outcome` carries a `Verdict` (`Fired(..)` or the first-class `NothingToSay`), plus the marker writes/clears that make each reason **fire once**. The `dynamo` crate gains marker and ledger persistence; `fulfillment` calls the engine on the existing sync tick and writes the ledger. No delivery surface is built.

**Tech Stack:** Rust 2024 workspace, `serde`, `thiserror`, `time` (`OffsetDateTime`), `aws-sdk-dynamodb`, existing `Store`. Tests: `cargo test`, workspace `dev-dependencies` (`serde_json`).

**Spec:** `docs/superpowers/specs/2026-08-17-surfacing-engine-design.md` — read it first; this plan argues from it.

## Global Constraints

- **No delivery to any human.** No Discord, email, or web surface. No `DigestMessage` type. Building the door is explicitly out of scope and building it early is the named mistake.
- **`NothingToSay` is a first-class verdict**, never an empty list. Criterion ①.
- **Every reason carries evidence sufficient to re-derive it from the store.** Criterion ② — *a notification you cannot verify is a rumour* (#171).
- **Every reason declares a first-run policy in its own definition** — a new reason cannot compile without choosing one. `Debt` ⇒ announce backlog; `Inventory` ⇒ seed silently. The test: *would he say "why didn't you tell me sooner?"*
- **Fires-once via clearing markers.** True today and true yesterday ⇒ does not fire. For a bare `bool` the marker **is** the transition detector.
- **The ledger records the fire time of every reason**, so phase 2's coalescing window is designed against measured timing.
- **No new EventBridge schedule.** Reuse the existing `sync` rule and the `fulfillment` lambda.
- **No thresholds shipped as final.** Defaults live in one struct, are recorded in the ledger alongside the verdict, and are Ben's to set. Criterion ④ / *the criteria are ours, the threshold is his.*
- **Signed commits, `code kitten <yourcodekitten@gmail.com>`.** Never commit on the default branch; this work lives on `feat/surfacing-engine-dry-run`.

---

### Task 1: Crate skeleton, reason vocabulary, and the first-run policy that cannot be skipped

**Files:**
- Create: `crates/surfacing/Cargo.toml`
- Create: `crates/surfacing/src/lib.rs`
- Create: `crates/surfacing/src/reason.rs`
- Modify: `Cargo.toml` (workspace `members`)

**Interfaces:**
- Consumes: nothing.
- Produces: `surfacing::reason::{Reason, ReasonKind, Evidence, FirstRun}`. `Reason { kind: ReasonKind, subject: String, evidence: Evidence, fired_at: OffsetDateTime }`. `ReasonKind::{UnreadThanks, StaleInvite, SurplusKey}`. `ReasonKind::first_run(self) -> FirstRun`. `FirstRun::{AnnounceBacklog, SeedSilently}`. `Evidence { claim: String, rederive: String }`.

- [ ] **Step 1: Write the failing test**

Create `crates/surfacing/src/reason.rs` with only this test module at the bottom (no implementation yet):

```rust
#[cfg(test)]
mod tests {
    use super::*;

    /// Every reason MUST declare a first-run policy. This test exists so a new
    /// ReasonKind cannot be added without choosing one — the match below stops
    /// compiling. Criterion: same mechanism, opposite defaults.
    #[test]
    fn every_reason_kind_declares_a_first_run_policy() {
        // Debt: he is owed these. Seeding them silently would ship a fix that
        // behaves exactly like the break.
        assert_eq!(ReasonKind::UnreadThanks.first_run(), FirstRun::AnnounceBacklog);
        // Inventory: a standing fact he already knows.
        assert_eq!(ReasonKind::StaleInvite.first_run(), FirstRun::SeedSilently);
        assert_eq!(ReasonKind::SurplusKey.first_run(), FirstRun::SeedSilently);
    }

    #[test]
    fn evidence_carries_a_rederivation_path() {
        let e = Evidence::new("1 unread thank-you note", "get_link(tok).thanked_at is Some");
        assert!(!e.rederive.is_empty(), "evidence without a re-derivation is a rumour");
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p surfacing`
Expected: FAIL — the crate does not exist yet (`error: package ID specification 'surfacing' did not match any packages`).

- [ ] **Step 3: Write minimal implementation**

`Cargo.toml` (workspace root) — add to `members`:

```toml
members = ["crates/domain", "crates/humble-client", "crates/dynamo", "crates/fulfillment", "crates/public-api", "crates/admin-api", "crates/steam-client", "crates/surfacing"]
```

`crates/surfacing/Cargo.toml`:

```toml
[package]
name = "surfacing"
version = "0.1.0"
edition.workspace = true
publish.workspace = true

[dependencies]
domain = { path = "../domain" }
serde.workspace = true
time.workspace = true

[dev-dependencies]
serde_json.workspace = true
```

`crates/surfacing/src/lib.rs`:

```rust
//! The surfacing engine: decides whether this app has anything worth telling Ben today.
//!
//! PURE BY CONSTRUCTION. This crate performs no I/O, opens no sockets, and knows nothing
//! about AWS. Everything it needs arrives as a snapshot; everything it wants written leaves
//! as data. That is what makes the dry run exhaustively testable — and the dry run is the
//! positive control for the product itself.
//!
//! NO DELIVERY LIVES HERE OR ANYWHERE IN PHASE 1. Building the sealed message door first
//! would feel like progress and is the named mistake: build the engine, run it dry, read
//! the log. See docs/superpowers/specs/2026-08-17-surfacing-engine-design.md.

pub mod reason;
```

`crates/surfacing/src/reason.rs` (above the existing test module):

```rust
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

/// What a reason does the FIRST time the engine ever observes it, when no marker exists.
///
/// This is a per-reason policy and getting it backwards is expensive in both directions:
/// on first deployment nothing has a marker, so every qualifying record reads as new.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum FirstRun {
    /// He is OWED this; silence is the bug being fixed. Seeding it silently would ship a
    /// fix whose output is indistinguishable from the defect.
    AnnounceBacklog,
    /// A standing fact he already knows. Record it, announce nothing; only later
    /// arrivals are news.
    SeedSilently,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ReasonKind {
    UnreadThanks,
    StaleInvite,
    SurplusKey,
}

impl ReasonKind {
    /// The one-question test: *would he say "why didn't you tell me sooner?"*
    /// Yes ⇒ debt ⇒ announce the backlog. No ⇒ inventory ⇒ seed silently.
    ///
    /// This match is deliberately exhaustive with no wildcard arm: a new ReasonKind
    /// must not compile until its author has answered the question.
    pub fn first_run(self) -> FirstRun {
        match self {
            ReasonKind::UnreadThanks => FirstRun::AnnounceBacklog,
            ReasonKind::StaleInvite => FirstRun::SeedSilently,
            ReasonKind::SurplusKey => FirstRun::SeedSilently,
        }
    }
}

/// A reason's claim plus the path to refute it. A reason that could not have been false
/// is not a reason.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Evidence {
    /// What is being asserted, in Ben's language.
    pub claim: String,
    /// How to go and check it against the store, in ours.
    pub rederive: String,
}

impl Evidence {
    pub fn new(claim: impl Into<String>, rederive: impl Into<String>) -> Self {
        Self { claim: claim.into(), rederive: rederive.into() }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Reason {
    pub kind: ReasonKind,
    /// The record this is about — a link token, a game id.
    pub subject: String,
    pub evidence: Evidence,
    /// When the engine decided this. Phase 2's coalescing window is designed against
    /// these, so they are recorded from day one rather than added later.
    #[serde(with = "time::serde::rfc3339")]
    pub fired_at: OffsetDateTime,
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p surfacing`
Expected: PASS, 2 tests.

- [ ] **Step 5: Commit**

```bash
git add Cargo.toml crates/surfacing
git commit -S -m "feat(surfacing): reason vocabulary with a first-run policy that cannot be skipped"
```

---

### Task 2: The verdict, with a null state that is first-class and tested against a sibling

**Files:**
- Create: `crates/surfacing/src/verdict.rs`
- Modify: `crates/surfacing/src/lib.rs`

**Interfaces:**
- Consumes: `reason::Reason` from Task 1.
- Produces: `surfacing::verdict::Verdict::{Fired(Vec<Reason>), NothingToSay}` and `Verdict::from_reasons(Vec<Reason>) -> Verdict`.

- [ ] **Step 1: Write the failing test**

Append to `crates/surfacing/src/verdict.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::reason::{Evidence, Reason, ReasonKind};
    use time::OffsetDateTime;

    fn a_reason() -> Reason {
        Reason {
            kind: ReasonKind::UnreadThanks,
            subject: "tok-1".into(),
            evidence: Evidence::new("1 unread thank-you note", "get_link(tok-1).thanked_at"),
            fired_at: OffsetDateTime::UNIX_EPOCH,
        }
    }

    /// THE NULL ARM. An empty reason set is NOT an empty Fired — it is a distinct verdict.
    #[test]
    fn no_reasons_is_nothing_to_say_not_an_empty_list() {
        assert_eq!(Verdict::from_reasons(vec![]), Verdict::NothingToSay);
    }

    /// THE SIBLING ARM. Without this, the null arm above proves nothing — a constructor
    /// that always returned NothingToSay would pass it.
    #[test]
    fn one_reason_fires() {
        match Verdict::from_reasons(vec![a_reason()]) {
            Verdict::Fired(rs) => assert_eq!(rs.len(), 1),
            Verdict::NothingToSay => panic!("a real reason must not be silent"),
        }
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p surfacing verdict`
Expected: FAIL — `cannot find type Verdict in this scope` / unresolved module.

- [ ] **Step 3: Write minimal implementation**

Prepend to `crates/surfacing/src/verdict.rs`:

```rust
use crate::reason::Reason;
use serde::{Deserialize, Serialize};

/// The engine's answer for one tick.
///
/// `NothingToSay` is a VERDICT, not an absence. A surfacing with no silent state is an
/// alarm that cannot be switched off, and an alarm nobody can switch off gets switched
/// off — so the quiet day is modelled explicitly and asserted in tests.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Verdict {
    Fired(Vec<Reason>),
    NothingToSay,
}

impl Verdict {
    pub fn from_reasons(reasons: Vec<Reason>) -> Self {
        if reasons.is_empty() { Verdict::NothingToSay } else { Verdict::Fired(reasons) }
    }
}
```

Add to `crates/surfacing/src/lib.rs`:

```rust
pub mod verdict;
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p surfacing`
Expected: PASS, 4 tests.

- [ ] **Step 5: Commit**

```bash
git add crates/surfacing
git commit -S -m "feat(surfacing): NothingToSay is a verdict, with a sibling arm that fires"
```

---

### Task 3: Snapshot, markers, and the fires-once mechanic — all three arms

**Files:**
- Create: `crates/surfacing/src/snapshot.rs`
- Create: `crates/surfacing/src/markers.rs`
- Modify: `crates/surfacing/src/lib.rs`

**Interfaces:**
- Consumes: `reason::ReasonKind`.
- Produces: `snapshot::Snapshot { links: Vec<domain::Link>, games: Vec<domain::Game> }`; `markers::MarkerKey { kind: ReasonKind, subject: String }`; `markers::MarkerSet` with `contains(&MarkerKey) -> bool`, `from_iter`, `iter`; `markers::MarkerDelta { write: Vec<MarkerKey>, clear: Vec<MarkerKey> }`.

- [ ] **Step 1: Write the failing test**

`crates/surfacing/src/markers.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::reason::ReasonKind;

    fn key(s: &str) -> MarkerKey {
        MarkerKey { kind: ReasonKind::SurplusKey, subject: s.into() }
    }

    /// ARM 1 — a marker that is absent is the "this is new" signal. For a bare bool with
    /// no observable history, this is the transition detector itself, not just
    /// anti-repeat plumbing.
    #[test]
    fn absent_marker_means_new() {
        let seen = MarkerSet::from_iter(vec![]);
        assert!(!seen.contains(&key("g1")));
    }

    /// ARM 2 — present means already announced; the reason must not fire again.
    #[test]
    fn present_marker_means_already_seen() {
        let seen = MarkerSet::from_iter(vec![key("g1")]);
        assert!(seen.contains(&key("g1")));
    }

    /// ARM 3 — the marker must CLEAR, or a reason can never be true again. A marker that
    /// only ever accumulates is a detector that fires once in its lifetime.
    #[test]
    fn a_delta_can_both_write_and_clear() {
        let d = MarkerDelta { write: vec![key("g2")], clear: vec![key("g1")] };
        assert_eq!(d.write.len(), 1);
        assert_eq!(d.clear.len(), 1);
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p surfacing markers`
Expected: FAIL — `cannot find type MarkerKey in this scope`.

- [ ] **Step 3: Write minimal implementation**

Prepend to `crates/surfacing/src/markers.rs`:

```rust
use crate::reason::ReasonKind;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

/// One reason's memory of one subject. Its ABSENCE is the signal that the subject is new.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct MarkerKey {
    pub kind: ReasonKind,
    pub subject: String,
}

/// The markers the engine has already recorded. Passed in, never fetched — purity.
#[derive(Debug, Clone, Default)]
pub struct MarkerSet(HashSet<MarkerKey>);

impl MarkerSet {
    pub fn from_iter(keys: impl IntoIterator<Item = MarkerKey>) -> Self {
        Self(keys.into_iter().collect())
    }
    pub fn contains(&self, k: &MarkerKey) -> bool {
        self.0.contains(k)
    }
    pub fn iter(&self) -> impl Iterator<Item = &MarkerKey> {
        self.0.iter()
    }
}

/// What the caller must persist after a tick.
///
/// `clear` is not optional politeness: a marker that never clears turns its reason into a
/// once-per-lifetime event. The pattern is host-agent-watch's — write above the line,
/// remove below it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MarkerDelta {
    pub write: Vec<MarkerKey>,
    pub clear: Vec<MarkerKey>,
}
```

`crates/surfacing/src/snapshot.rs`:

```rust
use domain::{Game, Link};

/// Everything the engine is allowed to look at, handed to it. The engine never fetches.
#[derive(Debug, Clone, Default)]
pub struct Snapshot {
    pub links: Vec<Link>,
    pub games: Vec<Game>,
}
```

Add to `crates/surfacing/src/lib.rs`:

```rust
pub mod markers;
pub mod snapshot;
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p surfacing`
Expected: PASS, 7 tests.

- [ ] **Step 5: Commit**

```bash
git add crates/surfacing
git commit -S -m "feat(surfacing): markers whose absence is the transition signal, and that clear"
```

---

### Task 4: `evaluate` and the unread-thanks reason — the debt that announces its backlog

**Files:**
- Create: `crates/surfacing/src/engine.rs`
- Modify: `crates/surfacing/src/lib.rs`

**Interfaces:**
- Consumes: `Snapshot`, `MarkerSet`, `MarkerDelta`, `Verdict`, `Reason`, `ReasonKind`, `Evidence`, `FirstRun`.
- Produces: `engine::Thresholds { stale_invite_days: i64, backlog_summary_at: usize }` with `Default`; `engine::Outcome { verdict: Verdict, markers: MarkerDelta, thresholds: Thresholds }`; `engine::evaluate(&Snapshot, &MarkerSet, OffsetDateTime, &Thresholds) -> Outcome`.

- [ ] **Step 1: Write the failing test**

Append to `crates/surfacing/src/engine.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::markers::{MarkerKey, MarkerSet};
    use crate::reason::ReasonKind;
    use crate::snapshot::Snapshot;
    use crate::verdict::Verdict;
    use domain::Link;
    use time::OffsetDateTime;

    fn now() -> OffsetDateTime {
        OffsetDateTime::from_unix_timestamp(1_760_000_000).unwrap()
    }

    fn link_thanked(token: &str, thanked: bool) -> Link {
        let mut l = Link::new_for_test(token);
        l.thanked_at = thanked.then(|| now());
        l
    }

    /// THE NULL ARM, at the engine level: nothing transitioned, so nothing is said.
    #[test]
    fn a_quiet_tick_says_nothing() {
        let snap = Snapshot { links: vec![link_thanked("t1", false)], games: vec![] };
        let out = evaluate(&snap, &MarkerSet::default(), now(), &Thresholds::default());
        assert_eq!(out.verdict, Verdict::NothingToSay);
    }

    /// THE SIBLING ARM from the same fixture shape — without it the null arm proves nothing.
    #[test]
    fn an_unread_thanks_fires_on_first_run_because_it_is_a_debt() {
        let snap = Snapshot { links: vec![link_thanked("t1", true)], games: vec![] };
        let out = evaluate(&snap, &MarkerSet::default(), now(), &Thresholds::default());
        match out.verdict {
            Verdict::Fired(rs) => {
                assert_eq!(rs.len(), 1);
                assert_eq!(rs[0].kind, ReasonKind::UnreadThanks);
                assert!(!rs[0].evidence.rederive.is_empty());
            }
            Verdict::NothingToSay => panic!("a debt must announce its backlog on first run"),
        }
    }

    /// FIRES ONCE: the same true state on the next tick, with the marker now present.
    #[test]
    fn the_same_unread_thanks_does_not_fire_twice() {
        let snap = Snapshot { links: vec![link_thanked("t1", true)], games: vec![] };
        let seen = MarkerSet::from_iter(vec![MarkerKey {
            kind: ReasonKind::UnreadThanks,
            subject: "t1".into(),
        }]);
        let out = evaluate(&snap, &seen, now(), &Thresholds::default());
        assert_eq!(out.verdict, Verdict::NothingToSay);
    }

    /// THE MARKER CLEARS: the thanks is gone (link deleted / un-thanked), so the stale
    /// marker must be removed or the reason can never fire for this subject again.
    #[test]
    fn a_marker_clears_when_its_condition_is_no_longer_true() {
        let snap = Snapshot { links: vec![link_thanked("t1", false)], games: vec![] };
        let seen = MarkerSet::from_iter(vec![MarkerKey {
            kind: ReasonKind::UnreadThanks,
            subject: "t1".into(),
        }]);
        let out = evaluate(&snap, &seen, now(), &Thresholds::default());
        assert_eq!(out.markers.clear.len(), 1, "a no-longer-true marker must clear");
    }

    /// A LARGE BACKLOG IS ONE EVENT, NOT N. Above the threshold the engine emits a single
    /// summarising reason — the backlog is a single event; its items are not.
    #[test]
    fn a_large_backlog_summarises_instead_of_firing_per_item() {
        let links: Vec<Link> = (0..14)
            .map(|i| link_thanked(&format!("t{i}"), true))
            .collect();
        let snap = Snapshot { links, games: vec![] };
        let out = evaluate(&snap, &MarkerSet::default(), now(), &Thresholds::default());
        match out.verdict {
            Verdict::Fired(rs) => {
                assert_eq!(rs.len(), 1, "14 unread thanks must be ONE message, not 14");
                assert!(rs[0].evidence.claim.contains("14"), "the size is the news");
            }
            Verdict::NothingToSay => panic!("a backlog of 14 is not silence"),
        }
        assert_eq!(out.markers.write.len(), 14, "but every item is marked, so none repeats");
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p surfacing engine`
Expected: FAIL — `cannot find function evaluate in this scope`, and `Link::new_for_test` unresolved.

- [ ] **Step 3: Write minimal implementation**

Add a test constructor to `crates/domain/src/lib.rs` (inside `impl Link`), because the engine's tests need a Link without a store:

```rust
    /// A minimal Link for tests in dependent crates. Not `#[cfg(test)]`: integration and
    /// sibling-crate tests need it, and a constructor that only exists in this crate's
    /// unit tests cannot serve them.
    #[doc(hidden)]
    pub fn new_for_test(token: &str) -> Self {
        Self {
            token: token.to_string(),
            created_at: time::OffsetDateTime::UNIX_EPOCH,
            thanked_at: None,
            expires_at: None,
            unlock_at: None,
            ..Default::default()
        }
    }
```

If `Link` does not derive `Default`, construct every field explicitly instead — read `crates/domain/src/lib.rs` and fill each one; do not add a `Default` derive to a domain type whose fields carry invariants.

Prepend to `crates/surfacing/src/engine.rs`:

```rust
use crate::markers::{MarkerDelta, MarkerKey, MarkerSet};
use crate::reason::{Evidence, FirstRun, Reason, ReasonKind};
use crate::snapshot::Snapshot;
use crate::verdict::Verdict;
use time::OffsetDateTime;

/// Tuning. Defaults are a STARTING POINT recorded in the ledger, never a shipped decision:
/// what is worth interrupting Ben for is a question about his attention, and the threshold
/// is his to set from real data.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Thresholds {
    pub stale_invite_days: i64,
    /// At or above this many items of one kind, emit a single summarising reason.
    pub backlog_summary_at: usize,
}

impl Default for Thresholds {
    fn default() -> Self {
        Self { stale_invite_days: 90, backlog_summary_at: 4 }
    }
}

#[derive(Debug, Clone)]
pub struct Outcome {
    pub verdict: Verdict,
    pub markers: MarkerDelta,
    /// Recorded with the verdict so a ledger row can be read years later without guessing
    /// what the thresholds were at the time.
    pub thresholds: Thresholds,
}

pub fn evaluate(
    snap: &Snapshot,
    seen: &MarkerSet,
    now: OffsetDateTime,
    cfg: &Thresholds,
) -> Outcome {
    let mut reasons = Vec::new();
    let mut delta = MarkerDelta::default();

    // ── unread thanks ────────────────────────────────────────────────────────────────
    // A write with no reader is the same as no write: thanked_at has been recorded since
    // the thank-you-notes feature shipped and nothing has ever surfaced it.
    let thanked: Vec<&domain::Link> =
        snap.links.iter().filter(|l| l.thanked_at.is_some()).collect();

    let unseen: Vec<&&domain::Link> = thanked
        .iter()
        .filter(|l| {
            !seen.contains(&MarkerKey {
                kind: ReasonKind::UnreadThanks,
                subject: l.token.clone(),
            })
        })
        .collect();

    // Debt: announce on first run. (ReasonKind::UnreadThanks.first_run() is the law; this
    // assert keeps the two from drifting apart silently.)
    debug_assert_eq!(ReasonKind::UnreadThanks.first_run(), FirstRun::AnnounceBacklog);

    if !unseen.is_empty() {
        let n = unseen.len();
        let oldest = unseen
            .iter()
            .filter_map(|l| l.thanked_at)
            .min()
            .unwrap_or(now);
        if n >= cfg.backlog_summary_at {
            reasons.push(Reason {
                kind: ReasonKind::UnreadThanks,
                subject: format!("{n} links"),
                evidence: Evidence::new(
                    format!("{n} unread thank-you notes, oldest {oldest}"),
                    "list_links() filtered to thanked_at.is_some(), minus recorded markers",
                ),
                fired_at: now,
            });
        } else {
            for l in &unseen {
                reasons.push(Reason {
                    kind: ReasonKind::UnreadThanks,
                    subject: l.token.clone(),
                    evidence: Evidence::new(
                        "an unread thank-you note",
                        format!("get_link({}).thanked_at is Some", l.token),
                    ),
                    fired_at: now,
                });
            }
        }
        for l in &unseen {
            delta.write.push(MarkerKey {
                kind: ReasonKind::UnreadThanks,
                subject: l.token.clone(),
            });
        }
    }

    // Clear markers whose condition is no longer true, so the reason can fire again if it
    // genuinely becomes true again.
    for k in seen.iter() {
        if k.kind == ReasonKind::UnreadThanks {
            let still_true = snap
                .links
                .iter()
                .any(|l| l.token == k.subject && l.thanked_at.is_some());
            if !still_true {
                delta.clear.push(k.clone());
            }
        }
    }

    Outcome { verdict: Verdict::from_reasons(reasons), markers: delta, thresholds: *cfg }
}
```

Add to `crates/surfacing/src/lib.rs`:

```rust
pub mod engine;
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p surfacing`
Expected: PASS, 12 tests.

- [ ] **Step 5: Commit**

```bash
git add crates/domain crates/surfacing
git commit -S -m "feat(surfacing): evaluate() and unread thanks — the debt that announces its backlog"
```

---

### Task 5: The stale-invite reason — a crossing, not an age

**Files:**
- Modify: `crates/surfacing/src/engine.rs`

**Interfaces:**
- Consumes: everything from Task 4.
- Produces: no new public names; `evaluate` now also emits `ReasonKind::StaleInvite`.

- [ ] **Step 1: Write the failing test**

Append to the `tests` module in `crates/surfacing/src/engine.rs`:

```rust
    fn link_aged(token: &str, days_old: i64) -> Link {
        let mut l = Link::new_for_test(token);
        l.created_at = now() - time::Duration::days(days_old);
        l
    }

    /// Below the line: not yet news.
    #[test]
    fn a_young_invite_is_not_stale() {
        let snap = Snapshot { links: vec![link_aged("t1", 10)], games: vec![] };
        let out = evaluate(&snap, &MarkerSet::default(), now(), &Thresholds::default());
        assert_eq!(out.verdict, Verdict::NothingToSay);
    }

    /// Across the line: the CROSSING is the event. Seed-silently applies only to the
    /// first-run marker sweep, not to a genuine crossing observed later — but on a true
    /// first run with no markers at all, inventory stays quiet.
    #[test]
    fn a_stale_invite_seeds_silently_on_first_run_then_fires_on_a_later_crossing() {
        let cfg = Thresholds::default();
        // First run: no markers anywhere. Inventory ⇒ record, say nothing.
        let snap = Snapshot { links: vec![link_aged("t1", 200)], games: vec![] };
        let first = evaluate(&snap, &MarkerSet::default(), now(), &cfg);
        assert_eq!(first.verdict, Verdict::NothingToSay, "inventory seeds silently");
        assert!(
            first.markers.write.iter().any(|k| k.kind == ReasonKind::StaleInvite),
            "but it must be recorded, or it will read as new forever"
        );

        // Later: a DIFFERENT link crosses, and the engine has seen the world before.
        let seen = MarkerSet::from_iter(first.markers.write.clone());
        let snap2 = Snapshot {
            links: vec![link_aged("t1", 201), link_aged("t2", 200)],
            games: vec![],
        };
        let second = evaluate(&snap2, &seen, now(), &cfg);
        match second.verdict {
            Verdict::Fired(rs) => {
                assert!(rs.iter().any(|r| r.kind == ReasonKind::StaleInvite && r.subject == "t2"));
                assert!(!rs.iter().any(|r| r.subject == "t1"), "t1 already counted");
            }
            Verdict::NothingToSay => panic!("a new crossing is news"),
        }
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p surfacing engine::tests::a_stale_invite -v`
Expected: FAIL — `assertion failed: first.markers.write.iter().any(...)` (no StaleInvite markers are produced yet).

- [ ] **Step 3: Write minimal implementation**

In `evaluate`, immediately before the "Clear markers" loop, insert:

```rust
    // ── stale invite ─────────────────────────────────────────────────────────────────
    // A state with an AGE has exactly one moment when the age crosses the line. Inventory:
    // on the very first run there are no markers at all, so recording without announcing
    // is what stops day one from being forty items.
    let first_ever_run = seen.iter().next().is_none();
    let stale_cutoff = now - time::Duration::days(cfg.stale_invite_days);

    for l in snap.links.iter().filter(|l| l.created_at <= stale_cutoff) {
        let key = MarkerKey { kind: ReasonKind::StaleInvite, subject: l.token.clone() };
        if seen.contains(&key) {
            continue;
        }
        delta.write.push(key);
        debug_assert_eq!(ReasonKind::StaleInvite.first_run(), FirstRun::SeedSilently);
        if !first_ever_run {
            reasons.push(Reason {
                kind: ReasonKind::StaleInvite,
                subject: l.token.clone(),
                evidence: Evidence::new(
                    format!("an invite from {} has gone unclaimed", l.created_at),
                    format!(
                        "get_link({}).created_at <= now - {} days",
                        l.token, cfg.stale_invite_days
                    ),
                ),
                fired_at: now,
            });
        }
    }
```

And extend the clearing loop to handle the new kind — replace the `if k.kind == ReasonKind::UnreadThanks {` block with:

```rust
        let still_true = match k.kind {
            ReasonKind::UnreadThanks => snap
                .links
                .iter()
                .any(|l| l.token == k.subject && l.thanked_at.is_some()),
            ReasonKind::StaleInvite => snap
                .links
                .iter()
                .any(|l| l.token == k.subject && l.created_at <= stale_cutoff),
            ReasonKind::SurplusKey => true, // implemented in Task 6
        };
        if !still_true {
            delta.clear.push(k.clone());
        }
```

(Delete the old `if k.kind == ReasonKind::UnreadThanks { ... }` body entirely; the `match` replaces it. Move the `stale_cutoff` binding above the clearing loop if the borrow checker complains about ordering.)

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p surfacing`
Expected: PASS, 14 tests.

- [ ] **Step 5: Commit**

```bash
git add crates/surfacing
git commit -S -m "feat(surfacing): stale invites — the crossing is the event, and day one is silent"
```

---

### Task 6: The surplus-key reason — a boolean with no history, detected by marker absence

**Files:**
- Modify: `crates/surfacing/src/engine.rs`

**Interfaces:**
- Consumes: everything above, plus `domain::Game`.
- Produces: no new public names; `evaluate` now also emits `ReasonKind::SurplusKey`.

- [ ] **Step 1: Write the failing test**

Append to the `tests` module in `crates/surfacing/src/engine.rs`:

```rust
    fn surplus_game(id: &str, owned: bool) -> domain::Game {
        let mut g = domain::Game::new_for_test(id);
        g.owned_by_ben = owned;
        g.giftable = true;
        g.claim_id = None;
        g
    }

    /// The flag is a bare bool: nothing in current state says WHEN it became true. The
    /// marker's absence is the only available transition signal — and on a true first run
    /// that would mean "all of them", so inventory seeds silently.
    #[test]
    fn surplus_keys_seed_silently_on_first_run() {
        let snap = Snapshot { links: vec![], games: vec![surplus_game("g1", true)] };
        let out = evaluate(&snap, &MarkerSet::default(), now(), &Thresholds::default());
        assert_eq!(out.verdict, Verdict::NothingToSay, "day one must not page forty items");
        assert_eq!(out.markers.write.len(), 1, "but it is recorded, or it is new forever");
    }

    /// A later arrival IS news — the 41st, not the 40.
    #[test]
    fn a_later_surplus_key_fires() {
        let cfg = Thresholds::default();
        let seed = evaluate(
            &Snapshot { links: vec![], games: vec![surplus_game("g1", true)] },
            &MarkerSet::default(),
            now(),
            &cfg,
        );
        let seen = MarkerSet::from_iter(seed.markers.write.clone());
        let out = evaluate(
            &Snapshot {
                links: vec![],
                games: vec![surplus_game("g1", true), surplus_game("g2", true)],
            },
            &seen,
            now(),
            &cfg,
        );
        match out.verdict {
            Verdict::Fired(rs) => {
                assert!(rs.iter().any(|r| r.kind == ReasonKind::SurplusKey && r.subject == "g2"));
            }
            Verdict::NothingToSay => panic!("a new surplus key is news"),
        }
    }

    /// Not surplus at all: owned but NOT giftable, or already claimed, must never fire.
    #[test]
    fn a_claimed_or_ungiftable_key_is_not_surplus() {
        let mut claimed = surplus_game("g3", true);
        claimed.claim_id = Some("c1".into());
        let mut ungiftable = surplus_game("g4", true);
        ungiftable.giftable = false;
        let snap = Snapshot { links: vec![], games: vec![claimed, ungiftable] };
        let out = evaluate(&snap, &MarkerSet::default(), now(), &Thresholds::default());
        assert!(out.markers.write.is_empty(), "neither qualifies, so neither is marked");
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p surfacing engine::tests::surplus -v`
Expected: FAIL — `Game::new_for_test` unresolved, and no SurplusKey markers written.

- [ ] **Step 3: Write minimal implementation**

Add to `crates/domain/src/lib.rs` inside `impl Game` (mirroring Task 4's `Link::new_for_test`; construct every field explicitly if `Game` has no `Default`):

```rust
    /// A minimal Game for tests in dependent crates. See `Link::new_for_test`.
    #[doc(hidden)]
    pub fn new_for_test(id: &str) -> Self {
        Self {
            id: id.to_string(),
            title: format!("game {id}"),
            bundle: "test bundle".into(),
            gamekey: "gk".into(),
            machine_name: "mn".into(),
            key_type: "steam".into(),
            giftable: true,
            hidden: false,
            status: crate::GameStatus::Available,
            claim_id: None,
            artwork_url: None,
            keyindex: 0,
            requires_choice: false,
            steam_app_id: None,
            appid_source: None,
            owned_by_ben: false,
            hidden_source: None,
        }
    }
```

If `GameStatus::Available` is not the correct variant name, read the enum at `crates/domain/src/lib.rs:7` and use the variant meaning "listable, unclaimed".

In `evaluate`, before the clearing loop, insert:

```rust
    // ── surplus key ──────────────────────────────────────────────────────────────────
    // owned_by_ben is a bare bool: a boolean has no transition without a prior value, so
    // nothing in current state distinguishes "flipped today" from "flipped last March".
    // The marker's ABSENCE is the transition detector. On a first run that would mean
    // every key at once — hence inventory, hence silence.
    let is_surplus =
        |g: &domain::Game| g.owned_by_ben && g.giftable && g.claim_id.is_none() && !g.hidden;

    for g in snap.games.iter().filter(|g| is_surplus(g)) {
        let key = MarkerKey { kind: ReasonKind::SurplusKey, subject: g.id.clone() };
        if seen.contains(&key) {
            continue;
        }
        delta.write.push(key);
        debug_assert_eq!(ReasonKind::SurplusKey.first_run(), FirstRun::SeedSilently);
        if !first_ever_run {
            reasons.push(Reason {
                kind: ReasonKind::SurplusKey,
                subject: g.id.clone(),
                evidence: Evidence::new(
                    format!("you already own {} on Steam — that key is spare", g.title),
                    format!(
                        "get_game({}) has owned_by_ben && giftable && claim_id.is_none()",
                        g.id
                    ),
                ),
                fired_at: now,
            });
        }
    }
```

And replace the `ReasonKind::SurplusKey => true,` arm in the clearing match with:

```rust
            ReasonKind::SurplusKey => snap.games.iter().any(|g| g.id == k.subject && is_surplus(g)),
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p surfacing`
Expected: PASS, 17 tests.

- [ ] **Step 5: Commit**

```bash
git add crates/domain crates/surfacing
git commit -S -m "feat(surfacing): surplus keys — marker absence as the transition detector"
```

---

### Task 7: Marker and ledger persistence

**Files:**
- Modify: `crates/dynamo/src/schema.rs`
- Modify: `crates/dynamo/src/lib.rs`
- Modify: `crates/dynamo/Cargo.toml` (add `surfacing = { path = "../surfacing" }`)

**Interfaces:**
- Consumes: `surfacing::markers::{MarkerKey, MarkerDelta}`, `surfacing::engine::Outcome`.
- Produces: `Store::list_surfacing_markers() -> Result<Vec<MarkerKey>, StoreError>`; `Store::apply_surfacing_markers(&MarkerDelta) -> Result<(), StoreError>`; `Store::append_surfacing_ledger(&LedgerRow) -> Result<(), StoreError>`; `dynamo::LedgerRow { at: OffsetDateTime, verdict_json: String, thresholds_json: String, fired: usize }`.

- [ ] **Step 1: Write the failing test**

Append to `crates/dynamo/src/schema.rs`:

```rust
#[cfg(test)]
mod surfacing_key_tests {
    use super::*;

    /// Markers and ledger rows share the single table and must not collide with games,
    /// links, or claims. A prefix test is cheap and a collision is silent corruption.
    #[test]
    fn surfacing_keys_are_namespaced_and_distinct() {
        let m = surfacing_marker_pk("UnreadThanks", "tok-1");
        let l = surfacing_ledger_pk();
        assert!(m.starts_with("SURFMARK#"), "got {m}");
        assert_eq!(l, "SURFLEDGER");
        assert_ne!(m, l);
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p dynamo surfacing_key_tests`
Expected: FAIL — `cannot find function surfacing_marker_pk in this scope`.

- [ ] **Step 3: Write minimal implementation**

Add to `crates/dynamo/src/schema.rs`:

```rust
/// Partition key for one surfacing marker. Namespaced so a marker can never be mistaken
/// for a game, link, or claim in the single table.
pub fn surfacing_marker_pk(kind: &str, subject: &str) -> String {
    format!("SURFMARK#{kind}#{subject}")
}

/// Partition key for the dry-run ledger. One partition, sort key is the ISO timestamp.
pub fn surfacing_ledger_pk() -> String {
    "SURFLEDGER".to_string()
}
```

Add to `crates/dynamo/src/lib.rs` (inside `impl Store`), following the existing `list_links` scan at `crates/dynamo/src/lib.rs:2256` for pagination style — read it and mirror it rather than inventing a new one:

```rust
    /// Every marker the surfacing engine has recorded. Scanned, like `list_links`.
    pub async fn list_surfacing_markers(
        &self,
    ) -> Result<Vec<surfacing::markers::MarkerKey>, StoreError> {
        // Mirror list_links(): scan with the same pagination loop, filter on the
        // SURFMARK# prefix, and map each item's `kind` + `subject` attributes back.
        todo!("mirror list_links at crates/dynamo/src/lib.rs:2256")
    }
```

**Note to the implementer:** the `todo!()` above is deliberate and must NOT survive this task — it marks the one place where the correct code is "copy the shape of the neighbouring scan", which cannot be written blind. Open `crates/dynamo/src/lib.rs:2256`, read `list_links`, and write the parallel implementation. The test in Step 1 does not cover it; add an integration test alongside the existing dynamo tests that round-trips one marker through `apply_surfacing_markers` → `list_surfacing_markers`.

Then:

```rust
    /// Apply a tick's marker delta: writes first, then clears. Order matters only for
    /// crash-safety — a duplicated marker suppresses one message; a lost one repeats it.
    pub async fn apply_surfacing_markers(
        &self,
        delta: &surfacing::markers::MarkerDelta,
    ) -> Result<(), StoreError> {
        for k in &delta.write {
            let pk = schema::surfacing_marker_pk(&format!("{:?}", k.kind), &k.subject);
            self.client
                .put_item()
                .table_name(&self.table)
                .item("pk", AttributeValue::S(pk.clone()))
                .item("sk", AttributeValue::S("MARK".into()))
                .item("kind", AttributeValue::S(format!("{:?}", k.kind)))
                .item("subject", AttributeValue::S(k.subject.clone()))
                .send()
                .await
                .map_err(StoreError::from)?;
        }
        for k in &delta.clear {
            let pk = schema::surfacing_marker_pk(&format!("{:?}", k.kind), &k.subject);
            self.client
                .delete_item()
                .table_name(&self.table)
                .key("pk", AttributeValue::S(pk))
                .key("sk", AttributeValue::S("MARK".into()))
                .send()
                .await
                .map_err(StoreError::from)?;
        }
        Ok(())
    }

    /// Append one dry-run ledger row. This is the whole product of phase 1: what the
    /// engine WOULD have said, with its evidence and the thresholds in force.
    pub async fn append_surfacing_ledger(&self, row: &LedgerRow) -> Result<(), StoreError> {
        self.client
            .put_item()
            .table_name(&self.table)
            .item("pk", AttributeValue::S(schema::surfacing_ledger_pk()))
            .item("sk", AttributeValue::S(row.at.format(&Rfc3339).unwrap()))
            .item("verdict", AttributeValue::S(row.verdict_json.clone()))
            .item("thresholds", AttributeValue::S(row.thresholds_json.clone()))
            .item("fired", AttributeValue::N(row.fired.to_string()))
            .send()
            .await
            .map_err(StoreError::from)?;
        Ok(())
    }
```

And the row type, near the other public types in `crates/dynamo/src/lib.rs`:

```rust
/// One tick of the dry run. `fired == 0` is a REAL row, not a skipped write: a quiet day
/// is evidence, and a ledger that only records fires cannot tell you the null rate.
#[derive(Debug, Clone)]
pub struct LedgerRow {
    pub at: time::OffsetDateTime,
    pub verdict_json: String,
    pub thresholds_json: String,
    pub fired: usize,
}
```

Match the existing imports (`AttributeValue`, `Rfc3339`) to whatever the file already uses; do not add a second time-formatting dependency.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p dynamo && cargo build --workspace`
Expected: PASS; workspace builds with no `todo!()` remaining.

- [ ] **Step 5: Commit**

```bash
git add crates/dynamo
git commit -S -m "feat(dynamo): surfacing markers and the dry-run ledger, namespaced in the single table"
```

---

### Task 8: Wire the engine into the existing sync tick

**Files:**
- Modify: `crates/fulfillment/src/lib.rs`
- Modify: `crates/fulfillment/Cargo.toml` (add `surfacing = { path = "../surfacing" }`)
- Create: `crates/fulfillment/src/surfacing_tick.rs`

**Interfaces:**
- Consumes: `Store::{list_links, list_listable_games, list_surfacing_markers, apply_surfacing_markers, append_surfacing_ledger}`, `surfacing::engine::{evaluate, Thresholds}`.
- Produces: `fulfillment::surfacing_tick::run_surfacing_tick(deps: &Deps, now: OffsetDateTime) -> Result<usize, StoreError>` returning the number of reasons fired (0 on a quiet tick).

- [ ] **Step 1: Write the failing test**

Create `crates/fulfillment/tests/surfacing_tick_test.rs`:

```rust
//! The tick must persist a ledger row EVEN ON A QUIET DAY. A ledger that only records
//! fires cannot measure the null rate, and the null rate is the thing phase 1 exists to
//! measure.

#[test]
fn a_quiet_tick_still_writes_a_ledger_row() {
    // This is a signature/compile guard until the dynamo test harness is wired: assert
    // the function exists with the shape later tasks depend on.
    fn _assert_signature(
        f: fn(
            &fulfillment::Deps,
            time::OffsetDateTime,
        ) -> std::pin::Pin<
            Box<dyn std::future::Future<Output = Result<usize, dynamo::StoreError>> + Send + '_>,
        >,
    ) {
        let _ = f;
    }
}
```

**Note to the implementer:** if `crates/fulfillment/tests/handler_test.rs` already provides a store harness (read it first), replace the signature guard above with a real end-to-end test: an empty store ⇒ `run_surfacing_tick` returns `Ok(0)` AND a ledger row exists; a store with one thanked link ⇒ returns `Ok(1)`. **Both arms, or the quiet arm proves nothing.**

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p fulfillment surfacing_tick`
Expected: FAIL — `unresolved import` / `run_surfacing_tick` not found.

- [ ] **Step 3: Write minimal implementation**

`crates/fulfillment/src/surfacing_tick.rs`:

```rust
//! One tick of the surfacing engine, run DRY.
//!
//! NO DELIVERY. This writes a ledger row and nothing else — no Discord, no email, no
//! operator ping. The dry run is the positive control for the digest: if the ledger is
//! boring, that is learned here rather than in Ben's notifications.

use crate::Deps;
use dynamo::{LedgerRow, StoreError};
use surfacing::engine::{evaluate, Thresholds};
use surfacing::markers::MarkerSet;
use surfacing::snapshot::Snapshot;
use time::OffsetDateTime;

pub async fn run_surfacing_tick(deps: &Deps, now: OffsetDateTime) -> Result<usize, StoreError> {
    let snapshot = Snapshot {
        links: deps.store.list_links().await?,
        games: deps.store.list_listable_games().await?,
    };
    let seen = MarkerSet::from_iter(deps.store.list_surfacing_markers().await?);
    let cfg = Thresholds::default();

    let outcome = evaluate(&snapshot, &seen, now, &cfg);

    let fired = match &outcome.verdict {
        surfacing::verdict::Verdict::Fired(rs) => rs.len(),
        surfacing::verdict::Verdict::NothingToSay => 0,
    };

    deps.store.apply_surfacing_markers(&outcome.markers).await?;

    // The quiet day is a row. A ledger that skips silence cannot measure the null rate.
    deps.store
        .append_surfacing_ledger(&LedgerRow {
            at: now,
            verdict_json: serde_json::to_string(&outcome.verdict).unwrap_or_default(),
            thresholds_json: format!("{:?}", outcome.thresholds),
            fired,
        })
        .await?;

    Ok(fired)
}
```

Add to `crates/fulfillment/src/lib.rs` near the other module declarations:

```rust
pub mod surfacing_tick;
```

And call it from the sync path. Find `run_sync` in `crates/fulfillment/src/lib.rs` and add, at its very end, after every existing pass completes:

```rust
    // Surfacing runs LAST and its failure must never fail the sync: the sync's job is the
    // library, and a dry-run ledger is not worth losing a sync over.
    if let Err(e) = crate::surfacing_tick::run_surfacing_tick(deps, time::OffsetDateTime::now_utc()).await {
        tracing::warn!(error = ?e, "surfacing tick failed; sync unaffected");
    }
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings`
Expected: PASS, no clippy warnings.

- [ ] **Step 5: Commit**

```bash
git add crates/fulfillment
git commit -S -m "feat(fulfillment): run the surfacing engine dry on the existing sync tick"
```

---

### Task 9: Prove no new schedule, and document the ledger for the reader who will judge it

**Files:**
- Modify: `README.md`
- Create: `docs/operator/surfacing-ledger.md`

**Interfaces:**
- Consumes: everything above.
- Produces: no code.

- [ ] **Step 1: Write the failing check**

Run this and expect it to disagree with the claim "no new schedule":

```bash
cd ~/bendobundles
git diff --stat main..HEAD -- terraform/ | tail -1
```
Expected: **empty output** — no terraform changed. If anything appears, the "no new EventBridge schedule" constraint has been violated and must be reverted.

- [ ] **Step 2: Verify the constraint holds**

Run: `git diff --name-only main..HEAD -- terraform/ terraform-iam/ | wc -l`
Expected: `0`

- [ ] **Step 3: Write the operator doc**

Create `docs/operator/surfacing-ledger.md`:

```markdown
# the surfacing ledger (dry run)

phase 1 of the surfacing engine writes one row per sync tick and **delivers nothing to
anyone**. this file is how to read it.

## why it exists

the engine decides whether the app has anything worth telling ben. before it is allowed a
channel, it runs dry so the product can be **measured rather than hoped for** — *a dry run
is the positive control for the digest itself*. if the ledger is boring, we learn that here
instead of in his notifications.

## reading a row

| field | meaning |
|---|---|
| `sk` | the tick's timestamp (RFC3339) |
| `fired` | how many reasons fired. **`0` is a real row, not a skipped write** — a ledger that records only fires cannot tell you the null rate, and the null rate is the point |
| `verdict` | the full verdict JSON, including each reason's **evidence**: a claim and the path to re-derive it from the store |
| `thresholds` | the thresholds in force at that tick, so a row read months later needs no guessing |

## what to look for

- **the null rate.** quiet days should be common. if `fired > 0` on most ticks, the engine
  is manufacturing content and criterion ① has failed.
- **whether a fired reason is worth a ping.** read the `claim` and ask honestly: would ben
  want to be interrupted for this?
- **whether the evidence re-derives.** pick a row, follow its `rederive` string, and check
  it against the store. a reason you cannot refute is a rumour.

## the thresholds are ben's

the defaults in `surfacing::engine::Thresholds` are a starting point recorded alongside
every verdict, **not a shipped decision.** what is worth interrupting him for is a question
about his attention. phase 1 produces the evidence; he sets the bar.
```

Fix the README's dead spec link while here (it points at a path that does not resolve; the file was archived):

```bash
sed -i 's|docs/superpowers/specs/2026-07-02-bendobundles-design.md|docs/superpowers/archive/specs/2026-07-02-bendobundles-design.md|g' README.md
```

Then verify, with a control:

```bash
grep -o '(\([^)]*\.md\))' README.md | tr -d '()' | while read -r f; do [ -f "$f" ] && echo "OK $f" || echo "MISSING $f"; done
```
Expected: every line `OK`. ⚠️ the README has few links, so this is a small population — say so rather than calling it a sweep.

- [ ] **Step 4: Run the full gate**

Run: `cargo fmt --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace`
Expected: all pass. (`cargo fmt --check` rides in every pre-push chain here — skipping it burned a CI cycle before.)

- [ ] **Step 5: Commit**

```bash
git add README.md docs/operator/surfacing-ledger.md
git commit -S -m "docs(surfacing): how to read the dry-run ledger, and fix the README's dead spec link"
```

---

## Self-Review

**1. Spec coverage.** Reason vocabulary + falsifiable evidence → Task 1. `NothingToSay` first-class → Task 2. Fires-once markers that clear → Tasks 3–6. Per-reason first-run policy → Task 1 (`first_run()`), enforced in Tasks 4–6. Unread thanks / stale invite / surplus key → Tasks 4, 5, 6. Large-backlog summarisation → Task 4. Fire timestamps → Task 1 (`Reason.fired_at`) and Task 7 (`LedgerRow.at`). Dry-run ledger → Tasks 7–8. Existing schedule reused → Task 9 asserts terraform is untouched. Thresholds are Ben's → Task 4 (`Thresholds` recorded per row) and Task 9 (documented).
**Gap found and closed:** the spec's *"the governor's silent path is a test"* is phase 2 and is correctly absent here — the spec already marks it carried-forward.

**2. Placeholder scan.** One `todo!()` in Task 7 Step 3, deliberately marked with a note demanding its removal inside the same task and naming the file:line to copy from — the alternative was inventing a scan-pagination loop blind against a real AWS SDK. No other TBDs.

**3. Type consistency.** `MarkerKey`/`MarkerSet`/`MarkerDelta` are used identically in Tasks 3–8. `evaluate(&Snapshot, &MarkerSet, OffsetDateTime, &Thresholds) -> Outcome` is stable from Task 4 onward. `Verdict::from_reasons` is the only constructor. `first_run()` is defined once and asserted (via `debug_assert_eq!`) at each use site so the policy and the engine cannot drift apart silently.
