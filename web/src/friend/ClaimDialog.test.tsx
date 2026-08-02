import { render, screen, waitFor, act } from '@testing-library/react';
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
  steam_app_id: null,
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

    it('Escape during the in-flight loading step does NOT close (URL not eaten)', async () => {
      const user = userEvent.setup();
      let resolveClaim!: (r: { kind: 'gifted'; gift_url: string }) => void;
      vi.mocked(claimGame).mockReturnValue(
        new Promise((resolve) => {
          resolveClaim = resolve;
        }),
      );
      render(<ClaimDialog token="tok" game={mockGame} onClose={onClose} onRefresh={onRefresh} />);

      await user.click(screen.getByRole('button', { name: /confirm/i }));
      expect(screen.getByText(/claiming/i)).toBeInTheDocument();

      await user.keyboard('{Escape}');
      expect(onClose).not.toHaveBeenCalled();

      resolveClaim({ kind: 'gifted', gift_url: GIFT_URL });
      await waitFor(() => {
        expect(screen.getByText(GIFT_URL)).toBeInTheDocument();
      });
    });

    it('two same-tick confirm activations fire exactly ONE claim POST (re-entry guard)', async () => {
      let resolveClaim!: (r: { kind: 'gifted'; gift_url: string }) => void;
      vi.mocked(claimGame).mockReturnValue(
        new Promise((resolve) => {
          resolveClaim = resolve;
        }),
      );
      render(<ClaimDialog token="tok" game={mockGame} onClose={onClose} onRefresh={onRefresh} />);

      // Dispatch both clicks synchronously in one act() — a double-click /
      // Enter-repeat / AT-synthesized pair can land before React re-renders
      // the confirm button away, so both handler closures still see
      // step === 'confirm'; only the ref guard stops the second POST.
      const confirmButton = screen.getByRole('button', { name: /confirm/i });
      await act(async () => {
        confirmButton.dispatchEvent(new MouseEvent('click', { bubbles: true }));
        confirmButton.dispatchEvent(new MouseEvent('click', { bubbles: true }));
      });

      expect(claimGame).toHaveBeenCalledTimes(1);

      // The single claim still completes normally
      resolveClaim({ kind: 'gifted', gift_url: GIFT_URL });
      await waitFor(() => {
        expect(screen.getByText(GIFT_URL)).toBeInTheDocument();
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

  describe('click-outside dismiss', () => {
    // Real clicks on the dimmed area land on the full-viewport dialog CONTAINER
    // (z-50, stacked above the visual backdrop) — the old z-40 backdrop never
    // receives pointer events, so the handler lives on the container with an
    // e.target === e.currentTarget guard.
    const clickBackdrop = () => {
      return userEvent.setup().click(screen.getByRole('dialog'));
    };

    it('on processing triggers onRefresh then onClose (same as the close button)', async () => {
      const user = userEvent.setup();
      vi.mocked(claimGame).mockResolvedValue({ kind: 'processing', message: 'generating' });
      render(<ClaimDialog token="tok" game={mockGame} onClose={onClose} onRefresh={onRefresh} />);

      await user.click(screen.getByRole('button', { name: /confirm/i }));
      await waitFor(() => {
        expect(screen.getByText('generating')).toBeInTheDocument();
      });

      await clickBackdrop();
      expect(onRefresh).toHaveBeenCalledOnce();
      expect(onClose).toHaveBeenCalledOnce();
    });

    it('on refused triggers onRefresh then onClose', async () => {
      const user = userEvent.setup();
      vi.mocked(claimGame).mockResolvedValue({ kind: 'refused', message: 'already claimed' });
      render(<ClaimDialog token="tok" game={mockGame} onClose={onClose} onRefresh={onRefresh} />);

      await user.click(screen.getByRole('button', { name: /confirm/i }));
      await waitFor(() => {
        expect(screen.getByText('already claimed')).toBeInTheDocument();
      });

      await clickBackdrop();
      expect(onRefresh).toHaveBeenCalledOnce();
      expect(onClose).toHaveBeenCalledOnce();
    });

    it('on confirm closes without onRefresh', async () => {
      render(<ClaimDialog token="tok" game={mockGame} onClose={onClose} onRefresh={onRefresh} />);
      await clickBackdrop();
      expect(onClose).toHaveBeenCalledOnce();
      expect(onRefresh).not.toHaveBeenCalled();
    });

    it('clicking INSIDE the panel does not dismiss (target guard)', async () => {
      const user = userEvent.setup();
      render(<ClaimDialog token="tok" game={mockGame} onClose={onClose} onRefresh={onRefresh} />);
      // The heading is inside the panel — its clicks bubble to the container
      // but target !== currentTarget, so no dismiss.
      await user.click(screen.getByText(/this uses 1 of your claims/i));
      expect(onClose).not.toHaveBeenCalled();
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

// ── Focus trap (a11y #63) — ClaimDialog is the app's second role="dialog"
// aria-modal surface; keyboard focus must not wander to the page behind the
// backdrop (WCAG 2.4.3 / 2.1.2). One test per step shape: multi-focusable
// (confirm, gifted), and the zero-focusable step (loading) that would otherwise
// drop focus to <body>. ──────────────────────────────────────────────────────
describe('focus trap', () => {
  const onClose = vi.fn();
  const onRefresh = vi.fn();

  beforeEach(() => {
    vi.clearAllMocks();
  });

  it('confirm step: focus opens on the dialog, and Tab from the last control wraps to the first', async () => {
    render(<ClaimDialog token="tok" game={mockGame} onClose={onClose} onRefresh={onRefresh} />);
    const dialog = screen.getByRole('dialog');
    expect(document.activeElement).toBe(dialog); // focus-on-open pin

    // Tab from the container lands on a focusable INSIDE the dialog.
    await userEvent.tab();
    const first = document.activeElement as HTMLElement;
    expect(dialog.contains(first)).toBe(true);

    // From the last control (confirm), Tab wraps back to the first (cancel).
    const confirm = screen.getByRole('button', { name: /confirm/i });
    confirm.focus();
    await userEvent.tab();
    expect(document.activeElement).toBe(first);
  });

  it('confirm step: Shift+Tab on the first control wraps to the last', async () => {
    render(<ClaimDialog token="tok" game={mockGame} onClose={onClose} onRefresh={onRefresh} />);
    const cancel = screen.getByRole('button', { name: /cancel/i });
    const confirm = screen.getByRole('button', { name: /confirm/i });

    cancel.focus();
    await userEvent.tab({ shift: true });
    expect(document.activeElement).toBe(confirm);
  });

  it('loading step (zero focusables): Tab does not escape the dialog to the page behind it', async () => {
    // Never-resolving claim keeps us on the loading step, which renders only a
    // <p>claiming...</p> — no focusables. The confirm button unmounts here, so
    // without a re-pin focus would fall to <body> and Tab would walk the page.
    vi.mocked(claimGame).mockReturnValue(new Promise<never>(() => {}));
    render(<ClaimDialog token="tok" game={mockGame} onClose={onClose} onRefresh={onRefresh} />);

    await userEvent.click(screen.getByRole('button', { name: /confirm/i }));
    await screen.findByText(/claiming/i);

    const dialog = screen.getByRole('dialog');
    // Focus is pinned to the dialog, not lost to <body>.
    expect(document.activeElement).toBe(dialog);

    // Tab is swallowed — focus stays on the dialog, never reaches page content.
    await userEvent.tab();
    expect(dialog.contains(document.activeElement)).toBe(true);
    expect(document.activeElement).not.toBe(document.body);
  });

  it('gifted step (multi-focusable, one-time URL): Tab wraps inside the dialog, never leaking', async () => {
    vi.mocked(claimGame).mockResolvedValue({ kind: 'gifted', gift_url: GIFT_URL });
    render(<ClaimDialog token="tok" game={mockGame} onClose={onClose} onRefresh={onRefresh} />);

    await userEvent.click(screen.getByRole('button', { name: /confirm/i }));
    await waitFor(() => expect(screen.getByText(GIFT_URL)).toBeInTheDocument());

    const dialog = screen.getByRole('dialog');
    const close = screen.getByRole('button', { name: /close/i });

    // From the last control, Tab wraps to the first focusable inside the dialog.
    close.focus();
    await userEvent.tab();
    expect(dialog.contains(document.activeElement)).toBe(true);
    expect(document.activeElement).not.toBe(close);
  });
});
