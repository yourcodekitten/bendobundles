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

## D1 — storage (domain)

`Link` gains:

```rust
/// ✍️ per-game gift tags on a curated link, keyed by game_id. The finest grain
/// of chosen-for-you: gift_note is the card on the whole present; these are the
/// stickers on each item. Only meaningful alongside curated_game_ids — admin-api
/// refuses orphan keys at the door, but old rows are trusted as written.
#[serde(default, skip_serializing_if = "Option::is_none")]
pub curated_notes: Option<std::collections::BTreeMap<String, String>>,
```

- **BTreeMap, not HashMap** — deterministic serialization (stable body bytes; a rewrite of an
  unchanged link is byte-identical).
- **Additive body-blob field** (postmark precedent): old bodies deserialize to `None`; absent
  key on the wire.
- **D1.1 — deployment-skew, named honestly (the #227 doctrine axis):** `update_link_meta` is a
  whole-body rewrite (`SET body = :b`). An OLD admin-api lambda editing a link (revoke /
  allowance / expiry) during the deploy window deserializes a body whose `curated_notes` it
  does not know and re-serializes without it — **notes silently dropped.** Accepted: the window
  is one deploy's length, the write requires ben editing a link inside it, and the loss is
  re-typeable. Mitigation: deploy admin-api first in the phase order; the PR body names the
  window (D4-clause precedent from the postmark arc). NOT mitigated by code — a
  preserve-unknown-fields `#[serde(flatten)]` future-proofing is a separate decision with its
  own blast radius, deliberately not smuggled in here.

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

**D2.1 — v1 is create-time only.** No post-create note editing. The gap is named in the PR body;
when wanted, `set_link_gift_note`'s single-attribute pattern is the shape (NOT read-modify-write
through `update_link_meta`, for that function's own documented race reason). Editing means
re-cutting the link in v1, which matches how ben actually composes (picks → wrap → send).

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
- **D3.3 — the detail endpoint carries the note too** iff it is link-scoped (both wire shapes
  build from the one projection precisely so they cannot drift field-by-field). If detail turns
  out to be link-free, the modal renders the note from the grid row it already holds and D3.3
  collapses to a comment; the plan pins which at task time.

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
- **D4.3 — ClaimDialog:** the tag appears in the confirm step, above the buttons — the why
  lands exactly where the friend's heart rate peaks (principle 1). Short quote style, same
  grammar as D4.2. If the ceremony feels crowded on review, D4.3 is the first thing family may
  cut — it is a placement, not a plumbing change.

## D5 — admin UI (Links.tsx compose flow)

The picked-games list in the create form gains a per-pick optional input — placeholder
`why this one? (optional)`, 280 cap with the gift_note editor's counter pattern, value carried
into `game_notes` only when non-empty-after-trim. Pick order and the pick-exposure line are
untouched. No separate screen, no new nav — it lives where ben already composes.

## D6 — out of scope (named, not implied)

- og/unfurl (link-level art and copy unchanged) · whisper · post-create note editing (D2.1) ·
  any open-shelf rendering.
- **ShelfPage / `ShelfGift`:** the shelf is dark in prod (zero SHELF# tokens). Carrying the tag
  onto `ShelfGift` is cheap and additive but unverifiable live — family call: fold it in now or
  leave the shelf exactly as dark as it is. Default: defer.

## invariants

1. Open-shelf wire payloads are byte-identical before/after (the field never serializes there).
2. A non-curated link cannot acquire notes through any endpoint (D2 refuses at the door).
3. Sealed views leak nothing (D3.1, by construction).
4. Absent note ⇒ zero rendered elements on every surface (chip, modal panel, ceremony line).
5. Same note, same bytes: BTreeMap keeps an unchanged link's body rewrite byte-identical.

## family questions (step 2)

1. **Cap:** 280 for the per-game tag (vs sharing the 500 gift_note budget)?
2. **Ghosts keep the tag** (D3.2) — agree the why is part of "what the gift was"?
3. **Ceremony placement** (D4.3) — does the tag belong in the ClaimDialog confirm, or is the
   card+modal pair enough and the ceremony stays lean?
4. **Shelf deferral** (D6) — fold `ShelfGift.note` in now while the surface is dark, or defer?
