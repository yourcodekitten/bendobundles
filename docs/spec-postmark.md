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
  — **NAIVE, no offset**, exactly the historical format. Parse: the naive format **assumed UTC**
  (primary, measured), RFC3339 second (defensive). At month/year display granularity a
  worst-case ±day is immaterial; the assumption is recorded here, not hidden.
- `#[serde(default)]` + unparseable ⇒ `None` with a `tracing::warn!` naming the gamekey — a bad
  or absent date NEVER fails an order read. The postmark is garnish; key truth stays the meal.
- Absence-tolerance stays even with A1 retired (one order measured, not fifteen years of them:
  early orders may predate the field or carry junk — any dateless order ships postmark-less and
  nothing else changes). Post-deploy coverage count remains as D-verify: games with `acquired_at`
  vs total, after the first full sync.

**D2 — domain.** `Game.acquired_at: Option<OffsetDateTime>`, `#[serde(default)]`, rfc3339 serde
like siblings. Storage: **body blob** — with the REAL reason written down (OMBB's D2 note,
family review): "immutable identity ⇒ body" answers only the *edit-race* threat. The
*stale-writer* threat is separate — any rolled-back binary whose `Game` lacks the field does
`SET body = :b` from a round-trip and silently drops it (the exact shape `dynamo/src/lib.rs:415`
documents for the claim path). What makes body storage acceptable HERE is that `run_sync` walks
every gamekey each pass, so **the erasure self-heals on the next sync — recoverable-and-quiet,
not immune.** Do not reuse "immutable ⇒ body is fine" on a future field that does not self-heal.
No new dynamo attribute, no GSI, no schema change; no claim path reads it.

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
`merge_acquired_at()` helper, called explicitly in BOTH branches** (the `merge_appid` pattern,
which already overrides `..fresh` in that second branch), plus a test pinning never-erase in the
`..fresh` branch specifically.

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
