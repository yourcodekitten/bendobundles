# chosen for you — per-link curation

status: accepted (family-reviewed 2026-08-19); implemented on this branch
author: kitten 🐱 (pounce arc)

## the idea

PRODUCT.md has promised it since line 12: a friend's job is to *"see which games were chosen
for them"*, and Design Principle 2 is **"Chosen-for-you, never shopping."** The implementation
never caught up — `handle_get_link` calls `list_listable_games()` with no per-link scoping, so
every friend on every link gets ben's ENTIRE listable catalog in a once-per-visit shuffle. The
only personalization a link carries today is `label` + `gift_note` + `claims_allowed`.

This spec makes a link able to carry **the specific games ben picked when he wrapped it**. The
product thesis, implemented instead of asserted.

## user stories

- ben browses his catalog, selects three games with a friend in mind, and creates a link that
  carries exactly those three — picked from the same catalog toolkit he already filters with.
- the friend opens the link and sees *those three games, in ben's order* — "ben chose these for
  you" — not two hundred cards in a shuffle. the unwrap gets more personal, not less.
- the friend claims one; `claims_allowed` still governs how many they may take, unchanged.
- every existing link (and any link ben creates without picking games) behaves exactly as today:
  the whole listable shelf. curation is a new mode, not a migration.

## design decisions (each enforced, none cosmetic)

### 1. domain: `curated_game_ids: Option<Vec<String>>` on `Link`, create-time-only, stored as a TOP-LEVEL attribute

- `None` = open shelf (every existing record; `#[serde(default)]` makes pre-field records read
  back as `None` — the same back-compat shape every optional `Link` field already uses).
- `Some(ids)` = this link IS those games. **Order is meaning**: the array order is ben's pick
  order and the friend's presentation order. No shuffle for curated links.
- **Create-time-only, like the seal.** `unlock_at`'s rationale transfers verbatim: the server
  can't know whether a friend already looked, so mutating the gift under them is off the table
  (spec 2026-08-05 §4). A wrapped gift's contents are chosen when it's wrapped.
- **Storage: a top-level dynamo `L`-of-`S` attribute (order-preserving list — never `SS`, string
  sets don't hold order and order is meaning), written once by `link_item` at create, STRIPPED
  from the body blob by `schema::link_body`, overridden on read by `link_from_item` (absent =
  `None`).** This reverses the first draft's body-blob choice, and the reversal is the merge of
  two threat axes (family review, both verified in source):
  - *Edit races* (OMBB, #69's axis): irrelevant here — the field is immutable, no edit exists
    to race. Body-blob was adequate on this axis alone.
  - *Stale writers* (Lilith's axis, and the decider): the claim tx rewrites `body` from a
    round-tripped `Link` (`SET body = :b`, `crates/dynamo/src/lib.rs:~1073`); so does
    `update_link_meta` (`:658`, called by revoke). A ROLLED-BACK binary — bad deploy reverted,
    lambda pinned to an old version — deserializes the body into a `Link` with no such field
    and its re-serialize ERASES the curation, silently, on the next claim or revoke. Deploy
    order protects the first rollout; nothing protects a rollback, forever. A top-level
    attribute is structurally immune: `SET body = :b` cannot touch what does not live in the
    body, and no existing update expression names the new attribute.
  - **The repo had already ruled and all three of us initially walked past it** (Lilith read it
    back to us): dynamo `lib.rs:379` — *"body for immutable identity, top-level attrs for
    enforcement AND for anything editable post-creation."* The claim gate READS the curated
    set, so it is an enforcement field. A field can be immutable and still be enforcement;
    the doctrine has two axes and each of us checked only the one we were holding.
  - What a stale binary CAN still do: serve the open shelf and skip the claim gate (it doesn't
    know the field exists). **The attribute does not buy immunity — it buys RECOVERABLE-AND-LOUD
    instead of UNRECOVERABLE-AND-SILENT** (Lilith's terms, adopted verbatim): a rollback shows
    the whole shelf on a curated link, visibly and undoably, and self-heals on redeploy; the
    body-blob choice would have let the first claim erase ben's picks forever.
  - Pinned tests (structural immunity is asserted, not assumed): create→read round-trip
    preserves order; a claim on a curated link leaves the attribute intact; revoke (today's one
    `update_link_meta` caller) leaves it intact; the stored `body` blob never contains the
    field (the `gift_note_never_persisted_in_body_blob` shape, same test file); **and the
    rollback pin** — `update_link_meta` and `claim_game` called with a `Link` carrying
    `curated_game_ids: None` (what a pre-field binary's deserialize produces) against a stored
    record that HAS the attribute must leave the attribute standing. That is the rollback,
    pinned (Lilith).
- Empty vec is refused at create (422) and never stored — `Some([])` would be a link to nothing,
  which is a typo, not a gift. Absent attribute is the single representation of "open shelf".

### 2. wire, friend side: `LinkView` gains `curated: bool`; curated `games[]` come from the set

- Curated link: `games[]` = `batch_get_games` over the curated ids, **PARTITIONED — never
  filtered** (Lilith caught the first draft filtering to `is_listable()` two lines above the
  ghost cards it was supposed to produce: you cannot mark what you dropped). Walk the stored
  id order; each id becomes a live card, a ghost card, or is skipped only when the game record
  no longer exists at all. Genres/tags ride the same slim steam-cache batch read as today.
  Open-shelf links keep the exact current path (`list_listable_games`), untouched.
- **Live vs ghost, decided out loud** (the first draft let `is_listable()` decide by accident;
  `gone` has six causes, not three — status `Pending`/`Gifted`/`BenRedeemed`/`Expired`,
  `!giftable`, `hidden`):
  - **LIVE** = `giftable && !hidden && status ∈ {Available, Pending}` — **`Pending` rescues
    only the STATUS axis, never the decided ones** (Lilith, sign-off review: my first cut was
    `is_listable() || Pending`, which let an in-flight claim override a deliberate hide —
    and hidden+Pending has NO path back to claimable, because compensate re-lists conditioned
    on `#st = :pending` while the hide outlives the claim; a live card that can never become
    claimable is the prediction-in-fact's-clothing objection pointing the other way). A plain
    Pending claim IS genuinely undecided (`compensate` → Available, success → Gifted,
    terminal → Expired), so it rides live and the claim transaction is the arbiter — its
    condition (`#st = :available AND attribute_exists(gsi1pk)`) refuses race-free with the
    existing "someone beat you to it" 409, exactly the semantics of an undecided race.
  - **GHOST** (`gone: true`) = the decided states: `Gifted`, `BenRedeemed`, `Expired`,
    `!giftable`, `hidden`. Rendered with title/art, dimmed, non-interactive, cause-neutral
    copy.
  - Consequence for the detail endpoint: its access gate would 404 a live Pending card's
    modal. **The contract is: the gate serves exactly what the grid offers live, plus this
    link's claims history — and the contract is implemented as ONE function with two callers,
    never as parallel arms** (Lilith, family round three; OMBB concurring — the gate's own
    `#154` comment documents the LAST time a hand-mirrored correspondence to the grid
    drifted, and this feature found drift #2). A single `live_on_link(link, game)` predicate:
    curated → member AND (`is_listable()` OR `Pending`); open shelf → `is_listable()`. The
    curated partition and the detail gate both call it; "mirrors the grid, not one id more"
    is then true by construction, not by maintenance.
  - **A welcome tightening falls out**: on a curated link, a listable NON-member 404s at the
    detail endpoint (it is not on this link's grid) — a curated token can no longer enumerate
    ben's whole catalog's details. Open-shelf links are unchanged (the predicate's None branch
    IS today's `is_listable()` arm). Ghosts stay non-interactive; the gate does not widen for
    them (the surface is not the boundary, in both directions).
- `curated: true` on the wire so the friend surface can shift its copy and drop the shuffle —
  inferring it from list size would misfire the day ben's open shelf dwindles to three games.
- The sealed state's withholding is unchanged: a sealed curated link returns `games: []` like
  any sealed link. Curation must not leak through the seal.
- A curated game already claimed **on this link** appears in the claims history exactly as
  today (that path already batch-reads titles for claimed ids).
- Ghosts appear rather than vanish: a three-game gift that quietly becomes two gaslights the
  friend; a storefront hides sold-out stock, a gift acknowledges it. **Copy stays
  cause-neutral** (OMBB): "already found a home" is a fib for a key ben pulled dead; the copy
  is e.g. "this one's spoken for", true under every cause.
- **The wire carries `gone: true` and NOT the cause — by decision, not by silence** (Lilith
  flagged that the first draft made this choice without noticing it was one). Rationale: the
  cause (`BenRedeemed`, `hidden`, dead key…) is ben's catalog management, and the friend wire
  is unauthenticated-beyond-token; the sealed view already established that withholding
  happens at the source and devtools is not a spoiler channel. Ben's admin surfaces carry full
  status for debugging. If a future UI wants differentiated ghost copy, adding a cause field
  is an additive wire change made THEN, out loud.

### 3. enforcement: the claim path refuses out-of-set games server-side

In `handle_post_claim`, after the `can_claim` domain gate and before the transaction: if the
link is curated and `body.game_id` is not in the set → **409** `"that one isn't part of this
gift"`. The friend surface never offers the button, but the surface is not the boundary — the
API is. (No transaction change: the curation set is immutable, so a pre-check against the
freshly-read link cannot race an edit that can't happen.)

### 4. admin: pick games in the catalog, wrap them at create

- `POST /admin/api/links` accepts optional `game_ids: [String]`. Validation, all 422 with the
  offending ids named: unknown id, non-listable game (hidden / claimed / dead key), empty array,
  duplicates. `claims_allowed > game_ids.len()` is also 422 — a 5-claim link over 3 games is a
  promise the link cannot keep, and admin-api's create already 422s on impossible inputs
  (born-exhausted, overflow-year expiry) rather than clamping silently.
- **The `allowed ≤ count` invariant is enforced on EVERY path that can move `claims_allowed`**
  (OMBB, family review — a create-only check is theater). Verified in source 2026-08-19, with a
  refinement: `update_link_meta` (`crates/dynamo/src/lib.rs:658`) CAN write `claims_allowed`,
  but its only admin caller today is `handle_revoke_link` (`admin-api/src/lib.rs:719-731`),
  which never changes the number — **no endpoint edits `claims_allowed` post-create yet.** So
  the enforcement lands where it can actually run: the sync create check, plus a doc-comment on
  `update_link_meta` obligating any future `claims_allowed` editor to re-check, plus a pinned
  test that revoke (today's one `update_link_meta` caller) preserves `curated_game_ids` through
  its body rewrite.
- Admin web: the **Catalog page grows multi-select** — tap cards into a pick, riding the
  existing toolkit (search/filter/sort/group already work; selection is the only new state) —
  then "wrap these into a link" carries the pick to the Links create form, which shows them as
  reorderable chips (pick order = default order). The Links form also works standalone: create
  with no picks = open-shelf link, exactly today's flow.
- Links list rows show a curated-count chip, and the **create-success card lists the chosen
  titles** — the "did i wrap the right things" confirmation, captured at create time at zero
  fetch cost. Retro-inspection of an OLDER link's full title list is deliberately out of scope
  (§6): the count chip and the API carry the ids; a titles join on the links page arrives with
  its own need.

### 5. friend surface: the curated unwrap

- No shuffle; ben's order. The "N gifts waiting" beacon is UNCHANGED — it counts remaining
  claims (`claims_allowed - claims_used`), which is the true number of gifts the friend can
  still take; a 3-game / 1-claim curated link correctly says "1 gift waiting". (This line
  originally said to count the curated set's live games — that was wrong, corrected 2026-08-19
  during planning.)
- Copy shift in the JRPG dialog framing: the grid header reads as chosen-for-you, not as the
  whole attic. Exact copy at implementation, gated by the existing typewriter flow — no new
  narrative machinery, the same box just says something more personal.
- Small-set layout: 1–3 games should not render as a lonely corner of a 3-column grid; the grid
  already collapses columns responsively, so this is a presentation tweak, not a new component.
- The `×N copies` dedupe chip logic is bypassed for curated sets — ben picked ids, not titles;
  if he deliberately picks two copies of the same game for a household, that is two cards.

### 6. explicitly out of scope (YAGNI'd on purpose)

- **Editing the set after create** — create-time-only per §1. If ben ever wants it, it arrives
  as its own spec with the `gift_note` top-level-attribute pattern and a "what did the friend
  already see" answer. Not silently bolted on.
- **Reservation** — curation does NOT hold keys. Two links may both include the same game; the
  first claim wins (the existing atomic tx already owns that). Reserving would lock ben's
  shared shelf against outstanding links and needs expiry-release machinery. Named so a future
  pass doesn't reach for it as the obvious tidy-up.
- **Friend-side search/filter for open-shelf links** — real gap (admin has a whole toolkit,
  friends have a shuffle), separate concern, separate spec.
- **Retro-inspection UI of an old link's chosen titles** — the created card confirms at wrap
  time (§4); a browse-back UI needs a titles join the links page doesn't have today.
- **Choice-game interaction**: curated sets may include `requires_choice` games; the claim path
  is identical to today's (curation gates WHICH ids, fulfillment owns HOW they redeem). No new
  choice logic.

## approaches considered

- **filter client-side in the friend app** — refused: enforcement must be at the API (the wire
  would still ship the full catalog to devtools, and the claim endpoint would honour any id).
  Sealed links already established that withholding happens at the source.
- **top-level dynamo attribute + edit endpoint** (the `gift_note` pattern) — deferred, not
  refused: it is the right shape *if* curation ever becomes editable. Today it buys storage
  complexity for a mutation §1 rules out.
- **reservation semantics** — refused for v1, see §6.
- **a separate `curations` table/record keyed by token** — refused: one more read on the hottest
  friend path, for a list that is small, immutable, and born with the link.

## testing (the placement-pin rule applies)

- domain: serde back-compat pin — a pre-field stored `Link` body deserializes with
  `curated_game_ids: None`; empty-vec refusal is create-path, not domain.
- public-api: curated `LinkView` shape (order preserved, `curated: true`, non-listable handling
  per the family answer); sealed curated link withholds `games` entirely; claim of an
  out-of-set id → 409 with the exact refusal string; open-shelf links byte-identical to today
  (pin the current shape so the new arm can't disturb the old).
- admin-api: each 422 arm (unknown id, non-listable, empty, dupes, allowed>count) named in its
  own test; happy path stores order verbatim.
- web (vitest): catalog multi-select state; create-form chips (order, reorder, removal); friend
  grid curated presentation (no shuffle — pin by rendering twice and asserting stable order);
  ghost-card arm per family answer.

## family review (2026-08-19, shared channel)

OMBB, 11:10Z (all three mechanisms verified in source before integrating):

1. **cross-link take / vanished game**: ghost card, agreed — with **cause-neutral copy**,
   because `gone` also covers hidden and dead-key games. Folded into §2.
2. **`claims_allowed > curated count`**: 422 at create — AND mirrored on the post-create
   `claims_allowed` edit path, which my draft missed (`update_link_meta` makes it editable;
   create-only enforcement is theater). Folded into §4.
3. **storage**: body blob holds; #69's top-level dance is an edit-race fix and curation has no
   edits. But the body has multiple WRITERS (claim tx `:~1073`, `update_link_meta` `:658`) —
   `schema::link_body` must carry the field, a pin test proves claim preserves it, and deploy
   order (body-writers before first curated create) is a stated release constraint. Folded
   into §1.

Lilith + OMBB, round two (11:14–11:17Z) — the round that reversed a storage decision:

4. **§2 contradicted itself** (filter-to-listable two lines above the ghosts it was meant to
   produce) — rewritten: partition, never filter.
5. **`gone` is six causes**, and `Pending` was being decided by omission — now decided out
   loud: Pending rides LIVE (in-flight, tx is the arbiter), decided states ghost. §2.
6. **Cause-blind wire made a conscious decision** with the privacy rationale written down. §2.
7. **Storage reversed: body blob → top-level `L` attribute.** Lilith named the axis nobody
   weighed (a ROLLBACK's stale binary erases a body-carried field via `SET body = :b`,
   silently, forever); OMBB took the correction against his own answer and brought the
   citation that overrules him — dynamo `lib.rs:379`'s own doctrine: enforcement fields go
   top-level, and the claim gate makes this an enforcement field. "Recoverable-and-loud
   instead of unrecoverable-and-silent" adopted verbatim, rollback pinned by test. §1.

Round three (11:20Z):

8. **The detail-gate arm may not ship as a third hand-mirrored `||` arm** (Lilith; OMBB
   amended his own endorsement — "mirrors the grid, not one id more" is a machine-checkable
   condition asserted in English, and the gate's own `#154` comment records the LAST time the
   grid↔gate correspondence drifted). Contract kept as the spec sentence; implementation is
   ONE `live_on_link(link, game)` predicate with two callers (grid partition + detail gate).
   Fallout is a deliberate tightening: a curated token cannot enumerate catalog details. §2.

All mechanisms in rounds one, two, and three were verified in source by at least two of the
three of us before folding — including each other's.
