import { fireEvent, render, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { vi, describe, it, expect, beforeEach } from 'vitest';
import { MemoryRouter, Route, Routes, useLocation } from 'react-router-dom';
import { Almanac } from './Almanac';
import type { AdminGame } from '../api';

vi.mock('../api', async (importOriginal) => {
  const actual = await importOriginal<typeof import('../api')>();
  return { ...actual, adminCatalog: vi.fn() };
});
import { adminCatalog } from '../api';

function game(over: Partial<AdminGame>): AdminGame {
  return {
    id: 'gk1:mn1', title: 'a game', bundle: 'Some Bundle', key_type: 'steam', giftable: true,
    hidden: false, status: 'available', claim_id: null, artwork_url: null, requires_choice: false,
    steam_app_id: null, owned_by_ben: false, steam: null, ...over,
  };
}

function LinksProbe() {
  const loc = useLocation();
  return <pre data-testid="picked">{JSON.stringify(loc.state)}</pre>;
}

function renderPage() {
  return render(
    <MemoryRouter initialEntries={['/admin/almanac']}>
      <Routes>
        <Route path="/admin/almanac" element={<Almanac currentYear={2026} />} />
        <Route path="/admin/links" element={<LinksProbe />} />
      </Routes>
    </MemoryRouter>,
  );
}

const fixture: AdminGame[] = [
  game({ id: 'p:1', title: 'pick one', bundle: 'September 2026', requires_choice: true }),
  game({ id: 'p:2', title: 'pick two', bundle: 'September 2026', requires_choice: true }),
  game({ id: 'o:1', title: 'old friend', bundle: 'Humble Indie Bundle 8', status: 'ben_redeemed', acquired_at: '2013-03-27T18:22:58Z' }),
  game({ id: 'u:1', title: 'mystery', bundle: 'Mystery Box', status: 'ben_redeemed' }),
];

describe('Almanac', () => {
  beforeEach(() => {
    vi.mocked(adminCatalog).mockReset();
  });

  it('renders the derived subtitle, the headline, years newest-first, and the undated shelf', async () => {
    vi.mocked(adminCatalog).mockResolvedValue(fixture);
    renderPage();
    expect(await screen.findByText('14 years of the attic, month by month.')).toBeInTheDocument();
    expect(screen.getByText('2 choice picks still waiting, across 1 month — never spent.')).toBeInTheDocument();
    const years = screen.getAllByRole('heading', { level: 2 }).map((h) => h.textContent);
    expect(years).toEqual(['2026', '2013', 'undated']);
    expect(screen.getByText(/the attic doesn't know when these arrived/)).toBeInTheDocument();
  });

  it('says which source dated each entry', async () => {
    vi.mocked(adminCatalog).mockResolvedValue(fixture);
    renderPage();
    const named = await screen.findByRole('article', { name: /september 2026/ });
    expect(within(named).getByText('🗓️')).toBeInTheDocument();
    expect(within(named).getByText('2 treasures · all 2 still waiting')).toBeInTheDocument();
    const posted = screen.getByRole('article', { name: /Humble Indie Bundle 8/ });
    expect(within(posted).getByText('📮')).toBeInTheDocument();
    expect(within(posted).getByText('1 treasure · all given or kept ♡')).toBeInTheDocument();
  });

  it('"wrap these" hands ONLY the waiting games to Links via the catalog contract', async () => {
    vi.mocked(adminCatalog).mockResolvedValue([
      ...fixture,
      game({ id: 'p:3', title: 'given away', bundle: 'September 2026', requires_choice: true, status: 'gifted' }),
    ]);
    renderPage();
    const named = await screen.findByRole('article', { name: /september 2026/ });
    await userEvent.click(within(named).getByRole('button', { name: /wrap the 2 waiting/ }));
    expect(JSON.parse(screen.getByTestId('picked').textContent ?? 'null')).toEqual({
      picked: [
        { id: 'p:1', title: 'pick one', requiresChoice: true },
        { id: 'p:2', title: 'pick two', requiresChoice: true },
      ],
    });
  });

  it('has no wrap button on an entry with nothing waiting', async () => {
    vi.mocked(adminCatalog).mockResolvedValue(fixture);
    renderPage();
    const posted = await screen.findByRole('article', { name: /Humble Indie Bundle 8/ });
    expect(within(posted).queryByRole('button')).toBeNull();
  });

  it('names choice picks whose month could not be read (OMBB D5)', async () => {
    vi.mocked(adminCatalog).mockResolvedValue([
      game({ id: 'r:1', bundle: 'Choice: Renamed', requires_choice: true }),
    ]);
    renderPage();
    expect(await screen.findByText("⚠️ 1 choice pick whose month we couldn't read")).toBeInTheDocument();
  });

  it('caps the art strip at 8 and says how many more', async () => {
    vi.mocked(adminCatalog).mockResolvedValue(
      Array.from({ length: 11 }, (_, i) => game({ id: `p:${i}`, title: `g${i}`, bundle: 'May 2022', steam_app_id: 100 + i })),
    );
    renderPage();
    const e = await screen.findByRole('article', { name: /may 2022/ });
    // thumbs are decorative (alt="" ⇒ role presentation), so count the DOM, not roles
    expect(e.querySelectorAll('img')).toHaveLength(8);
    expect(within(e).getByText('+3')).toBeInTheDocument();
  });

  it('a thumb whose image fails to load falls back to the colour block, never a broken icon (real render: 11/858)', async () => {
    vi.mocked(adminCatalog).mockResolvedValue([game({ id: 'p:1', bundle: 'May 2022', steam_app_id: 999, title: 'gone' })]);
    renderPage();
    const e = await screen.findByRole('article', { name: /may 2022/ });
    const img = e.querySelector('img')!;
    fireEvent.error(img);
    expect(e.querySelector('img')).toBeNull();
    expect(within(e).getByTitle('gone').querySelector('div[aria-hidden="true"]')).not.toBeNull();
  });

  it('shows a retry on load failure', async () => {
    vi.mocked(adminCatalog).mockRejectedValue(new Error('boom'));
    renderPage();
    expect(await screen.findByText("couldn't open the almanac — try again")).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'retry' })).toBeInTheDocument();
  });

  it('dims a hidden thumb but keeps it in the strip (D6)', async () => {
    vi.mocked(adminCatalog).mockResolvedValue([
      game({ id: 'p:1', bundle: 'May 2022', steam_app_id: 1 }),
      game({ id: 'p:2', bundle: 'May 2022', steam_app_id: 2, hidden: true, title: 'tucked' }),
    ]);
    renderPage();
    const e = await screen.findByRole('article', { name: /may 2022/ });
    expect(e.querySelectorAll('img')).toHaveLength(2);
    expect(within(e).getByTitle('tucked').className).toContain('opacity-40');
  });

  it('renders the undated shelf when ONLY an unparsed dated pick exists', async () => {
    vi.mocked(adminCatalog).mockResolvedValue([
      game({ id: 'r:1', bundle: 'Choice: Renamed', requires_choice: true, acquired_at: '2024-02-02T00:00:00Z' }),
    ]);
    renderPage();
    expect(await screen.findByRole('heading', { level: 2, name: 'undated' })).toBeInTheDocument();
    expect(screen.getByText("⚠️ 1 choice pick whose month we couldn't read")).toBeInTheDocument();
  });
});
