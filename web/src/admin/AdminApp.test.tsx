import { act, render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { vi, describe, it, expect, beforeEach } from 'vitest';
import { MemoryRouter, Route, Routes, useNavigate } from 'react-router-dom';
import { AdminApp } from './AdminApp';
import { withAuth } from './withAuth';
import { Unauthorized } from '../api';
import type { StatusView } from '../api';

// Factory mock: keeps the real Unauthorized class so instanceof checks in withAuth
// still work, but replaces adminStatus with a vi.fn() we can control per test.
vi.mock('../api', async (importOriginal) => {
  const actual = await importOriginal<typeof import('../api')>();
  return {
    ...actual,
    adminStatus: vi.fn(),
  };
});
import { adminStatus } from '../api';

const noSyncStatus: StatusView = { sync: null, sync_run: null, game_counts: {} };

const cookieOkStatus: StatusView = {
  sync: { last_run_epoch: 1_000_000, ok: true, cookie_ok: true, games_written: 0, message: '' },
  sync_run: null,
  game_counts: {},
};

const cookieBadStatus: StatusView = {
  sync: { last_run_epoch: 1_000_000, ok: true, cookie_ok: false, games_written: 0, message: '' },
  sync_run: null,
  game_counts: {},
};

// Stub child that exercises withAuth directly — validates the guard mechanism
// without needing a real API call (Unauthorized is thrown synchronously in the mock).
function GuardedChild({ onAction }: { onAction: () => Promise<void> }) {
  const navigate = useNavigate();
  return (
    <button
      onClick={() => {
        void withAuth(onAction, navigate);
      }}
    >
      trigger
    </button>
  );
}

function renderAdminWithChild(child: React.ReactElement) {
  return render(
    <MemoryRouter initialEntries={['/admin/catalog']}>
      <Routes>
        <Route path="/admin" element={<AdminApp />}>
          <Route path="catalog" element={child} />
        </Route>
        <Route path="/admin/login" element={<div>login page</div>} />
      </Routes>
    </MemoryRouter>,
  );
}


describe('AdminApp layout', () => {
  beforeEach(() => {
    vi.mocked(adminStatus).mockResolvedValue(noSyncStatus);
  });

  it('renders nav links for catalog, links, and ops', () => {
    renderAdminWithChild(<div>catalog content</div>);
    expect(screen.getByRole('link', { name: /catalog/i })).toBeInTheDocument();
    expect(screen.getByRole('link', { name: /links/i })).toBeInTheDocument();
    expect(screen.getByRole('link', { name: /ops/i })).toBeInTheDocument();
  });

  it('renders child route content via Outlet', () => {
    renderAdminWithChild(<div>catalog content</div>);
    expect(screen.getByText('catalog content')).toBeInTheDocument();
  });
});

describe('withAuth guard', () => {
  beforeEach(() => {
    vi.mocked(adminStatus).mockResolvedValue(noSyncStatus);
  });

  it('redirects to /admin/login when api call throws Unauthorized', async () => {
    const user = userEvent.setup();
    const throwUnauthorized = vi.fn().mockRejectedValue(new Unauthorized());

    renderAdminWithChild(<GuardedChild onAction={throwUnauthorized} />);

    await user.click(screen.getByRole('button', { name: /trigger/i }));

    await waitFor(() => {
      expect(screen.getByText('login page')).toBeInTheDocument();
    });
  });

  it('does not redirect when the call succeeds', async () => {
    const user = userEvent.setup();
    const success = vi.fn().mockResolvedValue(undefined);

    renderAdminWithChild(<GuardedChild onAction={success} />);

    await user.click(screen.getByRole('button', { name: /trigger/i }));

    await waitFor(() => {
      expect(success).toHaveBeenCalledOnce();
    });
    expect(screen.queryByText('login page')).not.toBeInTheDocument();
  });
});

describe('AdminApp banner — humble session attention', () => {
  it('shows red banner when cookie_ok is false', async () => {
    vi.mocked(adminStatus).mockResolvedValue(cookieBadStatus);
    renderAdminWithChild(<div>catalog content</div>);

    await waitFor(() => {
      expect(
        screen.getByRole('alert', {
          name: /humble session needs attention/i,
        }),
      ).toBeInTheDocument();
    });
  });

  it('banner text is "humble session needs attention — paste a fresh cookie in ops"', async () => {
    vi.mocked(adminStatus).mockResolvedValue(cookieBadStatus);
    renderAdminWithChild(<div>content</div>);

    await waitFor(() => {
      expect(
        screen.getByText(/humble session needs attention — paste a fresh cookie in ops/i),
      ).toBeInTheDocument();
    });
  });

  it('does NOT show banner when cookie_ok is true', async () => {
    vi.mocked(adminStatus).mockResolvedValue(cookieOkStatus);
    renderAdminWithChild(<div>content</div>);

    // Wait for the status load to complete
    await waitFor(() => expect(adminStatus).toHaveBeenCalled());

    expect(screen.queryByRole('alert')).not.toBeInTheDocument();
  });

  it('does NOT show banner when sync is null (no status yet)', async () => {
    vi.mocked(adminStatus).mockResolvedValue(noSyncStatus);
    renderAdminWithChild(<div>content</div>);

    await waitFor(() => expect(adminStatus).toHaveBeenCalled());

    expect(screen.queryByRole('alert')).not.toBeInTheDocument();
  });
});

describe('AdminApp status polling while a sync runs', () => {
  const runningStatus: StatusView = {
    sync: null,
    sync_run: { started_epoch: 1_000_000, running: true },
    game_counts: {},
  };

  // No fake timers here — react's scheduler also runs on jsdom timers and wedges under a
  // fully-faked clock. Spying on setInterval and invoking the captured callback directly
  // asserts the same contract (poll registered at 5s cadence; each tick re-fetches)
  // deterministically.
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it('registers a 5s status poll while sync_run.running is true, and each tick re-fetches', async () => {
    // Fulfillment writes SyncState only at the END of a run — without this poll the card
    // would show the previous run's result for the whole backfill.
    const intervalSpy = vi.spyOn(globalThis, 'setInterval');
    vi.mocked(adminStatus).mockResolvedValue(runningStatus);
    renderAdminWithChild(<div>content</div>);

    await waitFor(() => expect(adminStatus).toHaveBeenCalledTimes(1));

    let pollCall: unknown[] | undefined;
    await waitFor(() => {
      pollCall = intervalSpy.mock.calls.find((c) => c[1] === 5000);
      expect(pollCall).toBeDefined();
    });

    // Fire the poll tick by hand — each tick must re-fetch status.
    await act(async () => {
      (pollCall![0] as () => void)();
    });
    await waitFor(() => expect(adminStatus).toHaveBeenCalledTimes(2));
  });

  it('does NOT register a poll when no sync is running', async () => {
    const intervalSpy = vi.spyOn(globalThis, 'setInterval');
    vi.mocked(adminStatus).mockResolvedValue(noSyncStatus);
    renderAdminWithChild(<div>content</div>);

    await waitFor(() => expect(adminStatus).toHaveBeenCalledTimes(1));

    expect(intervalSpy.mock.calls.find((c) => c[1] === 5000)).toBeUndefined();
  });
});

describe('/admin index redirect', () => {
  beforeEach(() => {
    vi.mocked(adminStatus).mockResolvedValue(noSyncStatus);
  });

  it('redirects /admin index to /admin/catalog', async () => {
    // Render with Navigate wired at the index (mirrors App.tsx configuration)
    const { Navigate } = await import('react-router-dom');
    render(
      <MemoryRouter initialEntries={['/admin']}>
        <Routes>
          <Route path="/admin" element={<AdminApp />}>
            <Route index element={<Navigate to="catalog" replace />} />
            <Route path="catalog" element={<div>catalog page</div>} />
          </Route>
          <Route path="/admin/login" element={<div>login page</div>} />
        </Routes>
      </MemoryRouter>,
    );

    await waitFor(() => {
      expect(screen.getByText('catalog page')).toBeInTheDocument();
    });
  });
});
