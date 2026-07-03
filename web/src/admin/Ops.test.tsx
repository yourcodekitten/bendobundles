import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { vi, describe, it, expect, beforeEach, afterEach } from 'vitest';
import { MemoryRouter, Outlet, Route, Routes } from 'react-router-dom';
import { Ops } from './Ops';
import type { StatusView } from '../api';

vi.mock('../api');
import { adminPasteCookie, adminSync } from '../api';

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
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  describe('cookie panel — result copy variants', () => {
    it('shows "cookie validated ✓" when result.ok is true', async () => {
      const user = userEvent.setup();
      vi.mocked(adminPasteCookie).mockResolvedValue({ ok: true });
      renderOps();

      await user.type(screen.getByLabelText(/humble session cookie/i), 'valid-cookie');
      await user.click(screen.getByRole('button', { name: /submit/i }));

      await waitFor(() => {
        expect(screen.getByText('cookie validated ✓')).toBeInTheDocument();
      });
    });

    it('shows "that cookie failed validation — kept your previous one" when !ok && restored_previous', async () => {
      const user = userEvent.setup();
      vi.mocked(adminPasteCookie).mockResolvedValue({ ok: false, restored_previous: true });
      renderOps();

      await user.type(screen.getByLabelText(/humble session cookie/i), 'bad-cookie');
      await user.click(screen.getByRole('button', { name: /submit/i }));

      await waitFor(() => {
        expect(
          screen.getByText('that cookie failed validation — kept your previous one'),
        ).toBeInTheDocument();
      });
    });

    it('shows "humble unreachable — cookie state unknown, try again" when !ok && inconclusive', async () => {
      const user = userEvent.setup();
      vi.mocked(adminPasteCookie).mockResolvedValue({ ok: false, inconclusive: true });
      renderOps();

      await user.type(screen.getByLabelText(/humble session cookie/i), 'some-cookie');
      await user.click(screen.getByRole('button', { name: /submit/i }));

      await waitFor(() => {
        expect(
          screen.getByText('humble unreachable — cookie state unknown, try again'),
        ).toBeInTheDocument();
      });
    });

    it('shows "cookie failed validation" when !ok with no other flags set', async () => {
      const user = userEvent.setup();
      vi.mocked(adminPasteCookie).mockResolvedValue({ ok: false });
      renderOps();

      await user.type(screen.getByLabelText(/humble session cookie/i), 'bad-cookie');
      await user.click(screen.getByRole('button', { name: /submit/i }));

      await waitFor(() => {
        expect(screen.getByText('cookie failed validation')).toBeInTheDocument();
      });
    });

    it('clears input after submit and value is absent from DOM', async () => {
      const user = userEvent.setup();
      vi.mocked(adminPasteCookie).mockResolvedValue({ ok: true });
      renderOps();

      const input = screen.getByLabelText(/humble session cookie/i) as HTMLInputElement;
      await user.type(input, 'supersecretcookievalue');
      expect(input.value).toBe('supersecretcookievalue');

      await user.click(screen.getByRole('button', { name: /submit/i }));

      await waitFor(() => {
        expect(screen.getByText('cookie validated ✓')).toBeInTheDocument();
      });

      // Field must be cleared
      expect(input.value).toBe('');
      // Value must not appear anywhere in the DOM (not echoed into text, attrs, etc.)
      expect(document.body.innerHTML).not.toContain('supersecretcookievalue');
    });
  });

  describe('sync panel', () => {
    it('button is disabled and shows "syncing…" while in flight, re-enables after', async () => {
      const user = userEvent.setup();
      let resolveSync!: (v: { games_written: number; orders_failed: number }) => void;
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

      // Resolve the pending sync
      resolveSync({ games_written: 5, orders_failed: 0 });

      // After completion: button re-enabled with original label
      await waitFor(() => {
        expect(screen.getByRole('button', { name: /sync now/i })).not.toBeDisabled();
      });
    });

    it('shows "wrote N games, M orders failed" after successful sync', async () => {
      const user = userEvent.setup();
      vi.mocked(adminSync).mockResolvedValue({ games_written: 42, orders_failed: 3 });
      renderOps();

      await user.click(screen.getByRole('button', { name: /sync now/i }));

      await waitFor(() => {
        expect(screen.getByText('wrote 42 games, 3 orders failed')).toBeInTheDocument();
      });
    });

    it('shows "sync failed — check status panel" on sync error', async () => {
      const user = userEvent.setup();
      vi.mocked(adminSync).mockRejectedValue(new Error('sync failed — check status panel'));
      renderOps();

      await user.click(screen.getByRole('button', { name: /sync now/i }));

      await waitFor(() => {
        expect(screen.getByText('sync failed — check status panel')).toBeInTheDocument();
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
          game_counts: { available: 10, gifted: 5 },
        },
      });

      await waitFor(() => {
        expect(screen.getByText('available: 10')).toBeInTheDocument();
        expect(screen.getByText('gifted: 5')).toBeInTheDocument();
      });
    });

    it('shows "never" when sync is null', async () => {
      renderOps({
        status: {
          sync: null,
          game_counts: {},
        },
      });

      await waitFor(() => {
        expect(screen.getByText('never')).toBeInTheDocument();
      });
    });
  });

  describe('outlet context — refreshStatus callback', () => {
    it('calls refreshStatus after cookie submit', async () => {
      const user = userEvent.setup();
      const refreshStatus = vi.fn();
      vi.mocked(adminPasteCookie).mockResolvedValue({ ok: true });
      renderOps({ refreshStatus });

      await user.type(screen.getByLabelText(/humble session cookie/i), 'cookie');
      await user.click(screen.getByRole('button', { name: /submit/i }));

      await waitFor(() => {
        expect(refreshStatus).toHaveBeenCalled();
      });
    });

    it('calls refreshStatus after sync completes', async () => {
      const user = userEvent.setup();
      const refreshStatus = vi.fn();
      vi.mocked(adminSync).mockResolvedValue({ games_written: 1, orders_failed: 0 });
      renderOps({ refreshStatus });

      await user.click(screen.getByRole('button', { name: /sync now/i }));

      await waitFor(() => {
        expect(refreshStatus).toHaveBeenCalled();
      });
    });
  });
});
