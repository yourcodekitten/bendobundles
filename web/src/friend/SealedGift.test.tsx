import { render, screen, act, fireEvent } from '@testing-library/react';
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { SealedGift } from './SealedGift';

// The countdown ticks on real intervals — fake them.
beforeEach(() => {
  vi.useFakeTimers();
});
afterEach(() => {
  vi.useRealTimers();
  vi.restoreAllMocks();
});

const UNLOCKS_AT = '2026-12-25T05:00:00Z';

describe('SealedGift', () => {
  it('renders the label on the gift tag and the countdown from remaining seconds', () => {
    render(
      <SealedGift
        label="For Maya"
        unlocksInSeconds={90061}
        unlocksAt={UNLOCKS_AT}
        onRefetch={() => {}}
      />,
    );
    expect(screen.getByText('For Maya')).toBeInTheDocument();
    // 90061s = 1d 1h 1m 1s
    expect(screen.getByText(/1d 1h 1m 1s/)).toBeInTheDocument();
  });

  it('ticks down once per second', () => {
    render(
      <SealedGift
        label="For Maya"
        unlocksInSeconds={90061}
        unlocksAt={UNLOCKS_AT}
        onRefetch={() => {}}
      />,
    );
    act(() => {
      vi.advanceTimersByTime(1000);
    });
    expect(screen.getByText(/1d 1h 1m 0s/)).toBeInTheDocument();
  });

  it('calls onRefetch exactly once when the countdown reaches zero — it never self-unseals', () => {
    const onRefetch = vi.fn();
    render(
      <SealedGift
        label="For Maya"
        unlocksInSeconds={2}
        unlocksAt={UNLOCKS_AT}
        onRefetch={onRefetch}
      />,
    );
    act(() => {
      vi.advanceTimersByTime(5000);
    });
    expect(onRefetch).toHaveBeenCalledTimes(1);
    // Still renders the sealed view (0s floor) — whatever comes next is the
    // parent's refetch's decision, structurally.
    expect(screen.getByText('For Maya')).toBeInTheDocument();
  });

  it('a fresh unlocksInSeconds prop reseeds the countdown and re-arms the refetch (quiet backoff)', () => {
    const onRefetch = vi.fn();
    const { rerender } = render(
      <SealedGift
        label="For Maya"
        unlocksInSeconds={1}
        unlocksAt={UNLOCKS_AT}
        onRefetch={onRefetch}
      />,
    );
    act(() => {
      vi.advanceTimersByTime(3000);
    });
    expect(onRefetch).toHaveBeenCalledTimes(1);

    // Server still says sealed: fresh remaining arrives, clock continues, no error UI.
    rerender(
      <SealedGift
        label="For Maya"
        unlocksInSeconds={3}
        unlocksAt={UNLOCKS_AT}
        onRefetch={onRefetch}
      />,
    );
    expect(screen.getByText(/3s/)).toBeInTheDocument();
    act(() => {
      vi.advanceTimersByTime(4000);
    });
    expect(onRefetch).toHaveBeenCalledTimes(2);
  });

  it('refetches when the tab becomes visible again (drift resync)', () => {
    const onRefetch = vi.fn();
    render(
      <SealedGift
        label="For Maya"
        unlocksInSeconds={3600}
        unlocksAt={UNLOCKS_AT}
        onRefetch={onRefetch}
      />,
    );
    fireEvent(document, new Event('visibilitychange'));
    // jsdom documents are always "visible"
    expect(onRefetch).toHaveBeenCalledTimes(1);
  });

  it('no-motion environment (jsdom default): no sway class, countdown text still present', () => {
    // jsdom has no matchMedia ⇒ motionOK() is false — the ceremony gate's
    // "cannot confirm motion is welcome" branch.
    const { container } = render(
      <SealedGift
        label="For Maya"
        unlocksInSeconds={60}
        unlocksAt={UNLOCKS_AT}
        onRefetch={() => {}}
      />,
    );
    expect(container.querySelector('.gift-sway')).toBeNull();
    expect(screen.getByText(/1m 0s/)).toBeInTheDocument();
  });

  it('motion-welcome environment: the present sways', () => {
    vi.stubGlobal(
      'matchMedia',
      vi.fn().mockReturnValue({ matches: false }), // reduced-motion query: not reduced
    );
    const { container } = render(
      <SealedGift
        label="For Maya"
        unlocksInSeconds={60}
        unlocksAt={UNLOCKS_AT}
        onRefetch={() => {}}
      />,
    );
    expect(container.querySelector('.gift-sway')).not.toBeNull();
    vi.unstubAllGlobals();
  });
});
