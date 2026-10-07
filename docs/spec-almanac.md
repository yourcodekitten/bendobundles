# the almanac 📜 — spec

*2026-10-07, kitten. Status: v1 draft — for family review (OMBB + Lilith). Where narrative and
**decisions** disagree, the decisions win.*

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
   Where both sources exist (178 games), they **agree on 174 and disagree on 4**. The 4 are Choice
   months whose keys were redeemed the next month. So the two sources answer **different
   questions**: the postmark is *when the order was created*, the name is *which month the bundle
   belongs to*.

And one headline only the almanac can say: **589 Choice picks across 75 months were paid for and
never spent.** Every one is a gift nobody has opened yet.

## what it is

A new admin tab, **almanac** (`/admin/almanac`), between scrapbook and ops. It's one long page
that reads like a year-by-year almanac:

```
📜 the almanac
   fifteen years of the attic, month by month.
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
   the attic doesn't know when these arrived. (1 bundle · 1 treasure)
```

- **Years are dividers, months are rows, bundles are entries.** Newest year first: Ben opens it to
  remember *recent* forgetting, then scrolls back through time. (Q2.)
- Each bundle entry: provenance glyph + name, a count line in the attic voice, and an art strip of
  its games (≤ 8 thumbs, the rest as `+N`). The art comes from the same sources the catalog already
  renders (`artwork_url`, else steam header by `steam_app_id`), so CSP is unchanged
  (`*.steamstatic.com`, `hb.imgix.net` are already allowed).
- **Waiting is lit.** An entry with waiting treasures gets the warm accent. An entry with none
  reads quieter: *"all given or kept ♡"*. Waiting = `Game::is_listable` exactly (`available &&
  giftable && !hidden`), mirrored in the web. No new definition of "waiting" (the lantern's #252
  already shows what two definitions of one word cost).
- **"wrap these →"** hands the bundle's waiting treasures to Links through the **existing**
  `location.state.picked` contract the catalog uses (`Catalog.tsx:298`, consumed at
  `Links.tsx:114`), `requiresChoice` included. Size is bounded by the bundle (measured max 26), well
  under `CURATED_GAMES_MAX = 100`.
- Clicking a thumb opens the existing `GameDetailModal` (the postmark chip is already in it).

## decisions

**D1 — date the BUNDLE, not the game.** A bundle appears in exactly one month. Its date comes from
the first source that answers, in this order:
1. **name month**: the bundle name matches `^(Month) (YYYY)( Humble Choice)?$`. The name is the
   bundle's identity, and it beats the postmark (the 4 late redemptions above file under their
   Choice month, not the month after). Glyph 🗓️, copy *"the october 2021 choice"*.
2. **postmark**: the earliest `acquired_at` among the bundle's games. Glyph 📮.
3. **undated**: the honest shelf at the bottom. **Never invent a date.** Absence renders as
   *"the attic doesn't know when these arrived"*, never as a guessed year.
The provenance travels with the entry and the glyph says which source answered, so a name-month is
never presented as a purchase instant.

**D2 — buckets are UTC months**, the same as `postmark()` (`web/src/postmark.ts`, UTC by design).
A game whose modal chip says *"mar 2013"* sits under March 2013 in the almanac. A local-time
bucket would put a 23:30-EST order in the next month, and the two surfaces would disagree about
the same game.

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

**D5 — the headline is a sentence, and it is conditional.** *"N choice picks still waiting,
across M months — never spent."* N = waiting games whose bundle dated by name-month **and**
`requires_choice`. M = distinct such months. N = 0 ⇒ the line is omitted (no *"0 picks waiting"*).
No metric cards anywhere (PRODUCT.md anti-reference: SaaS dashboard chrome).

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

## open questions (for the family)

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
