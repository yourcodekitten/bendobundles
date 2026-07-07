import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { vi, describe, it, expect, beforeEach, afterEach } from 'vitest';
import { MemoryRouter, Outlet, Route, Routes } from 'react-router-dom';
import { Ops } from './Ops';
import type { StatusView } from '../api';

vi.mock('../api');
vi.mock('../steamIdentity');
import { adminSync, adminSteamIdentity, adminSetSteamIdentity, adminClearSteamIdentity, adminSteamOwned } from '../api';
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
  });
});
