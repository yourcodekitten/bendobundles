# the scrapbook 📖 — spec

*2026-09-14, kitten. Status: DRAFT v1 → family crossfire. Where narrative and **decisions**
disagree, the decisions win.*

## why this exists

The gift-shelf spec said it out loud and then deferred it: *"'chosen and waiting' is BEN's
feeling, which belongs to the deferred ben-facing scrapbook."* And one paragraph earlier:
*"fifteen years of generosity has no memory of itself."* The shelf fixed that for **friends** —
every friend now has a page where their gifts live. Ben has nothing. The giver's side of fifteen
years of generosity is four workbench tabs and a dynamo table.

PRODUCT.md allows exactly one warm admin surface: *"it shares the family warmth but never
upstages the friend surface."* The scrapbook is that warmth spent on the giver — the story of
his giving, told back to him: who he chose things for, what he wrote on the tags, what came
back, how long each treasure waited in the attic before someone opened it.

This is composition, not construction. Every raw material already exists and is measured:
`Link.gift_note` / `thank_note` / `thanked_at` / `friend_id` / `curated_notes` (gift tags ✍️),
`Claim.game_id` / `created_at` / `state`, `Friend.name`, `Game.artwork_url` / `acquired_at`
(postmark 📮). Nothing composes them *for the giver*. That's the whole feature.

## what it is

**A new admin page at `/admin/scrapbook`** (fifth nav tab, behind the existing login), read-only,
in the brand voice:

- **The story, grouped by year of unwrap** (claim `created_at`), **oldest year first** — a
  scrapbook reads forward; the story of fifteen years lands harder than a reverse-chron feed.
  Entry counts are dozens, not thousands (same scale argument as the shelf); no pagination.
- **Each opened gift is a keepsake card**: cover art (`artwork_url`, thin fallback when absent),
  title, who it went to (`Friend.name` when the link carries `friend_id`, else the link `label`),
  ben's words — the link's `gift_note` (the card on the present) and the per-game tag
  `curated_notes[game_id]` (the sticker on the item) when either exists — the friend's
  `thank_note` + `thanked_at` when it came back, and the **postmark span**: "bought 2014 ·
  opened 2023 — waited 9 years" (reusing `web/src/postmark.ts`, not re-deriving).
- **"chosen and waiting"** — the deferred feeling, finally on the page it belongs to: games
  curated on **live** links (not revoked, not expired, claims remaining) that nobody has claimed
  yet, grouped by link with the friend/label named. On the friend surface this would be pressure
  (shop grammar); on ben's own page it is anticipation, and the shelf review already ruled it his.
- **One prose summary line**, not stat tiles (PRODUCT.md anti-reference: no metric-card grids):
  "N gifts opened by M friends across Y years · K thank-yous ♡" — a sentence in lowercase, the
  numbers computed from the same payload the page already has.

### non-goals (decided, not omitted)

- **No friend-surface exposure.** Admin-auth only; nothing here changes what any friend can see.
- **No self-claims.** `LINK#SELF` claims are ben gifting himself; they have their own surface
  (`/admin/api/claims/self`) and their presence here would dilute the giving story. Excluded at
  the composition layer, stated in a test.
- **No edits from the scrapbook.** The workbench tabs own every write; this page owns none. A
  keepsake card may *link* to the owning links-tab row, nothing more.
- **No new write path at all.** Read-only composition; zero WCU by construction.
- **No pagination.** Bounded by the same argument as the shelf ("dozens, not thousands"), stated
  so the future knows it was considered.
- **No whisper/bell integration.** The scrapbook never pages anyone; visiting it rings nothing.

## architecture

**Store (`crates/dynamo`)** — one new method:
- `list_claims()` — paginated Scan, `FilterExpression = begins_with(sk, "CLAIM#")`, the exact
  completeness pattern `list_links` / `list_all_games` already use (claims are scattered under
  `LINK#<token>` partitions; no GSI exists that covers all claims, and pending-only gsi2 cannot —
  fulfilled claims leave it). Returns every claim with its parent link token recoverable from
  `pk` (`LINK#<token>`), so the composition can join claims → links without N+1 per-link queries.

**admin-api** — one new route:
- `GET /admin/api/scrapbook` → server-side composition: `list_claims()` + `list_links()` +
  `list_friends()` + `batch_get_games(claimed ∪ waiting ids)`. Response:
  ```json
  {
    "entries": [ { "claimed_at", "state", "game": {"id","title","artwork_url","acquired_at"},
                   "recipient", "gift_note", "tag", "thank_note", "thanked_at",
                   "link_token", "link_label" } ],
    "waiting": [ { "link_token", "link_label", "recipient",
                   "games": [ {"id","title","artwork_url","acquired_at"} ] } ]
  }
  ```
  `recipient` is resolved server-side (friend name > link label). The summary sentence is
  computed client-side from the payload — no server-side stat fields to drift.
- **Claim states**: `Fulfilled` entries are "opened". `Pending` is transient (fulfillment in
  flight) and renders as an opened card with a soft "unwrapping…" badge rather than vanishing —
  a gift mid-open is still a gift. `Compensated` and `Failed` are **excluded from entries** (the
  slot came back / the key was dead; the game returns to waiting naturally if still curated on a
  live link). Exclusion decided, not defaulted — a failed unwrap is machinery, and machinery
  stays invisible (PRODUCT.md principle 4).

**web** — `web/src/admin/Scrapbook.tsx` + the nav link + `api.ts` fetcher. Reuses `postmark.ts`
for the span line and the existing thin-fallback art pattern. Reduced-motion: the page is
static; no ceremony animation is planned, so nothing to gate.

**infra** — none. Same lambda, same table, same auth, one new route + one new page in the
existing SPA bundle. Deploy is the standing CI-zips runbook.

### cost note

One admin page-load = 2 Scans (links, claims) + 1 Query (friends) + 1 BatchGet (games).
Catalog-scale is single-digit MB (measured claim in `list_all_games`' own doc comment); at
admin-visit frequency (Ben, occasionally) this is noise. No caching layer — correctness over
cleverness at this traffic.

## open questions (for family crossfire)

1. **Order within a year**: chronological by `claimed_at` (proposed) — any case for grouping by
   friend inside a year instead?
2. **`Pending` rendering**: "unwrapping…" badge on the card (proposed) vs excluding until
   fulfilled. The badge keeps the page truthful mid-flight; is the transient state worth a UI
   string?
3. **Waiting-section scope**: curated games only (proposed), or should an uncurated live link
   (whole-catalog invite) appear as "an open invitation, N claims left"? Lean yes-as-one-line —
   it IS chosen-and-waiting, just coarser.
4. **Anything the composition should refuse to show**: revoked links' historical claims stay
   (the gift happened; revocation is about the future) — confirm.
