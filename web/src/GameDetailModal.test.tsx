// Hoisted so the vi.mock factory can capture the callback reference
const hlsCbCapture = vi.hoisted(() => {
  const ref: { errorCb: ((event: string, data: { fatal: boolean }) => void) | null } = {
    errorCb: null,
  };
  return ref;
});

vi.mock('hls.js', () => {
  class MockHls {
    static Events = { ERROR: 'hlsError' };
    loadSource = vi.fn();
    attachMedia = vi.fn();
    on = vi.fn().mockImplementation(
      (event: string, cb: (event: string, data: { fatal: boolean }) => void) => {
        if (event === 'hlsError') hlsCbCapture.errorCb = cb;
      },
    );
    destroy = vi.fn();
  }
  return { default: MockHls };
});

vi.mock('./api');

import { render, screen, waitFor, act } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { vi, describe, it, expect, beforeEach } from 'vitest';
import { GameDetailModal, clearGameDetailCache } from './GameDetailModal';
import type { GameView, AdminGame } from './api';
import { fetchGameDetail, adminGameDetail, Unauthorized } from './api';
import { withAuth } from './admin/withAuth';

// ── Fixtures based on Stardew Valley captures (413150) ────────────────────────

const steamDetailFixture = {
  app_id: 413150,
  name: 'Stardew Valley',
  developers: ['ConcernedApe'],
  publishers: ['ConcernedApe'],
  genres: ['Indie', 'RPG', 'Simulation'],
  release_date: 'Feb 26, 2016',
  short_description:
    "You've inherited your grandfather's old farm plot in Stardew Valley. Armed with hand-me-down tools and a few coins, you set out to begin your new life.",
  header_image:
    'https://shared.akamai.steamstatic.com/store_item_assets/steam/apps/413150/header.jpg',
  video_hls_url:
    'https://video.akamai.steamstatic.com/store_trailers/413150/336433/hls_264_master.m3u8',
  video_thumbnail:
    'https://shared.akamai.steamstatic.com/store_item_assets/steam/apps/256815967/movie.293x165.jpg',
};

const overallFixture = {
  desc: 'Overwhelmingly Positive',
  total_positive: 455578,
  total_negative: 5303,
  total_reviews: 460881,
};

const recentFixture = {
  percent_positive: 97,
  count: 8916,
};

const friendGame: GameView = {
  id: 'gx:stardew',
  title: 'Stardew Valley',
  bundle: 'Indie Gems Bundle',
  key_type: 'steam',
  artwork_url: 'https://example.com/stardew.jpg',
  steam_app_id: 413150,
};

const adminGame: AdminGame = {
  id: 'gx:stardew',
  title: 'Stardew Valley',
  bundle: 'Indie Gems Bundle',
  key_type: 'steam',
  giftable: true,
  hidden: false,
  status: 'available',
  claim_id: null,
  artwork_url: 'https://example.com/stardew.jpg',
  requires_choice: false,
  steam_app_id: 413150,
  owned_by_ben: false,
};

// ── Shared render helpers ─────────────────────────────────────────────────────

function friendLoadDetail(gameId: string) {
  return fetchGameDetail('tok123', gameId);
}

function adminLoadDetail(gameId: string) {
  return adminGameDetail(gameId);
}

describe('GameDetailModal', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    hlsCbCapture.errorCb = null;
    clearGameDetailCache();
  });

  it('renders full detail variant from a mocked response', async () => {
    vi.mocked(fetchGameDetail).mockResolvedValue({
      game: friendGame,
      steam: { detail: steamDetailFixture, overall: overallFixture, recent: recentFixture },
    });

    render(
      <GameDetailModal
        mount="friend"
        token="tok123"
        game={friendGame}
        active={true}
        onClaim={vi.fn()}
        onClose={vi.fn()}
        loadDetail={friendLoadDetail}
      />,
    );

    await waitFor(() => expect(screen.getByText('Stardew Valley')).toBeInTheDocument());
    // dev / pub line
    expect(screen.getByText(/ConcernedApe/)).toBeInTheDocument();
    // release date
    expect(screen.getByText(/Feb 26, 2016/)).toBeInTheDocument();
    // genre chip
    expect(screen.getByText('Indie')).toBeInTheDocument();
    // short description (partial match)
    expect(screen.getByText(/grandfather's old farm/)).toBeInTheDocument();
    // overall review badge
    expect(screen.getByText(/Overwhelmingly Positive/)).toBeInTheDocument();
    // recent review badge
    expect(screen.getByText(/97%.*positive/i)).toBeInTheDocument();
    // play button visible (video_hls_url present)
    expect(screen.getByRole('button', { name: /play trailer/i })).toBeInTheDocument();
  });

  it('shows thin fallback when steam is null', async () => {
    vi.mocked(fetchGameDetail).mockResolvedValue({
      game: friendGame,
      steam: null,
    });

    render(
      <GameDetailModal
        mount="friend"
        token="tok123"
        game={friendGame}
        active={true}
        onClaim={vi.fn()}
        onClose={vi.fn()}
        loadDetail={friendLoadDetail}
      />,
    );

    await waitFor(() =>
      expect(screen.getByText(/no steam page for this one/i)).toBeInTheDocument(),
    );
    // Shows bundle and key_type
    expect(screen.getByText('Indie Gems Bundle')).toBeInTheDocument();
    expect(screen.getByText('steam')).toBeInTheDocument();
    // No video, no review badges
    expect(screen.queryByRole('button', { name: /play trailer/i })).not.toBeInTheDocument();
    expect(screen.queryByText(/Overwhelmingly Positive/)).not.toBeInTheDocument();
  });

  it('claim button in modal footer calls onClaim with the game', async () => {
    const user = userEvent.setup();
    const onClaim = vi.fn();

    vi.mocked(fetchGameDetail).mockResolvedValue({
      game: friendGame,
      steam: null,
    });

    render(
      <GameDetailModal
        mount="friend"
        token="tok123"
        game={friendGame}
        active={true}
        onClaim={onClaim}
        onClose={vi.fn()}
        loadDetail={friendLoadDetail}
      />,
    );

    await waitFor(() =>
      expect(screen.getByRole('button', { name: /^claim$/i })).toBeInTheDocument(),
    );
    await user.click(screen.getByRole('button', { name: /^claim$/i }));
    expect(onClaim).toHaveBeenCalledWith(friendGame);
  });

  it('admin mount shows status badge', async () => {
    vi.mocked(adminGameDetail).mockResolvedValue({
      game: adminGame,
      steam: null,
    });

    render(
      <GameDetailModal
        mount="admin"
        game={adminGame}
        onClose={vi.fn()}
        armedId={null}
        claiming={null}
        onSelfClaim={vi.fn()}
        adminSteamId={null}
        selfClaimResult={null}
        loadDetail={adminLoadDetail}
      />,
    );

    await waitFor(() => expect(screen.getByText('available')).toBeInTheDocument());
  });

  // ── F2: honest HLS-fallback test ─────────────────────────────────────────────
  // The original test only asserted the play button was gone — but that's already
  // true once play is clicked (videoPlaying=true hides it), so deleting the
  // hlsFailed branch would still pass. The fix: assert the RECOVERED end-state
  // positively — artwork img is rendered and the video element is gone.
  //
  // Neuter-check: with hlsFailed handling removed from the component (temporarily
  // setting hlsFailed never triggers), the artwork img does NOT appear and the
  // video element remains. Verified RED before restoring.
  it('falls back to artwork when hls.js fires a fatal error', async () => {
    const user = userEvent.setup();

    vi.mocked(fetchGameDetail).mockResolvedValue({
      game: friendGame,
      steam: { detail: steamDetailFixture, overall: overallFixture, recent: recentFixture },
    });

    render(
      <GameDetailModal
        mount="friend"
        token="tok123"
        game={friendGame}
        active={true}
        onClaim={vi.fn()}
        onClose={vi.fn()}
        loadDetail={friendLoadDetail}
      />,
    );

    // Wait for play button to appear
    await waitFor(() =>
      expect(screen.getByRole('button', { name: /play trailer/i })).toBeInTheDocument(),
    );

    // Click play — hls.on registers the error callback
    await user.click(screen.getByRole('button', { name: /play trailer/i }));

    // Simulate fatal HLS error
    act(() => {
      hlsCbCapture.errorCb?.('hlsError', { fatal: true });
    });

    // After fatal error: artwork img is shown (positive assertion), video element is gone
    await waitFor(() =>
      expect(screen.getByRole('img', { name: /stardew valley/i })).toBeInTheDocument(),
    );
    expect(screen.queryByRole('button', { name: /play trailer/i })).not.toBeInTheDocument();
    expect(document.querySelector('video')).toBeNull();
  });

  // ── F1: admin loadDetail 401 navigates to login ───────────────────────────────
  // withAuth returns a forever-pending promise on Unauthorized (navigation in flight).
  // The modal must stay in the "loading" phase — never show an error state.
  it('admin loadDetail Unauthorized navigates to login, modal stays in loading phase', async () => {
    const navigate = vi.fn();
    vi.mocked(adminGameDetail).mockRejectedValue(new Unauthorized());

    render(
      <GameDetailModal
        mount="admin"
        game={adminGame}
        onClose={vi.fn()}
        armedId={null}
        claiming={null}
        onSelfClaim={vi.fn()}
        adminSteamId={null}
        selfClaimResult={null}
        loadDetail={(gameId) => withAuth(() => adminGameDetail(gameId), navigate)}
      />,
    );

    // withAuth redirects and the promise never resolves — navigate fires
    await waitFor(() =>
      expect(navigate).toHaveBeenCalledWith('/admin/login', { replace: true }),
    );
    // Modal must NOT show error state — it stays in loading (navigation is underway)
    expect(screen.queryByText(/couldn't load details/i)).not.toBeInTheDocument();
  });

  // ── F3: focus management ──────────────────────────────────────────────────────
  it('dialog container receives focus on open', async () => {
    vi.mocked(fetchGameDetail).mockResolvedValue({
      game: friendGame,
      steam: null,
    });

    render(
      <GameDetailModal
        mount="friend"
        token="tok123"
        game={friendGame}
        active={true}
        onClaim={vi.fn()}
        onClose={vi.fn()}
        loadDetail={friendLoadDetail}
      />,
    );

    // The dialog container (role=dialog) should receive focus on mount
    await waitFor(() => {
      const dialog = screen.getByRole('dialog');
      expect(document.activeElement).toBe(dialog);
    });
  });

  // ── F4: module-level cache survives close/reopen ──────────────────────────────
  // The useRef cache was destroyed on unmount; a module-level Map is not.
  // After close → reopen, the fetch must be called exactly once.
  it('does not refetch on reopen (per-session cache)', async () => {
    vi.mocked(fetchGameDetail).mockResolvedValue({
      game: friendGame,
      steam: null,
    });

    const { unmount } = render(
      <GameDetailModal
        mount="friend"
        token="tok123"
        game={friendGame}
        active={true}
        onClaim={vi.fn()}
        onClose={vi.fn()}
        loadDetail={friendLoadDetail}
      />,
    );

    // Wait for initial load
    await waitFor(() =>
      expect(screen.getByText(/no steam page for this one/i)).toBeInTheDocument(),
    );
    expect(fetchGameDetail).toHaveBeenCalledTimes(1);

    // Simulate close (unmount) → reopen (remount)
    unmount();
    render(
      <GameDetailModal
        mount="friend"
        token="tok123"
        game={friendGame}
        active={true}
        onClaim={vi.fn()}
        onClose={vi.fn()}
        loadDetail={friendLoadDetail}
      />,
    );

    // Cache should serve the result — fetch still called only once total
    await waitFor(() =>
      expect(screen.getByText(/no steam page for this one/i)).toBeInTheDocument(),
    );
    expect(fetchGameDetail).toHaveBeenCalledTimes(1);
  });

  // ── F5: delisted stub — steam non-null but detail: null ───────────────────────
  // Steam has review data but no app detail (game removed from store).
  // Badges must render, no video/play button, no crash, artwork shown.
  it('renders delisted stub: detail null, reviews present, no video, artwork shown', async () => {
    vi.mocked(fetchGameDetail).mockResolvedValue({
      game: friendGame,
      steam: {
        detail: null,
        overall: overallFixture,
        recent: recentFixture,
      },
    });

    render(
      <GameDetailModal
        mount="friend"
        token="tok123"
        game={friendGame}
        active={true}
        onClaim={vi.fn()}
        onClose={vi.fn()}
        loadDetail={friendLoadDetail}
      />,
    );

    // Review badges render
    await waitFor(() =>
      expect(screen.getByText(/Overwhelmingly Positive/)).toBeInTheDocument(),
    );
    expect(screen.getByText(/97%.*positive/i)).toBeInTheDocument();

    // Artwork is shown (falls back to game.artwork_url since detail.header_image is null)
    expect(screen.getByRole('img', { name: /stardew valley/i })).toBeInTheDocument();

    // No video element, no play button
    expect(screen.queryByRole('button', { name: /play trailer/i })).not.toBeInTheDocument();
    expect(document.querySelector('video')).toBeNull();
  });
});

// ── Media carousel (issue #61) ────────────────────────────────────────────────

const screenshotsFixture = [
  {
    thumbnail: 'https://example.com/ss1.600x338.jpg',
    full: 'https://example.com/ss1.1920x1080.jpg',
  },
  {
    thumbnail: 'https://example.com/ss2.600x338.jpg',
    full: 'https://example.com/ss2.1920x1080.jpg',
  },
];

const steamDetailWithScreenshots = {
  ...steamDetailFixture,
  screenshots: screenshotsFixture,
};

describe('media carousel', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    hlsCbCapture.errorCb = null;
    clearGameDetailCache();
  });

  function mockDetail(detail: object | null) {
    vi.mocked(fetchGameDetail).mockResolvedValue({
      game: friendGame,
      steam:
        detail === null
          ? null
          : { detail: detail as never, overall: overallFixture, recent: recentFixture },
    });
  }

  function renderFriendModal() {
    return render(
      <GameDetailModal
        mount="friend"
        token="tok123"
        game={friendGame}
        active={true}
        onClaim={vi.fn()}
        onClose={vi.fn()}
        loadDetail={friendLoadDetail}
      />,
    );
  }

  it('trailer + screenshots: trailer is slide 1, arrows + counter present', async () => {
    mockDetail(steamDetailWithScreenshots);
    renderFriendModal();
    expect(await screen.findByLabelText('play trailer')).toBeInTheDocument();
    expect(screen.getByLabelText('previous')).toBeInTheDocument();
    expect(screen.getByLabelText('next')).toBeInTheDocument();
    expect(screen.getByText('1 / 3')).toBeInTheDocument();
  });

  it('next advances to a screenshot and wraps past the end', async () => {
    mockDetail(steamDetailWithScreenshots);
    renderFriendModal();
    const next = await screen.findByLabelText('next');
    await userEvent.click(next);
    expect(screen.getByText('2 / 3')).toBeInTheDocument();
    expect(screen.getByAltText('Stardew Valley screenshot 1')).toBeInTheDocument();
    await userEvent.click(next);
    expect(screen.getByText('3 / 3')).toBeInTheDocument();
    await userEvent.click(next);
    expect(screen.getByText('1 / 3')).toBeInTheDocument(); // wrap
  });

  it('one screenshot, no trailer: image alone, zero carousel chrome', async () => {
    mockDetail({
      ...steamDetailFixture,
      video_hls_url: null,
      screenshots: [screenshotsFixture[0]],
    });
    renderFriendModal();
    expect(
      await screen.findByAltText('Stardew Valley screenshot 1'),
    ).toBeInTheDocument();
    expect(screen.queryByLabelText('previous')).not.toBeInTheDocument();
    expect(screen.queryByLabelText('next')).not.toBeInTheDocument();
    expect(screen.queryByText('1 / 1')).not.toBeInTheDocument();
  });

  it('no trailer, no screenshots: plain header image, no carousel chrome', async () => {
    mockDetail({ ...steamDetailFixture, video_hls_url: null });
    renderFriendModal();
    expect(await screen.findByAltText('Stardew Valley')).toBeInTheDocument();
    expect(screen.queryByLabelText('next')).not.toBeInTheDocument();
  });

  it('navigating away from the trailer pauses the video', async () => {
    const pauseSpy = vi
      .spyOn(HTMLMediaElement.prototype, 'pause')
      .mockImplementation(() => {});
    mockDetail(steamDetailWithScreenshots);
    renderFriendModal();
    await userEvent.click(await screen.findByLabelText('next'));
    expect(pauseSpy).toHaveBeenCalled();
    pauseSpy.mockRestore();
  });

  it('fatal HLS error mid-carousel drops the trailer slide and clamps the index', async () => {
    mockDetail(steamDetailWithScreenshots);
    renderFriendModal();
    await userEvent.click(await screen.findByLabelText('play trailer'));
    await waitFor(() => expect(hlsCbCapture.errorCb).not.toBeNull());
    act(() => hlsCbCapture.errorCb?.('hlsError', { fatal: true }));
    // Trailer gone: 2 screenshots remain, counter consistent, no crash.
    expect(await screen.findByText('1 / 2')).toBeInTheDocument();
    expect(screen.queryByLabelText('play trailer')).not.toBeInTheDocument();
  });

  it('off-screen slides are inert and aria-hidden', async () => {
    mockDetail(steamDetailWithScreenshots);
    renderFriendModal();
    await screen.findByLabelText('play trailer');
    const region = screen.getByRole('region', { name: 'media' });
    const hidden = region.querySelectorAll('[aria-hidden="true"][inert]');
    expect(hidden.length).toBe(2); // both screenshot slides while trailer is active
  });

  it('reduced motion: no transition class; motion allowed: transition present', async () => {
    const mm = vi.spyOn(window, 'matchMedia');
    mm.mockReturnValue({ matches: true } as MediaQueryList);
    mockDetail(steamDetailWithScreenshots);
    const { unmount } = renderFriendModal();
    await screen.findByLabelText('play trailer');
    const strip = () =>
      screen.getByRole('region', { name: 'media' }).firstElementChild as HTMLElement;
    expect(strip().className).not.toContain('transition-transform');
    unmount();
    clearGameDetailCache();
    mm.mockReturnValue({ matches: false } as MediaQueryList);
    mockDetail(steamDetailWithScreenshots);
    renderFriendModal();
    await screen.findByLabelText('play trailer');
    expect(strip().className).toContain('transition-transform');
    mm.mockRestore();
  });
});
