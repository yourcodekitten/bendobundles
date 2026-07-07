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
import { GameDetailModal } from './GameDetailModal';
import type { GameView, AdminGame } from './api';
import { fetchGameDetail, adminGameDetail } from './api';

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

describe('GameDetailModal', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    hlsCbCapture.errorCb = null;
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
      />,
    );

    await waitFor(() => expect(screen.getByText('available')).toBeInTheDocument());
  });

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

    // Play button gone; fallback shown (video section replaced)
    await waitFor(() =>
      expect(screen.queryByRole('button', { name: /play trailer/i })).not.toBeInTheDocument(),
    );
  });
});
