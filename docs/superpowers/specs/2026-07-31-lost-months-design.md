# the lost months — choice discovery walks to completion and survives every era

> ben has 50+ games he already paid for sitting unclaimed in 2019–2021 Choice months,
> and the catalog cannot see them. it also cannot see **july 2026** — the active month.
> both blindnesses have the same two roots. this closes them.

status: spec (family review pending)
author: code kitten
issue: #53 (vintage month detail pages fail strict parse — filed 2026-07-07, "not scheduled" — scheduled now)
prod receipts: cloudwatch `/aws/lambda/brd-prod-ue1-bendobundles-fulfillment`, queried 2026-07-31 ~11:05Z

## Problem — two failure classes, verified live this morning

1. **The month walk truncates.** `choice_months hit the page cap with a cursor still
   pending — max_pages=26 months=77` fires every sync. The pending tail holds at least
   december-2019 + january-2020 (real Choice months; dec-2019 alone has 6 unclaimed
   picks per the 2026-07-05 HAR findings) and then, in all likelihood, the pre-Choice
   **Humble Monthly era** (2015–2019 — the endpoint is literally named
   `humble_monthly`). A bigger cap alone would walk ~50 Monthly months every sync to
   reach `complete` — wrong shape entirely; the walk needs an *era-aware* stop.

2. **One strict field kills a month.** `choice discovery: month read failed — skipping`
   with `missing field \`gamekey\``, last 7 days: **may-2020 ×8 and july-2026 ×4.**
   july-2026 is `isActiveContent` — the month ben can claim RIGHT NOW is invisible in
   his catalog. The membership-blob wire (`model.rs` `ContentChoiceOptionsWire`) has
   exactly one required field left: `gamekey: String` (line 64). (`initial` was
   defaulted by the earlier claim-all hardening; the 2026-06-30 `missing field
   \`initial\`` cohort — oct/nov/dec-2020 + jan-2021 — parses clean today, which also
   proves humble's shapes drift week to week: hardening must be general, not
   month-specific.)

The two roots compound: the truncation window shifts as months accrue, so *which*
vintage months get walked varies run to run, and any walked month can then die on a
strict field. Result: a permanently-shifting blind spot over exactly the months where
ben's unclaimed value lives.

3. **(found while grounding this spec, 2026-07-31 ~11:20Z)** — **the id-agreement
   obligation is already violated in prod: 15 live duplicate pairs.** merge_sync's
   trust contract warns that if discovery's `game_id(gamekey, offered.machine_name)`
   diverges from key-sync's `game_id(gamekey, tpk.machine_name)`, "the stale `true`
   record lingers as a duplicate instead of flipping." It does — table scan
   (`brd-prod-ue1-bendobundles-table`, 1093 GAME rows) shows 15 offered/tpk sibling
   pairs (`mylittleuniverse` + `mylittleuniverse_row_choice_steam`, `atomicheart`,
   `wingspan`, `diabloiv`, …): choice tpk machine_names follow the grammar
   `<offered>[_row|_ww]_choice_<steam|origin|gog|battlenet>` (enumerated from all 175
   choice-suffixed rows), so the two paths *never* agree on an id for the same game.
   Every claim ben makes through a surfaced month mints another pair. Any fix that
   surfaces MORE claimable months without fixing this multiplies the duplicates —
   which is why D3/D7 below are in scope, not riders.

## Goal / acceptance (all verified in prod, not on fixtures)

- **A1** — july-2026's claimable games appear in the catalog (`requires_choice: true`
  rows written; `month processed` log line for july-2026 with `claimable > 0`).
- **A2** — may-2020 processes (or is skipped with a structured, understood reason —
  never a bare parse error).
- **A3** — the walk terminates *complete for the Choice era*: no truncation warning;
  a new summary line states pages walked, months seen, and the stop reason
  (`cursor-end` | `era-stop` | `cap` — cap alone still warns).
- **A4** — december-2019 and january-2020 are discovered and processed: the lost
  months surface. (dec-2019's 6 unclaimed picks become visible claimable rows.)
- **A5** — no regression: `orders_failed=0` stays 0; existing catalog rows change
  only through the D7 flip (`requires_choice` true→false + key-field refresh on a
  claim — the A6 behavior) or plain additions; sync duration stays inside the lambda
  budget. Baseline measured 2026-07-31: ~507s of a 900s timeout, 88MB of 256MB;
  this PR adds ≈14 paced list GETs + ≈3 month reads ≈ +5s nominal. Worst-case hang
  exposure is bounded by TWO layers (OMBB's arithmetic rider — 40 pages × a 30s
  per-request timeout would be 20min, past any budget): the per-request timeout from
  the sequenced pre-PR, AND a **total-walk deadline** on the list walk (≈120s; on
  breach the walk terminates as `cap`-style truncation — warn, partial prefix,
  never an error). Do the multiplication, then pick constants in plan.
- **A6a** — the duplicate-pair factory is closed: a claim on a discovered game flips
  the offered row (one GAME row per game, before and after) — proven by TEST, the
  one deliberate fixture carve-out from this header's "verified in prod" rule
  (proving it in prod would require spending a real pick).
- **A6b** — the 15 existing pairs are healed via the Q5 runbook path, proven by a
  post-heal re-scan showing zero state-gate-eligible pairs — run once, eyes on it
  (`mylittleuniverse` excluded until its claim resolves, then healed by hand).

## Design

### D1 — wire hardening: the last strict field goes tolerant
`ContentChoiceOptionsWire.gamekey` → `#[serde(default)] Option<String>`, following the
`SubProductWire` precedent (model.rs:144-150) exactly. Audit the remaining membership-
blob structs for any other strict field that isn't identity-critical; default them with
the same one-comment-per-field justification the file already uses.

### D2 — the gamekey ladder (resolution, not guessing)
A month detail with no blob gamekey resolves through, in order:
1. the **list walk's** gamekey for the same `product_url_path` (vintage months carry
   it in the list even when their page blob drops it);
2. the **order side**: the sync's order walk already knows every purchased month as
   `product.machine_name` (e.g. `july_2026_choice`) ↔ gamekey — match on
   `product_machine_name`. This is the resolution for **probe months** (the active
   month is exactly the one the list omits and the blob drops);
3. none → **skip loudly** with the D5 shape log. No empty-string writes: `game_id()`
   derivation and both claim writes (`choosecontent`, `redeemkey`) take the month
   gamekey, so a made-up value would poison ids — absence is a skip, never a default.
Sentinel decision (Q3, family review 2026-07-31): refactor the walk's empty-string
gamekey placeholder to `Option<String>` **end-to-end in this PR** — D2 already
rewires the field's main consumer, and `""` is one `game_id()` call away from a
`:machine_name`-poisoned id. Fallback rule: if the diff sprawls beyond discovery +
probe + tests, keep the sentinel and file the refactor separately — and in that
fallback the sentinel is NOT left silent (OMBB rider): loud comment + a test that
the walk never emits `""` downstream.

### D3 — claimed-set: the #30 rider becomes code (order-authoritative, per family review)
Discovery currently trusts the blob's `contentChoicesMade.initial` alone — and because
that field *defaults*, an absent block silently reads as `Some(vec![])` = "no picks
made", which is exactly the ASSUMED-wire-shape label OMBB flagged on #30 ("Some alone
doesn't license writing `requires_choice: true`"). This PR pays that debt:

- claimed-for-month is **order-authoritative, and ONLY order-derived** (family review
  2026-07-31, OMBB riders folded): the month's order (`tpkd_dict.all_tpks`) is the
  sole source of "claimed". The blob's role shrinks to *offered-side only* — it may
  widen what was offered, and it may NEVER alone mark a game claimed (blob-as-claimed
  gap-filler was "#30 wearing a hat" — a still-claimable game retired on the blob's
  word). NOT a union: `∪` can only grow claimed → shrink claimable → silently re-hide
  a game ben can still claim, the same-direction harm as the bug this PR kills.
- **order silent ⇒ skip the month this pass, loudly** (kitten counter to the
  bias-to-claimable rider, accepted rationale in-channel): D2's ladder guarantees a
  gamekey, so an order exists for every processed month — "order silent" only means
  a *transient fetch failure*, and biasing to claimable there would mint claimable
  rows that discovery's own additive/never-delete contract then preserves as
  PERSISTENT ghosts after the transient clears. Skip-and-retry has no ghost path;
  the month surfaces one sync later.
- The deliberate trade, stated so it's a decision and not an accident: order-wins can
  *under*-claim when the matcher misses an exotic tpk shape, and that failure is LOUD
  at claim time — **dependency + tripwire (OMBB rider 1): this trade is sound only
  while claim-time refusals ping** (dead-key truth's `RedeemRefused`/choose-refusal
  pings). The plan adds a test that fails if a choose-on-spent-pick park stops
  pinging — the trade's tripwire, not an assumption that a mechanism stays healthy.
- matching is **set-membership, not bijection**: each offered name is claimed iff ∃ a
  matching tpk. Unmatched `_choice_*` tpks are EXPECTED (1:N grants: base+DLC tpks
  from one pick) — logged, never month-fatal.
- `claimable = offered − claimed`. A month whose claimed set is unknowable from BOTH
  sources skips loudly (contract preserved: never write `true` without a known set).
- **the matcher** (one domain fn, shared by D3 and D7, unit-tested against the prod-
  enumerated grammar; three rungs, per family review 2026-07-31):
  1. **grammar rung** (the only matching rung — lilith retired her own title-
     fallback proposal: "fuzz has no place beside a measured grammar"): tpk `t`
     matches offered `o` iff `t == o` or stripping `(_row|_ww)?_choice_<platform>$`
     from `t` (platform open — `[a-z0-9]+`, so gog/origin/battlenet/future ride
     free; region tokens closed, enumerated from prod) yields `o`. Minted tpks are
     NEVER the bare offered name (`redeem_claimed_tpk` burns `tpk.machine_name`
     verbatim as keytype; 07-05 HAR shows `<machine>_choice_steam`) — and the
     `_row` pairs in prod are why the rung strips region *before* `_choice`, not
     `starts_with(o + "_choice")`.
  2. **canary**: a `_choice_*` tpk matching NO offered name ⇒ D5 structured warn +
     a summary count — expected for 1:N grants (base+DLC tpks), so logged, never
     month-fatal, never a silent guess. An unmatched-because-exotic tpk leaves its
     game reading unclaimed → listed claimable → LOUD at claim time: the accepted
     trade direction, observable in the canary count before anyone claims.
  Matching is **set-membership, not bijection** (each offered name claimed iff ∃ a
  matching tpk). The claim-all tier gets its own fixture — its mint may not carry
  `_choice` at all. Without this matcher, surfacing vintage months would re-list
  already-claimed games as claimable (→ `choosecontent` on a spent pick → the exact
  choose-blind park loop the backlog flags as safety-critical).

### D7 — key-sync stops minting duplicate rows for choice tpks (the pair-factory fix)
When the order walk meets a `_choice_*`-suffixed tpk, derive the offered-row id
candidates deterministically (no table scans, just `get_game` on candidate ids) and,
when an offered row exists, route the fresh key-sync record onto THAT id so
`merge_sync` flips it (`requires_choice` true→false, key fields refreshed) instead of
writing a sibling row. Non-choice tpks are untouched. Two knots (lilith):
- the candidate lookup is a **ladder, not a set**: exact base first (an offered name
  that itself ends `_row`), region-stripped second, first hit wins — never a guess
  between two live candidates.
- a `_choice_*` tpk the grammar can't parse **still mints normally, loudly** — a
  lost key is worse than a loud pair.
Acceptance-tested: claiming a discovered game never creates a second GAME row (the
15th duplicate pair is the last).

### D4 — walk to completion, era-aware
- **pace the list walk**: `SYNC_PACE` (300ms) between pages — the known backlog item;
  26+ rapid GETs from a lambda IP is the exact bot profile the constant exists to avoid.
- **era-stop**: terminate the walk once a full page yields only true pre-Choice
  products, reporting `era-stop` as a *complete-for-Choice* terminal. The discriminant
  (family review 2026-07-31, final form — the draft's `contentChoiceData`-absence
  test, and the two-signal AND that briefly replaced it, both retired for the same
  reason: no era decision may rest on a droppable blob sub-field): a product is
  pre-Choice iff its `product_machine_name` is NON-EMPTY and does not end `_choice`.
  A product with an EMPTY machine_name (the list wire defaults it) is an
  anomaly-warn and DISQUALIFIES its page from era-stopping — a dropped field must
  never read as "pre-Choice era". An empty-`products` page terminates as
  `cursor-end`, never `era-stop`. Chronology sanity (OMBB): a "non-choice" product
  whose title parses to a post-2019 month is an anomaly → warn, keep walking. The
  stop line logs the boundary month's machine_name (D5) so drift stays visible.
  Stopping early is silent data loss; walking a few extra Monthly pages is 300ms each.
  Monthly-era months are out of scope (their games are ordinary claimed keys the
  order walk already covers).
- **cap becomes a runaway guard only**: `CHOICE_DISCOVERY_MAX_PAGES` 26 → 40 (~120
  months ≥ full Choice era + years of margin). Hitting it still warns — that would
  mean the era-stop never fired, which is itself a signal.
- `ChoiceMonthsWalk.complete` semantics extend to carry the stop reason (A3).

### D5 — observability: shapes become log lines, never log-dives
- structured per-month shape warn on any fallback/skip: `month`, `gamekey_source`
  (`blob|list|order|none`), `choices_made_absent`, `skipped_reason`.
- run summary gains `months_walked`, `months_skipped`, `stop_reason`.
- **explicitly rejected**: logging raw blob content to "capture fixtures" — the blob
  carries `csrfToken` (and the page is session-fetched); fixtures are synthetic,
  modeled on the observed absence patterns (may-2020 / july-2026 / claim-all shapes).

### D6 — tests (wiremock, per the existing client/fulfillment matrix)
- gamekey ladder: blob-present, blob-absent+list-hit, blob-absent+order-hit (probe),
  all-absent→skip.
- claimed-set union: blob-only, order-only (absent `contentChoicesMade`), both,
  neither→skip; derivation-agreement test vs merge_sync's rule.
- era-stop: choice pages → monthly page → walk stops, reason=era-stop, complete.
- cap: nonstop cursor still bounded (existing test extends to reason=cap + warn).
- pacing: walk sleeps between pages (structural — assert the pace hook is called
  n-1 times, however the existing SYNC_PACE tests do it).

## Out of scope (deliberate)
- claiming/gifting flows — untouched; this is discovery only. The safety-critical
  choose-blind reconcile work (backlog) is *not* made worse: discovery writes the
  same `requires_choice` rows it does today, just for more months, with a *stricter*
  claimed-set gate than today.
- Monthly-era (pre-Choice) catalog surfacing — separate idea, separate spec if ever.
- humble-client HTTP timeouts (backlog #5) — **sequenced, not folded** (Q4, lilith):
  `.timeout()` ships as its own tiny PR FIRST and lost-months rebases on top. The
  client has no timeout today and this PR raises exposure 26→40 list GETs — one hung
  socket eats the lambda budget A5 promises to stay inside, so the timeout is a
  *precondition* of A5, and this PR stays pure.

## Family questions (answers fold into this spec before plan)
- **Q1 (era-stop rule)**: stop after one full page with zero `*_choice`
  machine-names — tight enough? prefer K consecutive non-choice *months* instead?
- **Q2 (claimed-set matching)**: tpk machine_name ↔ offered machine_name derivation —
  counterexamples from the wild (`_row` suffixes, non-steam keytypes) before I pin it?
- **Q3 (sentinel)**: keep the walk-internal empty-string gamekey placeholder, or
  refactor to `Option<String>` end-to-end in this PR?
- **Q4 (timeouts)**: fold backlog #5's `.timeout()` into this PR or keep it pure?
- **Q5 — RESOLVED (family review 2026-07-31, lilith's frame, OMBB concurring)**:
  - **Mechanism vs deletion, split.** D7's routing absorbs pair data naturally
    in-PR — that's sync doing its job. The orphan DELETE lives in a **maintenance
    script + runbook entry, never the sync path**: dry-run mode, printed pair-list,
    operator glance before it runs. "Sync never deletes" stays literally true.
  - **This is not delete-on-absence — named explicitly so the contract stays
    undiluted.** That contract forbids deleting because something *stopped
    appearing*. This delete rests on positive dual evidence: the offered row
    verifiably carries the key fields post-flip AND the sibling's id derives to the
    offered id through the one matcher fn. Different species.
  - **State-gated per pair, at run time.** Auto-heal only siblings with zero
    app-owned state: `Available`, no `claim_id`, hidden unset, `appid_source ≠
    Manual`. The scan *schedules* the heal; the live order at execution time
    *authorizes* it (re-derive each pair fresh).
  - **Order: D7 lands first, sweep runs second** — close the factory, then sweep.
  - The heal goes through `merge_sync` semantics (Manual-appid precedence, hidden
    preservation) — no hand-rolled row surgery.
  - `mylittleuniverse`'s pair is excluded by the state gate (its claim references
    the sibling id) → runbook'd, healed by hand after the claim resolves.
