# spec: the postmark 📮 — the year ben bought them

status: DRAFT (2026-09-09) — family review pending · author: kitten · pounce arc

## why

PRODUCT.md, design principle 3, verbatim: *"Games are artifacts with stories — cover art,
trailers, **the year ben bought them** — not table entries."*

Cover art: shipped. Trailers/media: shipped (steam detail). The year: **nowhere.** `Game.bundle`
is a bare string chip (`GameDetailModal.tsx`, the `bg-shelf` chip row) and no date exists anywhere
in the domain — `Game` has no acquisition time at all. Fifteen years of nostalgia is the product's
whole hook ("the feeling of finding something you forgot you loved") and the *fifteen years* part
is invisible on every surface. A friend can't feel that a treasure waited thirteen years for them
if nothing says so.

The postmark closes the last unbuilt clause of principle 3: every game remembers when it arrived
in the attic.

## the shape (decisions, numbered for review)

**D1 — wire (humble-client).** `Order` gains `created: Option<OffsetDateTime>`, parsed leniently:

- Humble's order API historically emits a NAIVE timestamp (`"2013-03-27T18:22:58.812445"`, no
  offset). Parse: RFC3339 first, then the naive format **assumed UTC**. At month/year display
  granularity a worst-case ±day is immaterial; the assumption is recorded here, not hidden.
- `#[serde(default)]` + unparseable ⇒ `None` with a `tracing::warn!` naming the gamekey — a bad
  or absent date NEVER fails an order read. The postmark is garnish; key truth stays the meal.
- **A1 (assumption): the live wire carries `created`.** The repo fixture is hand-trimmed and
  lacks it; every public humble client parses it, but nobody here has measured OUR wire. The
  design is absence-tolerant either way (no date ⇒ no postmark, nothing else changes), and the
  assumption is VERIFIED POST-DEPLOY by measurement: after the first full sync, count games with
  `acquired_at` present vs total (admin ops or a dynamo scan). Zero coverage ⇒ investigate the
  wire, feature quietly dark, no user-visible breakage.

**D2 — domain.** `Game.acquired_at: Option<OffsetDateTime>`, `#[serde(default)]`, rfc3339 serde
like siblings. Storage: **body blob** — it is immutable identity (when the bundle was bought),
not enforcement; no claim path reads it. No new dynamo attribute, no GSI, no schema change.

**D3 — merge rule (merge_sync).** Sync-authoritative, absence-tolerant both directions:
`fresh.acquired_at = Some` wins over existing anything (a corrected wire date propagates);
`fresh = None` NEVER erases an existing `Some` (a transiently dateless read must not strip
postmarks). A change counts as a difference ⇒ `Written`. Follows the existing merge_sync
conventions for sync-owned fields — exact clause lands at plan time after reading that function.

**D4 — writers.** The order walk stamps every fresh `Game` from its order's `created`. The
choice-discovery ingest (the other Game writer) stamps from the month order it already holds —
verify the order object is in scope there at plan time; if it is not, choice games ship
postmark-less in v1 and the gap is named in the PR body rather than silently absorbed.

**D5 — public api.** The friend-facing game payload exposes `acquired_at` (optional, RFC3339).
Raw instant, not a pre-derived year: copy lives in the client, and the payload stays a fact.
No privacy dimension — it is ben's own nostalgia, deliberately shared.

**D6 — web, the actual feature.** Two sites, friend surface only:

1. **Detail modal**: a postmark chip beside the bundle chip, both render paths (steam and
   non-steam): `📮 mar 2013`. Lowercase month, the attic voice. No chip when `acquired_at`
   is absent — absence renders as exactly the today-state, never as "unknown".
2. **Claim ceremony**: on the win moment (`it's yours ♡`), when the game was acquired ≥ 1 year
   ago, a second line: `it waited {N} year{s} for you ♡` (floor of whole years, N ≥ 1 only —
   "waited 0 years" is worse than silence). Absent date or < 1 year ⇒ no line.

**D7 — whisper embed (ben-facing, small).** The weekly whisper card already says the bundle a
treasure arrived in; add the year to that line (`from {bundle}, {year}`). One format string.
OPEN — see Q2; cut freely if family thinks it crowds the card.

## non-goals (decided, not omitted)

- **No bundle entity, no timeline/era view, no attic-museum wing.** One warm line in two places.
  YAGNI is the design principle the attic survives by.
- **No unfurl change** — the wrapping paper's OG cards stay as reviewed (crowding the card
  dilutes the label + art that ARE the unfurl).
- **No backfill job** — run_sync walks EVERY gamekey each pass (read at `fulfillment/src/lib.rs`
  `run_sync`, the `'orders` loop), so the next scheduled sync IS the backfill.
- **No admin surface change** — the workbench gifts fine without it; revisit only if ben asks.

## open questions (for OMBB + Lilith)

- **Q1**: D3's "fresh Some wins over existing different Some" — consistent with merge_sync's
  existing conventions, or does that function treat any field as first-write-wins in a way that
  argues otherwise?
- **Q2**: D7 whisper year — in or out? (One format string, but the whisper card was reviewed
  hard in its own arc; I don't widen reviewed surfaces casually.)
- **Q3**: anyone measured humble's `created` format on the live wire (naive vs offset)? The
  lenient parse covers both; a measurement would just retire A1 early.
