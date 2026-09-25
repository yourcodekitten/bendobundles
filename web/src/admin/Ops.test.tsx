import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { vi, describe, it, expect, beforeEach, afterEach } from 'vitest';
import { MemoryRouter, Outlet, Route, Routes } from 'react-router-dom';
import { Ops } from './Ops';
import type { StatusView } from '../api';

vi.mock('../api');
vi.mock('../steamIdentity');
import { adminSync, adminSteamIdentity, adminSetSteamIdentity, adminClearSteamIdentity, adminSteamOwned } from '../api';
import { adminStuckClaims, adminCompensateClaim } from '../api';
import { consumeReturnFragment, loadIdentity, beginConnect } from '../steamIdentity';

// Provides the Outlet context that Ops requires without needing the real AdminApp.
// Using <Outlet context={...} /> (react-router-dom) is the canonical approach
// when the component under test is a child route that calls useOutletContext().
// status is owned by the layout (AdminApp in prod) — Ops only renders it.
function TestLayout({
  status = null,
  refreshStatus,
}: {
  status?: StatusView | null;
  refreshStatus?: () => void;
}) {
  return <Outlet context={{ status, refreshStatus: refreshStatus ?? (() => {}) }} />;
}

function renderOps(opts: { status?: StatusView | null; refreshStatus?: () => void } = {}) {
  return render(
    <MemoryRouter initialEntries={['/admin/ops']}>
      <Routes>
        <Route
          path="/admin"
          element={<TestLayout status={opts.status} refreshStatus={opts.refreshStatus} />}
        >
          <Route path="ops" element={<Ops />} />
        </Route>
        <Route path="/admin/login" element={<div>login page</div>} />
      </Routes>
    </MemoryRouter>,
  );
}

describe('Ops', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    // Default: no fragment, no steam identity
    vi.mocked(consumeReturnFragment).mockReturnValue(null);
    vi.mocked(loadIdentity).mockReturnValue(null);
    vi.mocked(adminSteamIdentity).mockResolvedValue(null);
    vi.mocked(beginConnect).mockImplementation(() => {});
    // 🔴 Every one of this file's pre-existing tests now mounts the stuck-claims fetch.
    // Without this default the automock returns `undefined` and they all break — this line
    // is this task's blast-radius fix, not a convenience.
    vi.mocked(adminStuckClaims).mockResolvedValue({ claims: [], unreadable: [] });
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  describe('sync panel', () => {
    it('button is disabled and shows "syncing…" while the start request is in flight', async () => {
      const user = userEvent.setup();
      let resolveSync!: () => void;
      vi.mocked(adminSync).mockReturnValue(
        new Promise((r) => {
          resolveSync = r;
        }),
      );
      renderOps();

      await user.click(screen.getByRole('button', { name: /sync now/i }));

      // In-flight: button text changes to "syncing…" and is disabled
      await waitFor(() => {
        expect(screen.getByRole('button', { name: /syncing/i })).toBeDisabled();
      });
      resolveSync();
    });

    it('button STAYS locked after the 202 — a 202 means "queued", not "done"', async () => {
      // Regression guard for the concurrent-backfill hole: unlocking at the 202 (the old
      // .finally(setSyncing(false)) behavior) let a second click queue a second walk while
      // the first still ran. The button must stay locked while we wait for the run marker.
      const user = userEvent.setup();
      vi.mocked(adminSync).mockResolvedValue(undefined);
      renderOps();

      await user.click(screen.getByRole('button', { name: /sync now/i }));

      await waitFor(() => {
        expect(screen.getByText(/sync started — watch the status card/i)).toBeInTheDocument();
      });
      expect(screen.getByRole('button', { name: /syncing/i })).toBeDisabled();
    });

    it('button re-enables when the start request is rejected', async () => {
      const user = userEvent.setup();
      vi.mocked(adminSync).mockRejectedValue(new Error('couldn’t start sync — try again'));
      renderOps();

      await user.click(screen.getByRole('button', { name: /sync now/i }));

      await waitFor(() => {
        expect(screen.getByRole('button', { name: /sync now/i })).not.toBeDisabled();
      });
    });

    it('button is disabled without any click while the server reports a running sync', () => {
      renderOps({
        status: {
          sync: null,
          sync_run: { started_epoch: Math.floor(Date.now() / 1000) - 30, running: true },
          game_counts: {},
        },
      });

      expect(screen.getByRole('button', { name: /syncing/i })).toBeDisabled();
    });

    it('shows the already-running message on a 409 rejection', async () => {
      const user = userEvent.setup();
      vi.mocked(adminSync).mockRejectedValue(
        new Error('a sync is already running — watch the status card'),
      );
      renderOps();

      await user.click(screen.getByRole('button', { name: /sync now/i }));

      await waitFor(() => {
        expect(
          screen.getByText('a sync is already running — watch the status card'),
        ).toBeInTheDocument();
      });
    });

    it('shows the fire-and-forget "sync started" message on a 202', async () => {
      const user = userEvent.setup();
      vi.mocked(adminSync).mockResolvedValue(undefined);
      renderOps();

      await user.click(screen.getByRole('button', { name: /sync now/i }));

      await waitFor(() => {
        expect(screen.getByText(/sync started — watch the status card/i)).toBeInTheDocument();
      });
    });

    it('shows the start-failure message when the sync request is rejected', async () => {
      const user = userEvent.setup();
      vi.mocked(adminSync).mockRejectedValue(new Error('couldn’t start sync — try again'));
      renderOps();

      await user.click(screen.getByRole('button', { name: /sync now/i }));

      await waitFor(() => {
        expect(screen.getByText(/couldn’t start sync — try again/i)).toBeInTheDocument();
      });
    });
  });

  describe('status card', () => {
    it('shows relative time from last_run_epoch', async () => {
      const nowMs = 1_751_664_000_000;
      vi.spyOn(Date, 'now').mockReturnValue(nowMs);
      const nowSec = nowMs / 1000;
      renderOps({
        status: {
          sync: {
            last_run_epoch: nowSec - 180, // 3 minutes ago
            ok: true,
            cookie_ok: true,
            games_written: 0,
            message: '',
          },
          sync_run: null,
          game_counts: {},
        },
      });

      await waitFor(() => {
        expect(screen.getByText('3m ago')).toBeInTheDocument();
      });
    });

    it('title attr on relative-time element is ISO string of epoch', async () => {
      const epoch = 1_751_664_000;
      renderOps({
        status: {
          sync: {
            last_run_epoch: epoch,
            ok: true,
            cookie_ok: true,
            games_written: 0,
            message: '',
          },
          sync_run: null,
          game_counts: {},
        },
      });

      await waitFor(() => {
        const el = screen.getByTitle(new Date(epoch * 1000).toISOString());
        expect(el).toBeInTheDocument();
      });
    });

    it('shows ok ✓ and cookie ✓ badges when both true', async () => {
      renderOps({
        status: {
          sync: {
            last_run_epoch: Math.floor(Date.now() / 1000) - 60,
            ok: true,
            cookie_ok: true,
            games_written: 0,
            message: '',
          },
          sync_run: null,
          game_counts: {},
        },
      });

      await waitFor(() => {
        expect(screen.getByText('ok ✓')).toBeInTheDocument();
        expect(screen.getByText('cookie ✓')).toBeInTheDocument();
      });
    });

    it('shows ok ✗ and cookie ✗ badges when both false', async () => {
      renderOps({
        status: {
          sync: {
            last_run_epoch: Math.floor(Date.now() / 1000) - 60,
            ok: false,
            cookie_ok: false,
            games_written: 0,
            message: 'auth failed',
          },
          sync_run: null,
          game_counts: {},
        },
      });

      await waitFor(() => {
        expect(screen.getByText('ok ✗')).toBeInTheDocument();
        expect(screen.getByText('cookie ✗')).toBeInTheDocument();
      });
    });

    it('shows game_counts chips for each entry', async () => {
      renderOps({
        status: {
          sync: {
            last_run_epoch: Math.floor(Date.now() / 1000) - 60,
            ok: true,
            cookie_ok: true,
            games_written: 0,
            message: '',
          },
          sync_run: null,
          game_counts: { available: 10, gifted: 5 },
        },
      });

      await waitFor(() => {
        expect(screen.getByText('available: 10')).toBeInTheDocument();
        expect(screen.getByText('gifted: 5')).toBeInTheDocument();
      });
    });

    it('clamps future epochs to "just now" — clock skew must never render "-3s ago"', async () => {
      const nowMs = 1_751_664_000_000;
      vi.spyOn(Date, 'now').mockReturnValue(nowMs);
      renderOps({
        status: {
          sync: {
            last_run_epoch: nowMs / 1000 + 3, // server clock 3s ahead
            ok: true,
            cookie_ok: true,
            games_written: 0,
            message: '',
          },
          sync_run: null,
          game_counts: {},
        },
      });

      await waitFor(() => {
        expect(screen.getByText('just now')).toBeInTheDocument();
      });
      expect(screen.queryByText(/-\d+s ago/)).not.toBeInTheDocument();
    });

    it('shows "never" when sync is null', async () => {
      renderOps({
        status: {
          sync: null,
          sync_run: null,
          game_counts: {},
        },
      });

      await waitFor(() => {
        expect(screen.getByText('never')).toBeInTheDocument();
      });
    });

    it('shows the running line while a sync run is live', () => {
      const nowMs = 1_751_664_000_000;
      vi.spyOn(Date, 'now').mockReturnValue(nowMs);
      renderOps({
        status: {
          sync: null,
          sync_run: { started_epoch: nowMs / 1000 - 120, running: true },
          game_counts: {},
        },
      });

      expect(screen.getByText(/sync running — started 2m ago/)).toBeInTheDocument();
    });

    it('surfaces a dead run (marker present, not running) — a dropped backfill must not look idle', () => {
      // This is the observability half of the fire-and-forget contract: if fulfillment
      // crashes/times out before reporting, the leftover marker is the ONLY evidence.
      const nowMs = 1_751_664_000_000;
      vi.spyOn(Date, 'now').mockReturnValue(nowMs);
      renderOps({
        status: {
          sync: null,
          sync_run: { started_epoch: nowMs / 1000 - 1200, running: false },
          game_counts: {},
        },
      });

      expect(screen.getByText(/started 20m ago but never\s+reported/)).toBeInTheDocument();
      expect(screen.getByText(/likely failed; safe to retry/)).toBeInTheDocument();
    });
  });

  describe('outlet context — refreshStatus callback', () => {
    it('calls refreshStatus after sync is accepted', async () => {
      const user = userEvent.setup();
      const refreshStatus = vi.fn();
      vi.mocked(adminSync).mockResolvedValue(undefined);
      renderOps({ refreshStatus });

      await user.click(screen.getByRole('button', { name: /sync now/i }));

      await waitFor(() => {
        expect(refreshStatus).toHaveBeenCalled();
      });
    });
  });

  // ── steam connect panel ─────────────────────────────────────────────────────

  describe('steam connect panel', () => {
    it('shows connect button when no steam identity is configured', async () => {
      vi.mocked(adminSteamIdentity).mockResolvedValue(null);
      renderOps();
      await waitFor(() =>
        expect(screen.getByRole('button', { name: /connect steam/i })).toBeInTheDocument(),
      );
    });

    it('shows persona chip and disconnect button when identity is set', async () => {
      vi.mocked(adminSteamIdentity).mockResolvedValue('76561198000000001');
      vi.mocked(loadIdentity).mockReturnValue({
        steamid: '76561198000000001',
        persona: 'TestUser',
        owned: [],
        fetched_at: 0,
      });
      renderOps();
      await waitFor(() => expect(screen.getByText('TestUser')).toBeInTheDocument());
      expect(screen.getByRole('button', { name: /disconnect/i })).toBeInTheDocument();
    });

    it('calls adminSetSteamIdentity when steam fragment arrives on mount', async () => {
      vi.mocked(consumeReturnFragment).mockReturnValue({
        steamid: '76561198000000001',
        persona: 'Alice',
      });
      vi.mocked(adminSteamOwned).mockResolvedValue([]);
      vi.mocked(adminSetSteamIdentity).mockResolvedValue(undefined);
      renderOps();
      await waitFor(() => expect(adminSetSteamIdentity).toHaveBeenCalledWith('76561198000000001'));
    });

    it('calls adminClearSteamIdentity and removes local identity on disconnect', async () => {
      const user = userEvent.setup();
      vi.mocked(adminSteamIdentity).mockResolvedValue('76561198000000001');
      vi.mocked(loadIdentity).mockReturnValue({
        steamid: '76561198000000001',
        persona: 'TestUser',
        owned: [],
        fetched_at: 0,
      });
      vi.mocked(adminClearSteamIdentity).mockResolvedValue(undefined);
      renderOps();
      await waitFor(() => expect(screen.getByText('TestUser')).toBeInTheDocument());
      await user.click(screen.getByRole('button', { name: /disconnect/i }));
      await waitFor(() => expect(adminClearSteamIdentity).toHaveBeenCalled());
    });

    // C1 contract: Ops must initiate connect with exactly /admin/ops so the server-side
    // ctx_is_allowed allowlist accepts it. If this ctx ever drifts the feature silently breaks.
    it('initiates connect with ctx=/admin/ops — must match server ctx_is_allowed allowlist', async () => {
      const user = userEvent.setup();
      vi.mocked(adminSteamIdentity).mockResolvedValue(null);
      renderOps();
      await waitFor(() =>
        expect(screen.getByRole('button', { name: /connect steam/i })).toBeInTheDocument(),
      );
      await user.click(screen.getByRole('button', { name: /connect steam/i }));
      expect(beginConnect).toHaveBeenCalledWith('/admin/ops');
    });

    // FIX 3: error fragment from consumeReturnFragment must show an error message.
    // Before FIX 3, the 'error' case fell through silently — no message shown.
    // Recovery contract (regression guard): error branch must NOT early-return before identity
    // load — steamIdState must resolve to null so the connect button appears for retry.
    it('shows error message AND connect button when consumeReturnFragment returns verify_failed (FIX 3 + recovery)', async () => {
      vi.mocked(consumeReturnFragment).mockReturnValue({ error: 'verify_failed' });
      vi.mocked(adminSteamIdentity).mockResolvedValue(null);
      renderOps();
      // (a) error message renders
      await waitFor(() =>
        expect(screen.getByRole('status')).toBeInTheDocument(),
      );
      expect(screen.getByRole('status').textContent).toMatch(/verify/i);
      // (b) identity load was still invoked (not skipped by early return)
      expect(adminSteamIdentity).toHaveBeenCalled();
      // (c) connect button appears — steamIdState resolved to null, not stuck on "loading…"
      await waitFor(() =>
        expect(screen.getByRole('button', { name: /connect steam/i })).toBeInTheDocument(),
      );
    });

    it('shows error message when consumeReturnFragment returns steam_unreachable (FIX 3)', async () => {
      vi.mocked(consumeReturnFragment).mockReturnValue({ error: 'steam_unreachable' });
      renderOps();
      await waitFor(() =>
        expect(screen.getByRole('status')).toBeInTheDocument(),
      );
      expect(screen.getByRole('status').textContent).toMatch(/unavailable|unreachable/i);
    });
  });

  describe('stuck claims', () => {
    const SELF_ROW = {
      claim_id: 'c-old', game_id: 'gk:a', link_token: 'SELF',
      is_self: true, pending_since: '2026-07-06T00:00:00Z', age_hours: 1920,
    };
    const FRIEND_ROW = {
      claim_id: 'c-fr', game_id: 'gk:b', link_token: 'tok-friend',
      is_self: false, pending_since: '2026-09-24T00:00:00Z', age_hours: 30,
    };

    it('renders nothing-to-see when there are no stuck claims', async () => {
      vi.mocked(adminStuckClaims).mockResolvedValue({ claims: [], unreadable: [] });
      renderOps();
      expect(await screen.findByText(/no stuck claims/i)).toBeInTheDocument();
    });

    // ── #244: a partial answer has to announce itself ────────────────────────
    it('says the list is incomplete, and names the rows, when some could not be read', async () => {
      vi.mocked(adminStuckClaims).mockResolvedValue({
        claims: [FRIEND_ROW],
        unreadable: [{ pk: 'LINK#tok', sk: 'CLAIM#c-bad', why: 'bad body json' }],
      });
      renderOps();

      const banner = await screen.findByTestId('stuck-unreadable');
      expect(banner).toHaveTextContent(/could not be read/i);
      expect(banner).toHaveTextContent(/incomplete/i);
      // the KEY, so an operator can go look at the item rather than guess at it
      expect(banner).toHaveTextContent('CLAIM#c-bad');
      expect(banner).toHaveTextContent('bad body json');
      // and the readable row is still there — the whole point is BESIDE, not INSTEAD
      expect(await screen.findByTestId('claim-kind-c-fr')).toBeInTheDocument();
    });

    // The control. Without it, a banner that rendered unconditionally would pass the test above
    // while telling an operator every healthy board is incomplete — and a warning that is always
    // on is a warning nobody reads.
    it('shows no incompleteness banner when every row read', async () => {
      vi.mocked(adminStuckClaims).mockResolvedValue({ claims: [FRIEND_ROW], unreadable: [] });
      renderOps();
      expect(await screen.findByTestId('claim-kind-c-fr')).toBeInTheDocument();
      expect(screen.queryByTestId('stuck-unreadable')).not.toBeInTheDocument();
    });

    // "no stuck claims" is FALSE when a row exists that could not be read: the unreadable one may
    // be the very claim the operator came for.
    it('does not claim there are no stuck claims when a row was unreadable', async () => {
      vi.mocked(adminStuckClaims).mockResolvedValue({
        claims: [],
        unreadable: [{ pk: 'LINK#tok', sk: 'CLAIM#c-bad', why: 'missing body' }],
      });
      renderOps();
      expect(await screen.findByText(/no READABLE stuck claims/i)).toBeInTheDocument();
    });

    it('badges a self row and a friend row differently', async () => {
      vi.mocked(adminStuckClaims).mockResolvedValue({ claims: [SELF_ROW, FRIEND_ROW], unreadable: [] });
      renderOps();

      // BY TEST-ID, NOT BY TEXT. getByText(/self/i) matches the link_token "SELF" in the row
      // whether a badge exists or not — a vacuous assertion.
      expect(await screen.findByTestId('claim-kind-c-old')).toHaveTextContent('self');
      expect(await screen.findByTestId('claim-kind-c-fr')).toHaveTextContent('friend');

      // BY CONTENT, not presence: toBeInTheDocument() passes on an empty span and on one
      // rendering the wrong field entirely.
      expect(await screen.findByTestId('claim-since-c-old')).toHaveTextContent('2026-07-06');
      expect(await screen.findByTestId('claim-token-c-fr')).toHaveTextContent('tok-friend');
    });

    it('needs two presses and sends the CLAIM id, not the game id', async () => {
      const user = userEvent.setup();
      vi.mocked(adminStuckClaims).mockResolvedValue({ claims: [FRIEND_ROW], unreadable: [] });
      vi.mocked(adminCompensateClaim).mockResolvedValue(undefined);
      renderOps();

      await user.click(await screen.findByTestId('compensate-c-fr'));
      expect(adminCompensateClaim).not.toHaveBeenCalled();   // arming is not acting

      await user.click(await screen.findByTestId('confirm-c-fr'));
      // ASSERT THE ARGUMENTS: a component sending game_id would still have "called" it.
      await waitFor(() =>
        expect(adminCompensateClaim).toHaveBeenCalledWith('c-fr', 'tok-friend'),
      );
    });
  });
});
