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
    render(<GameGrid games={games} active={true} onClaim={vi.fn()} />);
    expect(screen.getByText('Portal 2')).toBeInTheDocument();
    expect(screen.queryByText(/copies/)).not.toBeInTheDocument();
  });

  it('groups games by title and shows ×N chip', () => {
    const games = [
      makeGame({ id: '1', title: 'Portal 2' }),
      makeGame({ id: '2', title: 'Portal 2' }),
      makeGame({ id: '3', title: 'Portal 2' }),
    ];
    render(<GameGrid games={games} active={true} onClaim={vi.fn()} />);
    // one card heading for Portal 2
    expect(screen.getAllByText('Portal 2')).toHaveLength(1);
    expect(screen.getByText('×3 copies')).toBeInTheDocument();
    // one claim button (one card)
    expect(screen.getAllByRole('button', { name: /claim/i })).toHaveLength(1);
  });

  it('groups multiple titles independently', () => {
    const games = [
      makeGame({ id: '1', title: 'Portal 2' }),
      makeGame({ id: '2', title: 'Portal 2' }),
      makeGame({ id: '3', title: 'Celeste' }),
    ];
    render(<GameGrid games={games} active={true} onClaim={vi.fn()} />);
    expect(screen.getByText('×2 copies')).toBeInTheDocument();
    expect(screen.queryByText('×1 copies')).not.toBeInTheDocument();
    expect(screen.getByText('Portal 2')).toBeInTheDocument();
    expect(screen.getByText('Celeste')).toBeInTheDocument();
    expect(screen.getAllByRole('button', { name: /claim/i })).toHaveLength(2);
  });

  it('claim buttons are disabled when active is false', () => {
    const games = [makeGame({ id: '1', title: 'Game' })];
    render(<GameGrid games={games} active={false} onClaim={vi.fn()} />);
    expect(screen.getByRole('button', { name: /claim/i })).toBeDisabled();
  });

  it('claim buttons are enabled when active is true', () => {
    const games = [makeGame({ id: '1', title: 'Game' })];
    render(<GameGrid games={games} active={true} onClaim={vi.fn()} />);
    expect(screen.getByRole('button', { name: /claim/i })).not.toBeDisabled();
  });

  it('calls onClaim with the game when claim button is clicked', async () => {
    const user = userEvent.setup();
    const onClaim = vi.fn();
    const game = makeGame({ id: '1', title: 'Hollow Knight' });
    render(<GameGrid games={[game]} active={true} onClaim={onClaim} />);
    await user.click(screen.getByRole('button', { name: /claim/i }));
    expect(onClaim).toHaveBeenCalledWith(game);
  });

  it('renders artwork image when artwork_url is present', () => {
    const games = [makeGame({ id: '1', title: 'Game', artwork_url: 'https://example.com/art.jpg' })];
    render(<GameGrid games={games} active={true} onClaim={vi.fn()} />);
    expect(screen.getByRole('img', { name: /game/i })).toBeInTheDocument();
  });

  it('renders fallback colored div when artwork_url is null', () => {
    const games = [makeGame({ id: '1', title: 'Game', artwork_url: null })];
    render(<GameGrid games={games} active={true} onClaim={vi.fn()} />);
    expect(screen.queryByRole('img')).not.toBeInTheDocument();
  });
});
