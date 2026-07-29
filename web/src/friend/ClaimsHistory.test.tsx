import { render, screen } from '@testing-library/react';
import { describe, it, expect } from 'vitest';
import { ClaimsHistory } from './ClaimsHistory';
import type { ClaimView } from '../api';

const makeClaim = (overrides: Partial<ClaimView> & { game_id: string }): ClaimView => ({
  title: 'Default Game',
  state: 'pending',
  gift_url: null,
  ...overrides,
});

describe('ClaimsHistory', () => {
  it('renders nothing when there are no claims', () => {
    const { container } = render(<ClaimsHistory claims={[]} />);
    expect(container).toBeEmptyDOMElement();
  });

  it('renders a "returned" chip for a failed claim', () => {
    const claims = [makeClaim({ game_id: 'g1', title: 'Dome Keeper', state: 'failed' })];
    render(<ClaimsHistory claims={claims} />);
    expect(screen.getByText('returned')).toBeInTheDocument();
  });

  it('shows the warm no-blame row detail for a failed claim, between the fulfilled and pending branches', () => {
    const claims = [makeClaim({ game_id: 'g1', title: 'Dome Keeper', state: 'failed' })];
    render(<ClaimsHistory claims={claims} />);
    expect(
      screen.getByText("that key expired on humble's side — your pick came back"),
    ).toBeInTheDocument();
  });

  it('does not show the fulfilled gift-url link or the pending processing label for a failed claim', () => {
    const claims = [makeClaim({ game_id: 'g1', title: 'Dome Keeper', state: 'failed' })];
    render(<ClaimsHistory claims={claims} />);
    expect(screen.queryByText(/lost the tab/)).not.toBeInTheDocument();
    expect(screen.queryByText('processing')).not.toBeInTheDocument();
  });
});
