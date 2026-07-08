import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { vi, describe, it, expect } from 'vitest';
import { GameGrid } from './GameGrid';
import type { GameView } from '../api';

const makeGame = (overrides: Partial<GameView> & { id: string }): GameView => ({
  title: 'Default Game',
  bundle: 'Default Bundle',
  key_type: 'steam',
  artwork_url: null,
  steam_app_id: null,
  ...overrides,
});

describe('GameGrid', () => {
  it('shows a single card for a single game', () => {
    const games = [makeGame({ id: '1', title: 'Portal 2' })];
    render(<GameGrid games={games} onDetail={vi.fn()} />);
    expect(screen.getByText('Portal 2')).toBeInTheDocument();
    expect(screen.queryByText(/copies/)).not.toBeInTheDocument();
  });

  it('groups games by title and shows ×N chip', () => {
    const games = [
      makeGame({ id: '1', title: 'Portal 2' }),
      makeGame({ id: '2', title: 'Portal 2' }),
      makeGame({ id: '3', title: 'Portal 2' }),
    ];
    render(<GameGrid games={games} onDetail={vi.fn()} />);
    // one card heading for Portal 2
    expect(screen.getAllByText('Portal 2')).toHaveLength(1);
    expect(screen.getByText('×3 copies')).toBeInTheDocument();
    // one details button (one card)
    expect(screen.getAllByRole('button', { name: /details/i })).toHaveLength(1);
  });

  it('groups multiple titles independently', () => {
    const games = [
      makeGame({ id: '1', title: 'Portal 2' }),
      makeGame({ id: '2', title: 'Portal 2' }),
      makeGame({ id: '3', title: 'Celeste' }),
    ];
    render(<GameGrid games={games} onDetail={vi.fn()} />);
    expect(screen.getByText('×2 copies')).toBeInTheDocument();
    expect(screen.queryByText('×1 copies')).not.toBeInTheDocument();
    expect(screen.getByText('Portal 2')).toBeInTheDocument();
    expect(screen.getByText('Celeste')).toBeInTheDocument();
    expect(screen.getAllByRole('button', { name: /details/i })).toHaveLength(2);
  });

  it('the grid never renders a claim button — claiming lives in the detail modal', () => {
    const games = [makeGame({ id: '1', title: 'Game' })];
    render(<GameGrid games={games} onDetail={vi.fn()} />);
    expect(screen.queryByRole('button', { name: /claim/i })).not.toBeInTheDocument();
  });

  it('calls onDetail with the game when the details button is clicked', async () => {
    const user = userEvent.setup();
    const onDetail = vi.fn();
    const game = makeGame({ id: '1', title: 'Hollow Knight' });
    render(<GameGrid games={[game]} onDetail={onDetail} />);
    await user.click(screen.getByRole('button', { name: /details/i }));
    expect(onDetail).toHaveBeenCalledWith(game);
  });

  it('calls onDetail with the game when the card body is clicked', async () => {
    const user = userEvent.setup();
    const onDetail = vi.fn();
    const game = makeGame({ id: '1', title: 'Hollow Knight' });
    render(<GameGrid games={[game]} onDetail={onDetail} />);
    await user.click(screen.getByText('Hollow Knight'));
    expect(onDetail).toHaveBeenCalledWith(game);
  });

  it('renders artwork image when artwork_url is present', () => {
    const games = [makeGame({ id: '1', title: 'Game', artwork_url: 'https://example.com/art.jpg' })];
    render(<GameGrid games={games} onDetail={vi.fn()} />);
    expect(screen.getByRole('img', { name: /game/i })).toBeInTheDocument();
  });

  it('renders fallback colored div when artwork_url is null', () => {
    const games = [makeGame({ id: '1', title: 'Game', artwork_url: null })];
    render(<GameGrid games={games} onDetail={vi.fn()} />);
    expect(screen.queryByRole('img')).not.toBeInTheDocument();
  });

  it('renders the steam capsule over the hash underlay when artwork_url is null but steam_app_id is set', () => {
    const games = [makeGame({ id: '1', title: 'Game', artwork_url: null, steam_app_id: 620 })];
    render(<GameGrid games={games} onDetail={vi.fn()} />);
    const img = screen.getByRole('img', { name: /game/i });
    expect(img).toHaveAttribute(
      'src',
      'https://shared.akamai.steamstatic.com/store_item_assets/steam/apps/620/capsule_616x353.jpg',
    );
  });

  it('humble artwork_url wins over the steam capsule when both exist', () => {
    const games = [
      makeGame({ id: '1', title: 'Game', artwork_url: 'https://example.com/art.jpg', steam_app_id: 620 }),
    ];
    render(<GameGrid games={games} onDetail={vi.fn()} />);
    expect(screen.getByRole('img', { name: /game/i })).toHaveAttribute(
      'src',
      'https://example.com/art.jpg',
    );
  });

  it('renders up to 4 genre chips straight from the list payload — no fetch', () => {
    const games = [
      makeGame({
        id: '1',
        title: 'Celeste',
        genres: ['Action', 'Indie', 'Platformer', 'Adventure', 'Casual'],
      }),
    ];
    render(<GameGrid games={games} onDetail={vi.fn()} />);
    expect(screen.getByText('Action')).toBeInTheDocument();
    expect(screen.getByText('Adventure')).toBeInTheDocument();
    // display cap is 4 — the 5th genre from the payload is not rendered
    expect(screen.queryByText('Casual')).not.toBeInTheDocument();
    // genre chips replace the key_type chip
    expect(screen.queryByText('steam')).not.toBeInTheDocument();
  });

  it('falls back to the key_type chip when the payload has no genres', () => {
    render(<GameGrid games={[makeGame({ id: '1', title: 'Game' })]} onDetail={vi.fn()} />);
    expect(screen.getByText('steam')).toBeInTheDocument();
  });
});
