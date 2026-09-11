# spec: the gift tags ✍️ — per-game notes on curated links

*2026-09-11 · kitten · status: draft for family review*

## why

The success sentence is *"a friend opens their link and feels chosen for."* Today the chosen-for
grammar has two grains: the curated set itself (ben picked THESE games) and the link-level
`gift_note` (the card on the whole present). What's missing is the finest grain — the little tag
on each item: **"why i picked this one for you."** A friend reading *"you loved hollow knight —
this one's got the same bones"* on a specific cart is principle 2 (chosen-for-you, never
shopping) at full depth. Curation says *these*; the tag says *this, because of you*.

Scope is deliberately curated-links-only: an open-shelf link is "take anything" — per-game notes
have no referent there, and the open-shelf wire stays byte-identical.

## D1 — storage: top-level attribute, `curated_game_ids`' exact contract

> **REVISED after family crossfire (2026-09-11 morning).** Draft v1 rode the body blob with a
> deploy-order mitigation. Both reviewers broke it, composing: OMBB — the stated mitigation
> ordered admin-api against services not in the race (a no-op reading as managed risk); Lilith
> — the BLOCKER: **the claim path is a second whole-body writer in a different service**
> (`dynamo::claim_game`, `SET body = :b ADD claims_used`, called from public-api), so a FRIEND
> CLAIMING through a draining old lambda erases every tag, silently — and the house doctrine at
> `dynamo/src/lib.rs:419-432` already says, in this author's own hand: *"`immutable` does not
> qualify a field for `body`, and neither does `single-writer`"* — deploy-order covers first
> rollout only; ROLLBACK reopens it. The spec's own D2.1 pointed at the right recipe and filed
> it under the wrong D.

`domain::Link` gains the field; **the body never carries it.** The contract is copied from
`curated_game_ids`, its exact sibling (created-once, order/content sacred, no scoped editor):

- `pub curated_notes: Option<std::collections::BTreeMap<String, String>>` on the struct,
  `#[serde(default, skip_serializing_if = "Option::is_none")]` — the struct's serde is the
  admin-list wire (read-back, D5.2), NOT the storage format.
- `schema::link_body`'s exhaustive destructure binds it `_` (the stripped set) — a new field
  breaks compilation there until its body fate is decided; this decides it.
- `schema::link_item` writes top-level `curated_notes` as an `AttributeValue::M` of `S` when
  `Some`; omitted when `None`.
- `link_from_item`: unconditional override, top-level attr is the ONLY source — absent ⇒
  `None`; `M` with any non-string value ⇒ `StoreError::Corrupt`; non-`M` ⇒ `Corrupt` (mirror
  the `curated_game_ids` arms exactly).
- **No update expression anywhere names the attribute.** That is the whole point:
  `claim_game`'s `SET body` and `update_link_meta`'s `SET body` cannot touch it in ANY binary,
  old or new — claim-erasure and rollback-erasure die **by construction**, not by runbook.
  Pinned by a `curated_notes_survive_a_claim`-shaped test
  (`gift_note_scoped_write_survives_stale_body_writers` is the house pattern).
- **Residual skew, stated honestly:** an old `public-api` binary (its own rollout window, or a
  rollback) lacks the read-override and serves note-less links — the friend sees no tags for
  those minutes. A **display gap, self-healing on read; zero data loss.** No deploy-order
  requirement survives this design.
- BTreeMap for deterministic serialization on the admin wire and stable attr bytes.
- **Cost rides shotgun with correctness (OMBB, ×2'd by Lilith):** `claim_game` rewrites the
  whole body on the friend-facing hot path via `transact_write_items` — billed at **2 WCU per
  KB**, link and game items both riding — so body-carried notes would cost **~56–224 WCU per
  claim at the cap** vs today's handful. (Billing model at worst case, not a table
  measurement; a cap exists to bound the worst case.) The attribute keeps the claim write
  exactly as big as it is today. Correctness-plus-cost is why this storage decision is
  forced, not preferred.

**D1.1 — aggregate bound (OMBB's finding, answered from the file):** `game_ids` is bounded by
`CURATED_GAMES_MAX = 100` (admin-api:569 — *"an unbounded admin array is still an unbounded
array"*), so worst case is 100 notes × 280 **chars** ≈ 112 KB at 4-byte UTF-8 — chars are not
bytes, so the byte ceiling is stated here rather than implied: comfortable against DynamoDB's
400 KB item limit beside `gift_note` (500) and `label` (200). The cap stays in chars for UX
consistency with `gift_note`.

## D2 — admin-api (create-time)

`CreateLinkBody` gains `game_notes: Option<BTreeMap<String, String>>`.

Validation (all 400s name the offending id):
- **keys ⊆ game_ids** — a note for a game not in the curated set is a caller bug, refused
  (`game_notes[{id}] has no matching game_ids entry`). `game_notes` present while `game_ids`
  absent → 400 (notes require curation).
- each value is trimmed; **empty-after-trim is dropped, not stored** (absence and blankness
  collapse deliberately — the friend surface gates on presence).
- per-note cap: `GAME_NOTE_MAX_CHARS = 280`. A tag, not a letter — the 500-char `gift_note`
  budget is the card; the sticker is smaller. (Family may re-cut this number.)
- notes are ben's words as typed — no lowercase enforcement (the Lowercase Rule governs OUR
  copy, not his).

Validation refusals are **422** naming the offending id (house convention — the `game_ids`
tests' shape), not 400 as draft v1 said. **Empty-map normalisation is explicit:** if every note
trims to empty, the stored value is `None`, never `Some({})` — absence has one spelling
(Lilith's near-invariant catch: `Some({})` would serialize a key on the admin wire and make
invariant 5 *nearly* true, the worst kind).

**D2.1 — v1 is create-time only.** No post-create note editing. The gap is named in the PR body;
when wanted, `set_link_gift_note`'s single-attribute pattern is the shape (NOT read-modify-write
through `update_link_meta`, for that function's own documented race reason). Editing means
re-cutting the link in v1, which matches how ben actually composes (picks → wrap → send).

**D2.2 — the tag is about the RECIPIENT.** More personal than the game data beside it — same
exposure class as `gift_note` and the link label (anyone holding the token reads it), accepted
on the same basis, said out loud rather than assumed.

## D3 — public-api

`GameView` gains:

```rust
/// ✍️ ben's per-game gift tag (curated links only). Absent when unset —
/// open-shelf payloads stay byte-identical.
#[serde(skip_serializing_if = "Option::is_none")]
note: Option<String>,
```

- Set at the **curated assembly site** (and only there) from `link.curated_notes` — `from_game`
  keeps building link-agnostic rows; the note is a link-scoped overlay applied where the curated
  partition is built. Open-shelf and non-link surfaces never populate it.
- **D3.1 — sealed links: withheld by construction.** The sealed view returns `games: []`; notes
  live on game rows, so there is no arm to add — stated so nobody adds one.
- **D3.2 — ghost rows KEEP the note.** A ghost is a decided pick rendered so "the friend sees
  what the gift WAS" — the why is part of the was. Ghost assembly applies the same overlay.
  Ghost causes include ben-side withdrawals (hidden / ungiftable), where a tag could read
  strangely — but ghosts are **cause-blind by decision** (GameView.gone's own doc), so the tag
  never contradicts a displayed cause; the strangeness is bounded and accepted (family q2,
  Lilith's caveat recorded).
- **D3.3 — the detail endpoint carries the note.** RESOLVED (draft v1 left this an `iff`; a
  fork in a spec becomes a coin-flip in execution — Lilith): `handle_game_detail` is
  token-scoped and resolves the link before projecting the game, so the same overlay applies
  there. Both wire shapes build from the one projection precisely so they cannot drift.

## D4 — friend UI

Three renders, all presence-gated (**delight never gates** — no note, no pixels, no layout
ghosts):

- **D4.1 — the cart card:** a small `✍` presence chip in the existing chip row (label tier,
  standard chip grammar; no new hue, no Burgundy — the tag is not the act of claiming). It says
  *there's a tag on this one*; it does not carry the text (carts stay compact).
- **D4.2 — GameDetailModal:** the tag rendered in full — Chivo body in quotes, attributed
  lowercase (`— ben`), on a Floor panel inside the existing layout. No new shadow (Ceremony
  Rule), no pixel-font prose (Pixel Grammar Rule), no ♡ (One Heart Rule: the modal's budget is
  already spent).
- **D4.3 — ClaimDialog: CUT from v1** (family q3, Lilith's call, taken). It was the only pure
  placement judgment of the three, and ceremony density cannot be judged from a spec with zero
  specimens — the draft's own "first thing to cut" flag was the tell. Card + modal establish
  the grammar; once real tags exist on a live link, ben knows in ten seconds whether the
  ceremony wants it. Moved to D6 as the named follow-up; the cap conversation keeps D4.3's
  mobile-width constraint alive (D2's 280 is provisional until someone measures the confirm
  step at mobile width — the cap is a layout invariant, not a style preference).

## D5 — admin UI (Links.tsx compose flow)

The picked-games list in the create form gains a per-pick optional input — placeholder
`why this one? (optional)`, 280 cap with the gift_note editor's counter pattern, value carried
into `game_notes` only when non-empty-after-trim. Pick order and the pick-exposure line are
untouched. No separate screen, no new nav — it lives where ben already composes.

**D5.2 — read-back (Lilith's usability catch):** ben must be able to SEE the tags he wrote on
an existing link — composing without read-back plus no-edit (D2.1) would mean he can't check
what he told someone. The admin links list already carries the data (it serializes
`domain::Link`, and `link_from_item` populates the field from the attribute), so this is
render-only: the per-link row region that shows `gift_note` renders the tags read-only.

## D6 — out of scope (named, not implied)

- og/unfurl (link-level art and copy unchanged) · whisper · post-create note editing (D2.1) ·
  any open-shelf rendering.
- **ClaimDialog placement (was D4.3) — a rejection AND a deferral, named apart** (Lilith's
  rule: the two wear the same word in a changelog, and only one expires by itself):
  **REJECTED — the confirm-step placement as drafted.** OMBB's grammar argument decides it,
  not review procedure: a reason-to-take placed AT THE BUTTON, pre-decision, stops being
  chosen-for and becomes *persuasion* — shopping grammar, the exact pattern principle 2
  exists to forbid. This does not quietly return in v2 as "we deferred that."
  **DEFERRED — a post-claim placement** (the gifted panel, after "it's yours! ♡"): after the
  act, persuasion is structurally impossible and the tag is pure warmth. Judged only with
  real tags on a live link, ben looking (Lilith's zero-specimens rule applies here, where the
  grammar objection doesn't).
- **ShelfPage / `ShelfGift`:** DEFERRED (family q4, agreed). An additive field on a dark
  surface is unverifiable by construction, and an unverified additive field is the shape that
  rots. The reminder lives where the shelf's un-darkening is governed —
  `docs/spec-gift-shelf.md` carries the one-line pointer — not in anyone's memory of this
  thread.

## invariants

1. Open-shelf wire payloads are byte-identical before/after (the field never serializes there).
2. A non-curated link cannot acquire notes through any endpoint (D2 refuses at the door).
3. Sealed views leak nothing (D3.1, by construction).
4. Absent note ⇒ zero rendered elements on every surface (chip, modal panel).
5. The body never carries `curated_notes`, and **no update expression anywhere names the
   attribute** — claim-path and rollback erasure are impossible by construction, pinned by a
   survives-a-claim test.
6. Absence has one spelling: `None`, never `Some({})` (D2's empty-map normalisation).

## family questions — ANSWERED (crossfire 2026-09-11 morning; the review record)

1. **Cap:** 280, kept — but as a **layout invariant, provisional** (Lilith): derived caps beat
   familiar numbers, and nobody has measured the narrowest surface at mobile width yet. Stated
   in bytes too (D1.1) because chars don't bound the item.
2. **Ghosts keep the tag:** agreed, strongly — withholding the tag would show the object and
   withhold the reason, the inverse of chosen-for. Ghost-cause caveat recorded in D3.2.
3. **Ceremony:** CUT from v1 — judge density with real specimens, not from a spec. → D6.
4. **Shelf:** defer; the pointer lives in `spec-gift-shelf.md`, not in memory. → D6.

**Blocker (Lilith) + no-op mitigation (OMBB), composed:** draft v1's body-blob storage was
wrong by the house's own `SET body` doctrine — the claim path (public-api) is a second
whole-body writer, so the erasure trigger was *a friend claiming*, and deploy-order mitigations
cover neither that nor rollback. → D1 rewritten to the `curated_game_ids` top-level-attribute
contract; every named window dies by construction. **Neither reviewer out-thought the file:
the decisive doctrine was already written at `dynamo/src/lib.rs:419-432` — the review read one
file further than the spec's own citation.** (OMBB's follow-up correction, kept with the same
honesty it was posted: his no-op verdict was right but its mechanism reasoned from the spec's
denominator, not the code's — writer-enumeration from prose is the filed root error.)
