# The Shortlist Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the Humble-pick cost of a gift visible to Ben while he composes it, and let him narrow 670 giftable games by that cost — without the app ever choosing.

**Architecture:** Pure frontend. `AdminGame` already carries `requires_choice`, so the cost facet is a new filter key in the existing `catalogToolkit` state machine plus a control in `ToolkitBar`, following the toolkit's own documented "filter N+1 registers in exactly one file" rule (`FILTER_KEYS`). The selection cost readout joins the existing `picked` id list against the already-loaded `games` array — **the `location.state.picked` router contract is NOT changed.**

**Tech Stack:** TypeScript, React, vitest + @testing-library/react, Tailwind classes already in the file.

**Spec:** `docs/superpowers/specs/2026-08-24-the-shortlist-design.md`

## Global Constraints

- **Voice is lowercase.** All user-facing copy in this codebase is lowercase (`PRODUCT.md`: "the voice is lowercase and affectionate"). No sentence case in labels.
- **No dashboard chrome.** `PRODUCT.md` anti-references forbid metric-card grids and BI styling **on the admin too** — "a workbench, not a dashboard". The cost readout is one line of text beside the existing "wrap these into a link" button, not a card.
- **No ranking, no score, no "recommended".** Design Principle 2 is *chosen-for-you, never shopping*. Facets narrow; they never order by cleverness.
- **`location.state.picked` shape is frozen:** `{ id: string; title: string }[]`, ben's click order. `Links.tsx` consumes it. Do not add fields.
- **A new FILTER key must be added to `FILTER_KEYS`** or `filtersActive()` and the ToolkitBar "clear filters" readout silently miss it. This is the documented bug class in `catalogToolkit.ts:24-26`.
- Reuse `controlClass` from `ToolkitBar.tsx:12` for any new control.

---

### Task 1: `cost` facet in the toolkit state machine

**Files:**
- Modify: `web/src/admin/catalogToolkit.ts`
- Test: `web/src/admin/catalogToolkit.test.ts`

**Interfaces:**
- Consumes: `AdminGame` from `../api` (already imported), field `requires_choice: boolean`.
- Produces: `export type CostFilter = 'all' | 'free' | 'spends-pick'`; `ToolkitState.cost: CostFilter`; `IDLE_TOOLKIT.cost === 'all'`; `'cost'` present in `FILTER_KEYS`. Task 2 and Task 3 rely on these exact names.

- [ ] **Step 1: Write the failing tests**

Append to `web/src/admin/catalogToolkit.test.ts`:

```ts
describe('cost filter', () => {
  const free: AdminGame = { ...base, id: 'free', title: 'Free Key', requires_choice: false };
  const costly: AdminGame = { ...base, id: 'costly', title: 'Choice Key', requires_choice: true };

  it('idles to all and shows both', () => {
    const r = applyToolkit([free, costly], IDLE_TOOLKIT);
    expect(r.shown).toBe(2);
  });

  it('free keeps only requires_choice=false', () => {
    const s: ToolkitState = { ...IDLE_TOOLKIT, cost: 'free' };
    const r = applyToolkit([free, costly], s);
    expect(r.groups[0].games.map((g) => g.id)).toEqual(['free']);
  });

  it('spends-pick keeps only requires_choice=true', () => {
    const s: ToolkitState = { ...IDLE_TOOLKIT, cost: 'spends-pick' };
    const r = applyToolkit([free, costly], s);
    expect(r.groups[0].games.map((g) => g.id)).toEqual(['costly']);
  });

  it('does NOT count filtered-out rows as excludedNoData', () => {
    // requires_choice is always present on AdminGame, so a cost filter can
    // never exclude for missing data. Guards against copying the tags/rating
    // branch, which increments excludedNoData.
    const s: ToolkitState = { ...IDLE_TOOLKIT, cost: 'free' };
    expect(applyToolkit([free, costly], s).excludedNoData).toBe(0);
  });

  it('registers in FILTER_KEYS so filtersActive and clear-filters see it', () => {
    expect(FILTER_KEYS).toContain('cost');
    expect(filtersActive({ ...IDLE_TOOLKIT, cost: 'free' })).toBe(true);
  });
});
```

Add `FILTER_KEYS` and `filtersActive` to the existing import block at the top of the test file.

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cd web && npx vitest run src/admin/catalogToolkit.test.ts`
Expected: FAIL — `cost` is not a property of `ToolkitState`; `FILTER_KEYS` does not contain `'cost'`.

- [ ] **Step 3: Implement**

In `web/src/admin/catalogToolkit.ts`:

```ts
/** 🎟️ Humble Choice cost: claiming a requires_choice game spends one of ben's
 * finite monthly picks (domain/src/lib.rs:979). The friend surface has always
 * said "confirm? spends 1 pick" at claim time; this is the same fact, moved to
 * where ben composes. */
export type CostFilter = 'all' | 'free' | 'spends-pick';
```

Add to `ToolkitState`:

```ts
  /** 🎟️ filter by whether a claim spends one of ben's monthly humble picks. */
  cost: CostFilter;
```

Add to `IDLE_TOOLKIT`: `cost: 'all',`

Change `FILTER_KEYS` to: `export const FILTER_KEYS = ['q', 'tags', 'rating', 'mature', 'cost'] as const;`

In `applyToolkit`'s `games.filter((g) => { ... })`, after the `mature` branch and before `return true`:

```ts
    // No excludedNoData here: requires_choice is non-optional on AdminGame, so
    // this filter can never exclude a row for missing data.
    if (state.cost === 'free' && g.requires_choice) return false;
    if (state.cost === 'spends-pick' && !g.requires_choice) return false;
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cd web && npx vitest run src/admin/catalogToolkit.test.ts`
Expected: PASS, all pre-existing tests still green.

- [ ] **Step 5: Typecheck and commit**

```bash
cd web && npx tsc --noEmit
git add web/src/admin/catalogToolkit.ts web/src/admin/catalogToolkit.test.ts
git commit -S -m "feat(admin): cost facet — filter the catalog by whether a claim spends a humble pick"
```

---

### Task 2: the cost control in ToolkitBar

**Files:**
- Modify: `web/src/admin/ToolkitBar.tsx`
- Test: `web/src/admin/ToolkitBar.test.tsx`

**Interfaces:**
- Consumes: `CostFilter`, `ToolkitState` from Task 1.
- Produces: a `<select aria-label="pick cost">` rendering options `any` / `free` / `spends a pick`, calling `onChange({ ...state, cost })`.

- [ ] **Step 1: Write the failing test**

Append to `web/src/admin/ToolkitBar.test.tsx`:

```tsx
it('offers a pick-cost filter and reports the choice upward', async () => {
  const onChange = vi.fn();
  render(
    <ToolkitBar
      state={IDLE_TOOLKIT}
      tagOptions={[]}
      shown={2}
      total={2}
      excludedNoData={0}
      onChange={onChange}
    />,
  );
  const select = screen.getByLabelText('pick cost');
  await userEvent.selectOptions(select, 'free');
  expect(onChange).toHaveBeenCalledWith({ ...IDLE_TOOLKIT, cost: 'free' });
});

it('labels the costly option in ben-facing words, lowercase', () => {
  render(
    <ToolkitBar
      state={IDLE_TOOLKIT}
      tagOptions={[]}
      shown={0}
      total={0}
      excludedNoData={0}
      onChange={() => {}}
    />,
  );
  expect(screen.getByRole('option', { name: 'spends a pick' })).toBeInTheDocument();
});
```

Match the existing import style at the top of that test file (`render`, `screen`, `userEvent`, `vi`, `IDLE_TOOLKIT`).

- [ ] **Step 2: Run the test to verify it fails**

Run: `cd web && npx vitest run src/admin/ToolkitBar.test.tsx`
Expected: FAIL — `Unable to find a label with the text of: pick cost`.

- [ ] **Step 3: Implement**

Add the options constant beside `GROUP_OPTIONS` in `ToolkitBar.tsx`:

```tsx
const COST_OPTIONS: { value: CostFilter; label: string }[] = [
  { value: 'all', label: 'any' },
  { value: 'free', label: 'free to give' },
  { value: 'spends-pick', label: 'spends a pick' },
];
```

Add `type CostFilter,` to the import from `./catalogToolkit`.

Render it next to the rating control, in the same wrapper markup the rating select uses:

```tsx
<select
  aria-label="pick cost"
  className={controlClass}
  value={state.cost}
  onChange={(e) => onChange({ ...state, cost: e.target.value as CostFilter })}
>
  {COST_OPTIONS.map((o) => (
    <option key={o.value} value={o.value}>
      {o.label}
    </option>
  ))}
</select>
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cd web && npx vitest run src/admin/ToolkitBar.test.tsx`
Expected: PASS.

- [ ] **Step 5: Typecheck and commit**

```bash
cd web && npx tsc --noEmit
git add web/src/admin/ToolkitBar.tsx web/src/admin/ToolkitBar.test.tsx
git commit -S -m "feat(admin): pick-cost control in the toolkit bar"
```

---

### Task 3: what this gift will cost — the compose-time readout

**Files:**
- Create: `web/src/admin/pickCost.ts`
- Modify: `web/src/admin/Catalog.tsx`
- Test: `web/src/admin/pickCost.test.ts`, `web/src/admin/Catalog.test.tsx`

**Interfaces:**
- Consumes: `AdminGame` from `../api`; the existing `picked: { id: string; title: string }[]` state in `Catalog.tsx:102`. **The `location.state.picked` contract is unchanged — cost is joined by id against the already-loaded `games`.**
- Produces: `export function pickCostLabel(picked: { id: string }[], games: AdminGame[]): string | null`.

- [ ] **Step 1: Write the failing tests**

Create `web/src/admin/pickCost.test.ts`:

```ts
import { describe, it, expect } from 'vitest';
import type { AdminGame } from '../api';
import { pickCostLabel } from './pickCost';

const g = (id: string, requires_choice: boolean): AdminGame => ({
  id, title: id, bundle: 'b', key_type: 'steam', giftable: true, hidden: false,
  status: 'available', claim_id: null, artwork_url: null, requires_choice,
  steam_app_id: null, owned_by_ben: false, hidden_source: null, steam: null,
});

describe('pickCostLabel', () => {
  const games = [g('a', false), g('b', true), g('c', true)];

  it('is null with nothing picked — no readout to render', () => {
    expect(pickCostLabel([], games)).toBeNull();
  });

  it('says free when nothing picked spends a pick', () => {
    expect(pickCostLabel([{ id: 'a' }], games)).toBe('free to give');
  });

  it('counts the picks a gift will spend', () => {
    expect(pickCostLabel([{ id: 'a' }, { id: 'b' }, { id: 'c' }], games)).toBe('spends 2 of your picks');
  });

  it('uses the singular for one', () => {
    expect(pickCostLabel([{ id: 'b' }], games)).toBe('spends 1 of your picks');
  });

  it('IGNORES picked ids absent from games rather than counting them as free', () => {
    // A stale pick (row filtered away, catalog reloaded) must not silently
    // read as free — that is the failure that would understate the cost.
    expect(pickCostLabel([{ id: 'ghost' }], games)).toBe('1 pick not costed — reload the catalog');
  });
});
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cd web && npx vitest run src/admin/pickCost.test.ts`
Expected: FAIL — cannot resolve `./pickCost`.

- [ ] **Step 3: Implement**

Create `web/src/admin/pickCost.ts`:

```ts
import type { AdminGame } from '../api';

/** 🎟️ What a composed gift will cost ben in humble choice picks.
 *
 * The app has always known this — `selfClaimLabel.ts:20` says "confirm? spends
 * 1 pick" — but only ever said it at CLAIM time, to the FRIEND. Ben composed
 * blind. 570 of his 670 giftable games are requires_choice.
 *
 * Unknown ids are reported, never assumed free: understating a cost is the
 * failure that matters here. */
export function pickCostLabel(
  picked: { id: string }[],
  games: AdminGame[],
): string | null {
  if (picked.length === 0) return null;
  const byId = new Map(games.map((g) => [g.id, g]));
  let spends = 0;
  let unknown = 0;
  for (const p of picked) {
    const g = byId.get(p.id);
    if (!g) unknown++;
    else if (g.requires_choice) spends++;
  }
  if (unknown > 0) return `${unknown} pick${unknown === 1 ? '' : 's'} not costed — reload the catalog`;
  if (spends === 0) return 'free to give';
  return `spends ${spends} of your picks`;
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cd web && npx vitest run src/admin/pickCost.test.ts`
Expected: PASS.

- [ ] **Step 5: Render it beside the wrap button**

In `web/src/admin/Catalog.tsx`, add the import:

```tsx
import { pickCostLabel } from './pickCost';
```

Inside the `{picked.length > 0 && (` block (`Catalog.tsx:290`), after the "clear picks" button and inside the same flex row:

```tsx
<span className="text-xs text-dust">{pickCostLabel(picked, games)}</span>
```

Use whatever the loaded catalog array is named in that component's scope; if it is not `games`, use that name — do not introduce a new fetch.

- [ ] **Step 6: Write the integration test**

Append to `web/src/admin/Catalog.test.tsx`, matching that file's existing render/mock setup:

```tsx
it('shows what a composed gift will cost in picks', async () => {
  // Uses the file's existing catalog mock; ensure at least one mocked game has
  // requires_choice: true and select it via the same row affordance the other
  // tests use to build `picked`.
  await selectFirstChoiceGame();
  expect(await screen.findByText(/spends 1 of your picks/)).toBeInTheDocument();
});
```

Replace `selectFirstChoiceGame()` with the row-selection helper already used in that file for the "wrap these into a link" tests; if none exists, click the row's select control directly as those tests do.

- [ ] **Step 7: Run the full web suite**

Run: `cd web && npx vitest run && npx tsc --noEmit`
Expected: PASS, no type errors.

- [ ] **Step 8: Commit**

```bash
git add web/src/admin/pickCost.ts web/src/admin/pickCost.test.ts web/src/admin/Catalog.tsx web/src/admin/Catalog.test.tsx
git commit -S -m "feat(admin): show what a gift costs in humble picks, at compose time"
```

---

## Explicitly out of scope for this plan

**"never offered on any link"** — spec §what-gets-built item 3. It needs `adminLinks()` data inside `Catalog.tsx` (a cross-page fetch) and, at n=18 links, will read approximately "everything", which is open question ② to the family. **Deferred pending that answer** — recorded as a decision, not an oversight. If the answer is "ship it", it is a fourth task of the same shape: a `neverOffered` facet fed by a `Set<string>` of every id in every link's `curated_game_ids`.

## Self-review

**Spec coverage:** §narrowers → Task 1+2 (cost facet; era/art/duplicate facets are additional instances of the identical pattern and are deliberately not multiplied here — one facet proves the shape, and the spec's headline claim is the pick cost). §pick-cost-honest-at-compose-time → Task 3. §never-offered → explicitly deferred above with a reason.

**Placeholder scan:** two soft references remain, both deliberate and both naming the exact fallback: the catalog array's identifier in Task 3 Step 5, and the row-selection helper in Task 3 Step 6 — each says what to do if the named thing is absent. No TBDs, no "add error handling", every code step carries real code.

**Type consistency:** `CostFilter` is defined in Task 1 and consumed by name in Task 2. `pickCostLabel(picked, games)` is defined and consumed in Task 3 only. `FILTER_KEYS` gains `'cost'` in Task 1, which is what makes the Task 1 `filtersActive` test pass.
