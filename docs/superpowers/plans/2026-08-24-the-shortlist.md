# The Shortlist Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the Humble-pick cost of a gift visible to Ben while he composes it, and let him narrow 670 giftable games by that cost — without the app ever choosing.

**Architecture:** Pure frontend. `AdminGame` already carries `requires_choice`, so the cost facet is a new filter key in the existing `catalogToolkit` state machine plus a control in `ToolkitBar`, following the toolkit's own documented "filter N+1 registers in exactly one file" rule (`FILTER_KEYS`).

> 🔴 **REVISED 2026-08-26 — the exposure readout moved SCREENS, and the sentence that used to sit here (*"the router contract is NOT changed"*) was load-bearing and false.** It is described rather than quoted below, because this is a plan an executor reads top-to-bottom and a quoted dead constraint greps identically to a live one.
>
> **Measured in the code, not recalled:**
> - `web/src/admin/Links.tsx:112` loads **`adminLinks()` and nothing else** — there is **no `games` array on that screen**, so `requires_choice` is not reachable there at all.
> - `Links.tsx:76` seeds `picked` from `location.state` as `{ id, title }[]`, and then **mutates it**: reorder `:420`/`:430`, remove `:439`. ⇒ **a count computed once in `Catalog.tsx` and passed as a number goes stale the moment ben removes a pick.**
> - `claims_allowed` is `Links.tsx:69`, clamped `Math.max(1, n)` at `:364`.
>
> ⇒ **`min( #requires_choice among picked , claims_allowed )` needs both operands, and exactly one screen can ever hold both.** The three routes and why two lose: **(a) carry `requiresChoice` in the router state** — recomputes correctly on every mutation, one extra boolean, additive; **(b) fetch the catalog inside `Links.tsx`** — a cross-page fetch, which is the *same* cost this plan already refused when it deferred the "never offered" facet, plus the readout cannot render until a second request lands; **(c) precompute a number in `Catalog.tsx`** — **wrong**, stale on the first removal.
> ⇒ **(a). The contract is amended, additively and backward-compatibly, in Task 3.** Flagged explicitly for OMBB's step-5 sign-off: it is the one thing in this plan that touches an interface a previous plan froze.

**Tech Stack:** TypeScript, React, vitest + @testing-library/react, Tailwind classes already in the file.

**Spec:** `docs/superpowers/specs/2026-08-24-the-shortlist-design.md`

## Global Constraints

- **Voice is lowercase.** All user-facing copy in this codebase is lowercase (`PRODUCT.md`: "the voice is lowercase and affectionate"). No sentence case in labels.
- **No dashboard chrome.** `PRODUCT.md` anti-references forbid metric-card grids and BI styling **on the admin too** — "a workbench, not a dashboard". The cost readout is one line of text beside the existing "wrap these into a link" button, not a card.
- **No ranking, no score, no "recommended".** Design Principle 2 is *chosen-for-you, never shopping*. Facets narrow; they never order by cleverness.
- **`location.state.picked` — AMENDED 2026-08-26, and the amendment is the only interface change in this plan.** Now `{ id: string; title: string; requiresChoice?: boolean }[]`, ben's click order. `Links.tsx` consumes it.
  - **The ORDER contract is untouched** — that is what the freeze was protecting (`Catalog.test.tsx:954`: *"never sorted"*), and nothing here sorts.
  - **`requiresChoice` is OPTIONAL on purpose.** Router state is not persisted, so a reload of `/admin/links` already yields `picked === []`; the field cannot introduce a durability hazard that the contract did not already have. **Absent ⇒ UNKNOWN ⇒ the readout says so and never says zero.**
  - **Add no OTHER fields.** The amendment is one boolean with a named consumer; the freeze still governs everything else.
- **A new FILTER key must be added to `FILTER_KEYS`** or `filtersActive()` and the ToolkitBar "clear filters" readout silently miss it. This is the documented bug class in `catalogToolkit.ts:24-26`.
- Reuse `controlClass` from `ToolkitBar.tsx:12` for any new control.

---

### Task 1: `cost` facet in the toolkit state machine

**Files:**
- Modify: `web/src/admin/catalogToolkit.ts`
- Test: `web/src/admin/catalogToolkit.test.ts`

**Interfaces:**
- Consumes: `AdminGame` from `../api` (already imported), field `requires_choice: boolean`.
- Produces: `export type CostFilter = 'all' | 'free' | 'spends-pick'`; `ToolkitState.cost: CostFilter`; `IDLE_TOOLKIT.cost === 'all'`; `'cost'` present in `FILTER_KEYS`. **Task 2 relies on these exact names; Tasks 3 and 4 do not touch the facet at all** (they are the exposure readout, a separate surface) — corrected 2026-08-26, when the old Task 3 was replaced.

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

### Task 3: carry `requiresChoice` across the compose boundary

> 🔴 **NEW 2026-08-26.** The interface change the exposure readout needs. It is small, additive and
> backward-compatible — and it is still an amendment to a contract a previous plan froze, so it is
> **its own task with its own test**, not a line smuggled into the next one.

**Files:**
- Modify: `web/src/admin/Catalog.tsx` (one object literal), `web/src/admin/Links.tsx` (one type)
- Test: `web/src/admin/Catalog.test.tsx`

**Interfaces:**
- Produces: `location.state.picked` as `{ id: string; title: string; requiresChoice?: boolean }[]`.
- Consumed by: Task 4 only. Nothing else reads the new field.

- [ ] **Step 1: Write the failing test**

`Catalog.test.tsx:15` already carries a `PickProbe` that renders on a **real** `<Route>`, so the
contract is asserted **across the navigation boundary** rather than against a mocked `navigate`. Extend
it — *do not add a second probe*:

```tsx
function PickProbe() {
  const { state } = useLocation() as {
    state: { picked: { id: string; title: string; requiresChoice?: boolean }[] };
  };
  return (
    <div>
      <div>{state.picked.map((p) => p.title).join('|')}</div>
      {/* the new field, rendered as `title:true|title:false` so an ABSENT boolean is
          visibly `undefined` and cannot pass as `false` — the whole point of the field. */}
      <div data-testid="pick-choice">
        {state.picked.map((p) => `${p.title}:${String(p.requiresChoice)}`).join('|')}
      </div>
    </div>
  );
}
```

Add one test beside the existing wrap-these tests (§`Multi-select → wrap these into a link`, ~`:952`),
using that section's existing fixture/selection helpers:

```tsx
it('carries requires_choice across the wrap boundary, per game', async () => {
  // Two games, DIFFERENT requires_choice — a fixture where both share a value
  // would pass under a hardcoded constant as readily as under the real field.
  await pickTwoGamesWithMixedChoice();   // one true, one false
  await user.click(screen.getByRole('button', { name: 'wrap these into a link (2)' }));
  expect(screen.getByTestId('pick-choice')).toHaveTextContent('Choice Game:true|Free Game:false');
});
```

⚠️ **The mixed fixture is the test.** A pair that agrees on `requires_choice` yields the expected
string under both the real implementation and a hardcoded literal — **two reasons to pass is zero
tests.** Replace `pickTwoGamesWithMixedChoice()` with the section's existing two-game selection
helper, overriding `requires_choice` per game off `gameFixture` (`Catalog.test.tsx:33`).

- [ ] **Step 2: Run it and watch it fail**

Run: `cd web && npx vitest run src/admin/Catalog.test.tsx -t 'carries requires_choice'`
Expected: FAIL — `Choice Game:undefined|Free Game:undefined`.

- [ ] **Step 3: Implement — one literal, one type**

`Catalog.tsx`, in the pick checkbox's `onChange` (`:530`), the appended object gains the field:

```tsx
: [...cur, { id: game.id, title: game.title, requiresChoice: game.requires_choice }],
```

Then widen the local `picked` state type in the same file (`:103`) to match, and widen the reader in
`Links.tsx:76-77`:

```tsx
const [picked, setPicked] = useState<{ id: string; title: string; requiresChoice?: boolean }[]>(
  () => (location.state as { picked?: { id: string; title: string; requiresChoice?: boolean }[] } | null)?.picked ?? [],
);
```

⚠️ **`requiresChoice?: boolean` — optional, deliberately.** Router state is not persisted; a reload of
`/admin/links` already yields `[]`. **Absent must mean UNKNOWN, never `false`** — Task 4's label
depends on being able to tell those apart, and a required `boolean` would erase the distinction at the
type level.

- [ ] **Step 4: Run the full web suite**

Run: `cd web && npx vitest run && npx tsc --noEmit`
Expected: PASS, no type errors. The existing order test (`:954`, *"never sorted"*) must still pass —
**that is the half of the frozen contract this amendment does not touch, and its green is the proof.**

- [ ] **Step 5: Commit**

```bash
git add web/src/admin/Catalog.tsx web/src/admin/Links.tsx web/src/admin/Catalog.test.tsx
git commit -S -m "feat(admin): carry requires_choice across the compose boundary"
```

---

### Task 4: the exposure readout — beside `claims_allowed`, on `Links.tsx`

> 🔴 **REWRITTEN 2026-08-26. The task that stood here had FOUR defects, and only the first was on the
> card.** Recorded because an executor needs to know the old shape is dead, and a reviewer needs to
> know what changed:
> 1. **Wrong screen.** It modified `Catalog.tsx`; the spec puts the readout *"beside the `claims_allowed`
>    input in `admin/Links.tsx`, because `min()` needs both the picks and the allowance and only that
>    screen has both."*
> 2. **Unreachable data**, which is why (1) is not a one-line move: `Links.tsx` never loads games. Fixed
>    by Task 3.
> 3. 🔑 **It rendered NOTHING in the case the spec exists for.** Its label returned `null` on an empty
>    selection — but *"every link ben has ever sent is an open shelf, so every one of them exposes
>    `claims_allowed` picks to whoever holds it, and nothing has ever said so"* is **the spec's
>    strongest finding**, and the spec says that row *"must render first, not as a footnote."*
>    ***The old task's silent case was the headline.***
> 4. **Pre-revision semantics.** It printed a flat *"spends 2 of your picks"* — a confident count of a
>    spend that **the friend** makes, on their clock, possibly never. The spec was revised away from
>    exactly that: it is an **exposure**, bounded by `min(…, claims_allowed)`, phrased *"if they claim"*.

**Files:**
- Create: `web/src/admin/pickExposure.ts`
- Modify: `web/src/admin/Links.tsx`
- Test: `web/src/admin/pickExposure.test.ts`, `web/src/admin/Links.test.tsx`

**Interfaces:**
- Consumes: the `picked` state (`Links.tsx:76`, with Task 3's `requiresChoice`) and `claimsAllowed`
  (`Links.tsx:69`).
- Produces: `export function exposureLabel(picked: { requiresChoice?: boolean }[], claimsAllowed: number): string`.
  **Returns a string always — there is no null case.** The empty selection is the open shelf, and the
  open shelf is the finding.

- [ ] **Step 1: Write the failing tests**

Create `web/src/admin/pickExposure.test.ts`:

```ts
import { describe, it, expect } from 'vitest';
import { exposureLabel } from './pickExposure';

const choice = { requiresChoice: true };
const free = { requiresChoice: false };
const unknown = {};

describe('exposureLabel', () => {
  it('an EMPTY selection is an open shelf, and says what it exposes', () => {
    // The spec's strongest finding: 18 of 18 links in production are open
    // shelves and none of them ever said this. Silence here is the defect.
    expect(exposureLabel([], 3)).toBe('open shelf — whoever holds this link can spend up to 3 of your monthly picks');
  });

  it('uses the singular for an allowance of one', () => {
    expect(exposureLabel([], 1)).toBe('open shelf — whoever holds this link can spend up to 1 of your monthly picks');
  });

  it('says free when nothing picked spends a pick', () => {
    expect(exposureLabel([free, free], 3)).toBe('free to give — no picks at stake');
  });

  it('counts only the picks that spend', () => {
    expect(exposureLabel([free, choice, choice], 5)).toBe('up to 2 of your monthly picks, if they claim');
  });

  it('is CEILINGED by claims_allowed — the claim path refuses past it', () => {
    // domain/src/lib.rs:314 refuses a claim once claims_used >= claims_allowed,
    // so three choice games behind an allowance of one expose ONE pick, not three.
    expect(exposureLabel([choice, choice, choice], 1)).toBe('up to 1 of your monthly picks, if they claim');
  });

  it('an UNKNOWN pick is reported, never counted as free', () => {
    // Understating a finite resource is the dangerous direction. An absent
    // requiresChoice means the state predates Task 3 or arrived some other way.
    expect(exposureLabel([choice, unknown], 5)).toBe('up to 1 of your monthly picks, and 1 not costed — reopen from the catalog');
  });

  it('reports unknowns even when nothing known spends', () => {
    expect(exposureLabel([free, unknown], 5)).toBe('up to 0 of your monthly picks, and 1 not costed — reopen from the catalog');
  });

  it('is total for a nonsense allowance rather than printing a negative', () => {
    // The UI clamps to >= 1 (Links.tsx:364), but this function is exported and
    // tested on its own; a future caller has no such clamp.
    expect(exposureLabel([choice], 0)).toBe('free to give — no picks at stake');
  });
});
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cd web && npx vitest run src/admin/pickExposure.test.ts`
Expected: FAIL — cannot resolve `./pickExposure`.

- [ ] **Step 3: Implement**

Create `web/src/admin/pickExposure.ts`:

```ts
/** 🎟️ What a link EXPOSES of ben's finite monthly humble picks.
 *
 * Not a cost. The spend is the FRIEND's act, at a time ben does not control,
 * and it may never happen (`fulfillment/src/lib.rs:127` dispatches the
 * choosecontent two-write on CLAIM, not on create). So this is an upper bound,
 * phrased as one.
 *
 * The ceiling is real, not cosmetic: `domain/src/lib.rs:314` refuses a claim
 * once `claims_used >= claims_allowed`, so N choice games behind an allowance
 * of A expose min(N, A) — true under every claim ordering.
 *
 * An EMPTY selection is not "nothing to say" — it is the open shelf, which is
 * 18 of 18 links in production, exposing the full allowance with nothing on
 * screen ever having said so.
 *
 * An UNKNOWN pick is reported and never counted as free: understating a finite
 * resource is the direction that costs ben something he cannot get back. */
export function exposureLabel(
  picked: { requiresChoice?: boolean }[],
  claimsAllowed: number,
): string {
  const allowance = Number.isFinite(claimsAllowed) ? Math.max(0, Math.trunc(claimsAllowed)) : 0;

  if (picked.length === 0) {
    return `open shelf — whoever holds this link can spend up to ${allowance} of your monthly picks`;
  }

  let spends = 0;
  let unknown = 0;
  for (const p of picked) {
    if (p.requiresChoice === undefined) unknown++;
    else if (p.requiresChoice) spends++;
  }

  const exposure = Math.min(spends, allowance);
  if (unknown > 0) {
    return `up to ${exposure} of your monthly picks, and ${unknown} not costed — reopen from the catalog`;
  }
  if (exposure === 0) return 'free to give — no picks at stake';
  return `up to ${exposure} of your monthly picks, if they claim`;
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cd web && npx vitest run src/admin/pickExposure.test.ts`
Expected: PASS.

- [ ] **Step 5: Render it beside the allowance**

In `web/src/admin/Links.tsx`, add the import:

```tsx
import { exposureLabel } from './pickExposure';
```

Immediately **after** the `claims allowed` `<label>` closes (`:355`–`:366`), inside the same row,
render one line of text — **not a card, not a metric tile** (`PRODUCT.md`: the admin is *a workbench,
not a dashboard*):

```tsx
<p className="self-end text-xs text-dust" data-testid="pick-exposure">
  {exposureLabel(picked, claimsAllowed)}
</p>
```

⚠️ **It renders UNCONDITIONALLY — there is no `picked.length > 0 &&` guard.** Wrapping it in one
re-introduces defect (3): the open-shelf case is the case this whole build exists to make visible.

- [ ] **Step 6: Write the integration tests**

Append to `web/src/admin/Links.test.tsx`, matching that file's existing render/mock setup. **Two
tests, because the readout has two live inputs and a single test cannot show it tracks either:**

```tsx
it('says what an uncurated link exposes, with nothing picked', async () => {
  renderLinks();                       // no router state ⇒ picked === []
  await user.clear(screen.getByLabelText('claims allowed'));
  await user.type(screen.getByLabelText('claims allowed'), '3');
  expect(screen.getByTestId('pick-exposure'))
    .toHaveTextContent('open shelf — whoever holds this link can spend up to 3 of your monthly picks');
});

it('tracks the allowance the moment ben changes it', async () => {
  // Arrive WITH picks, via router state, exactly as the catalog sends them.
  renderLinksWithPicks([
    { id: 'a', title: 'A', requiresChoice: true },
    { id: 'b', title: 'B', requiresChoice: true },
  ]);
  expect(screen.getByTestId('pick-exposure')).toHaveTextContent('up to 2 of your monthly picks');
  await user.clear(screen.getByLabelText('claims allowed'));
  await user.type(screen.getByLabelText('claims allowed'), '1');
  // the CEILING, in the UI: two choice games behind an allowance of one.
  expect(screen.getByTestId('pick-exposure')).toHaveTextContent('up to 1 of your monthly picks');
});
```

✅ **Both helpers ALREADY EXIST and are named here as measured, not guessed** — `renderLinks()` at
`Links.test.tsx:35` and `renderLinksWithPicks(picked)` at `:46`, which already seeds a `MemoryRouter`
with `{ pathname: '/admin/links', state: { picked } }`. **Do not write a third.**
⚠️ **One edit is required:** `renderLinksWithPicks`' parameter is typed `{ id: string; title: string }[]`
(`:46`) and must be widened with `requiresChoice?: boolean` or the fixtures above will not typecheck.

⚠️ **One more assertion is owed if the helper makes it cheap:** remove a pick via the `×` button
(`Links.tsx:439`) and assert the readout drops. **That is the mutation that made a precomputed number
wrong**, and it is the reason the field is carried rather than the count.

- [ ] **Step 7: Run the full web suite**

Run: `cd web && npx vitest run && npx tsc --noEmit`
Expected: PASS, no type errors.

- [ ] **Step 8: Commit**

```bash
git add web/src/admin/pickExposure.ts web/src/admin/pickExposure.test.ts web/src/admin/Links.tsx web/src/admin/Links.test.tsx
git commit -S -m "feat(admin): show what a link exposes of ben's monthly picks, while he composes it"
```

---

## Explicitly out of scope for this plan

**"never offered on any link"** — spec §what-gets-built item 3. It needs `adminLinks()` data inside `Catalog.tsx` (a cross-page fetch) and, at n=18 links, will read approximately "everything", which is open question ② to the family. **Deferred pending that answer** — recorded as a decision, not an oversight. If the answer is "ship it", it is a fourth task of the same shape: a `neverOffered` facet fed by a `Set<string>` of every id in every link's `curated_game_ids`.

## Self-review

> 🔴 **RE-REVIEWED 2026-08-26 after Tasks 3–4 were rewritten. The self-review below is the NEW one;
> the old one certified a task that no longer exists — and it certified it CLEAN, which is the part
> worth remembering. *A self-review passes against the plan it was written beside, not against the
> spec.*** The old scan reported "no TBDs, every code step carries real code" and was **true**, while
> the task it scanned rendered nothing in the spec's headline case. **A placeholder scan cannot see a
> missing REQUIREMENT** — it only sees a missing *word*.

**Spec coverage:** §narrowers → Tasks 1+2 (cost facet; era/art/duplicate facets are additional instances of the identical pattern and are deliberately not multiplied here — one facet proves the shape). §exposure-honest-at-compose-time → Task 4, **including the open-shelf row the spec requires to render first** (`exposureLabel` has no null case, and the Links.tsx render carries no `picked.length > 0` guard). §the-ceiling (`min(…, claims_allowed)`) → Task 4 unit test *"is CEILINGED by claims_allowed"* **and** the UI test that retypes the allowance — a pure-function assertion alone would not show the readout is wired to the live input. §never-offered → explicitly deferred above with a reason.

**Interface change:** exactly one, Task 3, `location.state.picked` gains an optional `requiresChoice`. Isolated in its own task with its own cross-boundary test, and the ORDER half of the frozen contract (`Catalog.test.tsx:954`) is asserted untouched. **Flagged for OMBB's step-5 sign-off.**

**Placeholder scan:** **one** soft reference remains and it names its exact fallback — the two-game mixed-choice selection helper in Task 3 Step 1. Every other named symbol was verified to exist at the line cited: `renderLinks` `Links.test.tsx:35`, `renderLinksWithPicks` `:46`, `PickProbe` `Catalog.test.tsx:15`, `gameFixture` `:33`, `claimsAllowed` `Links.tsx:69`, `picked` `:76`, the `×` remove `:439`, the allowance clamp `:364`. *The previous version's two soft references both pointed at things on the wrong screen.*

**Type consistency:** `CostFilter` is defined in Task 1 and consumed by name in Task 2. `FILTER_KEYS` gains `'cost'` in Task 1, which is what makes the Task 1 `filtersActive` test pass. `exposureLabel(picked, claimsAllowed)` is defined and consumed in Task 4 only, and takes `{ requiresChoice?: boolean }[]` — **structurally satisfied by the Task 3 pick shape without importing it**, so Task 4 has no compile-time dependency on Task 3's type beyond the field existing.

**Ordering:** Task 3 MUST land before Task 4 — Task 4's fixtures set `requiresChoice`, which does not exist on the contract until Task 3. Tasks 1 and 2 are independent of both.
