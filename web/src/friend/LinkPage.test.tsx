import { render, screen, waitFor } from '@testing-library/react';
import { MemoryRouter, Route, Routes } from 'react-router-dom';
import { vi, describe, it, expect, beforeEach } from 'vitest';
import { LinkPage } from './LinkPage';
import type { LinkView } from '../api';

vi.mock('../api');

// importOriginal is used in other tests; here we just use auto-mock + vi.mocked
import { fetchLink } from '../api';

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

  it('shows not-found view on error', async () => {
    vi.mocked(fetchLink).mockRejectedValue(new Error('not found'));
    renderLinkPage();
    await waitFor(() => {
      expect(screen.getByRole('heading', { name: /link not found/i })).toBeInTheDocument();
    });
  });

  it('shows loaded state with label and claim counts', async () => {
    vi.mocked(fetchLink).mockResolvedValue({ ...baseLink });
    renderLinkPage();
    await waitFor(() => {
      expect(screen.getByText('Test Bundle')).toBeInTheDocument();
      expect(screen.getByText(/1\/3 claims used/)).toBeInTheDocument();
    });
  });

  it('shows exhausted banner and disabled grid when active:false + games present', async () => {
    vi.mocked(fetchLink).mockResolvedValue({
      ...baseLink,
      active: false,
      games: [{ id: '1', title: 'Portal', bundle: 'B', key_type: 'steam', artwork_url: null }],
    });
    renderLinkPage();
    await waitFor(() => {
      expect(screen.getByRole('alert')).toHaveTextContent("you've used all your claims");
    });
    // grid is visible but claim button is disabled
    expect(screen.getByRole('button', { name: /claim/i })).toBeDisabled();
  });

  it('shows revoked banner and no grid when active:false + games empty', async () => {
    vi.mocked(fetchLink).mockResolvedValue({
      ...baseLink,
      active: false,
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

  it('renders exhausted with grid visible vs revoked without grid (distinction)', async () => {
    // exhausted: games present → grid rendered (even if disabled)
    vi.mocked(fetchLink).mockResolvedValue({
      ...baseLink,
      active: false,
      games: [{ id: '1', title: 'Celeste', bundle: 'B', key_type: 'steam', artwork_url: null }],
    });
    const { unmount } = renderLinkPage('exhausted-token');
    await waitFor(() => {
      expect(screen.getByText('Celeste')).toBeInTheDocument();
    });
    unmount();

    // revoked: no games → no grid
    vi.mocked(fetchLink).mockResolvedValue({
      ...baseLink,
      active: false,
      games: [],
    });
    renderLinkPage('revoked-token');
    await waitFor(() => {
      expect(screen.getByRole('alert')).toHaveTextContent("this invite isn't active anymore");
    });
    expect(screen.queryByText('Celeste')).not.toBeInTheDocument();
  });
});
