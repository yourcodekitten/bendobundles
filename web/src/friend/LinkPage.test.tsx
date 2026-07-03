import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { MemoryRouter, Route, Routes } from 'react-router-dom';
import { vi, describe, it, expect, beforeEach } from 'vitest';
import { LinkPage } from './LinkPage';
import type { LinkView } from '../api';

// Partial mock: fetch functions mocked, error classes REAL so instanceof
// checks in LinkPage exercise the production classes.
vi.mock('../api', async (importOriginal) => {
  const actual = await importOriginal<typeof import('../api')>();
  return {
    ...actual,
    fetchLink: vi.fn(),
    claimGame: vi.fn(),
  };
});

import { fetchLink, claimGame, NotFound, FetchFailed } from '../api';

function renderLinkPage(token = 'abc123') {
  return render(
    <MemoryRouter initialEntries={[`/l/${token}`]}>
      <Routes>
        <Route path="/l/:token" element={<LinkPage />} />
      </Routes>
    </MemoryRouter>,
  );
}

const baseLink: LinkView = {
  label: 'Test Bundle',
  claims_allowed: 3,
  claims_used: 1,
  active: true,
  state: 'active',
  games: [],
  claims: [],
};

describe('LinkPage', () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it('shows loading state initially', () => {
    // never resolves
    vi.mocked(fetchLink).mockImplementation(() => new Promise(() => {}));
    renderLinkPage();
    expect(screen.getByText(/loading/i)).toBeInTheDocument();
  });

  it('shows not-found view on NotFound (genuine 404)', async () => {
    vi.mocked(fetchLink).mockRejectedValue(new NotFound());
    renderLinkPage();
    await waitFor(() => {
      expect(screen.getByRole('heading', { name: /link not found/i })).toBeInTheDocument();
    });
  });

  it('shows retryable error view (NOT "link not found") on transient failure', async () => {
    vi.mocked(fetchLink).mockRejectedValue(new FetchFailed());
    renderLinkPage();
    await waitFor(() => {
      expect(screen.getByRole('heading', { name: /couldn't load this page/i })).toBeInTheDocument();
    });
    expect(screen.queryByText(/link not found/i)).not.toBeInTheDocument();
    expect(screen.getByRole('button', { name: /retry/i })).toBeInTheDocument();
  });

  it('retry after a transient failure loads the link', async () => {
    const user = userEvent.setup();
    vi.mocked(fetchLink)
      .mockRejectedValueOnce(new FetchFailed())
      .mockResolvedValueOnce({ ...baseLink });
    renderLinkPage();
    await waitFor(() => {
      expect(screen.getByRole('button', { name: /retry/i })).toBeInTheDocument();
    });

    await user.click(screen.getByRole('button', { name: /retry/i }));
    await waitFor(() => {
      expect(screen.getByText('Test Bundle')).toBeInTheDocument();
    });
  });

  it('refresh after a claim keeps the page visible (no full-page loading flash)', async () => {
    const user = userEvent.setup();
    const withGame: LinkView = {
      ...baseLink,
      games: [{ id: '1', title: 'Portal', bundle: 'B', key_type: 'steam', artwork_url: null }],
    };
    // First load resolves; the refreshTick refetch hangs forever — the old view must stay.
    vi.mocked(fetchLink)
      .mockResolvedValueOnce(withGame)
      .mockImplementation(() => new Promise(() => {}));
    vi.mocked(claimGame).mockResolvedValue({ kind: 'refused', message: 'already claimed' });

    renderLinkPage();
    await waitFor(() => {
      expect(screen.getByText('Portal')).toBeInTheDocument();
    });

    // Full claim round-trip: open dialog → confirm → refused → close (triggers refresh)
    await user.click(screen.getByRole('button', { name: /claim/i }));
    await user.click(screen.getByRole('button', { name: /confirm/i }));
    await waitFor(() => {
      expect(screen.getByText('already claimed')).toBeInTheDocument();
    });
    await user.click(screen.getByRole('button', { name: /close/i }));

    // Soft refresh: header and grid still there, no full-page spinner
    expect(screen.getByText('Test Bundle')).toBeInTheDocument();
    expect(screen.getByText('Portal')).toBeInTheDocument();
    expect(screen.queryByText(/^loading\.\.\.$/)).not.toBeInTheDocument();
  });

  it('shows loaded state with label and claim counts', async () => {
    vi.mocked(fetchLink).mockResolvedValue({ ...baseLink });
    renderLinkPage();
    await waitFor(() => {
      expect(screen.getByText('Test Bundle')).toBeInTheDocument();
      expect(screen.getByText(/1\/3 claims used/)).toBeInTheDocument();
    });
  });

  it('shows exhausted banner and disabled grid on state:"exhausted"', async () => {
    vi.mocked(fetchLink).mockResolvedValue({
      ...baseLink,
      active: false,
      state: 'exhausted',
      games: [{ id: '1', title: 'Portal', bundle: 'B', key_type: 'steam', artwork_url: null }],
    });
    renderLinkPage();
    await waitFor(() => {
      expect(screen.getByRole('alert')).toHaveTextContent("you've used all your claims");
    });
    // grid is visible but claim button is disabled
    expect(screen.getByRole('button', { name: /claim/i })).toBeDisabled();
  });

  it('shows revoked banner and no grid on state:"revoked"', async () => {
    vi.mocked(fetchLink).mockResolvedValue({
      ...baseLink,
      active: false,
      state: 'revoked',
      games: [],
    });
    renderLinkPage();
    await waitFor(() => {
      expect(screen.getByRole('alert')).toHaveTextContent(
        "this invite isn't active anymore — bug ben",
      );
    });
    // no claim button rendered
    expect(screen.queryByRole('button', { name: /claim/i })).not.toBeInTheDocument();
  });

  it('shows the same dead banner on state:"expired"', async () => {
    vi.mocked(fetchLink).mockResolvedValue({
      ...baseLink,
      active: false,
      state: 'expired',
      games: [],
    });
    renderLinkPage();
    await waitFor(() => {
      expect(screen.getByRole('alert')).toHaveTextContent("this invite isn't active anymore");
    });
  });

  it('banner follows state, not games.length: revoked + games present is still revoked', async () => {
    // The exact ambiguity the state field exists to kill: a revoked link that
    // (for any backend reason) still carries a games array must NOT render the
    // amber exhausted banner.
    vi.mocked(fetchLink).mockResolvedValue({
      ...baseLink,
      active: false,
      state: 'revoked',
      games: [{ id: '1', title: 'Celeste', bundle: 'B', key_type: 'steam', artwork_url: null }],
    });
    renderLinkPage();
    await waitFor(() => {
      expect(screen.getByRole('alert')).toHaveTextContent("this invite isn't active anymore");
    });
    expect(screen.queryByText(/used all your claims/i)).not.toBeInTheDocument();
    // dead link → grid hidden regardless of games payload
    expect(screen.queryByText('Celeste')).not.toBeInTheDocument();
  });
});
