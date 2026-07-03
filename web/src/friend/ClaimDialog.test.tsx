import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { vi, describe, it, expect, beforeEach } from 'vitest';
import { ClaimDialog } from './ClaimDialog';
import type { GameView } from '../api';

vi.mock('../api');
import { claimGame } from '../api';

const mockGame: GameView = {
  id: 'game-1',
  title: 'Hollow Knight',
  bundle: 'Indie Bundle',
  key_type: 'steam',
  artwork_url: null,
};

const GIFT_URL = 'https://www.humblebundle.com/gift?key=abc123xyz';

describe('ClaimDialog', () => {
  const onClose = vi.fn();
  const onRefresh = vi.fn();

  beforeEach(() => {
    vi.clearAllMocks();
  });

  it('shows confirm step with game title and claim cost copy', () => {
    render(<ClaimDialog token="tok" game={mockGame} onClose={onClose} onRefresh={onRefresh} />);
    expect(screen.getByText(/Hollow Knight/)).toBeInTheDocument();
    expect(screen.getByText(/this uses 1 of your claims/i)).toBeInTheDocument();
    expect(screen.getByRole('button', { name: /confirm/i })).toBeInTheDocument();
  });

  it('cancel in confirm step calls onClose without onRefresh', async () => {
    const user = userEvent.setup();
    render(<ClaimDialog token="tok" game={mockGame} onClose={onClose} onRefresh={onRefresh} />);
    await user.click(screen.getByRole('button', { name: /cancel/i }));
    expect(onClose).toHaveBeenCalledOnce();
    expect(onRefresh).not.toHaveBeenCalled();
  });

  describe('gifted path', () => {
    it('shows exact gift URL after confirm', async () => {
      const user = userEvent.setup();
      vi.mocked(claimGame).mockResolvedValue({ kind: 'gifted', gift_url: GIFT_URL });
      render(<ClaimDialog token="tok" game={mockGame} onClose={onClose} onRefresh={onRefresh} />);

      await user.click(screen.getByRole('button', { name: /confirm/i }));

      await waitFor(() => {
        expect(screen.getByText(GIFT_URL)).toBeInTheDocument();
      });
    });

    it('shows one-time warning and region-lock disclaimer in gifted view', async () => {
      const user = userEvent.setup();
      vi.mocked(claimGame).mockResolvedValue({ kind: 'gifted', gift_url: GIFT_URL });
      render(<ClaimDialog token="tok" game={mockGame} onClose={onClose} onRefresh={onRefresh} />);

      await user.click(screen.getByRole('button', { name: /confirm/i }));

      await waitFor(() => {
        expect(screen.getByText(/redeem it to YOUR humble account/i)).toBeInTheDocument();
        expect(screen.getByText(/keys may be region-locked/i)).toBeInTheDocument();
      });
    });

    it('Escape does NOT close the gifted view', async () => {
      const user = userEvent.setup();
      vi.mocked(claimGame).mockResolvedValue({ kind: 'gifted', gift_url: GIFT_URL });
      render(<ClaimDialog token="tok" game={mockGame} onClose={onClose} onRefresh={onRefresh} />);

      await user.click(screen.getByRole('button', { name: /confirm/i }));
      await waitFor(() => {
        expect(screen.getByText(GIFT_URL)).toBeInTheDocument();
      });

      await user.keyboard('{Escape}');
      expect(onClose).not.toHaveBeenCalled();
      // URL still present — not eaten by Escape
      expect(screen.getByText(GIFT_URL)).toBeInTheDocument();
    });

    it('close button calls onRefresh then onClose', async () => {
      const user = userEvent.setup();
      vi.mocked(claimGame).mockResolvedValue({ kind: 'gifted', gift_url: GIFT_URL });
      render(<ClaimDialog token="tok" game={mockGame} onClose={onClose} onRefresh={onRefresh} />);

      await user.click(screen.getByRole('button', { name: /confirm/i }));
      await waitFor(() => {
        expect(screen.getByText(GIFT_URL)).toBeInTheDocument();
      });

      await user.click(screen.getByRole('button', { name: /close/i }));
      expect(onRefresh).toHaveBeenCalledOnce();
      expect(onClose).toHaveBeenCalledOnce();
    });
  });

  describe('refused path', () => {
    it('shows refused message', async () => {
      const user = userEvent.setup();
      vi.mocked(claimGame).mockResolvedValue({
        kind: 'refused',
        message: 'this key has already been claimed',
      });
      render(<ClaimDialog token="tok" game={mockGame} onClose={onClose} onRefresh={onRefresh} />);

      await user.click(screen.getByRole('button', { name: /confirm/i }));
      await waitFor(() => {
        expect(screen.getByText('this key has already been claimed')).toBeInTheDocument();
      });
    });

    it('close triggers onRefresh then onClose', async () => {
      const user = userEvent.setup();
      vi.mocked(claimGame).mockResolvedValue({ kind: 'refused', message: 'already claimed' });
      render(<ClaimDialog token="tok" game={mockGame} onClose={onClose} onRefresh={onRefresh} />);

      await user.click(screen.getByRole('button', { name: /confirm/i }));
      await waitFor(() => {
        expect(screen.getByText('already claimed')).toBeInTheDocument();
      });

      await user.click(screen.getByRole('button', { name: /close/i }));
      expect(onRefresh).toHaveBeenCalledOnce();
      expect(onClose).toHaveBeenCalledOnce();
    });
  });

  describe('processing path', () => {
    it('shows server message and check-later copy', async () => {
      const user = userEvent.setup();
      vi.mocked(claimGame).mockResolvedValue({
        kind: 'processing',
        message: 'your key is being generated',
      });
      render(<ClaimDialog token="tok" game={mockGame} onClose={onClose} onRefresh={onRefresh} />);

      await user.click(screen.getByRole('button', { name: /confirm/i }));
      await waitFor(() => {
        expect(screen.getByText('your key is being generated')).toBeInTheDocument();
        expect(screen.getByText(/check this page later/i)).toBeInTheDocument();
      });
    });

    it('close triggers onRefresh then onClose', async () => {
      const user = userEvent.setup();
      vi.mocked(claimGame).mockResolvedValue({
        kind: 'processing',
        message: 'generating',
      });
      render(<ClaimDialog token="tok" game={mockGame} onClose={onClose} onRefresh={onRefresh} />);

      await user.click(screen.getByRole('button', { name: /confirm/i }));
      await waitFor(() => {
        expect(screen.getByText('generating')).toBeInTheDocument();
      });

      await user.click(screen.getByRole('button', { name: /close/i }));
      expect(onRefresh).toHaveBeenCalledOnce();
      expect(onClose).toHaveBeenCalledOnce();
    });
  });

  describe('error path', () => {
    it('shows generic error message', async () => {
      const user = userEvent.setup();
      vi.mocked(claimGame).mockResolvedValue({
        kind: 'error',
        message: 'something hiccuped — try again',
      });
      render(<ClaimDialog token="tok" game={mockGame} onClose={onClose} onRefresh={onRefresh} />);

      await user.click(screen.getByRole('button', { name: /confirm/i }));
      await waitFor(() => {
        expect(screen.getByText('something hiccuped — try again')).toBeInTheDocument();
      });
    });
  });
});
