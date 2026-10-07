# The Almanac 📜 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A new admin tab `/admin/almanac` that shows Ben's whole attic month by month, 2012 → now. Each entry carries an honest date source, waiting treasures are lit, and "wrap these →" hands an entry's waiting games to Links.

**Architecture:** One optional field (`acquired_at`) is added to the admin catalog response. All grouping is a pure, deterministic function in `web/src/almanac.ts` over the payload the catalog already fetches. The page component only loads and renders. The month bucketing for postmark-dated games is **the same function the postmark chip uses** (`postmarkMonth`, extracted in `postmark.ts`), so the two can never disagree, even after #267 is fixed.

**Tech Stack:** Rust (axum, serde, `time` 0.3 with the `serde`/`macros` features), React 19 + react-router, Tailwind v4 theme tokens, vitest + Testing Library.

> ⚠️ **ERRATA (post-execution, review 2): this plan is the AS-PLANNED record, not the as-built one.** It still names
> `capsule_616x353` (built: `capsule_231x87`), entry `<h3>` (built: h4 under a month, h3 on the undated shelf), a single
> `thumbSrc` (built: a `thumbSrcs` ladder), and a test asserting the undated shelf renders for an unparsed dated pick (built: the
> opposite; the ⚠️ line lives in the header). Also built, not planned: the same-label-within-a-month merge (spec D1 ②a). Where they
> differ, **the code and `docs/spec-almanac.md` are the truth.**

**Spec:** `docs/spec-almanac.md` (v2, `d362c58`). **Plan v2:** a cold plan review (fresh subagent, real repo, every web test block run in a scratch copy: T2 12/12, T3 17/17, T4 7/7 + 14/14 green) found 3 blockers and 4 majors. All are integrated below and marked *(review B1…)*. Read it before any task; where this plan and the spec disagree, the spec's **decisions** win.

## Global Constraints

- Voice: lowercase, warm, `♡` is canon. No metric cards, no dashboard chrome (PRODUCT.md anti-references).
- "Waiting" = `status === 'available' && giftable && !hidden`, exactly `Game::is_listable` (`crates/domain/src/lib.rs:383`). Do not invent a second definition.
- Name-month regex: `^(January|February|March|April|May|June|July|August|September|October|November|December) (\d{4})( Humble Choice)?$`, year bounded to `[2010, currentYear + 1]`.
- Postmark month = UTC year/month of `acquired_at` via `postmarkMonth` in `web/src/postmark.ts`. **Never** a second implementation of UTC bucketing.
- Postmark/undated entries group by **order** = the part of `id` before the first `:`. Name-month entries group by `(year, month)`.
- Handoff to Links uses the existing router-state contract: `navigate('/admin/links', { state: { picked } })` with `picked: { id: string; title: string; requiresChoice?: boolean }[]` (`Catalog.tsx:298`, consumed at `Links.tsx:114-122`).
- Thumb URL for a steam app: `https://shared.akamai.steamstatic.com/store_item_assets/steam/apps/${id}/capsule_616x353.jpg` (already shipped in `GameGrid.tsx:77`, already allowed by CSP `img-src *.steamstatic.com`).
- No `Date.now()` or locale calls inside `almanac.ts`; `currentYear` is a parameter. No `toLocaleString`/`Intl` (postmark.ts's no-ICU rule).
- Tests never depend on the real clock.
- CI's exact web chain, run from `web/`: `npm run lint && npm run typecheck && npm test -- --run && npm run build`. Rust: `cargo fmt --check`, `cargo clippy --workspace --all-targets --all-features -- -D warnings`, `cargo test --workspace`. **NO LOCAL CARGO, by anyone (OMBB's step-5 condition, `1557353989245239363`):** a `cargo test` link on this box (concurrent `ld`) parked three tenants on 09-30, and `nice` does not bound memory. The admin-api integration tests also **panic** locally without `DYNAMODB_LOCAL_URL`; they don't skip. ⇒ **every Rust red and green is read from the CI log on the draft PR.**
- Commits are GPG-signed (`git commit -S`) as `code kitten <yourcodekitten@gmail.com>`.
- **The Button Burgundy Rule** (`DESIGN.md:153`): burgundy (`*-give*`) only where giving/claiming happens, never as ambient decoration, never above 10% of a screen. ⇒ the almanac uses **no** `give` tokens. Its "wrap these →" uses `bg-control`, exactly like the catalog's own "wrap these into a link" (`Catalog.tsx:296-303`). *(review minor)*
- **Shared box:** 2 cores, seven tenants. Run local test/lint/build commands under `nice -n 15`, one at a time, never in parallel. CI is the authority.
- CI runs on **`pull_request` only** for branches (`.github/workflows/ci.yml`). Nothing "in CI" is observable until the draft PR exists (Task 1 Step 5 opens it). *(review M2)*

## File Structure

| File | Responsibility |
|---|---|
| `crates/admin-api/src/lib.rs` (modify `CatalogGameView` ~:325, `handle_catalog` ~:416) | emit `acquired_at` when known |
| `crates/admin-api/tests/api_test.rs` (append) | test: present when known, absent key when unknown |
| `web/src/api.ts` (modify the `AdminGame.acquired_at` doc comment ~:107) | doc only: "sent when known" |
| `web/src/postmark.ts` + `postmark.test.ts` | extract `postmarkMonth`; `postmark()` delegates to it |
| `web/src/almanac.ts` + `almanac.test.ts` (create) | pure grouping, copy helpers, thumb source |
| `web/src/admin/Almanac.tsx` + `Almanac.test.tsx` (create) | load + render + wrap handoff |
| `web/src/App.tsx`, `web/src/admin/AdminApp.tsx`, `web/src/admin/AdminApp.test.tsx` | route + nav link |

---

### Task 1: admin catalog sends `acquired_at` (one constructor, both sites)

**Files:**
- Modify: `crates/admin-api/src/lib.rs`: `CatalogGameView` (~:325), the literal in `handle_catalog` (~:417), **and the second literal in `handle_game_detail` (~:543)** *(review B1: a field added to only one site is E0063)*
- Modify: `web/src/api.ts`: the `AdminGame.acquired_at` doc comment (~:107)
- Test: a new `#[cfg(test)] mod catalog_view_tests` at the end of `crates/admin-api/src/lib.rs`, after `mod friend_name_sanitize_tests` (~:1670): **local, dynamo-free**
- Test: `crates/admin-api/tests/api_test.rs`, appended after `catalog_exposes_requires_choice`: integration test, **CI only**
- Test: `web/src/GameDetailModal.test.tsx`: one admin-mount test (the declared side effect, *review M1*)

**Interfaces:**
- Produces: `fn catalog_view(g: domain::Game, steam: Option<SteamSummaryView>) -> CatalogGameView`, private to `lib.rs` and used by **both** handlers. `GET /admin/api/catalog` **and** `GET /admin/api/games/{id}` rows carry `"acquired_at": "<rfc3339>"` when known and **no key** when unknown. The web type `AdminGame.acquired_at?: string` is unchanged.
- ⚠️ **Declared side effect (review M1, deliberate):** the admin catalog modal (`GameDetailModal`, admin mount) already renders `postmark(game.acquired_at)` at `GameDetailModal.tsx:349` and `:511`, and has rendered nothing there only because the admin payload never sent the field. Shipping this lights the 📮 chip and the "tucked into the attic" line in the **admin** modal. That reverses spec-postmark's "no admin surface change" non-goal, on purpose: it's the same fact, on the giver's own workbench, and the almanac is the reason to send it.

- [ ] **Step 1: Write the failing LOCAL test.** Extract the constructor first, so the test has something to call. In `lib.rs`, add directly after `fn steam_summary(…)`:

```rust
/// One projection for every admin game row (catalog list AND detail) — so a field added
/// here reaches both endpoints, and a third call site cannot drift (almanac review B1).
fn catalog_view(g: domain::Game, steam: Option<SteamSummaryView>) -> CatalogGameView {
    CatalogGameView {
        steam,
        id: g.id,
        title: g.title,
        bundle: g.bundle,
        key_type: g.key_type,
        giftable: g.giftable,
        hidden: g.hidden,
        status: g.status,
        claim_id: g.claim_id,
        artwork_url: g.artwork_url,
        requires_choice: g.requires_choice,
        steam_app_id: g.steam_app_id,
        owned_by_ben: g.owned_by_ben,
        hidden_source: g.hidden_source,
    }
}
```

Replace the struct literal in `handle_catalog` with:

```rust
                .map(|g| {
                    let steam = g
                        .steam_app_id
                        .and_then(|id| caches.get(&id))
                        .and_then(steam_summary);
                    catalog_view(g, steam)
                })
```

and the one in `handle_game_detail` with:

```rust
    let game_view = catalog_view(game, cache.as_ref().and_then(steam_summary));
```

(If `cache` or `game` is used after that line in `handle_game_detail`, keep the borrow order as it is: `game` is moved here exactly as the old literal moved its fields. CI's build on the draft PR will say so. Do not run cargo locally.)

Then append the test module to the end of `lib.rs`:

```rust
#[cfg(test)]
mod catalog_view_tests {
    use super::*;

    fn game() -> domain::Game {
        domain::Game {
            id: "gk:mn".into(),
            title: "t".into(),
            bundle: "b".into(),
            gamekey: "gk".into(),
            machine_name: "mn".into(),
            key_type: "steam".into(),
            giftable: true,
            hidden: false,
            status: domain::GameStatus::Available,
            claim_id: None,
            artwork_url: None,
            keyindex: 0,
            requires_choice: false,
            steam_app_id: None,
            appid_source: None,
            owned_by_ben: false,
            hidden_source: None,
            acquired_at: None,
        }
    }

    /// 📜 almanac D3: present when known; an ABSENT key (never null) when unknown.
    #[test]
    fn catalog_view_carries_acquired_at_and_omits_it_when_unknown() {
        let mut g = game();
        g.acquired_at = Some(time::macros::datetime!(2013-03-27 18:22:58 UTC));
        let v = serde_json::to_value(catalog_view(g.clone(), None)).unwrap();
        assert_eq!(v["acquired_at"], "2013-03-27T18:22:58Z");
        let v = serde_json::to_value(catalog_view(game(), None)).unwrap();
        assert!(
            v.as_object().unwrap().get("acquired_at").is_none(),
            "unknown must be an ABSENT key, not null: {v}"
        );
    }
}
```

- [ ] **Step 2: Commit the red, open the DRAFT PR, read the red FROM CI** (no local cargo, see Global Constraints).

```bash
git add crates/admin-api/src/lib.rs
git commit -S -m "📜 red: catalog_view constructor + acquired_at test (almanac D3)"
git push
gh pr create -R yourcodekitten/bendobundles --draft --head kitten/almanac --base main \
  --title "📜 the almanac" --body "draft — the almanac (docs/spec-almanac.md). body filled at PR-up."
```

Wait for the Rust job (`~/code-kitten/ops/branch-green.sh yourcodekitten/bendobundles kitten/almanac`, which reads the runs endpoint). Expected: `fmt`/`clippy` pass (the refactor is behaviour-identical), and `catalog_view_tests::catalog_view_carries_acquired_at_and_omits_it_when_unknown` **FAILS on the first `assert_eq!`** (left `Null`) in the test log. A compile error or a clippy failure is **not** this red: fix it, push, and read again.

- [ ] **Step 3: Implement.** In `CatalogGameView`, after `hidden_source`:

```rust
    /// 📮 the postmark (spec-almanac D3): when ben's order was created. Same serde shape as
    /// the domain field — rfc3339, and ABSENT (not null) when unknown, so the almanac reads
    /// absence as "no postmark" (falls through to name-month/undated).
    #[serde(
        with = "time::serde::rfc3339::option",
        skip_serializing_if = "Option::is_none"
    )]
    acquired_at: Option<time::OffsetDateTime>,
```

and in `catalog_view`, after `hidden_source: g.hidden_source,` add `acquired_at: g.acquired_at,`.

In `web/src/api.ts`, replace the `AdminGame.acquired_at` doc comment with:

```ts
  /** 📮 rfc3339 acquisition instant — sent by the admin catalog AND game-detail
   *  endpoints when known (spec-almanac D3); ABSENT when unknown, and absent from an
   *  old lambda during a deploy window. Absent ⇒ exactly the today-state. */
```

Add the CI-only integration test to `crates/admin-api/tests/api_test.rs`, after `catalog_exposes_requires_choice`. It **panics locally** (`test_app_with_call_invoker` `.expect`s `DYNAMODB_LOCAL_URL`); it does NOT skip. Run it only in CI.

```rust
/// 📜 almanac D3 end-to-end through the real route + dynamo round-trip (CI: dynamodb-local).
#[tokio::test]
async fn catalog_carries_acquired_at_when_known_and_omits_it_when_unknown() {
    let (app, store, _) = test_app_with_call_invoker(
        "catalog_carries_acquired_at_when_known_and_omits_it_when_unknown",
        FulfillResponse::RevealedKey {
            key: "unused".into(),
        },
    )
    .await;
    let mut dated = sample_game("gkA:mnA");
    dated.acquired_at = Some(time::macros::datetime!(2013-03-27 18:22:58 UTC));
    store.put_game(&dated).await.unwrap();
    store.put_game(&sample_game("gkU:mnU")).await.unwrap();

    let resp = authed_get(&app, "/admin/api/catalog").await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = body_json(resp).await;
    let rows = body.as_array().unwrap();
    let find = |id: &str| {
        rows.iter()
            .find(|g| g["id"] == id)
            .unwrap_or_else(|| panic!("{id} must be in catalog"))
            .clone()
    };
    assert_eq!(find("gkA:mnA")["acquired_at"], "2013-03-27T18:22:58Z");
    let undated = find("gkU:mnU");
    assert!(
        undated.as_object().unwrap().get("acquired_at").is_none(),
        "unknown must be an ABSENT key, not null: {undated}"
    );
}
```

Add the declared-side-effect test to `web/src/GameDetailModal.test.tsx`. Use **its existing admin-mount render helper and fixture** (find the existing test that renders `mount="admin"` and copy its setup exactly). The assertion:

```tsx
  it('admin mount shows the 📮 postmark once the catalog sends acquired_at (almanac D3 side effect)', async () => {
    // render the admin mount exactly as the neighbouring admin tests do, with the
    // fixture game extended by: acquired_at: '2013-03-27T18:22:58Z'
    expect(await screen.findByText('📮 mar 2013')).toBeInTheDocument();
  });
```

(This one is green on arrival: the modal already handles the field. It pins the side effect so a future refactor can't silently drop it.)

- [ ] **Step 4: Verify the web half locally.** `nice -n 15 npx vitest run src/GameDetailModal.test.tsx` from `web/`. PASS.

- [ ] **Step 5: Commit, push, read GREEN from CI**

```bash
git add crates/admin-api/src/lib.rs crates/admin-api/tests/api_test.rs web/src/api.ts web/src/GameDetailModal.test.tsx
git commit -S -m "📜 admin catalog + detail carry acquired_at when known (almanac D3)"
git push
```

Expected in the CI Rust job log: `catalog_view_carries_acquired_at_and_omits_it_when_unknown ... ok` **and** `catalog_carries_acquired_at_when_known_and_omits_it_when_unknown ... ok`, with fmt and clippy green.

---

### Task 2: `postmarkMonth` — one UTC bucketing for chip and almanac

**Files:**
- Modify: `web/src/postmark.ts`
- Test: `web/src/postmark.test.ts`

**Interfaces:**
- Produces: `export type YearMonth = { year: number; month: number }` (month 0–11) and `export function postmarkMonth(iso: string | undefined): YearMonth | null`. `postmark()` keeps its exact behaviour and is re-implemented over `postmarkMonth`.

- [ ] **Step 1: Write the failing test** (append to `postmark.test.ts`; also add `postmarkMonth` to the import)

```ts
describe("postmarkMonth — the ONE UTC bucketing (spec-almanac D2, #267)", () => {
  it("is the UTC year and 0-based month", () => {
    expect(postmarkMonth("2013-03-27T18:22:58Z")).toEqual({ year: 2013, month: 2 });
  });
  it("puts an evening-of-the-31st order in the NEXT month — #267's known bug, pinned so a fix is visible", () => {
    // 2023-10-31 21:34 EDT, measured from prod
    expect(postmarkMonth("2023-11-01T01:34:23.072355Z")).toEqual({ year: 2023, month: 10 });
  });
  it("is null on absent or junk", () => {
    expect(postmarkMonth(undefined)).toBeNull();
    expect(postmarkMonth("not a date")).toBeNull();
  });
  it("agrees with postmark() by construction", () => {
    const iso = "2012-08-15T19:41:25.76507Z";
    const ym = postmarkMonth(iso);
    expect(ym).toEqual({ year: 2012, month: 7 });
    expect(postmark(iso)).toBe("aug 2012");
  });
});
```

- [ ] **Step 2: Run, expect FAIL.** From `web/`: `npx vitest run src/postmark.test.ts`. Expected: fails on the missing export `postmarkMonth`.

- [ ] **Step 3: Implement.** In `postmark.ts`, add above `postmark()`:

```ts
/** A calendar month; `month` is 0-based (jan = 0). */
export type YearMonth = { year: number; month: number };

/** 📮 the ONE place an acquisition instant becomes a month (UTC). The chip and the
 *  almanac (docs/spec-almanac.md D2) both call this, so fixing #267 here moves BOTH —
 *  never add a second bucketing. Null when unknown/junk. */
export function postmarkMonth(iso: string | undefined): YearMonth | null {
  if (iso === undefined) return null;
  const t = Date.parse(iso);
  if (Number.isNaN(t)) return null;
  const d = new Date(t);
  return { year: d.getUTCFullYear(), month: d.getUTCMonth() };
}
```

and replace `postmark()`'s body with:

```ts
export function postmark(iso: string | undefined): string | null {
  const ym = postmarkMonth(iso);
  return ym === null ? null : `${MONTHS[ym.month]} ${ym.year}`;
}
```

- [ ] **Step 4: Run.** `npx vitest run src/postmark.test.ts`. Expected: PASS, including every pre-existing `postmark`/`waitedYears` test unchanged.

- [ ] **Step 5: Commit**

```bash
git add web/src/postmark.ts web/src/postmark.test.ts
git commit -S -m "📮 postmarkMonth: one UTC bucketing shared by chip and almanac (#267 coupling)"
```

---

### Task 3: `almanac.ts` — the pure grouping

**Files:**
- Create: `web/src/almanac.ts`
- Test: `web/src/almanac.test.ts`

**Interfaces:**
- Consumes: `AdminGame` (`web/src/api.ts`), `postmarkMonth`/`YearMonth` (Task 2).
- Produces (exact):

```ts
export const MONTH_NAMES: readonly string[]; // 'january' … 'december'
export function nameMonth(bundle: string, currentYear: number): YearMonth | null;
export function isWaiting(g: AdminGame): boolean;
export function orderKey(g: AdminGame): string;
export function thumbSrc(g: AdminGame): string | null;
export type EntrySource = 'name' | 'postmark' | 'undated';
export type AlmanacEntry = { key: string; source: EntrySource; label: string; games: AdminGame[]; waiting: AdminGame[]; tucked: number };
export type AlmanacMonth = { month: number; entries: AlmanacEntry[] };
export type AlmanacYear = { year: number; months: AlmanacMonth[] };
export type Almanac = { years: AlmanacYear[]; undated: AlmanacEntry[]; span: number | null; picks: { waiting: number; months: number; unparsed: number } };
export function buildAlmanac(games: AdminGame[], currentYear: number): Almanac;
export function countLine(e: AlmanacEntry): string;
export function headline(a: Almanac): string | null;
export function subtitle(a: Almanac): string;
```

- [ ] **Step 1: Write the failing tests** (`web/src/almanac.test.ts`)

```ts
import { describe, expect, it } from 'vitest';
import type { AdminGame } from './api';
import {
  buildAlmanac,
  countLine,
  headline,
  isWaiting,
  nameMonth,
  orderKey,
  subtitle,
  thumbSrc,
  type AlmanacEntry,
} from './almanac';

const Y = 2026; // injected current year — never the real clock

function game(over: Partial<AdminGame>): AdminGame {
  return {
    id: 'gk1:mn1',
    title: 'a game',
    bundle: 'Some Bundle',
    key_type: 'steam',
    giftable: true,
    hidden: false,
    status: 'available',
    claim_id: null,
    artwork_url: null,
    requires_choice: false,
    steam_app_id: null,
    owned_by_ben: false,
    steam: null,
    ...over,
  };
}

describe('nameMonth', () => {
  it('reads both measured spellings to the same (year, month)', () => {
    expect(nameMonth('October 2021', Y)).toEqual({ year: 2021, month: 9 });
    expect(nameMonth('October 2021 Humble Choice', Y)).toEqual({ year: 2021, month: 9 });
  });
  it('is strict: anything else falls through (null)', () => {
    expect(nameMonth('october 2021', Y)).toBeNull(); // case: measured names are capitalised
    expect(nameMonth('Humble Monthly — March 2017', Y)).toBeNull(); // pre-Choice era string
    expect(nameMonth('October 2021 Bundle', Y)).toBeNull();
    expect(nameMonth('Sept 2021', Y)).toBeNull();
  });
  it('bounds the year to [2010, currentYear + 1]', () => {
    expect(nameMonth('March 0000', Y)).toBeNull();
    expect(nameMonth('March 2009', Y)).toBeNull();
    expect(nameMonth('March 2010', Y)).toEqual({ year: 2010, month: 2 });
    expect(nameMonth('January 2027', Y)).toEqual({ year: 2027, month: 0 });
    expect(nameMonth('January 2028', Y)).toBeNull();
  });
});

describe('isWaiting — exactly Game::is_listable', () => {
  it('needs available AND giftable AND not hidden', () => {
    expect(isWaiting(game({}))).toBe(true);
    expect(isWaiting(game({ status: 'ben_redeemed' }))).toBe(false);
    expect(isWaiting(game({ giftable: false }))).toBe(false);
    expect(isWaiting(game({ hidden: true }))).toBe(false);
  });
});

describe('orderKey / thumbSrc', () => {
  it('orderKey is the gamekey prefix of id', () => {
    expect(orderKey(game({ id: 'zApDsS:wingspan_steam' }))).toBe('zApDsS');
    expect(orderKey(game({ id: 'no-colon' }))).toBe('no-colon');
  });
  it('thumbSrc prefers artwork, then the steam capsule, else null', () => {
    expect(thumbSrc(game({ artwork_url: 'https://hb.imgix.net/x.png', steam_app_id: 1 }))).toBe(
      'https://hb.imgix.net/x.png',
    );
    expect(thumbSrc(game({ steam_app_id: 413150 }))).toBe(
      'https://shared.akamai.steamstatic.com/store_item_assets/steam/apps/413150/capsule_616x353.jpg',
    );
    expect(thumbSrc(game({}))).toBeNull();
  });
});

describe('buildAlmanac', () => {
  it('merges both spellings of one Choice month into ONE entry, and the name beats the postmark', () => {
    const a = buildAlmanac(
      [
        game({ id: 'p1:a', bundle: 'November 2021', requires_choice: true }),
        // order key redeemed on the evening of nov 30 EST ⇒ postmark says DEC; the name wins
        game({ id: 'zApDsS:b', bundle: 'November 2021 Humble Choice', status: 'ben_redeemed', acquired_at: '2021-12-01T01:45:48.153511Z' }),
      ],
      Y,
    );
    expect(a.years).toHaveLength(1);
    expect(a.years[0]!.year).toBe(2021);
    expect(a.years[0]!.months).toEqual([
      expect.objectContaining({ month: 10, entries: [expect.objectContaining({ source: 'name', label: 'november 2021' })] }),
    ]);
    expect(a.years[0]!.months[0]!.entries[0]!.games).toHaveLength(2);
  });

  it('groups the postmark path by ORDER, never by name (9 orders share one name in prod)', () => {
    const a = buildAlmanac(
      [
        game({ id: 'k1:a', bundle: 'A very special gift just for you', acquired_at: '2014-05-10T12:00:00Z' }),
        game({ id: 'k2:b', bundle: 'A very special gift just for you', acquired_at: '2019-02-01T12:00:00Z' }),
      ],
      Y,
    );
    expect(a.years.map((y) => y.year)).toEqual([2019, 2014]);
    expect(a.years.every((y) => y.months[0]!.entries[0]!.source === 'postmark')).toBe(true);
  });

  it('dates an order by its EARLIEST valid acquired_at and keeps its undated siblings with it', () => {
    const a = buildAlmanac(
      [
        game({ id: 'k1:a', bundle: 'B', acquired_at: '2015-06-02T00:00:00Z' }),
        game({ id: 'k1:b', bundle: 'B', acquired_at: '2015-04-02T00:00:00Z' }),
        game({ id: 'k1:c', bundle: 'B' }),
        game({ id: 'k1:d', bundle: 'B', acquired_at: 'junk' }),
      ],
      Y,
    );
    expect(a.years[0]!.months[0]!.month).toBe(3); // april, the earliest
    expect(a.years[0]!.months[0]!.entries[0]!.games).toHaveLength(4);
    expect(a.undated).toEqual([]);
  });

  it('a pre-Choice name with a postmark lands on its ORDER date, not undated (Lilith Q3)', () => {
    const a = buildAlmanac(
      [game({ id: 'hm:a', bundle: 'Humble Monthly — March 2017', acquired_at: '2017-03-03T00:00:00Z' })],
      Y,
    );
    expect(a.years[0]!.year).toBe(2017);
    expect(a.years[0]!.months[0]!.entries[0]!.source).toBe('postmark');
  });

  it('never invents a date: no name-month and no postmark ⇒ the undated shelf', () => {
    const a = buildAlmanac([game({ id: 'u:a', bundle: 'Mystery Box' })], Y);
    expect(a.years).toEqual([]);
    expect(a.undated).toEqual([expect.objectContaining({ source: 'undated', label: 'Mystery Box' })]);
    expect(a.span).toBeNull();
  });

  it('orders years and months newest first, entries by label then key', () => {
    const a = buildAlmanac(
      [
        game({ id: 'a:1', bundle: 'March 2020' }),
        game({ id: 'b:1', bundle: 'Zeta', acquired_at: '2020-07-09T00:00:00Z' }),
        game({ id: 'c:1', bundle: 'Alpha', acquired_at: '2020-07-01T00:00:00Z' }),
        game({ id: 'd:1', bundle: 'June 2023' }),
      ],
      Y,
    );
    expect(a.years.map((y) => y.year)).toEqual([2023, 2020]);
    expect(a.years[1]!.months.map((m) => m.month)).toEqual([6, 2]);
    expect(a.years[1]!.months[0]!.entries.map((e) => e.label)).toEqual(['Alpha', 'Zeta']);
    expect(a.span).toBe(4); // 2023 − 2020 + 1, derived
  });

  it('counts waiting and tucked per entry', () => {
    const a = buildAlmanac(
      [
        game({ id: 'p:1', bundle: 'May 2022' }),
        game({ id: 'p:2', bundle: 'May 2022', hidden: true }),
        game({ id: 'p:3', bundle: 'May 2022', status: 'gifted' }),
      ],
      Y,
    );
    const e = a.years[0]!.months[0]!.entries[0]!;
    expect(e.waiting.map((g) => g.id)).toEqual(['p:1']);
    expect(e.tucked).toBe(1);
  });

  it('counts N over EVERY waiting choice pick, whatever its date source (OMBB D5)', () => {
    const a = buildAlmanac(
      [
        game({ id: 'p:1', bundle: 'May 2022', requires_choice: true }),
        game({ id: 'p:2', bundle: 'May 2022', requires_choice: true }),
        game({ id: 'p:3', bundle: 'June 2022', requires_choice: true }),
        game({ id: 'p:4', bundle: 'June 2022', requires_choice: true, status: 'gifted' }), // not waiting
        game({ id: 'r:1', bundle: 'Choice: Renamed Month', requires_choice: true }), // a rename
      ],
      Y,
    );
    expect(a.picks).toEqual({ waiting: 4, months: 2, unparsed: 1 });
  });
});

describe('copy', () => {
  function entry(over: Partial<AlmanacEntry>): AlmanacEntry {
    return { key: 'k', source: 'name', label: 'may 2022', games: [], waiting: [], tucked: 0, ...over };
  }
  const g = game({});

  it('countLine: all waiting / some waiting / none / single / tucked', () => {
    expect(countLine(entry({ games: [g, g, g], waiting: [g, g, g] }))).toBe('3 treasures · all 3 still waiting');
    expect(countLine(entry({ games: [g, g, g], waiting: [g] }))).toBe('3 treasures · 1 still waiting');
    expect(countLine(entry({ games: [g, g] }))).toBe('2 treasures · all given or kept ♡');
    expect(countLine(entry({ games: [g], waiting: [g] }))).toBe('1 treasure · still waiting');
    expect(countLine(entry({ games: [g, g], waiting: [g], tucked: 1 }))).toBe('2 treasures · 1 still waiting · 1 tucked away');
    expect(countLine(entry({ games: [g, g], tucked: 2 }))).toBe('2 treasures · all tucked away');
    expect(countLine(entry({ games: [g, g, g], tucked: 1 }))).toBe('3 treasures · the rest given or kept ♡ · 1 tucked away');
  });

  it('headline: conditional, pluralised, never "0 picks"', () => {
    const base = { years: [], undated: [], span: null };
    expect(headline({ ...base, picks: { waiting: 0, months: 0, unparsed: 0 } })).toBeNull();
    expect(headline({ ...base, picks: { waiting: 589, months: 75, unparsed: 0 } })).toBe(
      '589 choice picks still waiting, across 75 months — never spent.',
    );
    expect(headline({ ...base, picks: { waiting: 1, months: 1, unparsed: 0 } })).toBe(
      '1 choice pick still waiting, across 1 month — never spent.',
    );
    expect(headline({ ...base, picks: { waiting: 2, months: 0, unparsed: 2 } })).toBe(
      '2 choice picks still waiting — never spent.',
    );
  });

  it('subtitle derives the span and drops it when unknown', () => {
    const base = { years: [], undated: [], picks: { waiting: 0, months: 0, unparsed: 0 } };
    expect(subtitle({ ...base, span: 15 })).toBe('15 years of the attic, month by month.');
    expect(subtitle({ ...base, span: 1 })).toBe('1 year of the attic, month by month.');
    expect(subtitle({ ...base, span: null })).toBe('the attic, month by month.');
  });
});
```

- [ ] **Step 2: Run, expect FAIL.** `npx vitest run src/almanac.test.ts`. Expected: cannot resolve `./almanac`.

- [ ] **Step 3: Implement** (`web/src/almanac.ts`)

```ts
// 📜 the almanac (docs/spec-almanac.md) — ben's attic as a calendar of months.
// Pure and deterministic: no Date.now(), no locale/ICU (postmark.ts's rule); the
// current year is a parameter. Helper module per web convention (postmark.ts,
// tags.ts): the page component only renders what this returns.
import type { AdminGame } from './api';
import { postmarkMonth, type YearMonth } from './postmark';

export const MONTH_NAMES = [
  'january', 'february', 'march', 'april', 'may', 'june',
  'july', 'august', 'september', 'october', 'november', 'december',
] as const;
const CAPITALISED = MONTH_NAMES.map((m) => m.charAt(0).toUpperCase() + m.slice(1));
// D1: strict on purpose — a miss falls through to the postmark/undated, never to a
// wrong month. Measured over prod 2026-10-07: 590/590 discovery picks and 21/21
// order-key stragglers match.
const NAME_MONTH_RE = new RegExp(`^(${CAPITALISED.join('|')}) (\\d{4})( Humble Choice)?$`);
const MIN_YEAR = 2010;

/** D1 ①: the month a bundle's NAME says it is, or null. Year bounded to
 *  [2010, currentYear + 1] so a typo can never draw a `0000` divider. */
export function nameMonth(bundle: string, currentYear: number): YearMonth | null {
  const m = NAME_MONTH_RE.exec(bundle);
  if (m === null) return null;
  const year = Number(m[2]);
  if (year < MIN_YEAR || year > currentYear + 1) return null;
  return { year, month: CAPITALISED.indexOf(m[1] as string) };
}

/** Exactly `Game::is_listable` (domain/src/lib.rs). One definition of "waiting". */
export function isWaiting(g: AdminGame): boolean {
  return g.status === 'available' && g.giftable && !g.hidden;
}

/** The order a game came from: the gamekey prefix of `id` ("{gamekey}:{machine_name}").
 *  D1 ②: a NAME is not a bundle — 9 prod orders share "A very special gift just for you". */
export function orderKey(g: AdminGame): string {
  const i = g.id.indexOf(':');
  return i === -1 ? g.id : g.id.slice(0, i);
}

/** Thumb source order: humble artwork → steam capsule (GameGrid.tsx's shipped URL) → null. */
export function thumbSrc(g: AdminGame): string | null {
  if (g.artwork_url !== null) return g.artwork_url;
  if (g.steam_app_id !== null) {
    return `https://shared.akamai.steamstatic.com/store_item_assets/steam/apps/${g.steam_app_id}/capsule_616x353.jpg`;
  }
  return null;
}

export type EntrySource = 'name' | 'postmark' | 'undated';
export type AlmanacEntry = {
  key: string;
  source: EntrySource;
  label: string;
  games: AdminGame[];
  waiting: AdminGame[];
  tucked: number;
};
export type AlmanacMonth = { month: number; entries: AlmanacEntry[] };
export type AlmanacYear = { year: number; months: AlmanacMonth[] };
export type Almanac = {
  years: AlmanacYear[];
  undated: AlmanacEntry[];
  /** newest − oldest dated year + 1; null when nothing is dated. Derived, never typed. */
  span: number | null;
  picks: { waiting: number; months: number; unparsed: number };
};

function makeEntry(key: string, source: EntrySource, label: string, games: AdminGame[]): AlmanacEntry {
  return { key, source, label, games, waiting: games.filter(isWaiting), tucked: games.filter((g) => g.hidden).length };
}

// plain code-unit comparison — deterministic, no ICU (localeCompare is locale-dependent)
function cmp(a: string, b: string): number {
  return a < b ? -1 : a > b ? 1 : 0;
}
function byLabelThenKey(a: AlmanacEntry, b: AlmanacEntry): number {
  return cmp(a.label, b.label) || cmp(a.key, b.key);
}

export function buildAlmanac(games: AdminGame[], currentYear: number): Almanac {
  const named = new Map<string, { ym: YearMonth; games: AdminGame[] }>();
  const orders = new Map<string, { label: string; games: AdminGame[] }>();
  for (const g of games) {
    const nm = nameMonth(g.bundle, currentYear);
    if (nm !== null) {
      const k = `${nm.year}-${String(nm.month).padStart(2, '0')}`;
      const slot = named.get(k) ?? { ym: nm, games: [] };
      slot.games.push(g);
      named.set(k, slot);
    } else {
      const k = orderKey(g);
      const slot = orders.get(k) ?? { label: g.bundle, games: [] };
      slot.games.push(g);
      orders.set(k, slot);
    }
  }

  const dated: { ym: YearMonth; entry: AlmanacEntry }[] = [];
  const undated: AlmanacEntry[] = [];
  for (const [k, { ym, games: gs }] of named) {
    dated.push({ ym, entry: makeEntry(`name:${k}`, 'name', `${MONTH_NAMES[ym.month]} ${ym.year}`, gs) });
  }
  for (const [k, { label, games: gs }] of orders) {
    // D1 ②: the order's EARLIEST valid instant, bucketed by the chip's own function (D2)
    let earliest: string | undefined;
    for (const g of gs) {
      if (g.acquired_at === undefined || Number.isNaN(Date.parse(g.acquired_at))) continue;
      if (earliest === undefined || Date.parse(g.acquired_at) < Date.parse(earliest)) earliest = g.acquired_at;
    }
    const ym = postmarkMonth(earliest);
    if (ym === null) undated.push(makeEntry(`order:${k}`, 'undated', label, gs));
    else dated.push({ ym, entry: makeEntry(`order:${k}`, 'postmark', label, gs) });
  }

  const byYear = new Map<number, Map<number, AlmanacEntry[]>>();
  for (const { ym, entry } of dated) {
    const months = byYear.get(ym.year) ?? new Map<number, AlmanacEntry[]>();
    const list = months.get(ym.month) ?? [];
    list.push(entry);
    months.set(ym.month, list);
    byYear.set(ym.year, months);
  }
  const years: AlmanacYear[] = [...byYear.entries()]
    .sort(([a], [b]) => b - a)
    .map(([year, months]) => ({
      year,
      months: [...months.entries()]
        .sort(([a], [b]) => b - a)
        .map(([month, entries]) => ({ month, entries: entries.sort(byLabelThenKey) })),
    }));
  undated.sort(byLabelThenKey);

  // D5: N over EVERY waiting choice pick — a rename must show as `unparsed`, never shrink N
  const picks = games.filter((g) => g.requires_choice && isWaiting(g));
  const pickMonths = new Set<string>();
  let unparsed = 0;
  for (const g of picks) {
    const nm = nameMonth(g.bundle, currentYear);
    if (nm === null) unparsed += 1;
    else pickMonths.add(`${nm.year}-${nm.month}`);
  }

  const span = years.length === 0 ? null : years[0]!.year - years[years.length - 1]!.year + 1;
  return { years, undated, span, picks: { waiting: picks.length, months: pickMonths.size, unparsed } };
}

export function countLine(e: AlmanacEntry): string {
  const n = e.games.length;
  const w = e.waiting.length;
  const noun = n === 1 ? 'treasure' : 'treasures';
  if (w === 0 && e.tucked === n && n > 0) return `${n} ${noun} · all tucked away`;
  let s: string;
  // with some tucked away, "all given or kept" would be false — say "the rest" (plan review)
  if (w === 0) s = e.tucked > 0 ? `${n} ${noun} · the rest given or kept ♡` : `${n} ${noun} · all given or kept ♡`;
  else if (w === n) s = n === 1 ? '1 treasure · still waiting' : `${n} ${noun} · all ${n} still waiting`;
  else s = `${n} ${noun} · ${w} still waiting`;
  if (e.tucked > 0) s += ` · ${e.tucked} tucked away`;
  return s;
}

/** D5: one sentence, never a card; null when nothing is waiting (no "0 picks"). */
export function headline(a: Almanac): string | null {
  const { waiting, months } = a.picks;
  if (waiting === 0) return null;
  const picks = `${waiting} choice ${waiting === 1 ? 'pick' : 'picks'} still waiting`;
  if (months === 0) return `${picks} — never spent.`;
  return `${picks}, across ${months} ${months === 1 ? 'month' : 'months'} — never spent.`;
}

export function subtitle(a: Almanac): string {
  if (a.span === null) return 'the attic, month by month.';
  return `${a.span} ${a.span === 1 ? 'year' : 'years'} of the attic, month by month.`;
}
```

- [ ] **Step 4: Run.** `npx vitest run src/almanac.test.ts`. Expected: PASS (all describe blocks).

- [ ] **Step 5: Commit**

```bash
git add web/src/almanac.ts web/src/almanac.test.ts
git commit -S -m "📜 almanac.ts: pure grouping — name-month by (year,month), postmark by order, honest undated"
```

---

### Task 4: the `Almanac` page, route and nav

**Files:**
- Create: `web/src/admin/Almanac.tsx`, `web/src/admin/Almanac.test.tsx`
- Modify: `web/src/App.tsx` (route after `scrapbook`), `web/src/admin/AdminApp.tsx` (nav link after scrapbook), `web/src/admin/AdminApp.test.tsx` (nav assertion)

**Interfaces:**
- Consumes (existing): `adminCatalog(): Promise<AdminGame[]>` (`api.ts:385`); `withAuth<T>(fn: () => Promise<T>, navigate): Promise<T>` (`./withAuth`); `titleColorClass(title: string): string` (`../titleColor`).
- Consumes (Task 3, `../almanac`, exact): `MONTH_NAMES: readonly string[]` ('january'…'december'); `buildAlmanac(games: AdminGame[], currentYear: number): Almanac`; `countLine(e: AlmanacEntry): string`; `headline(a: Almanac): string | null`; `subtitle(a: Almanac): string`; `thumbSrc(g: AdminGame): string | null`; `type AlmanacEntry = { key: string; source: 'name' | 'postmark' | 'undated'; label: string; games: AdminGame[]; waiting: AdminGame[]; tucked: number }`; `type Almanac = { years: { year: number; months: { month: number; entries: AlmanacEntry[] }[] }[]; undated: AlmanacEntry[]; span: number | null; picks: { waiting: number; months: number; unparsed: number } }`.
- Produces: `export function Almanac({ currentYear }: { currentYear?: number })`. `currentYear` defaults to `new Date().getUTCFullYear()` and is injectable for tests.

- [ ] **Step 1: Write the failing tests** (`web/src/admin/Almanac.test.tsx`)

```tsx
import { render, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { vi, describe, it, expect, beforeEach } from 'vitest';
import { MemoryRouter, Route, Routes, useLocation } from 'react-router-dom';
import { Almanac } from './Almanac';
import type { AdminGame } from '../api';

vi.mock('../api', async (importOriginal) => {
  const actual = await importOriginal<typeof import('../api')>();
  return { ...actual, adminCatalog: vi.fn() };
});
import { adminCatalog } from '../api';

function game(over: Partial<AdminGame>): AdminGame {
  return {
    id: 'gk1:mn1', title: 'a game', bundle: 'Some Bundle', key_type: 'steam', giftable: true,
    hidden: false, status: 'available', claim_id: null, artwork_url: null, requires_choice: false,
    steam_app_id: null, owned_by_ben: false, steam: null, ...over,
  };
}

function LinksProbe() {
  const loc = useLocation();
  return <pre data-testid="picked">{JSON.stringify(loc.state)}</pre>;
}

function renderPage() {
  return render(
    <MemoryRouter initialEntries={['/admin/almanac']}>
      <Routes>
        <Route path="/admin/almanac" element={<Almanac currentYear={2026} />} />
        <Route path="/admin/links" element={<LinksProbe />} />
      </Routes>
    </MemoryRouter>,
  );
}

const fixture: AdminGame[] = [
  game({ id: 'p:1', title: 'pick one', bundle: 'September 2026', requires_choice: true }),
  game({ id: 'p:2', title: 'pick two', bundle: 'September 2026', requires_choice: true }),
  game({ id: 'o:1', title: 'old friend', bundle: 'Humble Indie Bundle 8', status: 'ben_redeemed', acquired_at: '2013-03-27T18:22:58Z' }),
  game({ id: 'u:1', title: 'mystery', bundle: 'Mystery Box', status: 'ben_redeemed' }),
];

describe('Almanac', () => {
  beforeEach(() => {
    vi.mocked(adminCatalog).mockReset();
  });

  it('renders the derived subtitle, the headline, years newest-first, and the undated shelf', async () => {
    vi.mocked(adminCatalog).mockResolvedValue(fixture);
    renderPage();
    expect(await screen.findByText('14 years of the attic, month by month.')).toBeInTheDocument();
    expect(screen.getByText('2 choice picks still waiting, across 1 month — never spent.')).toBeInTheDocument();
    const years = screen.getAllByRole('heading', { level: 2 }).map((h) => h.textContent);
    expect(years).toEqual(['2026', '2013', 'undated']);
    expect(screen.getByText(/the attic doesn't know when these arrived/)).toBeInTheDocument();
  });

  it('says which source dated each entry', async () => {
    vi.mocked(adminCatalog).mockResolvedValue(fixture);
    renderPage();
    const named = await screen.findByRole('article', { name: /september 2026/ });
    expect(within(named).getByText('🗓️')).toBeInTheDocument();
    expect(within(named).getByText('2 treasures · all 2 still waiting')).toBeInTheDocument();
    const posted = screen.getByRole('article', { name: /Humble Indie Bundle 8/ });
    expect(within(posted).getByText('📮')).toBeInTheDocument();
    expect(within(posted).getByText('1 treasure · all given or kept ♡')).toBeInTheDocument();
  });

  it('"wrap these" hands ONLY the waiting games to Links via the catalog contract', async () => {
    vi.mocked(adminCatalog).mockResolvedValue([
      ...fixture,
      game({ id: 'p:3', title: 'given away', bundle: 'September 2026', requires_choice: true, status: 'gifted' }),
    ]);
    renderPage();
    const named = await screen.findByRole('article', { name: /september 2026/ });
    await userEvent.click(within(named).getByRole('button', { name: /wrap the 2 waiting/ }));
    expect(JSON.parse(screen.getByTestId('picked').textContent ?? 'null')).toEqual({
      picked: [
        { id: 'p:1', title: 'pick one', requiresChoice: true },
        { id: 'p:2', title: 'pick two', requiresChoice: true },
      ],
    });
  });

  it('has no wrap button on an entry with nothing waiting', async () => {
    vi.mocked(adminCatalog).mockResolvedValue(fixture);
    renderPage();
    const posted = await screen.findByRole('article', { name: /Humble Indie Bundle 8/ });
    expect(within(posted).queryByRole('button')).toBeNull();
  });

  it('names choice picks whose month could not be read (OMBB D5)', async () => {
    vi.mocked(adminCatalog).mockResolvedValue([
      game({ id: 'r:1', bundle: 'Choice: Renamed', requires_choice: true }),
    ]);
    renderPage();
    expect(await screen.findByText("⚠️ 1 choice pick whose month we couldn't read")).toBeInTheDocument();
  });

  it('caps the art strip at 8 and says how many more', async () => {
    vi.mocked(adminCatalog).mockResolvedValue(
      Array.from({ length: 11 }, (_, i) => game({ id: `p:${i}`, title: `g${i}`, bundle: 'May 2022', steam_app_id: 100 + i })),
    );
    renderPage();
    const e = await screen.findByRole('article', { name: /may 2022/ });
    // thumbs are decorative (alt="" ⇒ role presentation), so count the DOM, not roles
    expect(e.querySelectorAll('img')).toHaveLength(8);
    expect(within(e).getByText('+3')).toBeInTheDocument();
  });

  it('shows a retry on load failure', async () => {
    vi.mocked(adminCatalog).mockRejectedValue(new Error('boom'));
    renderPage();
    expect(await screen.findByText("couldn't open the almanac — try again")).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'retry' })).toBeInTheDocument();
  });
});
```

Also in Step 1 (so they go red first): in `AdminApp.test.tsx`, rename the nav test to `'renders nav links for catalog, links, friends, scrapbook, almanac, and ops'` and add `expect(screen.getByRole('link', { name: /almanac/i })).toBeInTheDocument();`. And add these two to `Almanac.test.tsx`:

```tsx
  it('dims a hidden thumb but keeps it in the strip (D6)', async () => {
    vi.mocked(adminCatalog).mockResolvedValue([
      game({ id: 'p:1', bundle: 'May 2022', steam_app_id: 1 }),
      game({ id: 'p:2', bundle: 'May 2022', steam_app_id: 2, hidden: true, title: 'tucked' }),
    ]);
    renderPage();
    const e = await screen.findByRole('article', { name: /may 2022/ });
    expect(e.querySelectorAll('img')).toHaveLength(2);
    expect(within(e).getByTitle('tucked').className).toContain('opacity-40');
  });

  it('renders the undated shelf when ONLY an unparsed dated pick exists', async () => {
    vi.mocked(adminCatalog).mockResolvedValue([
      game({ id: 'r:1', bundle: 'Choice: Renamed', requires_choice: true, acquired_at: '2024-02-02T00:00:00Z' }),
    ]);
    renderPage();
    expect(await screen.findByRole('heading', { level: 2, name: 'undated' })).toBeInTheDocument();
    expect(screen.getByText("⚠️ 1 choice pick whose month we couldn't read")).toBeInTheDocument();
  });
```

- [ ] **Step 2: Run, expect FAIL.** `nice -n 15 npx vitest run src/admin/Almanac.test.tsx src/admin/AdminApp.test.tsx`. Expected: `Almanac.test.tsx` cannot resolve `./Almanac`; `AdminApp.test.tsx` fails **one** assertion (no `almanac` link).

- [ ] **Step 3: Implement** (`web/src/admin/Almanac.tsx`)

```tsx
import { useState, useEffect, useCallback, useMemo } from 'react';
import { useNavigate } from 'react-router-dom';
import { adminCatalog, type AdminGame } from '../api';
import { withAuth } from './withAuth';
import { titleColorClass } from '../titleColor';
import {
  MONTH_NAMES,
  buildAlmanac,
  countLine,
  headline,
  subtitle,
  thumbSrc,
  type AlmanacEntry,
} from '../almanac';

// The almanac 📜 — ben's attic as a calendar of months (docs/spec-almanac.md).
// Read-only; every grouping decision lives in ../almanac.ts. This page renders.

type PageState =
  | { phase: 'loading' }
  | { phase: 'loaded'; games: AdminGame[] }
  | { phase: 'error' };

const STRIP_MAX = 8;
const GLYPH: Record<AlmanacEntry['source'], string> = { name: '🗓️', postmark: '📮', undated: '📦' };

function Thumb({ g }: { g: AdminGame }) {
  const src = thumbSrc(g);
  return src !== null ? (
    <img src={src} alt="" loading="lazy" className="h-10 w-16 flex-shrink-0 rounded object-cover" />
  ) : (
    <div aria-hidden="true" className={`h-10 w-16 flex-shrink-0 rounded ${titleColorClass(g.title)}`} />
  );
}

function Entry({ e, onWrap }: { e: AlmanacEntry; onWrap: (e: AlmanacEntry) => void }) {
  const lit = e.waiting.length > 0;
  const shown = e.games.slice(0, STRIP_MAX);
  const more = e.games.length - shown.length;
  return (
    <article
      aria-label={e.label}
      // lit = raised shelf; quiet = flat + faded. No burgundy: DESIGN.md's Button Burgundy Rule
      className={`flex flex-col gap-2 rounded p-4 ${lit ? 'bg-shelf' : 'bg-floor opacity-80'}`}
    >
      <div className="flex flex-wrap items-baseline gap-2">
        <span aria-hidden="true">{GLYPH[e.source]}</span>
        <h3 className="font-medium text-ink">{e.label}</h3>
        <span className="text-sm text-dust">{countLine(e)}</span>
        {lit && (
          <button
            type="button"
            onClick={() => onWrap(e)}
            aria-label={`wrap the ${e.waiting.length} waiting from ${e.label} into a link`}
            className="ml-auto rounded bg-control px-3 py-1 text-sm hover:bg-control-bright"
          >
            wrap these →
          </button>
        )}
      </div>
      <div className="flex flex-wrap items-center gap-2">
        {shown.map((g) => (
          <span key={g.id} title={g.title} className={g.hidden ? 'opacity-40' : undefined}>
            <Thumb g={g} />
          </span>
        ))}
        {more > 0 && <span className="text-sm text-dust">+{more}</span>}
      </div>
    </article>
  );
}

export function Almanac({ currentYear = new Date().getUTCFullYear() }: { currentYear?: number }) {
  const navigate = useNavigate();
  const [state, setState] = useState<PageState>({ phase: 'loading' });

  const load = useCallback(() => {
    setState({ phase: 'loading' });
    withAuth(() => adminCatalog(), navigate)
      .then((games) => setState({ phase: 'loaded', games }))
      .catch(() => setState({ phase: 'error' }));
  }, [navigate]);

  useEffect(() => {
    load();
  }, [load]);

  const almanac = useMemo(
    () => (state.phase === 'loaded' ? buildAlmanac(state.games, currentYear) : null),
    [state, currentYear],
  );

  // the catalog's exact handoff contract (Catalog.tsx → Links.tsx location.state.picked)
  const wrap = (e: AlmanacEntry) =>
    navigate('/admin/links', {
      state: { picked: e.waiting.map((g) => ({ id: g.id, title: g.title, requiresChoice: g.requires_choice })) },
    });

  if (state.phase === 'loading') return <p className="text-dust">loading…</p>;
  if (state.phase === 'error' || almanac === null) {
    return (
      <div className="flex flex-col gap-4">
        <p className="text-dust">couldn't open the almanac — try again</p>
        <button onClick={load} className="w-fit rounded bg-control px-4 py-2 text-sm hover:bg-control-bright">
          retry
        </button>
      </div>
    );
  }

  const line = headline(almanac);
  const { unparsed } = almanac.picks;
  return (
    <div className="flex flex-col gap-8">
      <header className="flex flex-col gap-1">
        <h1 className="text-xl font-medium text-ink">📜 the almanac</h1>
        <p className="text-ink-soft">{subtitle(almanac)}</p>
        {line && <p className="font-medium text-ink">{line}</p>}
      </header>

      {almanac.years.map((y) => (
        <section key={y.year} className="flex flex-col gap-4">
          <h2 className="border-b border-line pb-1 text-lg text-ink">{y.year}</h2>
          {y.months.map((m) => (
            <div key={m.month} className="flex flex-col gap-2 sm:flex-row sm:gap-4">
              <p className="w-24 flex-shrink-0 text-sm text-dust">{MONTH_NAMES[m.month]}</p>
              <div className="flex min-w-0 flex-1 flex-col gap-2">
                {m.entries.map((e) => (
                  <Entry key={e.key} e={e} onWrap={wrap} />
                ))}
              </div>
            </div>
          ))}
        </section>
      ))}

      {(almanac.undated.length > 0 || unparsed > 0) && (
        <section className="flex flex-col gap-3">
          <h2 className="border-b border-line pb-1 text-lg text-ink">undated</h2>
          <p className="text-sm text-dust">the attic doesn't know when these arrived.</p>
          {unparsed > 0 && (
            <p className="text-sm text-ink">
              {`⚠️ ${unparsed} choice ${unparsed === 1 ? 'pick' : 'picks'} whose month we couldn't read`}
            </p>
          )}
          {almanac.undated.map((e) => (
            <Entry key={e.key} e={e} onWrap={wrap} />
          ))}
        </section>
      )}
    </div>
  );
}
```

Route, in `web/src/App.tsx`: add `import { Almanac } from './admin/Almanac';` beside the other admin imports, and after `<Route path="scrapbook" element={<Scrapbook />} />`:

```tsx
          <Route path="almanac" element={<Almanac />} />
```

Nav, in `web/src/admin/AdminApp.tsx`, after the scrapbook `NavLink`:

```tsx
        <NavLink to="/admin/almanac" className={navLinkClass}>
          almanac
        </NavLink>
```


- [ ] **Step 4: Run.** `nice -n 15 npx vitest run src/admin/Almanac.test.tsx src/admin/AdminApp.test.tsx`. Expected: PASS. Then run the **full CI web chain**, serially and niced, last, on the final tree: `nice -n 15 npm run lint && nice -n 15 npm run typecheck && nice -n 15 npm test -- --run && nice -n 15 npm run build`. Every step must pass. This is the step-9 lesson from the 10-05 pounce: no "clean" claim from a run that preceded an edit.

- [ ] **Step 5: Commit**

```bash
git add web/src/admin/Almanac.tsx web/src/admin/Almanac.test.tsx web/src/App.tsx web/src/admin/AdminApp.tsx web/src/admin/AdminApp.test.tsx
git commit -S -m "📜 the almanac page: /admin/almanac, newest-first months, wrap-these handoff"
```

---

### Task 5: real-data proof (no deploy)

**Files:** none committed. Throwaway files live **only** under the scratchpad
`/tmp/claude-1003/-home-code-kitten-code-kitten/4231eac3-a00d-46c2-b578-4314a998b6e6/scratchpad/`.

**Input** *(review B3)*: `…/scratchpad/games.json` is the raw output of `aws dynamodb scan` (read-only, `kitten-debug`, 2026-10-07T07:1x-04:00) over `pk begins_with GAME#`. The shape is `{ "Items": [ { "pk": {"S"}, "sk": {"S"}, "body": {"S": "<JSON of domain::Game>"}, … } ], "Count": 1134 }`. **The game is `JSON.parse(item.body.S)`.** If the file is missing, re-run the same scan rather than inventing data.

- [ ] **Step 1: Project and assert** with a throwaway vitest file at `web/src/__almanac_proof.test.ts`. **Delete it before committing anything**, and `git status` must not show it.

```ts
import { readFileSync } from 'node:fs';
import { expect, it } from 'vitest';
import { buildAlmanac } from './almanac';
import type { AdminGame } from './api';

it('prod proof', () => {
  const raw = JSON.parse(readFileSync('/tmp/claude-1003/-home-code-kitten-code-kitten/4231eac3-a00d-46c2-b578-4314a998b6e6/scratchpad/games.json', 'utf8'));
  const games: AdminGame[] = raw.Items.map((it: { body: { S: string } }) => {
    const g = JSON.parse(it.body.S);
    return {
      id: g.id, title: g.title, bundle: g.bundle, key_type: g.key_type, giftable: g.giftable,
      hidden: g.hidden, status: g.status, claim_id: g.claim_id ?? null, artwork_url: g.artwork_url ?? null,
      requires_choice: g.requires_choice ?? false, steam_app_id: g.steam_app_id ?? null,
      owned_by_ben: g.owned_by_ben ?? false, ...(g.acquired_at ? { acquired_at: g.acquired_at } : {}), steam: null,
    };
  });
  expect(games).toHaveLength(1134);
  const a = buildAlmanac(games, 2026);
  console.log(JSON.stringify({ picks: a.picks, span: a.span, years: a.years.map((y) => y.year), undated: a.undated.length }));
  expect(a.picks).toEqual({ waiting: 589, months: 75, unparsed: 0 }); // 75 = distinct months among the 589 LISTABLE picks, measured 07:1x
  expect(a.span).toBe(15);
  const all = a.years.flatMap((y) => y.months.flatMap((m) => m.entries));
  expect(all.filter((e) => e.label === 'A very special gift just for you')).toHaveLength(9);
  const nov21 = a.years.find((y) => y.year === 2021)!.months.find((m) => m.month === 10)!.entries;
  expect(nov21.filter((e) => e.source === 'name')).toHaveLength(1); // both spellings, one entry
  expect(nov21[0]!.games.some((g) => g.title === 'Wingspan')).toBe(true); // the #267 row filed by name
});
```

Run: `nice -n 15 npx vitest run src/__almanac_proof.test.ts` from `web/`. Record the printed JSON line verbatim. If an assertion fails, the **data** is the authority: read which number moved and why before touching code. A pre-registered number is a prediction, not a requirement.

- [ ] **Step 2: Real-browser render** *(review M3)*. With `nice -n 15 npm run dev` running in `web/`, drive the Playwright MCP. Before navigating, register mocks with `browser_run_code_unsafe`:

```js
async (page) => {
  const fs = require('fs');
  const raw = JSON.parse(fs.readFileSync('/tmp/claude-1003/-home-code-kitten-code-kitten/4231eac3-a00d-46c2-b578-4314a998b6e6/scratchpad/games.json', 'utf8'));
  const games = raw.Items.map((it) => { const g = JSON.parse(it.body.S); return { ...g, steam: null }; });
  await page.route('**/admin/api/catalog', (r) => r.fulfill({ json: games }));
  await page.route('**/admin/api/status', (r) => r.fulfill({ json: { sync: null, sync_run: null, game_counts: {} } }));
}
```

(If `require` isn't available in that sandbox, write the projection to `…/scratchpad/catalog.json` with node first and fulfill with `path:`.) Navigate to `http://localhost:5173/admin/almanac`. Screenshot at 1280px and at 390px into the scratchpad. Check, at 390px: `document.querySelector('main, .flex.flex-col.gap-8').scrollWidth <= window.innerWidth` **for the almanac content**. The admin **nav** may already overflow at 390px without this change (7 items, `flex gap-6 px-6`, no wrap). **Measure it on `main` first.** If it overflows there too, record it as pre-existing and do **not** fix the nav in this PR. Stop the dev server after.

- [ ] **Step 3: Record** the Step 1 JSON line and both screenshot paths for the PR body. The response size stays labelled a **reconstruction** (778,799 → 801,551 B) until it's measured live after deploy.
