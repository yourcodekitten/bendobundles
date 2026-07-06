# bendobundles Plan 3: Frontend Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** The TypeScript SPA — the friend claim page (`/l/:token`) and ben's admin surface
(`/admin`) — talking to plan 2's lambdas, built as a static bundle for plan 4's S3/CloudFront.

**Architecture:** One vite + react + TS app in `web/`. React Router with two trees (friend +
admin) and a bare landing page. A single typed API layer (`src/api.ts`) mirrors the rust serde
shapes exactly — it is the ONLY place `fetch` appears. Styling = tailwind v4, dark-first, minimal
custom CSS. Tests = vitest + @testing-library/react with fetch mocked at the API-layer boundary.

**Tech Stack:** vite 6, react 18, react-router-dom 6, typescript 5 strict, tailwindcss v4,
vitest + @testing-library/react + @testing-library/user-event, eslint flat config + prettier.

**Read first:** spec §7/§8/§10 (docs/superpowers/specs/2026-07-02-bendobundles-design.md); the
API truth lives in `crates/public-api/src/lib.rs` and `crates/admin-api/src/lib.rs` — response
shapes in this plan were transcribed from them; if code and plan disagree, THE RUST WINS (read it).

## Global Constraints

- All commits GPG-signed, authored code kitten. `npm run lint && npm run typecheck && npm test -- --run && npm run build` green at every commit (from `web/`).
- TS strict; no `any` (eslint-enforced) except in test mock plumbing where unavoidable + commented.
- The API layer is the only fetch site. Components never build URLs or parse JSON.
- Friend surface: never render internal error strings — the API layer normalizes; unknown/invalid
  link renders the same not-found view (no enumeration hints), matching the server's byte-identical
  404 policy.
- The pasted humble cookie value: password-type input, never echoed back into the DOM after
  submit, never logged (no console.* of form values anywhere — eslint no-console on).
- Session handling: the admin session cookie is HttpOnly (server-set) — the SPA never reads it;
  a 401 from any admin call redirects to the login route.
- Visual polish is explicitly iterate-with-ben-later; this plan ships clean/dark/functional. Do
  not spend steps on pixel work beyond the tailwind classes given.
- Dev proxy: vite proxies `/api` and `/admin/api` to a configurable target (env
  `VITE_API_PROXY`, default http://localhost:9000) — real wiring is plan 4; tests never hit it.
- CI: `.github/workflows/ci.yml` gains a `web` job (see Task 1) — do NOT touch the rust `test` job.

## API Contract (transcribed from plan-2 rust — the types Task 2 must encode)

```
GET  /api/l/:token            → 200 LinkView | 404 {error} | 500 {error}
     LinkView = { label: string, claims_allowed: number, claims_used: number, active: boolean,
                  games: GameView[], claims: ClaimView[] }
     GameView = { id, title, bundle, key_type: string, artwork_url: string|null }
     ClaimView = { game_id: string, title?: string, state: "pending"|"fulfilled"|"compensated",
                   gift_url: string|null }
POST /api/l/:token/claim {game_id} → 200 {gift_url} | 202 {status:"processing", message}
                                   | 404/409/410 {error} | 500 {error}
POST /admin/api/login {password}   → 200 (Set-Cookie) | 401
GET  /admin/api/catalog            → 200 Game[] (full domain fields incl hidden/status/claim_id/keyindex)
POST /admin/api/games/:id/hidden {hidden} → 200 | 404 | 409 {error}
POST /admin/api/links {label, claims_allowed, expires_days?} → 200 {token, url_path}
GET  /admin/api/links              → 200 Link[] (token,label,claims_allowed,claims_used,revoked,expires_at,created_at)
POST /admin/api/links/:token/revoke → 200
GET  /admin/api/links/:token/claims → 200 ClaimRecord[] (id,link_token,game_id,state,gift_url,created_at)
POST /admin/api/cookie {cookie}    → 200 {ok, restored_previous?, inconclusive?}
POST /admin/api/sync               → 200 {result:"sync_done", games_written, orders_failed} | error shape
GET  /admin/api/status             → 200 {sync: SyncState|null, game_counts: Record<string,number>}
     (all admin routes: 401 when session missing/expired)
```

## File Structure (locked)

```
web/
  index.html  vite.config.ts  tsconfig.json  eslint.config.js  package.json
  src/
    main.tsx  App.tsx  index.css
    api.ts                 # ALL fetch + types + error normalization
    friend/LinkPage.tsx    # loader + state machine for /l/:token
    friend/GameGrid.tsx    # title-grouped grid, copy counts, claim buttons
    friend/ClaimDialog.tsx # confirm → result (gift url / refused / processing)
    friend/ClaimsHistory.tsx
    friend/Landing.tsx     # bare-root cute page (no data)
    admin/AdminApp.tsx     # layout + auth guard + nav
    admin/Login.tsx
    admin/Catalog.tsx      # badges, search, hidden toggles
    admin/Links.tsx        # create/list/revoke/audit + copy invite URL
    admin/Ops.tsx          # cookie paste, sync-now, status panel
  src/**/*.test.tsx        # co-located vitest files
.github/workflows/ci.yml   # + web job
```

---

### Task 1: web scaffold + CI job

**Files:** create the `web/` toolchain (`npm create vite@latest web -- --template react-ts` shape,
then configure), Modify `.github/workflows/ci.yml`.

- [ ] Scaffold vite react-ts in `web/`; add tailwind v4 (`@tailwindcss/vite` plugin, `@import "tailwindcss";` in index.css); strict tsconfig (`"strict": true, "noUncheckedIndexedAccess": true`).
- [ ] eslint flat config: typescript-eslint recommended + react-hooks + `no-console: "error"` + prettier config; scripts: `lint` (eslint .), `typecheck` (tsc --noEmit), `test` (vitest), `build` (tsc -b && vite build).
- [ ] vitest config in vite.config.ts (`environment: "jsdom"`, globals true, setup file adding @testing-library/jest-dom matchers). Vite dev proxy for `/api` + `/admin/api` from `VITE_API_PROXY`.
- [ ] One smoke test (`App.test.tsx`: renders landing headline) — run red on an empty App, then implement a placeholder App with the landing headline, green.
- [ ] CI: add `web` job to ci.yml (independent of the rust job): actions/checkout (SHA-pinned, same ref as the test job), actions/setup-node (resolve current LTS major + SHA-pin, `cache: npm`, `cache-dependency-path: web/package-lock.json`), then `npm ci`, `npm run lint`, `npm run typecheck`, `npm test -- --run`, `npm run build` all with `working-directory: web`.
- [ ] Verify: full script suite green locally; commit `feat(web): vite+react+ts scaffold, tailwind, vitest, ci job`.

### Task 2: the API layer

**Files:** Create `web/src/api.ts` + `web/src/api.test.ts`.

**Produces (the contract every component consumes — exact):**
```ts
export type GameView = { id: string; title: string; bundle: string; key_type: string; artwork_url: string | null };
export type ClaimView = { game_id: string; title?: string; state: "pending" | "fulfilled" | "compensated"; gift_url: string | null };
export type LinkView = { label: string; claims_allowed: number; claims_used: number; active: boolean; games: GameView[]; claims: ClaimView[] };
export type ClaimResult =
  | { kind: "gifted"; gift_url: string }
  | { kind: "processing"; message: string }
  | { kind: "refused"; message: string }     // 409/410 — friendly server message
  | { kind: "error"; message: string };      // 500/network — generic copy, never internals
export type AdminGame = { id: string; title: string; bundle: string; key_type: string; giftable: boolean; hidden: boolean; status: string; claim_id: string | null; artwork_url: string | null; keyindex: number };
export type AdminLink = { token: string; label: string; claims_allowed: number; claims_used: number; revoked: boolean; expires_at: string | null; created_at: string };
export type CookieResult = { ok: boolean; restored_previous?: boolean; inconclusive?: boolean };
export type StatusView = { sync: { last_run_epoch: number; ok: boolean; cookie_ok: boolean; games_written: number; message: string } | null; game_counts: Record<string, number> };

export class Unauthorized extends Error {}   // admin 401 → caller redirects to login
export class NotFound extends Error {}       // friend 404 → not-found view

export async function fetchLink(token: string): Promise<LinkView>;              // throws NotFound
export async function claimGame(token: string, gameId: string): Promise<ClaimResult>; // never throws — normalizes
export async function adminLogin(password: string): Promise<boolean>;           // true on 200, false on 401
export async function adminCatalog(): Promise<AdminGame[]>;                     // throws Unauthorized
export async function adminSetHidden(id: string, hidden: boolean): Promise<{ ok: true } | { ok: false; message: string }>;
export async function adminCreateLink(label: string, claims: number, expiresDays?: number): Promise<{ token: string; url_path: string }>;
export async function adminLinks(): Promise<AdminLink[]>;
export async function adminRevoke(token: string): Promise<void>;
export async function adminLinkClaims(token: string): Promise<ClaimView[]>;     // map ClaimRecord→ClaimView shape
export async function adminPasteCookie(cookie: string): Promise<CookieResult>;
export async function adminSync(): Promise<{ games_written: number; orders_failed: number }>;
export async function adminStatus(): Promise<StatusView>;
```
- claimGame maps: 200→gifted, 202→processing (server message), 409/410→refused (server's `error`),
  anything else (incl. thrown fetch) → `{kind:"error", message:"something hiccuped — try again"}`.
- Every admin fn: response.status===401 → throw Unauthorized BEFORE any parse.
- [ ] Tests (vi.stubGlobal fetch): one per mapping above incl. 401→Unauthorized, 404→NotFound,
  claimGame's five outcomes, cookie result passthrough. Red → implement → green → commit
  `feat(web): typed api layer with normalized claim outcomes`.

### Task 3: friend surface

**Files:** Create friend/* components + tests; wire routes in App.tsx
(`/` → Landing, `/l/:token` → LinkPage, `*` → Landing).

Behavior contract:
- **Landing:** app name + one playful line ("a friend has to hand you a key for this door ♡"),
  zero data fetches.
- **LinkPage:** on mount fetchLink → loading / not-found (NotFound ⇒ same view as garbage token)
  / loaded. Header: label + `{claims_used}/{claims_allowed} claims used`. Inactive link
  (`active:false` with games present = exhausted): banner "you've used all your claims" + grid
  visible with claim buttons disabled. Inactive with empty games (revoked/expired): banner "this
  invite isn't active anymore — bug ben" + history still shown.
- **GameGrid:** group by title (multiple copies = one card, "×N copies" chip); card = artwork
  (fallback: colored div from title hash), title, bundle, key_type chip, claim button
  (disabled when !active). Sorted by title (server pre-sorts; don't re-sort, just group stably).
- **ClaimDialog:** confirm step ("claim <title>? this uses 1 of your claims") → on confirm
  claimGame → outcome views: gifted (the gift_url as a big copyable link + "open on humble"
  anchor + "this link is one-time — redeem it to YOUR humble account"), processing (server
  message + "check this page later"), refused (message + dialog close re-fetches the link so the
  grid updates), error. Dialog result NEVER auto-dismisses a gifted outcome (the URL must not be
  lost by a mis-click — require explicit close after copy).
- **ClaimsHistory:** "your gifts" list — per claim: state chip + gift_url link when fulfilled
  ("lost the tab? it's right here"), pending shows "processing".
- [ ] Tests (mock api module with vi.mock): LinkPage 3 states; exhausted-vs-revoked rendering
  difference; grid grouping ×N; ClaimDialog happy path (confirm → gifted view shows exact URL,
  close → onRefresh called); refused path; processing path. Red → implement → green → commit
  `feat(web): friend surface — link page, grid, claim dialog, history`.

### Task 4: admin shell + login

**Files:** Create admin/AdminApp.tsx, admin/Login.tsx + tests; routes `/admin/login` +
`/admin/*` (guarded layout with nav: catalog / links / ops).

- AuthGuard pattern: admin pages call api fns; any Unauthorized thrown ANYWHERE routes to
  /admin/login (error boundary or catch-and-redirect helper `withAuth(fn, navigate)` — pick one,
  use it consistently). Login form: password input (type=password, autoFocus), submit →
  adminLogin → true ⇒ navigate to /admin/catalog; false ⇒ inline "nope." (and nothing else —
  no lockout UI, the server has the 500ms throttle).
- [ ] Tests: login success navigates; failure shows message + password never appears in DOM after
  submit; guard redirects on Unauthorized. Red → implement → green → commit
  `feat(web): admin shell, login, auth guard`.

### Task 5: admin catalog

**Files:** Create admin/Catalog.tsx + tests.

- Table/grid of AdminGame: artwork thumb, title, bundle, key_type, status badge
  (available=green, pending=amber, gifted=violet, ben_redeemed=slate, expired=red — snake_case
  values from serde), giftable chip, hidden toggle (switch). Search box filters title/bundle
  client-side. Toggle → adminSetHidden → ok:false shows the server message inline (the mid-claim
  409 case) + reverts the switch; success updates local state. Summary line: counts by status
  (client-computed from the loaded list).
- [ ] Tests: renders + filters; toggle success flips; toggle 409 reverts + shows message.
  Red → implement → green → commit `feat(web): admin catalog — badges, search, hidden toggles`.

### Task 6: admin links

**Files:** Create admin/Links.tsx + tests.

- Create form (label, claims_allowed number ≥1, optional expires_days) → adminCreateLink →
  prepend to list + show the FULL invite URL (`${window.location.origin}${url_path}`) with a
  copy button (this is the thing ben hands to a friend). List: label, used/allowed, created,
  expires, revoked chip; actions: copy URL, revoke (confirm popover → adminRevoke → refresh),
  expand → adminLinkClaims audit (game_id/state/gift_url-present-or-not — do NOT render the
  gift_url VALUE in admin, only "issued ✓": the URL is the friend's bearer secret, admin only
  needs to know it exists).
- [ ] Tests: create flow renders copyable full URL; revoke confirm calls api + refreshes; audit
  expand renders states WITHOUT any gift_url text in the DOM. Red → implement → green → commit
  `feat(web): admin links — create, copy invite, revoke, audit`.

### Task 7: admin ops + polish pass

**Files:** Create admin/Ops.tsx + tests; final App wiring.

- Cookie panel: password-type textarea-ish input (single line, type=password), paste + submit →
  adminPasteCookie → result copy: ok ⇒ "cookie validated ✓"; !ok && restored_previous ⇒ "that
  cookie failed validation — kept your previous one"; !ok && inconclusive ⇒ "humble unreachable —
  cookie state unknown, try again"; !ok else ⇒ "cookie failed validation". Input clears after
  submit regardless.
- Sync panel: sync-now button (disabled while in flight, shows games_written/orders_failed on
  completion) + status card from adminStatus (last run as relative time from last_run_epoch —
  compute client-side, render absolute ISO on hover; ok/cookie_ok badges; message line;
  game_counts chips). cookie_ok=false renders a red "humble session needs attention" banner at
  the top of EVERY admin page (lift status fetch into AdminApp context, refresh on nav).
- [ ] Tests: each cookie-result copy variant; sync button lifecycle; banner appears when
  cookie_ok=false. Red → implement → green → commit `feat(web): admin ops — cookie paste, sync, status banner`.

### Task 8: final whole-branch review + PR

- [ ] Final code reviewer (most capable model): spec §7/§10 fidelity, the friend-surface
  never-leaks rule, cookie-value DOM hygiene, api.ts as sole fetch site, a11y basics (labels,
  button semantics, dialog focus), test honesty (no assertion-free tests). Fix wave if needed.
- [ ] PR on `kitten/plan3-frontend`; CI green (both jobs); ready for ben.

## Self-Review (at write time)
1. Spec coverage: §7 token states (T3), §8 admin surface complete (T5/6/7 incl. per-link audit +
   cookie paste + sync-now + status), §10 frontend shape (grid/copy-counts/search/history/badges),
   region-lock disclaimer line — ADD: put the one-liner in ClaimDialog's gifted view ("keys may be
   region-locked") — folded into T3's dialog copy. Landing = §7 bare-root page (T3).
2. Placeholders: none — behavior contracts + exact copy strings given where they matter.
3. Type consistency: api.ts types transcribed from rust serde (snake_case retained); ClaimResult
   kinds match public-api's four outcome classes; CookieResult matches admin R6 shape.
