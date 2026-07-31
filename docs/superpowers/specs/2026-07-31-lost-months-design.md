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
- **A5** — no regression: `orders_failed=0` stays 0, no existing catalog row changes
  except additions, sync duration stays comfortably inside the lambda budget
  (current baseline to be measured in plan; walk adds ≈14 paced list GETs ≈ 4s and
  ≈3 month reads ≈ 1s).

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
The list already uses an empty-string placeholder internally (`unwrap_or_default`,
lib.rs:753) — that stays internal to the walk; discovery's ladder output is a real
gamekey or a skip. (Family Q3: whether to refactor the sentinel to `Option` while
we're here or leave it.)

### D3 — claimed-set: the #30 rider becomes code
Discovery currently trusts the blob's `contentChoicesMade.initial` alone — and because
that field *defaults*, an absent block silently reads as `Some(vec![])` = "no picks
made", which is exactly the ASSUMED-wire-shape label OMBB flagged on #30 ("Some alone
doesn't license writing `requires_choice: true`"). This PR pays that debt:

- claimed-for-month := `choices_made ∪ tpk-derived claimed set from the month's order
  (`tpkd_dict.all_tpks`)` — the order is the *authoritative* record of spent picks
  (confirmed in the 2026-07-05 findings; May-2021's claimed Metro Exodus + Vane appear
  there byte-for-byte as bundle-shaped tpks).
- `claimable = offered − claimed`. A month whose claimed set is unknowable from BOTH
  sources skips loudly (contract preserved: never write `true` without a known set).
- the tpk↔offered machine-name matching rule must be pinned to the same derivation
  `merge_sync` / key-sync use (the #30 id-agreement obligation), with a unit test that
  fails if the two derivations drift. (Family Q2: known counterexamples — non-steam
  choice tpks, `_row` variants — welcome before this hardens.)

### D4 — walk to completion, era-aware
- **pace the list walk**: `SYNC_PACE` (300ms) between pages — the known backlog item;
  26+ rapid GETs from a lambda IP is the exact bot profile the constant exists to avoid.
- **era-stop**: terminate the walk once a full page yields only pre-Choice products
  (no `*_choice` `product_machine_name` / absent `contentChoiceData`), reporting
  `era-stop` as a *complete-for-Choice* terminal. Monthly-era months are out of scope
  (their games are ordinary claimed keys the order walk already covers).
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
- humble-client HTTP timeouts (backlog #5) — adjacent; Family Q4 whether to fold the
  one-line `.timeout()` in while touching the client or keep the PR pure.

## Family questions (answers fold into this spec before plan)
- **Q1 (era-stop rule)**: stop after one full page with zero `*_choice`
  machine-names — tight enough? prefer K consecutive non-choice *months* instead?
- **Q2 (claimed-set matching)**: tpk machine_name ↔ offered machine_name derivation —
  counterexamples from the wild (`_row` suffixes, non-steam keytypes) before I pin it?
- **Q3 (sentinel)**: keep the walk-internal empty-string gamekey placeholder, or
  refactor to `Option<String>` end-to-end in this PR?
- **Q4 (timeouts)**: fold backlog #5's `.timeout()` into this PR or keep it pure?
