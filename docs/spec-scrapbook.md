# the scrapbook 📖 — spec

*2026-09-14, kitten. Status: v2 — both crossfire passes integrated (OMBB: 3 blockers, 4 majors,
3 unasked; Lilith: 1 blocker, 4 contracts, 3 unasked; Q1–Q4 resolved, Q3 by their synthesis).
Where narrative and **decisions** disagree, the decisions win.*

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

- **The story, grouped by year of unwrap** (`claimed_at`), **oldest year first** — a scrapbook
  reads forward. Within a year: **ascending chronological, tiebroken `(claimed_at, game_id)`**
  (Lilith: a bulk unwrap lands several claims in one second; without a secondary sort the page
  reshuffles between loads). Entry counts are dozens, not thousands (same scale argument as the
  shelf); no pagination. **A year jump-list rides the top of the page** (Lilith #6: oldest-first
  is right for a story and wrong for a habit — the jump-list keeps both, and the story axis is
  the one guaranteed to grow).
- **Each opened gift is a keepsake card**: cover art (`artwork_url`, thin fallback when absent),
  title, who it went to (`Friend.name` when the link carries `friend_id`, else the link `label`),
  ben's words — the link's `gift_note` (the card on the present) and the per-game tag
  `curated_notes[game_id]` (the sticker on the item) when either exists — the friend's
  `thank_note` + `thanked_at` when it came back, and the **postmark span**:
  **"bought aug 2014 · opened sep 2023 — waited 9 years"** — month + year, exactly what
  `postmark()` produces (Lilith: the v1 example "bought 2014" was a string the reused function
  cannot emit; adopting the function's own format is both honest and warmer).
  The card deep-links to its owning links-tab row, which already shows revoked state — a warm
  memory must not click into an unexplained dead link (OMBB Q4 rider).
- **Thank-you rendering** (Lilith #5): by domain contract `thanked_at` is `Some` iff
  `thank_note` is `Some` (`set_link_thanks` writes both in one update — domain doc). The card
  renders them as one unit. Defensively: if a corrupt record ever carries only one, render what
  exists and invent nothing.
- **"chosen and waiting"** — the deferred feeling, finally on the page it belongs to:
  **curated ∧ `is_listable()` ∧ no `Fulfilled`/`Pending` claim ∧ live link**, the listability
  test applied **server-side** during composition (OMBB B1+B2 — this is a listability test, NOT
  a claim-absence test, and an implementer must not build the latter):
  - `Failed` games retire forever (`is_listable` excludes `Expired`) — without this filter a
    dead-key game reads as "chosen and waiting" *permanently*, a promise the fulfillment path
    has already decided can never be kept.
  - `hidden` and non-`giftable` games are things ben deliberately took off the shelf — the
    filter keeps them from being called *chosen*.
  - A curated-but-never-claimed game on a **revoked** link deliberately leaves this section —
    stated so a future reader doesn't file the vanish as a bug (OMBB Q4 rider).
- **"doors left open"** — its own small heading, visually separate from "chosen and waiting"
  (Q3, settled between both reviewers): uncurated live links are *"ben left the door open,"* not
  *"ben picked THIS for YOU,"* and the word **chosen** stays off them. One line per link,
  **recipient named** ("the door ben left open for sam · 2 claims left") — never a count in the
  summary sentence, because *a count erases the person* and naming who is the page's thesis
  (Lilith, conceded by OMBB).
- **One prose summary line**, not stat tiles (PRODUCT.md anti-reference: no metric-card grids):
  "N gifts opened by M people across Y years · K thank-yous ♡" — **"people," not "friends"**:
  recipients fall back to link labels when `friend_id` is absent, so the honest count is
  distinct recipients (OMBB, unasked #1). Computed **client-side from the same payload the
  cards render from — a correctness property, not a convenience: the numbers cannot disagree
  with the cards because they share a derivation.** Stated so nobody "optimises" it server-side
  into a second source of truth (Lilith #7).

### claim states on the page

- **`Fulfilled`** — a keepsake card. "Opened."
- **`Pending`** — **the "transient" premise was checked and is FALSE** (Lilith Q2: answer the
  empirical question first). Measured in prod 2026-09-14: the oldest Pending claim is
  **70 days old** (`2026-07-06`, a `LINK#SELF` self-claim — excluded from this page, but proof
  the mechanism has no reaper). So: a Pending claim **≤ 48h old** renders as a keepsake card
  with a soft "unwrapping…" badge — mid-flight is a gift, a dead key is not (the
  principle-4 inconsistency accepted out loud, per OMBB Q2). A Pending **older than 48h** is a
  fulfillment defect owned by the ops surface: the scrapbook renders it **neither** as a card
  (it is not live) **nor** as waiting (its game sits in `pending` status, `is_listable` false) —
  the page must not wallpaper over a stuck claim with a live-looking badge. 48h is provisional
  and layout-invariant.
- **`Compensated` / `Failed`** — excluded from entries. A `Compensated` game re-lists
  (`pending → available`) and so returns to "chosen and waiting" naturally; a `Failed` game
  retires as `Expired` and **does not** (OMBB B1 — the v1 sentence gave these two states one
  truth value, and they have two). Exclusion decided, not defaulted: a failed unwrap is
  machinery, and machinery stays invisible (PRODUCT.md principle 4).

### non-goals (decided, not omitted)

- **No friend-surface exposure.** Admin-auth only; nothing here changes what any friend can see.
- **No self-claims.** `LINK#SELF` claims are ben gifting himself; they have their own surface
  (`/admin/api/claims/self`). Excluded structurally at the join (see architecture), not merely
  unrendered.
- **No edits from the scrapbook.** The workbench tabs own every write; this page owns none.
- **No new write path at all.** Read-only composition; zero WCU by construction.
- **No pagination.** Bounded by the same argument as the shelf ("dozens, not thousands"), stated
  so the future knows it was considered.
- **No whisper/bell integration.** The scrapbook never pages anyone; visiting it rings nothing.

## architecture

**Store (`crates/dynamo`)** — one new method:
- `list_claims()` — paginated Scan, `FilterExpression = begins_with(pk, "LINK#") AND
  begins_with(sk, "CLAIM#")` (both halves constrained — OMBB M1: nothing in the schema forbids
  a future non-`LINK#` partition growing a `CLAIM#` sort key). This is the **same completeness
  loop** as `list_links` / `list_all_games` (`last_evaluated_key` until exhausted) with a
  **deliberately wider filter — it spans partitions on purpose, which is exactly why
  `LINK#SELF` arrives in the result set and must be excluded downstream** (Lilith's wording; the
  v1 "exact pattern" claim was doing reassurance work the code does not support). A Scan is the
  only complete read: `gsi2pk = "PENDINGCLAIM"` is written only while pending and consumed on
  transition, so no GSI can enumerate fulfilled claims (verified by both reviewers).

**Storage-doctrine contract (stated, not implied — OMBB's crossfire find):** the scrapbook
introduces **zero new body-field dependencies**. Links are read ONLY through
`link_from_item`-backed methods (`list_links`), so every editable/authoritative field the page
shows (`gift_note`, `thank_note`, `curated_notes`, enforcer fields) comes from its top-level
attribute, never out of the `body` blob — a later `SET body = :b` writer cannot silently erase
what the page depends on. Claims are read through the same `parse_body` path every existing
claim reader uses (`get_claim` / `claims_for_link`), which is correct *for claims* because claim
transitions are whole-item `PutItem` rewrites (`claim_item(&claim)`, measured at
`fulfill_claim`) — claim `body` IS the authoritative record; there is no top-level twin to
diverge from. `list_claims()` reuses that parse function verbatim.

**The claims→links join (OMBB B3 + Lilith's invariant declaration):** the composition joins
*claims whose parent link META is present*. `LINK#SELF` claims are dropped **by pk, before the
join** (`pk == "LINK#SELF"` — no META item ever exists for that partition, so "drop before
join" and "structural exclusion" are the same act). Any **other** claim with no matching link
META is an orphan: **counted in the payload (`orphan_claim_count`) and logged server-side**,
never silently skipped — an orphan that is not SELF is a data fact, and this is the only
surface that would ever see it (the page renders a quiet footnote only when the count is
nonzero). **Declared invariant:** the join assumes every non-`SELF` claim has a live parent
LINK META — true today because links are never deleted (`lib.rs:1064` documents that invariant
and its revisit-list); **a link-deletion feature must revisit this join** and is hereby the
second member of that list.

**admin-api** — one new route:
- `GET /admin/api/scrapbook` → server-side composition: `list_claims()` + `list_links()` +
  `list_friends()` + `batch_get_games(entry ∪ waiting ids)`. Response:
  ```json
  {
    "entries": [ { "claimed_at", "state", "game": {"id","title","artwork_url","acquired_at"},
                   "recipient", "gift_note", "tag", "thank_note", "thanked_at",
                   "link_token", "link_label" } ],
    "waiting": [ { "link_token", "link_label", "recipient",
                   "games": [ {"id","title","artwork_url","acquired_at"} ] } ],
    "doors_open": [ { "link_token", "link_label", "recipient", "claims_left", "created_at" } ],
    "orphan_claim_count": 0,
    "stale_pending_count": 0
  }
  ```
  **`stale_pending_count`** (Lilith's vanish-twin catch): a stale-Pending game vanishes from the
  page entirely — not a card, not waiting — and unlike the revoked-link vanish (Ben's own act,
  documented above) a stuck claim is a defect he never chose. The count rides the same quiet
  footnote pattern as `orphan_claim_count` (rendered only when nonzero), so the page cannot lie
  by omission about gifts it is deliberately not showing. The systemic gap itself is filed as
  bendobundles#234, not here.
  The payload stays thin **because every filter is server-side**: `is_listable()` is applied
  during composition (OMBB B2 — the client never receives `status`/`giftable`/`hidden` and so
  can never mis-apply them), stale-Pending is dropped during composition, `recipient` is
  resolved server-side (friend name > link label).
- **`claimed_at` is the claim's `created_at`, renamed once at the API boundary and used
  everywhere** — grouping, sorting, the postmark span (OMBB M3). For a `Pending` card it is
  when the gift was picked up, which is what "opened" means mid-flight.

**web** — `web/src/admin/Scrapbook.tsx` + the nav link + `api.ts` fetcher.

**The postmark span's two call sites (Lilith's BLOCKER — spec'd exactly because the reuse is
where the bug lives):** `waitedYears(iso, now = Date.now())` measures **to now** by default.
The default is correct for exactly one of this page's two calls:
- **keepsake card**: `waitedYears(game.acquired_at, Date.parse(entry.claimed_at))` — the span
  **ended at the unwrap**. Called with the default it renders "waited 12 years" today and 13
  next January: well-formatted, plausible, drifting.
- **waiting section**: `waitedYears(game.acquired_at)` — still waiting, the span genuinely runs
  to now; the default is right here.
- **doors section**: `waitedYears(link.created_at)` — a **different field** (an uncurated link
  has no chosen game by definition, so there is no `acquired_at` to span; Lilith's v2 catch —
  the first draft of this list claimed a call site the payload could not feed). The door line
  may say "open 2 years"; under a year `waitedYears` returns null and the clause is omitted,
  same rule as the cards. Default-now is correct here too: the door is still open.
A **frozen-clock test pins the divergence** (both call shapes, one fixture, different expected
years). And `postmark.ts:39`'s doc comment "`now` is injectable for tests" gets amended in the
same edit — the keepsake card is a *production* caller that must inject, and a comment labeling
the parameter as test scaffolding points the next reader away from the bug (OMBB's rider).
**Fallbacks (OMBB M4):** `postmark()`/`waitedYears()` return null on missing/junk
`acquired_at` → the span line is simply omitted (the card still shows "opened <postmark of
claimed_at>"). If `acquired_at` postdates `claimed_at` (bad data), the "waited" clause is
omitted — never a negative year. Guard at the call site in `Scrapbook.tsx`, not inside
`waitedYears` (whose calendar semantics are shared and correct).

**infra** — none. Same lambda, same table, same auth, one new route + one new page in the
existing SPA bundle. Deploy is the standing CI-zips runbook.

### cost note

One admin page-load = **two full-table Scans** (links, claims) + 1 Query (friends) + 1 BatchGet
(games). A Scan is billed on every item **read, before the `FilterExpression` applies** — the
filter bounds the wire, not the RCU (OMBB M2, Lilith). The honest denominator is the **table**,
not the catalog: measured 2026-09-14 via `DescribeTable`, the prod table is **4.31 MB / 2,043
items across its eleven partition types** — so a full Scan is ~½ MB of RCU-eligible read at
eventual consistency, twice per page-load, at Ben-occasionally frequency. Genuinely noise, now
at table scale rather than catalog scale. Watch-item, not blocker: `WHISPER#` is an append-only
log with no TTL or prune path (Lilith) — if the table's shape ever changes character, this note's
number is dated and re-derivable, not a law.

### tests owed by this spec (beyond per-task TDD)

- **Revoked-link two-half fixture** (Lilith Q4): one revoked link carrying one `Fulfilled` claim
  and one curated-unclaimed game → the claim **appears in `entries`**, the game **does not
  appear in `waiting`**. Two assertions, one fixture — so a future filter change cannot silently
  take the gift with it.
- **Frozen-clock postmark divergence** (Lilith blocker): entry-shaped call vs waiting-shaped
  call on the same `acquired_at`, distinct expected years.
- **Waiting-is-listability** (OMBB B1/B2): a `Failed`-retired game, a `hidden` game, a
  non-`giftable` game, **and a stale-Pending game** each curated on a live link → none in
  `waiting`. The stale-Pending arm is there because that Q2 behaviour is *derived from* this
  filter (the game sits in `pending` status), and an untested derivation goes invisible the day
  someone edits the filter (OMBB round 2, ①).
- **48h boundary, frozen clock** (OMBB round 2 — a time-relative rule reuses the frozen clock or
  it reintroduces the class it fixed): the composition takes `now: OffsetDateTime` as a
  parameter (the handler passes `now_utc()`); fixtures sit ON the boundary — one Pending aged
  47h renders as a badged card, one aged 49h is dropped **and increments
  `stale_pending_count`**. A fixture aged 3h asserts nothing about a 48h rule.
- **SELF-drop + orphan count** (OMBB B3): a `LINK#SELF` claim → absent everywhere,
  `orphan_claim_count` 0; a synthetic non-SELF orphan → counted.

## crossfire record (Q1–Q4 as resolved)

1. **Order within a year** → ascending chronological, tiebreak `(claimed_at, game_id)`.
2. **Pending** → badge ≤ 48h, dropped after (premise "transient" measured false: 70-day Pending
   exists in prod; the stuck specimen is SELF, but the mechanism has no reaper).
3. **Uncurated live links** → own "doors left open" heading, recipient named, one line each;
   never under "chosen", never a scalar in the summary (both reviewers, synthesized).
4. **Revoked links keep historical claims** → confirmed; vanish-from-waiting stated; deep-link
   rider; two-half fixture owed.
