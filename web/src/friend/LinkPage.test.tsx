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
    steamOwnedForLink: vi.fn(),
    fetchGameDetail: vi.fn(),
  };
});

vi.mock('../steamIdentity');

import { fetchLink, claimGame, NotFound, FetchFailed, steamOwnedForLink, fetchGameDetail } from '../api';
import { clearGameDetailCache } from '../GameDetailModal';
import { consumeReturnFragment, loadIdentity, beginConnect } from '../steamIdentity';

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
  state: 'active',
  games: [],
  claims: [],
};

describe('LinkPage', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    clearGameDetailCache();
    // Default steam state: no fragment, no stored identity
    vi.mocked(consumeReturnFragment).mockReturnValue(null);
    vi.mocked(loadIdentity).mockReturnValue(null);
    vi.mocked(beginConnect).mockImplementation(() => {});
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
      games: [{ id: '1', title: 'Portal', bundle: 'B', key_type: 'steam', artwork_url: null, steam_app_id: null }],
    };
    // First load resolves; the refreshTick refetch hangs forever — the old view must stay.
    vi.mocked(fetchLink)
      .mockResolvedValueOnce(withGame)
      .mockImplementation(() => new Promise(() => {}));
    vi.mocked(fetchGameDetail).mockResolvedValue({ game: withGame.games[0]!, steam: null });
    vi.mocked(claimGame).mockResolvedValue({ kind: 'refused', message: 'already claimed' });

    renderLinkPage();
    await waitFor(() => {
      expect(screen.getByText('Portal')).toBeInTheDocument();
    });

    // Full claim round-trip: details → modal claim → dialog confirm → refused → close (refresh)
    await user.click(screen.getByRole('button', { name: /details/i }));
    await waitFor(() => {
      expect(screen.getByRole('button', { name: /^claim$/i })).toBeInTheDocument();
    });
    await user.click(screen.getByRole('button', { name: /^claim$/i }));
    await user.click(screen.getByRole('button', { name: /confirm/i }));
    await waitFor(() => {
      expect(screen.getByText('already claimed')).toBeInTheDocument();
    });
    await user.click(screen.getByRole('button', { name: /close/i }));

    // Soft refresh: header and grid still there, no full-page spinner.
    // (waitFor: the dialog-box title types in, so the full label lands async.)
    await waitFor(() => {
      expect(screen.getByText('Test Bundle')).toBeInTheDocument();
    });
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

  it('shows exhausted banner; grid browsable but the modal claim is disabled', async () => {
    const user = userEvent.setup();
    const game = { id: '1', title: 'Portal', bundle: 'B', key_type: 'steam', artwork_url: null, steam_app_id: null };
    vi.mocked(fetchLink).mockResolvedValue({
      ...baseLink,
      state: 'exhausted',
      games: [game],
    });
    vi.mocked(fetchGameDetail).mockResolvedValue({ game, steam: null });
    renderLinkPage();
    await waitFor(() => {
      expect(screen.getByRole('alert')).toHaveTextContent("you've used all your claims");
    });
    // the grid never claims directly — details still browsable, modal claim disabled
    expect(screen.queryByRole('button', { name: /^claim$/i })).not.toBeInTheDocument();
    await user.click(screen.getByRole('button', { name: /details/i }));
    await waitFor(() => {
      expect(screen.getByRole('button', { name: /^claim$/i })).toBeDisabled();
    });
  });

  it('shows revoked banner and no grid on state:"revoked"', async () => {
    vi.mocked(fetchLink).mockResolvedValue({
      ...baseLink,
      state: 'revoked',
      games: [],
    });
    renderLinkPage();
    await waitFor(() => {
      expect(screen.getByRole('alert')).toHaveTextContent(
        "this invite isn't active anymore — bug ben",
      );
    });
    // no grid rendered at all — neither details nor claim affordances
    expect(screen.queryByRole('button', { name: /details/i })).not.toBeInTheDocument();
    expect(screen.queryByRole('button', { name: /claim/i })).not.toBeInTheDocument();
  });

  it('shows the same dead banner on state:"expired"', async () => {
    vi.mocked(fetchLink).mockResolvedValue({
      ...baseLink,
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
      state: 'revoked',
      games: [{ id: '1', title: 'Celeste', bundle: 'B', key_type: 'steam', artwork_url: null, steam_app_id: null }],
    });
    renderLinkPage();
    await waitFor(() => {
      expect(screen.getByRole('alert')).toHaveTextContent("this invite isn't active anymore");
    });
    expect(screen.queryByText(/used all your claims/i)).not.toBeInTheDocument();
    // dead link → grid hidden regardless of games payload
    expect(screen.queryByText('Celeste')).not.toBeInTheDocument();
  });

  // ── steam identity ──────────────────────────────────────────────────────────

  describe('steam identity', () => {
    it('shows connect button when no steam identity', async () => {
      vi.mocked(fetchLink).mockResolvedValue(baseLink);
      renderLinkPage();
      await waitFor(() => expect(screen.getByText('Test Bundle')).toBeInTheDocument());
      expect(screen.getByRole('button', { name: /connect to steam/i })).toBeInTheDocument();
    });

    it('shows persona chip and disconnect button when identity is stored', async () => {
      vi.mocked(fetchLink).mockResolvedValue(baseLink);
      vi.mocked(loadIdentity).mockReturnValue({
        steamid: '76561198000000001',
        persona: 'Alice',
        owned: [],
        fetched_at: 0,
      });
      renderLinkPage();
      await waitFor(() => expect(screen.getByText('Alice')).toBeInTheDocument());
      expect(screen.getByRole('button', { name: /disconnect/i })).toBeInTheDocument();
    });

    it('shows "you own this" pill on a card whose steam_app_id is in the owned set', async () => {
      vi.mocked(fetchLink).mockResolvedValue({
        ...baseLink,
        games: [
          {
            id: '1',
            title: 'Portal',
            bundle: 'B',
            key_type: 'steam',
            artwork_url: null,
            steam_app_id: 420,
          },
        ],
      });
      vi.mocked(loadIdentity).mockReturnValue({
        steamid: '123',
        persona: 'Alice',
        owned: [420],
        fetched_at: 0,
      });
      renderLinkPage();
      await waitFor(() => expect(screen.getByText('Portal')).toBeInTheDocument());
      expect(screen.getByText(/you own this/i)).toBeInTheDocument();
    });

    it('does NOT show "you own this" pill when steam_app_id is not in owned set', async () => {
      vi.mocked(fetchLink).mockResolvedValue({
        ...baseLink,
        games: [
          {
            id: '1',
            title: 'Portal',
            bundle: 'B',
            key_type: 'steam',
            artwork_url: null,
            steam_app_id: 420,
          },
        ],
      });
      vi.mocked(loadIdentity).mockReturnValue({
        steamid: '123',
        persona: 'Alice',
        owned: [730],
        fetched_at: 0,
      });
      renderLinkPage();
      await waitFor(() => expect(screen.getByText('Portal')).toBeInTheDocument());
      expect(screen.queryByText(/you own this/i)).not.toBeInTheDocument();
    });

    it('fetches owned on steam fragment, saves identity, shows persona chip', async () => {
      vi.mocked(fetchLink).mockResolvedValue(baseLink);
      vi.mocked(consumeReturnFragment).mockReturnValue({
        steamid: '76561198000000001',
        persona: 'Alice',
      });
      vi.mocked(steamOwnedForLink).mockResolvedValue([420, 730]);
      renderLinkPage();
      await waitFor(() => expect(screen.getByText('Alice')).toBeInTheDocument());
    });

    it('shows privacy message when steamOwnedForLink returns "private"', async () => {
      vi.mocked(fetchLink).mockResolvedValue(baseLink);
      vi.mocked(consumeReturnFragment).mockReturnValue({
        steamid: '76561198000000001',
        persona: 'Alice',
      });
      vi.mocked(steamOwnedForLink).mockResolvedValue('private');
      renderLinkPage();
      // The <em> tag splits the text node — check the em element directly
      await waitFor(() =>
        expect(screen.getByText('game details')).toBeInTheDocument(),
      );
      // And the surrounding paragraph contains the privacy copy
      expect(screen.getByText(/couldn't read your library/i)).toBeInTheDocument();
    });

    it('shows error message on verify_failed fragment', async () => {
      vi.mocked(fetchLink).mockResolvedValue(baseLink);
      vi.mocked(consumeReturnFragment).mockReturnValue({ error: 'verify_failed' });
      renderLinkPage();
      await waitFor(() => expect(screen.getByText(/couldn't verify/i)).toBeInTheDocument());
    });

    it('shows error message on steam_unreachable fragment', async () => {
      vi.mocked(fetchLink).mockResolvedValue(baseLink);
      vi.mocked(consumeReturnFragment).mockReturnValue({ error: 'steam_unreachable' });
      renderLinkPage();
      await waitFor(() => expect(screen.getByText(/steam.*unavailable|unavailable.*steam/i)).toBeInTheDocument());
    });
  });
});
