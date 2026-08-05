# out-of-band redemption — the sync learns what humble already knows (#158)

**date:** 2026-08-05 · **author:** code kitten · **status:** draft — family review, then ben's gate

## the incident (all receipts banked, all re-derived post-clear)

ben self-claimed My Little Universe (feb-2025 choice) on jul 6; the order had **zero tpks** that
day (`claim.choice_pre_tpks: []` — the durable snapshot says so). humble provisioned the key
later — before jul 31, under the choice name `mylittleuniverse_row_choice_steam` — and ben
revealed+redeemed it from humble's email, after jul 31, never clicking choose in the app. result,
live in prod today: the claim nags generically every morning (29+ days), and a **sibling row sits
`available`+giftable in the LISTABLE GSI whose key is already on ben's steam** — a landmine that
would hand a friend a spent key. receipt: fresh server-side order pull shows the tpk with
`redeemed_key_val` present (len 17), `is_gift: false`, `is_expired: false`
(`code-kitten/state/receipts/2026-08-05-mylittleuniverse-order-excerpt.json`, re-pullable).

## how humble tracks it (observable in their payload)

- `redeemed_key_val` latches the moment **anyone** reveals the key, by any route — email, web,
  app. one-way; never unlatches. humble's own UI refuses to gift a revealed key. this is
  **positive evidence**, not absence: a populated field, readable in the order payload the sync
  already fetches daily.
- `is_expired` / `num_days_until_expired` — the independent death clock.
- `is_gift` — on the wire, **not currently deserialized** (`TpkWire` drops it). it distinguishes
  key-converted-to-gift-link from self-reveal, and would answer the standing crash-after-gift
  question at fulfillment/src/lib.rs:3943.

our wire layer already consumes the first two correctly: `redeemed = redeemed_key_val.is_some()`,
`giftable = !redeemed && !expired` (humble-client/src/lib.rs:670-680), pinned by tests. **the sync
is not blind — specific rows and claims are starved of the truth it reads every day.** that
reframing (verified, correcting issue #158's own text) is what this spec fixes.

## root cause — two correct features composed into a trap (verified at source + live data)

1. **the discriminator went stale.** reconcile picks choice-vs-bundle procedure by
   `game.requires_choice` (fulfillment/src/lib.rs:3881-3882), whose comment still says "durable."
   D7 (2ea66b5, jul 31) made it mutable by design: the moment a tpk appears, fresh-wins flips it
   `false`. so the day MLU's key finally arrived, its claim silently switched to the bundle
   procedure — exact-match on a machine_name that isn't in the order — and parked forever.
   meanwhile `reconcile_choice_claim` arm **B3** (unique new tpk vs snapshot, already redeemed,
   SELF → `recover_already_redeemed_key` → fulfill autonomously) is *precisely this scenario*,
   built and tested, unreachable.
2. **the sibling row is frozen.** D7 routing diverts every order-walk write for the tpk onto the
   offered row (it exists → route), where `merge_sync`'s Pending arm pins status/giftable. the
   sibling row receives nothing, ever again. confirmed live: `available`, `gsi1pk: LISTABLE`,
   giftable, unhidden, no `version` attribute (untouched since pre-#134 schema).

three gaps, three prongs. no prong touches D7 or `merge_sync` — both are correct; the fixes go
where the evidence lives.

## design decisions

### 1. reconcile routes by the claim's own birth certificate

`run_sync`'s choice-branch condition widens from `game.requires_choice` to:

```rust
game.requires_choice || claim.choice_pre_tpks.is_some()
```

- `choice_pre_tpks` is written once at claim time and never mutated — it is the durable fact the
  comment *believed* `requires_choice` was. `Some([])` (MLU's case) means "born a choice claim,
  order empty that day"; `None` means bundle claim → bundle path, byte-for-byte unchanged.
- **widen, never narrow**: every claim that routes choice today still routes choice
  (`requires_choice` stays in the OR — a legacy choice claim from before snapshots exists only
  behind that half). no behavior is removed; one starved path is reconnected.
- transient `get_game` miss falls through to the bundle path exactly as today (the choice branch
  still needs the row for title disambiguation + messages).
- **what then happens to MLU, mechanically:** snapshot `[]` → `find_new_tpk` → exactly one new
  tpk → `Unique`, `redeemed` → **B3** → SELF → `recover_already_redeemed_key` → key recorded,
  claim `Fulfilled`. the nag dies because the claim completes, and the permanent record says
  "completed — key recovered from order" instead of a hand-authored "failed". auto-COMPLETE on
  positive evidence for ben's own claim is *already shipped, designed behavior* (B3 + the bundle
  path's identical arm at :3902-3918); this prong only restores its intended reach.
- **the B1 rule (three reviewers converged; smaller than any first proposal):**
  **auto-compensate only when `claim.choice_pre_tpks` is `None` — i.e. arm A keeps its
  compensate; arm B1 parks with a specific ping on EVERY route.** immutable, claim-intrinsic;
  no order probe, no grammar, no `requires_choice`.
  the reasoning trail, banked because each step killed a prior proposal:
  1. the hazard (lilith; severity calibrated by OMBB's rider 2): A/B1's documented backstop —
     humble refuses a re-choose, "churn + pings, never value" (lib.rs:1925-1929) — proves a
     *pick* can't double-spend. it says nothing about **re-listing a row whose key exists and
     is revealed**. stated at its true size: the re-listed offered row is the one D7 routes
     truth onto, so the same run's order walk flips it `BenRedeemed` minutes later — the
     hazard is a **minutes-wide window** (plus the fetch-fail case, where the flip doesn't
     come), not a persistent landmine. a minutes-wide door handing out a spent key is still a
     door; park closes it.
  2. the record-vs-world split (OMBB): "no new tpk ⇒ this claim's in-app choose never
     committed" is true of OUR WRITES regardless of flip timing — but "pick not spent" is a
     claim about the WORLD, false in a reachable case: a tpk already in the snapshot, revealed
     by ben out of band after the claim. this issue's founding incident is the existence proof.
  3. that case does not need the row to have flipped (lilith) — it fires identically on
     `requires_choice: true`, so keying any safety rule on the flag is wrong in BOTH
     directions, and the draft's "legacy half stays byte-for-byte" rationale was false.
  4. an order-evidence gate (probe for a matching redeemed tpk) would need the tpk→game
     grammar — promoting a heuristic this spec itself demotes to messaging-only (OMBB's tooth,
     lilith's concession). no gate, then: **B1 simply never re-lists.**
  **arm A is refit to LOOK before it acts (OMBB's rider 1 found the blindness; lilith's repair):**
  A's "choose never ran ⇒ genuinely unspent" is record-truth with the same structural hole —
  an out-of-band spend never touches our choose flow, and A never diffs the order at all
  (no snapshot, nothing to diff). but `find_new_tpk` needs *a* baseline, not a *recorded* one,
  and arm A has one: **the empty set.** diff `order.keys` against `&[]` — every tpk is "new,"
  the existing title match picks this game's out (the same function and trust level MLU's
  `Some([])` rides to B3; nothing promoted, nothing invented):
  - any title-matched tpk found (redeemed, clean, or ambiguous) → **park + specific ping**
    naming what was found — never an autonomous write, SELF included, permanently (OMBB):
    `None` and `Some([])` are not the same world state — **`Some([])` is a measurement of
    emptiness; `None` is the absence of a measurement.** on a `None` claim a found tpk cannot
    be attributed: the MLU story (provisioned after claim) and the stale-legacy story (existed
    all along) are indistinguishable, and auto-fulfilling would bind an old key to this
    claim's record and grow the autonomous-write set past the `Some`-only bound enumerated
    for ben. stronger still (lilith, verified): `TpkWire` carries no timestamp and nothing
    ordinal — **the snapshot is the only mechanism in the entire data model that can establish
    "this key arrived after the claim."** auto-attribution on `None` isn't weakly-founded,
    it's unfounded. **auto-recovery is a `Some`-only privilege, because `Some` is a receipt
    and `None` is a shrug.** the ping carries everything found; the human does the attributing.
  - genuinely nothing in the order for this game → **compensate — now having actually looked.**
  why A-compensates-on-nothing while B1-parks-on-nothing is principled, not inconsistent:
  a `Some` snapshot can HIDE a spent pick (an in-snapshot tpk revealed after the claim — the
  founding case), so B1's "nothing new" is not "nothing"; A's verified-nothing is about the
  order as it exists NOW — zero tpks present for this game means zero keys anyone could have
  revealed, whatever the unmeasured claim-time state was. on an unmeasured baseline,
  automation gets exactly two moves: compensate on verified-nothing, park on anything-found. **A's live exposure set is enumerated EMPTY
  today** (`pending ∧ choice-routed ∧ snapshot None` = 0 — deploy-day changelist below), so
  the refit's cost is zero live members. grammar stays exactly where the spec put it:
  **enriching pings, arming nothing.**
  **the honest cost:** a genuinely-crashed legacy choose (snapshot landed, `choosecontent`
  never committed, no tpk ever appears) now parks for a human nod instead of self-healing.
  a nag traded against a re-listed revealed key — taken.
  arms B2/B3 (positive-evidence completions) act exactly as today.
  **net: the widening adds completions and specific parks, and removes an unsafe
  auto-compensate; the only compensates that survive are A's, where humble's re-choose
  refusal provably bounds the damage to churn.**
- **alternatives rejected:** (a) grammar-fallback matching in the bundle path
  (`choice_tpk_bases` inference) — heuristic inference where authoritative evidence exists; the
  snapshot IS the record. grammar stays a *messaging* aid (prong 3), never a *routing* authority.
  (b) exempting claim-referenced rows from D7's flip — re-tangles sync state with claim state,
  the exact coupling that froze things; `requires_choice` is humble-truth and should track it.

### 2. the shelf-truth audit — an invariant, not a patch

new final pass in `run_sync`: **no listable row may reference a key humble marks revealed or
expired.**

- while walking orders (already fetched — zero extra humble calls), build an in-memory map
  `(gamekey, machine_name) → (redeemed, expired, key-fields)`. after the order walk, sweep the
  full catalog scan — **which becomes unconditional (OMBB caught this): today's shared scan is
  gated on `deps.steam.is_some()` (fulfillment/src/lib.rs:3408), an unrelated feature's
  dependency. an "every sync" invariant cannot inherit a stranger's off-switch. hoist the
  `list_all_games` scan out of the steam gate (the title/ownership passes keep their own
  steam-gating on their logic, not on the scan; the audit becomes the scan's third consumer —
  one Scan either way).** for each row where `is_listable()`
  and the map holds its exact `(gamekey, machine_name)` and `(redeemed || expired)` — write the
  fresh truth **to that row's own id**, no D7 routing (we are correcting an existing row, not
  minting; routing exists to prevent minting). `merge_sync`'s Available arm does the rest: fresh
  wins → `BenRedeemed`/`Expired` → giftable false → the LISTABLE marker drops (schema.rs:75), and
  with it the friend-claim condition (`attribute_exists(gsi1pk)`) and the public listing.
  status flip also closes the self-claim door (its condition requires `available` — the one gate
  a listability-only fix would have missed).
- **positive evidence only.** a listable row whose `(gamekey, machine_name)` is absent from the
  map is untouched — acting on absence stays banned (dead-key-truth). a failed order fetch means
  that order's rows simply aren't in the map: same clause, no special case.
- one `ping()` per pulled row: `"shelf audit: pulled <title> (<id>) — key <revealed outside the
  app | expired> on humble"` — never a key value. audit count lands in the sync summary string.
- **set-driven, every sync, no once-markers** (pending_age_sweep's philosophy): a contested
  write (`SkippedInFlight`) or a missed pass self-corrects next run.
- **why an invariant instead of patching D7 to also-update siblings:** the audit catches *any*
  listable row drifted from order truth — frozen siblings, pre-D7 leftovers, future routing
  bugs, non-choice rows — one mechanism, one rule, mirrored from humble's own ("we never gift a
  revealed key"). a D7 side-write would fix exactly one known path and nothing else.
- **de-list ≠ delete.** sibling *deletion* remains the heal runbook's human-gated job; the audit
  only takes rows off the shelf and says so. no new `GameStatus` variant (dead-key-truth
  precedent: states are lifecycle; the ping carries the why).
- **companion fix — the heal gate (regression ben's question caught):** `heal_pairs::pair_verdict`
  skips any sibling whose status isn't `Available` (heal_pairs.rs:21). an audited sibling flips
  to `BenRedeemed`/`Expired` — without a companion change, every audited sibling becomes
  permanently unhealable and runbook step 4 breaks. widen the gate: sibling status
  `Available`, or `BenRedeemed`/`Expired` **when the live order's matching tpk confirms it**
  (redeemed/expired respectively) — the dual-evidence rule gets *stronger*, not weaker. bin
  stays human-gated, dry-run default, unchanged otherwise.
- **a residual may become a feature — conditionally (OMBB's downgrade):** the compensate arm's
  documented worst case (crash-after-gift falsely compensated → burned key re-listed,
  fulfillment/src/lib.rs:3942-3951) is a silent landmine of exactly this species — **iff a
  gifted key latches `redeemed_key_val`, which is precisely the open question that arm's own
  risk note names and prong 4's receipt exists to answer.** if gifts latch, the audit nets it;
  if they don't, the residual stands as documented. claim no comfort the evidence doesn't back;
  prong 4 decides.
- **ping volume:** if one run pulls >3 rows (ben bulk-redeeming out of band), collapse to one
  summary ping listing them; the sync summary string carries the count either way.

### 3. the nag says what it knows

at the bundle path's absent-from-order park (fulfillment/src/lib.rs:3889-3900, where `order` is
in scope), enrich the reason before `alert_unreconcilable`: probe `order.keys` with
`choice_tpk_bases` — if some tpk's derived base equals the claim's machine_name, append:
`"a key for this game exists on humble under `<tpk_machine_name>`<, already revealed outside
the app><, expired>"`. detection-only, zero writes, no key values, `alert_unreconcilable` itself
stays generic. with prongs 1–2 this arm should rarely fire — when it does, the operator learns
which conversation to have instead of "still pending after N days."

### 4. wire: model `is_gift`, detection-only

one serde line: `is_gift: Option<bool>` onto `TpkWire` → `KeyEntry` — **`Option`, not
`bool + default` (lilith):** this field's entire job is accumulating a receipt about whether
humble sends it at all; a defaulted `false` would make an order that never carried the field
indistinguishable from one that said no, and the receipt would report data that never existed.
`redeemed_key_val` already models absence correctly; follow it. consumed
nowhere except: the audit's ping and reconcile's existing redeemed-arm logs mention it when
`true`. this accumulates the "does a gift set `redeemed_key_val`?" live receipt the compensate
arm's risk note has been waiting for (fulfillment/src/lib.rs:3943-3951) — **no gate consults it
until a receipt confirms semantics.** (family: strike this if it reads as creep; my defense is
one field + one log line against an open risk question in a write path.)

### ordering note — reconcile before the audit, and why that's now safe by design

**stated invariant, not an accident of today's function order (OMBB): the audit runs LAST in
`run_sync`, after every writer** — pending sweep → reconcile → order walk → passes → **audit**.
with the B1 rule, reconcile's only remaining re-lists are arm A's (choice-flow rows, bounded to
churn by the re-choose refusal) and the bundle path's existing compensate (unchanged behavior,
its own documented risk note, conditionally netted by the audit per §2). nothing reconcile can
re-list carries a key the audit's map could see and miss. (lilith's ④, discharged by the B1
rule.)

### the deploy-day changelist — enumerate N before the gate (lilith's ③, step-zero applied)

ben's criterion names one row and one claim; prong 1 and the audit act on *sets*. before ben's
gate, enumerate — read-only, live prod — exactly what the first post-deploy run will touch:
(a) every pending claim with `choice_pre_tpks: Some(_)` on a flipped row (the re-routed set),
split SELF vs gift (OMBB: only SELF members auto-*write* via B3; gift members only park
better); (b) every LISTABLE row whose order tpk shows revealed/expired (the audit's pull
list). the enumerated list goes in front of ben *with* the spec gate: he signs the instance
set, not just the method. coverage authorizes the method, never the instance.

**ENUMERATED 2026-08-05 ~18:05Z (read-only sweep, receipt in code-kitten
`state/receipts/2026-08-05-158-deploy-day-changelist.json`): every set is exactly ben's two
named objects and nothing else.** (a) pending claims: 2 total; **re-routed set = 1** — claim
`3f46c058` (MLU, SELF → B3 auto-complete; no gift-flavored members, so the only autonomous
*writer* on run one is ben's own claim). the other (`3da0c011`, soulcalibur6, SELF) has
`choice_pre_tpks: None` AND `requires_choice: false` → bundle path, untouched. (b) **arm-A
exposure set (`pending ∧ choice-routed ∧ snapshot None`) = 0 members** (OMBB's rider 1
clause). (c) audit pull list over all 671 listable rows across 93 orders, 0 fetch failures:
**1** — `GK:mylittleuniverse_row_choice_steam` (revealed, not expired). the acceptance
criterion covers the entire first-run instance set.

**and the count is a measurement, not a bound (lilith's deploy-time rider):** it was taken at
`f7f34d7`; ben signs later; deploy is later still, and nothing stops a new member entering a
set in between. so the enumeration is a **deploy-time step, not a pre-gate artifact**: re-run
all three clauses immediately before the first post-deploy sync fires. **if it does not still
return 1 / 0 / 1, stop — the changelist goes back to ben before anything runs.** a count
decays exactly like an approval does; this converts "we checked and it was one" into "it is
one, now, as we press."

## non-goals

- **no auto-fail, no auto-compensate changes.** terminal fates stay human except the
  already-designed SELF auto-complete on positive evidence. gift claims with burned keys stay
  pending + human-recovery ping, unchanged.
- no D7 routing changes, no `merge_sync` changes (Pending-arm pinning is *correct* while a claim
  lives — the claim's resolution, not the sync, unfreezes the row).
- no sibling deletion (heal runbook's job, human-gated), no new `GameStatus` variant, no schema
  migration (`choice_pre_tpks` and `version` already exist).
- #157's operator bin: untouched by this spec; prong 1 likely makes it unnecessary (the claim
  fulfills itself with a truer story). closing it is ben's call, not this spec's.

## wrong in both directions (dead-key-truth's bar)

- **the audit is a net, not a gate (OMBB's words, lilith cosigned):** it shrinks the landmine
  window from *forever* to ≤ one sync interval plus a race; it does not close the claim-time
  door (the `Game` struct carries no redemption field, so the friend-claim condition cannot
  check one). a friend claiming in the window between reveal and the next audit pass still
  wins the row; the existing reconcile arms then catch the burned key at fulfillment time.
- **missed detection** → exactly today's behavior: row stays listed, nag stays generic — and the
  audit re-runs every sync, so a transient miss (fetch failure, contested write) heals on the
  next pass. degraded, never corrupted.
- **false detection** → a row leaves the shelf with a loud ping naming its evidence; nothing
  terminal happens to any claim; ben can contradict. since `redeemed_key_val` cannot unlatch, a
  false positive requires humble's payload to lie — accepted residual.
- **false auto-complete** (prong 1's worst case) → B3 requires unique-new-tpk against an
  immutable snapshot AND `SELF` AND `redeemed_key_val` present; the completion pings, and a
  wrong completion is correctable by hand (fail/compensate). this exposure shipped with B3;
  the routing fix restores reach, not new risk.

## testing

wiremock + dynamodb-local in `fulfillment/tests/handler_test.rs`, existing helpers
(`order_json`'s `redeemed` toggle, `remount_order` for two-pass truth changes, `sync_deps`):

- **the MLU test, end to end (the named known positive):** claim `pre_tpks: []` + offered row
  flipped `requires_choice: false` + frozen sibling row listable + order shows the tpk redeemed
  → one `run_sync` → claim `Fulfilled` with recovered key, sibling de-listed (`BenRedeemed`, no
  `gsi1pk`), both pings fired. this test IS the acceptance criterion in miniature — and both
  game items are seeded **without a `version` attribute** (prod's real shape: the live rows are
  pre-#134 legacy items; the guarded write's `attribute_not_exists(version)` adopt-at-1 arm is
  exactly what the deploy-day run will exercise, so the test exercises it too).
- discriminator: snapshot-`Some` + `requires_choice: false` routes choice (B3); snapshot-`None`
  still routes bundle; `requires_choice: true` + snapshot-`None` still routes choice.
- the B1 rule: snapshot `Some` + no new tpk → parks + specific ping on BOTH routing halves
  (flipped and unflipped rows — two tests).
- arm A refit: snapshot `None` + order carries a title-matched tpk (each of: redeemed, clean,
  ambiguous) → parks + specific ping, does NOT compensate; snapshot `None` + order genuinely
  empty for this game → compensates (the having-looked pin).
- audit runs with `steam: None` (lilith: assert the decoupling, or the next person re-couples
  it and every fixture still passes) — the de-list fires with no steam client configured.
- heal gate: `BenRedeemed` sibling + order-confirmed redeemed tpk → `Heal`; `BenRedeemed`
  sibling + order tpk NOT redeemed → `Skip` (evidence mismatch); `Gifted` sibling still skips.
- audit: two-pass remount (clean → redeemed) de-lists on pass 2; absent-from-order row untouched
  (absence pin); expired variant; contested-write row de-lists on the following sync.
- enrichment: message text pin for the grammar-hit case; no-hit stays generic (exact current
  string).
- wire: `is_gift` parse + default pins in `humble-client/tests/client_test.rs`.

## acceptance criterion (ben's, discord 2026-08-05 17:12Z — the live known positive)

the currently-live row `GAME#HAXSVMZHBvK2E7dW:mylittleuniverse_row_choice_steam` stays exactly as
it is until deploy. **the first post-deploy sync must, with no human action: de-list that row
(audit) and fulfill claim `3f46c058` with the recovered key (discriminator → B3), with pings for
both.** if that run doesn't resolve that exact row and that exact claim, the feature is not
ship-green, whatever the tests say.
