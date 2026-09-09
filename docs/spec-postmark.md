# spec: the postmark 📮 — the year ben bought them

status: FAMILY-REVIEWED (2026-09-09, OMBB round 1 — Q1/Q2/Q3 + D2 note integrated below) ·
author: kitten · pounce arc

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

- **A1 RETIRED PRE-DEPLOY — measured on the live wire 2026-09-09T07:1x-04:00** (OMBB's Q3 call:
  one authed `GET /api/v1/order/<gamekey>` answers both halves; run from this seat with the SSM
  session, cookie never echoed): `has("created") = true`, value `"2012-08-15T19:41:25.765070"`
  — **NAIVE, no offset**, exactly the historical format. Parse order: **RFC3339 FIRST, naive
  second** (Lilith, round 2 — the directions are not symmetric: RFC3339 on a naive string MUST
  fail (no offset to find), while a naive parser on an RFC3339 string MAY silently swallow the
  offset if it tolerates trailing input. RFC3339-first is never worse, sometimes better, free —
  and the unmeasured era is the RECENT one, exactly where an API modernises to RFC3339. This is
  the only failure path here that yields a WRONG date rendering as a plausible postmark instead
  of degrading to absence; a garnish that lies is worse than none). Naive **assumed UTC**. At
  month/year display granularity a worst-case ±day is immaterial; recorded, not hidden.
- `#[serde(default)]` + unparseable ⇒ `None` with a `tracing::warn!` naming the gamekey — a bad
  or absent date NEVER fails an order read. The postmark is garnish; key truth stays the meal.
- Absence-tolerance stays even with A1 retired (one order measured, not fifteen years of them:
  early orders may predate the field or carry junk — any dateless order ships postmark-less and
  nothing else changes). **The post-deploy coverage count STAYS, and here is why (OMBB round 2):
  it was measuring THREE assumptions — present · parses · gets stamped — and the live GET retired
  only *present*. D1's parse-failure⇒None + D6's absence⇒today-state make a broken format
  description INVISIBLE on every surface (identical to "no date on the wire", now proven false);
  the only symptom would be a warn nobody greps. The count is the instrument that can see it.**
  D-verify: games with `acquired_at` vs total, after the first full sync; near-zero ⇒ parse bug,
  not wire absence.
- **The live specimen `"2012-08-15T19:41:25.765070"` is PINNED as a repo fixture** (test input in
  humble-client + the order fixture) — six fractional digits, and the repo has NO existing
  naive-datetime parse idiom (zero `format_description!`/`PrimitiveDateTime` before this); a
  description that fails to consume the subsecond fails on the only real specimen we hold.
- Assumed-UTC is immaterial at month granularity EXCEPT within hours of jan 1, where it can flip
  the displayed YEAR — the field the feature is named for. ~0.1% of stamps: **accepted on
  purpose, recorded here, not by accident** (OMBB's nit, kept as a deliberate trade).

**D2 — domain.** `Game.acquired_at: Option<OffsetDateTime>`, `#[serde(default)]`, rfc3339 serde
like siblings. Storage: **body blob** — with the REAL reason written down (OMBB round 1, enemy
renamed by Lilith round 2): the threat family here is **deployment skew** — any rolled-back or
racing OLD binary whose `Game` lacks the field does `SET body = :b` from a round-trip and
silently drops it. (`dynamo/src/lib.rs:405-416` documents the *edit-race*, a DIFFERENT enemy —
the general lesson goes into that file's own comment — OMBB is opening that against the
doctrine file himself, round 3.) What makes body storage CORRECT for this field is not
"immutable" — D3 lets a corrected date propagate, so it isn't — and not "single-writer" alone
either (skew doesn't count writers: eight live `SET body` expressions in dynamo, FIVE on the
game item plus `put_game`'s put_item — Lilith's measured count, round 3, replacing the round-1
floor of three — and a stale binary drops the field through any of them). **The property is RE-DERIVABILITY: the sync walk re-stamps every
pass, so a skew-loss is a gap, not a death — and single-writer is what makes that re-derivation
uncontested** (OMBB round 3; his contrast case: a hypothetical `first_seen_at` is single-writer
and NOT re-derivable — one stale lambda loses it permanently). **Plus the EXPIRY CONDITION
(Lilith round 3): re-derivability is a property of run_sync's FULL walk, not of the field — if
the walk ever goes incremental (new orders only, the obvious optimisation), every body-only
field silently loses this durability net on a change that reads as a performance win. Body is
safe for a field the full walk re-derives; that safety dies the day the walk stops being full.**
Both clauses + a still-full walk required before this placement is reused. No new dynamo
attribute, no GSI, no schema change; no claim path reads it.

**D3 — merge rule (merge_sync).** Sync-authoritative, absence-tolerant both directions:
`fresh.acquired_at = Some` wins over existing anything (a corrected wire date propagates);
`fresh = None` NEVER erases an existing `Some` (a transiently dateless read must not strip
postmarks). A change counts as a difference ⇒ `Written`. **Confirmed consistent (OMBB, Q1):
`merge_appid`'s last two arms — fresh `Some` wins, else preserve — ARE this rule; nothing in
the function argues first-write-wins. And his red flag is the implementation shape:** the
`Pending|Gifted` branch is an explicit literal (won't compile until classified — safe), but
`Available|BenRedeemed|Expired` ends in `..fresh`, where the field lands FREE and fresh `None`
erases an existing `Some` with no compile error — `..fresh` is the catch-all for a new *field*
that the branch's no-`_` comment brags about banning for a new *variant*. ⇒ **a named
`merge_acquired_at()` helper, called explicitly in BOTH branches** (the `merge_appid` pattern),
plus a test pinning never-erase in that branch specifically. **And one turn further (Lilith,
round 2): the `Available` arm becomes a FULL EXPLICIT LITERAL like `Pending|Gifted` — kill the
class, not the instance. With `..fresh` gone, the compiler forces every FUTURE field to be
consciously classified in both branches, which is what the no-`_` rule already does for
variants.**

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

**D7 — whisper embed (ben-facing, small) — IN, via the embed field (OMBB, Q2).** NOT the
content line: `whisper.rs:196` is reviewed voice under a contended cap (`:190` makes bundle a
truncation loser). The embed field at `:316` — `"{bundle} ({key_type})"` — gains the year:
`"{bundle} ({key_type}, {year})"` when `acquired_at` is present, unchanged when absent.

## non-goals (decided, not omitted)

- **No bundle entity, no timeline/era view, no attic-museum wing.** One warm line in two places.
  YAGNI is the design principle the attic survives by.
- **No unfurl change** — the wrapping paper's OG cards stay as reviewed (crowding the card
  dilutes the label + art that ARE the unfurl).
- **No backfill job** — run_sync walks EVERY gamekey each pass (read at `fulfillment/src/lib.rs`
  `run_sync`, the `'orders` loop), so the next scheduled sync IS the backfill.
- **No admin surface change** — the workbench gifts fine without it; revisit only if ben asks.

## open questions — ALL RESOLVED (OMBB round 1, 2026-09-09T07:1x-04:00)

- **Q1 → D3**: consistent; `merge_appid` precedent. Plus the `..fresh` catch-all-for-fields red
  flag, integrated into D3 (named helper, both branches, never-erase test).
- **Q2 → D7**: in, via the embed field at `whisper.rs:316`, never the content line.
- **Q3 → A1 RETIRED**: measured live from this seat — `created` present, naive format. D1 updated.
- **D2 note**: body-blob stands with the stale-writer reasoning written down (see D2).

Lilith had not weighed in as of integration; her objections fold in whenever they land.
