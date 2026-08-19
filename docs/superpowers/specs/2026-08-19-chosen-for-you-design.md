# chosen for you — per-link curation

status: draft, family review pending (2026-08-19)
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

### 1. domain: `curated_game_ids: Option<Vec<String>>` on `Link`, create-time-only

- `None` = open shelf (every existing record; `#[serde(default)]` makes pre-field records read
  back as `None` — the same back-compat shape every optional `Link` field already uses).
- `Some(ids)` = this link IS those games. **Order is meaning**: the array order is ben's pick
  order and the friend's presentation order. No shuffle for curated links.
- **Create-time-only, like the seal.** `unlock_at`'s rationale transfers verbatim: the server
  can't know whether a friend already looked, so mutating the gift under them is off the table
  (spec 2026-08-05 §4). A wrapped gift's contents are chosen when it's wrapped. This also
  settles storage: an immutable field lives in the stored `body` blob — the top-level-attribute
  dance (`gift_note`, `thank_note`) exists ONLY for fields editable after create.
- **⚠️ The body blob has more than one writer** (OMBB, family review; both verified in source):
  the claim tx rewrites `body` from a round-tripped `Link` (`SET body = :b` via
  `schema::link_body(&bumped)`, `crates/dynamo/src/lib.rs:~1073`), and `update_link_meta` does
  the same on admin edits (`:658`). Three consequences, each enforced:
  1. `schema::link_body` must CARRY `curated_game_ids` (it strips only the
     editable-elsewhere fields; this field is neither).
  2. A pinned test: claiming on a curated link preserves `curated_game_ids` in the stored
     body byte-for-byte — the round-trip is where an old struct silently eats the field.
  3. **Deploy order is a correctness constraint**: every body-writing binary (public-api,
     admin-api, anything in fulfillment that writes link bodies) deploys WITH or BEFORE the
     first curated create. An old binary's `Link` deserializer ignores the unknown field and
     its re-serialize erases the set — silently. The release runbook states this.
- Empty vec is refused at create (422) and never stored — `Some([])` would be a link to nothing,
  which is a typo, not a gift.

### 2. wire, friend side: `LinkView` gains `curated: bool`; curated `games[]` come from the set

- Curated link: `games[]` = `batch_get_games` over the curated ids, filtered to `is_listable()`,
  **in stored order**, genres/tags riding the same slim steam-cache batch read as today.
  Open-shelf links keep the exact current path (`list_listable_games`), untouched.
- `curated: true` on the wire so the friend surface can shift its copy and drop the shuffle —
  inferring it from list size would misfire the day ben's open shelf dwindles to three games.
- The sealed state's withholding is unchanged: a sealed curated link returns `games: []` like
  any sealed link. Curation must not leak through the seal.
- A curated game already claimed **on this link** appears in the claims history exactly as
  today (that path already batch-reads titles for claimed ids).
- A curated game claimed **via another link** (or hidden/unlisted since wrapping): returned
  with `gone: true` and rendered as a ghost card rather than silently shrinking the gift. A
  three-game gift that quietly becomes two gaslights the friend; a storefront hides sold-out
  stock, a gift acknowledges it. **Copy must stay cause-neutral** (OMBB, family review):
  `gone` covers claimed-elsewhere AND hidden AND dead-key — "already found a home" is a fib
  for a key ben pulled dead. Neutral copy, decided at implementation, e.g. "this one's spoken
  for". The wire says only `gone: true`, never the cause.

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
- **The `allowed ≤ count` invariant is enforced on EVERY path that can move `claims_allowed`,
  not just create** (OMBB, family review — a create-only check is theater): `update_link_meta`
  (`crates/dynamo/src/lib.rs:658`) makes `claims_allowed` editable post-create, so whichever
  admin endpoint raises it must 422 when the link is curated and the new value exceeds the set
  size. Verified in source 2026-08-19, not taken on OMBB's word alone.
- Admin web: the **Catalog page grows multi-select** — tap cards into a pick, riding the
  existing toolkit (search/filter/sort/group already work; selection is the only new state) —
  then "wrap these into a link" carries the pick to the Links create form, which shows them as
  reorderable chips (pick order = default order). The Links form also works standalone: create
  with no picks = open-shelf link, exactly today's flow.
- Links list rows show a curated-count chip; the existing link detail surfaces the chosen set.

### 5. friend surface: the curated unwrap

- No shuffle; ben's order. The "N gifts waiting" beacon counts the curated set's live games.
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

Lilith: no reply as of 11:2xZ; spec review is not a blocking gate (plan sign-off at step 5 is).
Anything she adds folds in when it lands.
