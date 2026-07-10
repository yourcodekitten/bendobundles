import { render, screen, waitFor, fireEvent } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { vi, describe, it, expect, beforeEach } from 'vitest';
import { MemoryRouter, Route, Routes } from 'react-router-dom';
import { Catalog } from './Catalog';
import type { AdminGame } from '../api';
import { Unauthorized } from '../api';

vi.mock('../api');
import { adminCatalog, adminSetHidden, adminSelfClaim, adminSelfClaims, adminSteamIdentity } from '../api';

function renderCatalog() {
  return render(
    <MemoryRouter initialEntries={['/admin/catalog']}>
      <Routes>
        <Route path="/admin/catalog" element={<Catalog />} />
        <Route path="/admin/login" element={<div>login page</div>} />
      </Routes>
    </MemoryRouter>,
  );
}

// Base fixture — spread and override in per-test mocks
const gameFixture: AdminGame = {
  id: 'gx:base',
  title: 'Base Game',
  bundle: 'Base Bundle',
  key_type: 'steam',
  giftable: false,
  hidden: false,
  status: 'available',
  claim_id: null,
  artwork_url: null,
  requires_choice: false,
  steam_app_id: null,
  owned_by_ben: false,
  steam: null,
};

const gameAvailable: AdminGame = {
  id: 'g1',
  title: 'Hollow Knight',
  bundle: 'Indie Gems Vol 1',
  key_type: 'steam',
  giftable: true,
  hidden: false,
  status: 'available',
  claim_id: null,
  artwork_url: null,
  requires_choice: false,
  steam_app_id: null,
  owned_by_ben: false,
  steam: null,
};

const gamePending: AdminGame = {
  id: 'g2',
  title: 'Celeste',
  bundle: 'Metroidvania Bundle',
  key_type: 'humble',
  giftable: false,
  hidden: true,
  status: 'pending',
  claim_id: 'c-999',
  artwork_url: 'https://example.com/celeste.jpg',
  requires_choice: false,
  steam_app_id: null,
  owned_by_ben: false,
  steam: null,
};

const gameGifted: AdminGame = {
  id: 'g3',
  title: 'Hades',
  bundle: 'Roguelike Pack',
  key_type: 'steam',
  giftable: false,
  hidden: false,
  status: 'gifted',
  claim_id: 'c-100',
  artwork_url: null,
  requires_choice: false,
  steam_app_id: null,
  owned_by_ben: false,
  steam: null,
};

describe('Catalog', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    vi.mocked(adminSelfClaims).mockResolvedValue([]);
    // Default: no steam identity — badges hidden. Override per steam-badge test.
    vi.mocked(adminSteamIdentity).mockResolvedValue(null);
  });

  describe('loading + rendering', () => {
    it('renders games with their titles, bundles, and status badges', async () => {
      vi.mocked(adminCatalog).mockResolvedValue([gameAvailable, gamePending]);
      renderCatalog();

      await waitFor(() => expect(screen.getByText('Hollow Knight')).toBeInTheDocument());

      expect(screen.getByText('Indie Gems Vol 1')).toBeInTheDocument();
      expect(screen.getByText('available')).toBeInTheDocument();
      expect(screen.getByText('Celeste')).toBeInTheDocument();
      expect(screen.getByText('Metroidvania Bundle')).toBeInTheDocument();
      expect(screen.getByText('pending')).toBeInTheDocument();
    });

    it('renders giftable chip only for giftable games', async () => {
      vi.mocked(adminCatalog).mockResolvedValue([gameAvailable, gamePending]);
      renderCatalog();

      await waitFor(() => screen.getByText('Hollow Knight'));

      // gameAvailable is giftable
      expect(screen.getByText('giftable')).toBeInTheDocument();
    });

    it('renders artwork image when artwork_url is set', async () => {
      vi.mocked(adminCatalog).mockResolvedValue([gamePending]);
      renderCatalog();

      await waitFor(() => screen.getByRole('img', { name: 'Celeste' }));
      expect(screen.getByRole('img', { name: 'Celeste' })).toHaveAttribute(
        'src',
        'https://example.com/celeste.jpg',
      );
    });

    it('renders colored fallback div when artwork_url is null', async () => {
      vi.mocked(adminCatalog).mockResolvedValue([gameAvailable]);
      renderCatalog();

      await waitFor(() => screen.getByText('Hollow Knight'));
      // artwork_url is null → no img element
      expect(screen.queryByRole('img')).not.toBeInTheDocument();
    });

    it('renders summary line with counts by status', async () => {
      vi.mocked(adminCatalog).mockResolvedValue([gameAvailable, gamePending, gameGifted]);
      renderCatalog();

      await waitFor(() => screen.getByText('Hollow Knight'));
      // Summary should contain each status with its count
      const summary = screen.getByText(/available.*pending.*gifted|gifted.*available.*pending/i);
      expect(summary).toBeInTheDocument();
    });

    it('renders all status badge colors (spot-check gifted = give accent)', async () => {
      vi.mocked(adminCatalog).mockResolvedValue([gameGifted]);
      renderCatalog();

      await waitFor(() => screen.getByText('Hades'));
      const badge = screen.getByText('gifted');
      expect(badge.className).toMatch(/give/);
    });
  });

  describe('search filtering', () => {
    it('filters by title (case-insensitive)', async () => {
      const user = userEvent.setup();
      vi.mocked(adminCatalog).mockResolvedValue([gameAvailable, gamePending]);
      renderCatalog();

      await waitFor(() => screen.getByText('Hollow Knight'));

      await user.type(screen.getByRole('searchbox'), 'celeste');

      expect(screen.queryByText('Hollow Knight')).not.toBeInTheDocument();
      expect(screen.getByText('Celeste')).toBeInTheDocument();
    });

    it('filters by bundle (case-insensitive)', async () => {
      const user = userEvent.setup();
      vi.mocked(adminCatalog).mockResolvedValue([gameAvailable, gamePending]);
      renderCatalog();

      await waitFor(() => screen.getByText('Hollow Knight'));

      await user.type(screen.getByRole('searchbox'), 'metroid');

      expect(screen.queryByText('Hollow Knight')).not.toBeInTheDocument();
      expect(screen.getByText('Celeste')).toBeInTheDocument();
    });

    it('shows all games when search is empty', async () => {
      vi.mocked(adminCatalog).mockResolvedValue([gameAvailable, gamePending]);
      renderCatalog();

      await waitFor(() => screen.getByText('Hollow Knight'));
      expect(screen.getByText('Celeste')).toBeInTheDocument();
    });
  });

  describe('hidden toggle', () => {
    it('toggle success flips local hidden state', async () => {
      const user = userEvent.setup();
      vi.mocked(adminCatalog).mockResolvedValue([gameAvailable]); // hidden: false
      vi.mocked(adminSetHidden).mockResolvedValue({ ok: true });
      renderCatalog();

      await waitFor(() => screen.getByText('Hollow Knight'));

      const toggle = screen.getByRole('switch', { name: /hide Hollow Knight/i });
      expect(toggle).not.toBeChecked();

      await user.click(toggle);

      await waitFor(() => expect(adminSetHidden).toHaveBeenCalledWith('g1', true));
      expect(toggle).toBeChecked();
    });

    it('toggle success calls adminSetHidden with toggled value', async () => {
      const user = userEvent.setup();
      vi.mocked(adminCatalog).mockResolvedValue([gamePending]); // hidden: true
      vi.mocked(adminSetHidden).mockResolvedValue({ ok: true });
      renderCatalog();

      await waitFor(() => screen.getByText('Celeste'));

      await user.click(screen.getByRole('switch', { name: /hide Celeste/i }));

      await waitFor(() => expect(adminSetHidden).toHaveBeenCalledWith('g2', false));
    });

    it('toggle refused (ok:false) reverts the switch to original state', async () => {
      const user = userEvent.setup();
      vi.mocked(adminCatalog).mockResolvedValue([gameAvailable]); // hidden: false
      vi.mocked(adminSetHidden).mockResolvedValue({
        ok: false,
        message: 'game is currently being claimed',
      });
      renderCatalog();

      await waitFor(() => screen.getByText('Hollow Knight'));

      const toggle = screen.getByRole('switch', { name: /hide Hollow Knight/i });
      expect(toggle).not.toBeChecked(); // starts unchecked

      await user.click(toggle);

      // Must revert to unchecked
      await waitFor(() => expect(toggle).not.toBeChecked());
    });

    it('toggle refused shows the server message inline', async () => {
      const user = userEvent.setup();
      vi.mocked(adminCatalog).mockResolvedValue([gameAvailable]);
      vi.mocked(adminSetHidden).mockResolvedValue({
        ok: false,
        message: 'game is currently being claimed',
      });
      renderCatalog();

      await waitFor(() => screen.getByText('Hollow Knight'));

      await user.click(screen.getByRole('switch', { name: /hide Hollow Knight/i }));

      await waitFor(() =>
        expect(screen.getByText('game is currently being claimed')).toBeInTheDocument(),
      );
    });

    it('concurrent toggles: refused revert on row A does not clobber row B', async () => {
      const user = userEvent.setup();
      vi.mocked(adminCatalog).mockResolvedValue([gameAvailable, gameGifted]); // both hidden:false
      let resolveA!: (r: { ok: false; message: string }) => void;
      vi.mocked(adminSetHidden).mockImplementation((id) => {
        if (id === 'g1') {
          return new Promise((resolve) => {
            resolveA = resolve;
          });
        }
        return Promise.resolve({ ok: true });
      });
      renderCatalog();

      await waitFor(() => screen.getByText('Hollow Knight'));

      const toggleA = screen.getByRole('switch', { name: /hide Hollow Knight/i });
      const toggleB = screen.getByRole('switch', { name: /hide Hades/i });

      await user.click(toggleA); // A in flight, unresolved
      await user.click(toggleB); // B flips and succeeds while A is pending
      await waitFor(() => expect(toggleB).toBeChecked());

      // A comes back refused → only row A reverts
      resolveA({ ok: false, message: 'game is currently being claimed' });

      await waitFor(() => expect(toggleA).not.toBeChecked());
      // B's committed flip must survive A's revert
      expect(toggleB).toBeChecked();
    });

    it('toggle refused error clears on subsequent successful toggle', async () => {
      const user = userEvent.setup();
      vi.mocked(adminCatalog).mockResolvedValue([gameAvailable]);
      // First toggle fails, second succeeds
      vi.mocked(adminSetHidden)
        .mockResolvedValueOnce({
          ok: false,
          message: 'game is currently being claimed',
        })
        .mockResolvedValueOnce({ ok: true });
      renderCatalog();

      await waitFor(() => screen.getByText('Hollow Knight'));

      const toggle = screen.getByRole('switch', { name: /hide Hollow Knight/i });

      // First toggle — fails, shows error
      await user.click(toggle);
      await waitFor(() =>
        expect(screen.getByText('game is currently being claimed')).toBeInTheDocument(),
      );

      // Second toggle — succeeds, error message is cleared
      await user.click(toggle);
      await waitFor(() =>
        expect(screen.queryByText('game is currently being claimed')).not.toBeInTheDocument(),
      );
    });
  });

  describe('error state', () => {
    it('shows error message when load fails', async () => {
      vi.mocked(adminCatalog).mockRejectedValue(new Error('network timeout'));
      renderCatalog();

      await waitFor(() =>
        expect(screen.getByText(/couldn't load the catalog/i)).toBeInTheDocument(),
      );
      expect(screen.getByRole('button', { name: /retry/i })).toBeInTheDocument();
    });

    it('retry button re-calls adminCatalog', async () => {
      const user = userEvent.setup();
      vi.mocked(adminCatalog)
        .mockRejectedValueOnce(new Error('network timeout'))
        .mockResolvedValue([gameAvailable]);

      renderCatalog();

      await waitFor(() => screen.getByRole('button', { name: /retry/i }));
      await user.click(screen.getByRole('button', { name: /retry/i }));

      await waitFor(() => expect(screen.getByText('Hollow Knight')).toBeInTheDocument());
      expect(adminCatalog).toHaveBeenCalledTimes(2);
    });

    // Carried from Task 4 review: withAuth must propagate non-Unauthorized errors
    // to the caller (component error state), not redirect to login.
    it('non-Unauthorized error surfaces as page error state, not a login redirect', async () => {
      vi.mocked(adminCatalog).mockRejectedValue(new Error('ECONNREFUSED'));
      renderCatalog();

      await waitFor(() =>
        expect(screen.getByText(/couldn't load the catalog/i)).toBeInTheDocument(),
      );
      // Login page must NOT be visible
      expect(screen.queryByText('login page')).not.toBeInTheDocument();
    });
  });

  describe('self-claim', () => {
    it('self-claim is two-step: arm then confirm, loud on choice games', async () => {
      vi.mocked(adminCatalog).mockResolvedValue([
        { ...gameFixture, id: 'gk:choice1', title: 'Choicey', status: 'available', requires_choice: true },
      ]);
      vi.mocked(adminSelfClaim).mockResolvedValue({ kind: 'processing' });
      renderCatalog();
      const btn = await screen.findByRole('button', { name: /claim for me/i });
      fireEvent.click(btn);
      expect(adminSelfClaim).not.toHaveBeenCalled();         // armed, not fired
      expect(screen.getByRole('button', { name: /confirm\? spends 1 pick/i })).toBeInTheDocument();
      fireEvent.click(screen.getByRole('button', { name: /confirm\? spends 1 pick/i }));
      await waitFor(() => expect(adminSelfClaim).toHaveBeenCalledTimes(1));
      expect(adminSelfClaim).toHaveBeenCalledWith('gk:choice1');
    });

    it('revealed key panel shows copy box and steam register link for steam keys', async () => {
      vi.mocked(adminCatalog).mockResolvedValue([
        { ...gameFixture, id: 'gk:s1', status: 'available', requires_choice: false, key_type: 'steam' },
      ]);
      vi.mocked(adminSelfClaim).mockResolvedValue({ kind: 'revealed', key: 'AAAA-BBBB', keyType: 'steam' });
      renderCatalog();
      fireEvent.click(await screen.findByRole('button', { name: /claim for me/i }));
      fireEvent.click(screen.getByRole('button', { name: /confirm\?/i }));
      expect(await screen.findByText('AAAA-BBBB')).toBeInTheDocument();
      const link = screen.getByRole('link', { name: /redeem on steam/i });
      expect(link).toHaveAttribute('href', 'https://store.steampowered.com/account/registerkey?key=AAAA-BBBB');
    });

    it('non-steam reveal shows key without the steam button', async () => {
      vi.mocked(adminCatalog).mockResolvedValue([
        { ...gameFixture, id: 'gk:g1', status: 'available', requires_choice: false, key_type: 'gog' },
      ]);
      vi.mocked(adminSelfClaim).mockResolvedValue({ kind: 'revealed', key: 'GOG-KEY-1', keyType: 'gog' });
      renderCatalog();
      fireEvent.click(await screen.findByRole('button', { name: /claim for me/i }));
      fireEvent.click(screen.getByRole('button', { name: /confirm\?/i }));
      expect(await screen.findByText('GOG-KEY-1')).toBeInTheDocument();
      expect(screen.queryByRole('link', { name: /redeem on steam/i })).not.toBeInTheDocument();
    });

    // Carried from final review I2: handleSelfClaim must use withAuth so an expired session
    // navigates to login instead of leaving the button stuck disabled with an unhandled rejection.
    it('adminSelfClaim Unauthorized navigates to login', async () => {
      vi.mocked(adminCatalog).mockResolvedValue([
        { ...gameFixture, id: 'gk:auth1', status: 'available', requires_choice: false },
      ]);
      vi.mocked(adminSelfClaim).mockRejectedValue(new Unauthorized());
      renderCatalog();
      fireEvent.click(await screen.findByRole('button', { name: /claim for me/i }));
      fireEvent.click(screen.getByRole('button', { name: /confirm\?/i }));
      await waitFor(() => expect(screen.getByText('login page')).toBeInTheDocument());
    });
  });

  describe('steam ownership badges', () => {
    it('shows owned-by-ben badge when game.owned_by_ben and adminSteamIdentity is non-null', async () => {
      vi.mocked(adminSteamIdentity).mockResolvedValue('76561198000000001');
      vi.mocked(adminCatalog).mockResolvedValue([
        { ...gameFixture, id: 'gx:owned', owned_by_ben: true, steam_app_id: 730 },
      ]);
      renderCatalog();
      await waitFor(() => expect(screen.getByText('Base Game')).toBeInTheDocument());
      expect(screen.getByText(/already own on steam/i)).toBeInTheDocument();
    });

    it('hides owned-by-ben badge when adminSteamIdentity is null (frozen-stamps caveat)', async () => {
      vi.mocked(adminSteamIdentity).mockResolvedValue(null);
      vi.mocked(adminCatalog).mockResolvedValue([
        { ...gameFixture, id: 'gx:owned2', owned_by_ben: true, steam_app_id: 730 },
      ]);
      renderCatalog();
      await waitFor(() => expect(screen.getByText('Base Game')).toBeInTheDocument());
      expect(screen.queryByText(/already own on steam/i)).not.toBeInTheDocument();
    });

    it('hides owned-by-ben badge when owned_by_ben is false', async () => {
      vi.mocked(adminSteamIdentity).mockResolvedValue('76561198000000001');
      vi.mocked(adminCatalog).mockResolvedValue([
        { ...gameFixture, id: 'gx:notowned', owned_by_ben: false, steam_app_id: 730 },
      ]);
      renderCatalog();
      await waitFor(() => expect(screen.getByText('Base Game')).toBeInTheDocument());
      expect(screen.queryByText(/already own on steam/i)).not.toBeInTheDocument();
    });

    it('armed confirm says "you already own this on steam — sure?" when game.owned_by_ben and identity set', async () => {
      vi.mocked(adminSteamIdentity).mockResolvedValue('76561198000000001');
      vi.mocked(adminCatalog).mockResolvedValue([
        { ...gameFixture, id: 'gx:armown', status: 'available', owned_by_ben: true, steam_app_id: 730 },
      ]);
      vi.mocked(adminSelfClaim).mockResolvedValue({ kind: 'processing' });
      renderCatalog();
      const btn = await screen.findByRole('button', { name: /claim for me/i });
      fireEvent.click(btn);
      expect(
        screen.getByRole('button', { name: /you already own this on steam/i }),
      ).toBeInTheDocument();
    });

    it('armed confirm still says "confirm?" when owned_by_ben is false', async () => {
      vi.mocked(adminSteamIdentity).mockResolvedValue('76561198000000001');
      vi.mocked(adminCatalog).mockResolvedValue([
        { ...gameFixture, id: 'gx:notarmown', status: 'available', owned_by_ben: false },
      ]);
      vi.mocked(adminSelfClaim).mockResolvedValue({ kind: 'processing' });
      renderCatalog();
      const btn = await screen.findByRole('button', { name: /claim for me/i });
      fireEvent.click(btn);
      expect(screen.getByRole('button', { name: /confirm\?/i })).toBeInTheDocument();
      expect(screen.queryByText(/you already own this on steam/i)).not.toBeInTheDocument();
    });

    // FIX 2: when both owned_by_ben=true AND requires_choice=true, the armed confirm must
    // show the pick-cost. Before FIX 2, the owned_by_ben branch swallowed it.
    it('armed confirm shows pick-cost when BOTH owned_by_ben and requires_choice are true (FIX 2)', async () => {
      vi.mocked(adminSteamIdentity).mockResolvedValue('76561198000000001');
      vi.mocked(adminCatalog).mockResolvedValue([
        {
          ...gameFixture,
          id: 'gx:ownedchoice',
          status: 'available',
          owned_by_ben: true,
          requires_choice: true,
          steam_app_id: 730,
        },
      ]);
      vi.mocked(adminSelfClaim).mockResolvedValue({ kind: 'processing' });
      renderCatalog();
      const btn = await screen.findByRole('button', { name: /claim for me/i });
      fireEvent.click(btn);
      // The pick-cost MUST appear even when owned_by_ben is true.
      expect(
        screen.getByRole('button', { name: /spends 1 pick/i }),
      ).toBeInTheDocument();
    });

    // FIX 4: when owned_by_ben=true but steam identity is null, the armed confirm must NOT
    // claim ownership (stale stamp, no connected identity).
    it('armed confirm does NOT claim ownership when owned_by_ben=true but steam identity null (FIX 4)', async () => {
      vi.mocked(adminSteamIdentity).mockResolvedValue(null);
      vi.mocked(adminCatalog).mockResolvedValue([
        {
          ...gameFixture,
          id: 'gx:ownednull',
          status: 'available',
          owned_by_ben: true,
          steam_app_id: 730,
        },
      ]);
      vi.mocked(adminSelfClaim).mockResolvedValue({ kind: 'processing' });
      renderCatalog();
      const btn = await screen.findByRole('button', { name: /claim for me/i });
      fireEvent.click(btn);
      // Must NOT say "you already own this on steam" when steam identity is disconnected.
      expect(
        screen.queryByRole('button', { name: /you already own this on steam/i }),
      ).not.toBeInTheDocument();
      // Should fall back to plain confirm.
      expect(screen.getByRole('button', { name: /confirm\?/i })).toBeInTheDocument();
    });
  });
});

// ── Toolkit wiring (URL state, grouped view, row readout) ─────────────────────

function renderCatalogAt(entry: string) {
  return render(
    <MemoryRouter initialEntries={[entry]}>
      <Routes>
        <Route path="/admin/catalog" element={<Catalog />} />
        <Route path="/admin/login" element={<div>login page</div>} />
      </Routes>
    </MemoryRouter>,
  );
}

const steamNull = null;
const troveGames: AdminGame[] = [
  {
    ...gameFixture,
    id: 'tk:action',
    title: 'Action Hit',
    bundle: 'June 2026',
    steam: {
      genres: ['Action'],
      developers: ['DevA'],
      publishers: ['Pub House'],
      release_date: '12 Nov 2019',
      release_date_iso: '2019-11-12',
      review_desc: 'Very Positive',
      review_percent: 94,
      review_count: 1200,
      recent_percent: 91,
    },
  },
  {
    ...gameFixture,
    id: 'tk:indie',
    title: 'Indie Gem',
    bundle: 'June 2026',
    steam: {
      genres: ['Indie'],
      developers: ['DevB'],
      publishers: ['Pub House'],
      release_date: 'Mar 2021',
      release_date_iso: '2021-03-01',
      review_desc: 'Mixed',
      review_percent: 55,
      review_count: 300,
      recent_percent: 50,
    },
  },
  {
    ...gameFixture,
    id: 'tk:zork',
    title: 'Zork Prime',
    bundle: 'March 2025',
    steam: steamNull,
  },
];

describe('Catalog toolkit wiring', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    vi.mocked(adminSelfClaims).mockResolvedValue([]);
    vi.mocked(adminSteamIdentity).mockResolvedValue(null);
    vi.mocked(adminCatalog).mockResolvedValue(troveGames);
  });

  it('restores toolkit state from URL params and filters accordingly', async () => {
    renderCatalogAt(
      '/admin/catalog?tags=Action&rating=very-positive&sort=date-new&group=publisher&q=action',
    );
    await waitFor(() => screen.getByText('Action Hit'));

    expect((screen.getByLabelText('search games') as HTMLInputElement).value).toBe('action');
    expect((screen.getByLabelText('rating') as HTMLSelectElement).value).toBe('very-positive');
    expect((screen.getByLabelText('sort') as HTMLSelectElement).value).toBe('date-new');
    expect((screen.getByLabelText('group') as HTMLSelectElement).value).toBe('publisher');
    expect(screen.queryByText('Indie Gem')).not.toBeInTheDocument();
    expect(screen.queryByText('Zork Prime')).not.toBeInTheDocument();
  });

  it('toggling a tag chip filters without reload; clear filters restores the trove', async () => {
    renderCatalogAt('/admin/catalog');
    await waitFor(() => screen.getByText('Action Hit'));

    fireEvent.click(screen.getByText('tags'));
    fireEvent.click(screen.getByRole('button', { name: 'Action (1)' }));
    await waitFor(() => expect(screen.queryByText('Indie Gem')).not.toBeInTheDocument());
    expect(screen.queryByText('Zork Prime')).not.toBeInTheDocument();
    expect(screen.getByText(/1 missing tag or rating data hidden/)).toBeInTheDocument();

    fireEvent.click(screen.getByRole('button', { name: 'clear filters' }));
    await waitFor(() => expect(screen.getByText('Indie Gem')).toBeInTheDocument());
    expect(screen.getByText('Zork Prime')).toBeInTheDocument();
  });

  it('group=publisher renders collapsible sections with counts, unmapped last', async () => {
    renderCatalogAt('/admin/catalog?group=publisher');
    await waitFor(() => screen.getByText('Action Hit'));

    // Group headers are <summary> elements — tag chips also end in "(n)",
    // so scope the query by tag name before asserting order.
    const headers = screen
      .getAllByText(/\(\d+\)$/)
      .filter((el) => el.tagName === 'SUMMARY')
      .map((el) => el.textContent?.trim().replace(/\s+/g, ' '));
    expect(headers).toEqual(['Pub House (2)', 'unmapped (1)']);
  });

  it('stateful panels render once under grouping: revealed key appears only at first appearance of a multi-publisher game', async () => {
    vi.mocked(adminCatalog).mockResolvedValue([
      {
        ...gameFixture,
        id: 'tk:multi',
        title: 'Multi Pub Game',
        status: 'available',
        key_type: 'steam',
        steam: {
          genres: ['Action'],
          developers: ['DevA'],
          publishers: ['Big Pub', 'Small Pub'],
          release_date: null,
          release_date_iso: null,
          review_desc: null,
          review_percent: null,
          review_count: null,
          recent_percent: null,
        },
      },
    ]);
    vi.mocked(adminSelfClaim).mockResolvedValue({
      kind: 'revealed',
      key: 'DUPE-CHECK-1',
      keyType: 'steam',
    });
    renderCatalogAt('/admin/catalog?group=publisher');
    await waitFor(() => screen.getAllByText('Multi Pub Game'));

    // the game row itself IS honestly duplicated across both publisher groups
    expect(screen.getAllByText('Multi Pub Game')).toHaveLength(2);

    const claims = screen.getAllByRole('button', { name: /claim for me/i });
    fireEvent.click(claims[0]!);
    fireEvent.click(screen.getAllByRole('button', { name: /confirm\?/i })[0]!);

    // ...but the revealed key panel renders exactly once (first appearance)
    await waitFor(() => screen.getByText('DUPE-CHECK-1'));
    expect(screen.getAllByText('DUPE-CHECK-1')).toHaveLength(1);
  });

  it('rows show a compact rating/date readout when steam data exists', async () => {
    renderCatalogAt('/admin/catalog');
    await waitFor(() => screen.getByText('Action Hit'));

    expect(screen.getByText('Very Positive · 94% · 2019')).toBeInTheDocument();
    expect(screen.getByText('Mixed · 55% · 2021')).toBeInTheDocument();
    // steam-null row: no readout — its row renders only title + bundle
    expect(screen.queryByText(/null/)).not.toBeInTheDocument();
  });
});
