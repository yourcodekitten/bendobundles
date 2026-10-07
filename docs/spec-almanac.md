# the almanac 📜 — spec

*2026-10-07, kitten. Status: v2. Family review integrated: Lilith Q1–Q3 + the 4-row mechanism, OMBB
Q4 (signed) + the headline-count and derived-years points. OMBB spec gate passed 07:09. Where
narrative and **decisions** disagree, the decisions win.*

## why this exists

PRODUCT.md: *"it exists because ben forgets his bundles exist 95% of the time."* The admin answers
that with a **catalog**: a searchable list of 1134 titles. Search works only when you remember the
title. Ben doesn't remember titles. He remembers **eras**: *the 2014 indie bundles, the pandemic
Choice months, that summer he bought everything.* No surface in the attic is organized by when.

The postmark 📮 gave every order-backed game its year and deliberately stopped there (*"no
timeline/era view, no attic-museum wing … YAGNI"*, spec-postmark non-goals). The almanac reopens
that non-goal on purpose. A free-range pounce is the place to argue it, and the measurement below
is the argument.

## what the data actually is (measured 2026-10-07T07:1x-04:00, prod, read-only scan)

| | games |
|---|---|
| total `GAME#` items (scan complete, no `LastEvaluatedKey`) | **1134** |
| carry `acquired_at` (postmark) | 522 |
| **no `acquired_at`** | **612 (54%)** |
| ↳ Humble Choice picks (`requires_choice = true`), bundle named `Month YYYY` | **590** — 75 distinct months, **589 `available`** |
| ↳ `Month YYYY Humble Choice` order keys, `ben_redeemed` | 21 |
| ↳ other | 1 |

Two facts fall out of that table, and they shape every decision below:

1. **The postmark's backfill promise was true only for order-backed games.** *"run_sync walks EVERY
   gamekey each pass, so the next scheduled sync IS the backfill"* holds for `order.keys` games.
   Choice picks enter through the discovery ingest, which has no order instant. Four weeks after
   postmark, more than half the library is still undated, and **85% of the treasures still waiting
   to be given are among the undated** (589 of 688 `is_listable`). A timeline built on `acquired_at` alone would leave out
   most of what it exists to surface.
2. **For most of those, the date is in the name.** All 590 Choice-pick bundle names match
   `^(January|…|December) \d{4}$`, and all 21 order-key stragglers match `… \d{4} Humble Choice$`.
   Where both sources exist (178 games), they **agree on 174 and disagree on 4**. The 4 are
   **2 orders × 2 games**, with `acquired_at` `2021-12-01T01:45Z` and `2023-11-01T01:34Z`, which is
   the **evening of Nov 30 / Oct 31 in Ben's zone**. A Choice order is created when Ben makes his
   picks, usually late in the month (the day-of-month spread of Choice-key `acquired_at` is mostly
   24–31), and the postmark's **UTC** month pushes an evening pick into the next month. *(v1
   explained this as "keys redeemed a month late". That was a story, never measured, and Lilith
   caught it.)* So the two sources answer **different questions**: the postmark is *when the order
   was created* (UTC-bucketed, with a known boundary bug filed as **#267**), the name is *which
   month the bundle belongs to*.
3. **A name is not a bundle.** `A very special gift just for you` is **9 different orders**
   (distinct gamekeys) across years. Grouping by name would collapse them into one entry dated at
   the oldest (Lilith's catch, measured).

And one headline only the almanac can say: **589 Choice picks across 75 months were paid for and
never spent.** Every one is a gift nobody has opened yet.

## what it is

A new admin tab, **almanac** (`/admin/almanac`), between scrapbook and ops. It's one long page
that reads like a year-by-year almanac:

```
📜 the almanac
   15 years of the attic, month by month.            ← N derived from the dated years, never typed
   589 choice picks still waiting, across 75 months — never spent.      ← one sentence, not a card

── 2026 ──────────────────────────────────────────────
   september  🗓️ the september 2026 choice · 10 picks · all 10 still waiting   [wrap these →]
              [art][art][art][art][art][art] …
   july       🗓️ the july 2026 choice · 10 picks · all 10 still waiting       [wrap these →]
   ...
── 2013 ──────────────────────────────────────────────
   march      📮 humble indie bundle 8 · 7 treasures · all given or kept ♡
   ...
── undated ───────────────────────────────────────────
   the attic doesn't know when these arrived.
   ⚠️ 3 choice picks we couldn't date                ← only when a Choice pick's name didn't parse
```

- **Years are dividers, months are rows, bundles are entries.** Newest year first: Ben opens it to
  remember *recent* forgetting, then scrolls back through time. (Q2.)
- Each entry: provenance glyph + label, a count line in the attic voice, and an art strip of its
  games (≤ 8 thumbs, the rest as `+N`). Thumb source order: `artwork_url` → the steam capsule by
  `steam_app_id` (the exact URL shape `GameGrid.tsx:77` already ships) → the catalog's
  `titleColorClass` block. CSP is unchanged (`*.steamstatic.com` and `hb.imgix.net` are already in
  `img-src`). Thumbs are **static**: `GameDetailModal`'s admin mount carries the whole self-claim
  apparatus (`Catalog.tsx:336-345`), and re-plumbing that into a second page is out of scope (v1 said
  "click opens the modal", and that was cut while planning).
- **Waiting is lit.** An entry with waiting treasures gets the warm accent. An entry with none
  reads quieter: *"all given or kept ♡"*. Waiting = `Game::is_listable` exactly (`available &&
  giftable && !hidden`), mirrored in the web. No new definition of "waiting" (the lantern's #252
  already shows what two definitions of one word cost).
- **"wrap these →"** hands the bundle's waiting treasures to Links through the **existing**
  `location.state.picked` contract the catalog uses (`Catalog.tsx:298`, consumed at
  `Links.tsx:114`), `requiresChoice` included. Size is bounded by the bundle (measured max 26), well
  under `CURATED_GAMES_MAX = 100`.

## decisions

**D1 — date the ENTRY, not the game, and say which source answered.** An entry appears in exactly
one month. Each game resolves in this order:
1. **name month**: the bundle name matches `^(January|…|December) (\d{4})( Humble Choice)?$` **and**
   the year is in `[2010, currentYear + 1]` (Lilith: a typo must never produce a `0000` divider).
   The name is the bundle's identity and beats the postmark (Lilith + OMBB: yes). **Entries group by
   the normalised `(year, month)`**, so `October 2021` (discovery picks) and `October 2021 Humble
   Choice` (order keys) are **one** entry (Lilith). Glyph 🗓️, label `{month} {year}` in lowercase
   (*"october 2021"*). The label does NOT say "choice", so a non-Choice bundle that happens to be
   named after a month is not mislabelled.
2. **postmark**: otherwise, the game's `acquired_at`. **Entries group by ORDER**, meaning the gamekey
   prefix of `id` (`"{gamekey}:{machine_name}"`, a documented-accepted exposure per
   `admin-api/src/lib.rs` CatalogGameView's doc). They never group by name (the 9-orders measurement
   above). The entry's date is its earliest valid `acquired_at`. Glyph 📮, label = the bundle name.
3. **undated**: neither answered. The honest shelf at the bottom, grouped by order like (2).
   **Never invent a date.**
A pre-Choice name that doesn't match the regex (e.g. a Humble Monthly era string) falls through to
the postmark. That's pinned by a test (Lilith Q3), so if that era is ever ingested it lands on its
order date and not on undated.

**D2 — postmark buckets are UTC months**, the same as `postmark()` (`web/src/postmark.ts`). A game
whose chip says *"mar 2013"* sits under March 2013 here. ⚠️ **This deliberately inherits the
chip's known month bug (#267: an end-of-month evening order in Ben's zone shows as the next
month).** Fixing #267 is a **two-surface change**: the chip and the almanac's buckets must flip in
the same PR, or they disagree about one game, which is the thing D2 exists to prevent (OMBB, and
it's written into #267). Name-month entries are immune (no instant involved).

**D3 — one backend field.** `CatalogGameView` gains `acquired_at: Option<OffsetDateTime>`
(rfc3339, `skip_serializing_if = "Option::is_none"`), the same serde shape as the domain field.
The web `AdminGame.acquired_at?` already exists, typed optional; its comment changes from "not
sent" to "sent when known". **Deploy skew is safe in both directions:** an old lambda omits the
field ⇒ the web falls through to name-month / undated (correct, just less precise); a new lambda
with an old SPA ⇒ an extra field nobody reads.

**D4 — grouping is pure and lives in `web/src/almanac.ts`** with a twin test file (web convention:
`postmark.ts`, `tags.ts`). `buildAlmanac(games) → { headline, years: [{ year, months: [{ month,
entries: [{ bundle, source, games, waiting }] }] }], undated }`. Deterministic, no `Date.now()`,
no ICU (`postmark.ts`'s twelve-strings rule). The page component only renders.

**D5 — the headline is a sentence, and it is conditional.** *"N choice picks still waiting, across M
months — never spent."* **N = every game with `requires_choice && is_listable`, WHATEVER its date
source** (OMBB: counting only name-dated picks means a Humble rename silently *shrinks* N with no
error). M = distinct name-months among those N. If some of the N didn't date by name, the undated
shelf names them: *"⚠️ K choice picks we couldn't date"*. A regex miss then becomes a **visible
line**, not a smaller number. N = 0 ⇒ the line is omitted (no *"0 picks waiting"*). The subtitle's
*"N years"* = `newestDatedYear − oldestDatedYear + 1`, derived and never typed (OMBB: typed, it
rots the first January after shipping); no dated years ⇒ the subtitle drops the count. No metric
cards anywhere (PRODUCT.md anti-reference: SaaS dashboard chrome).

**D6 — hidden games count as not waiting but stay visible** in their bundle's art strip, dimmed.
The almanac is a memory of the whole attic, and leaving them out would make bundle counts
disagree with the catalog. The count line names them only when present: *"· 1 tucked away"*.

**D7 — admin-only, friend surfaces untouched.** No public-api change, no new dynamo attribute, no
new lambda, no new IAM, no terraform. The diff touches `admin-api` (one field), `web/src/api.ts`
(a comment), `web/src/almanac.ts` (+test), `web/src/admin/Almanac.tsx` (+test), the route and the
nav link.

## non-goals (decided, not omitted)

- **No backfill of `acquired_at` for Choice picks.** There is no order instant to backfill from, and
  writing the name-month into `acquired_at` would launder a month-of-identity into a
  purchase-instant field that the postmark chip, the claim ceremony ("waited N years") and the
  whisper embed all read as purchase time. D1 keeps the two sources separate on purpose.
- **No bundle entity in dynamo.** A bundle stays a string on the game, and grouping is a client
  computation over one payload the catalog already fetches.
- **No friend-facing almanac.** Browsing the whole attic is shopping grammar (Principle 2). Friends
  get chosen-for-them, never inventory.
- **No "spend my Choice picks" automation.** The almanac shows the unspent picks. Spending one
  stays the existing claim path (choose-at-fulfillment).

## resolved questions (family review, 2026-10-07T07:07–07:10-04:00)

- **Q1 → D1** (both): the name month beats the postmark. Lilith's correction to the mechanism is
  integrated above.
- **Q2 → newest first** (both).
- **Q3 → D1** (Lilith): keep the strict anchor (a miss fails toward the postmark/undated, never a
  wrong month), normalise `(year, month)`, group the postmark path by order, pin the pre-Choice
  fall-through, and bound the year. **+ D5** (OMBB): "fails safe" held for month placement and broke
  for the headline count, so N counts every pick and any unparsed pick is named.
- **Q4 → signed** (OMBB), as a **reconstruction**: the same scan (1134 games + 879 steam caches)
  pushed through the view's projection gives **778,799 B → 801,551 B (+22,752 B, +2.9%)**, which is
  12.7% of Lambda's 6 MB sync ceiling. It's a construction, not a captured response; the live size
  is measured after the deploy.

## (v1) open questions, as asked

- **Q1 (D1):** Is name-month-beats-postmark right? The alternative is "postmark beats name", which
  files the 4 late redemptions under their order month. I think the name wins because a Choice
  month *is* that month, but it's a judgment call.
- **Q2:** Newest year first, or a journey that starts in 2012? I'm leaning newest-first. The
  unspent picks are recent, and the eye lands where the action is.
- **Q3:** Is the name-month regex too narrow? It's measured over today's 1134 names, with no false
  negatives among the Choice population. A bundle literally named `March 2013` that is *not* a
  Choice month would be dated by its name, which is still the month it says. Is there a failure
  mode I can't see?
- **Q4 (OMBB):** Does the extra field on the catalog response raise any payload or lambda concern?
  Measured: only the 522 dated rows carry it (`skip_serializing_if`), at ~44 B each (`,"acquired_at":"2018-02-23T08:50:11.29284Z"`), ≈ 23 KB uncompressed on an already-fat response.
