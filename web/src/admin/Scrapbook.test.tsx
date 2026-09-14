import { render, screen, within } from '@testing-library/react';
import { vi, describe, it, expect, beforeEach, afterEach } from 'vitest';
import { MemoryRouter } from 'react-router-dom';
import { Scrapbook } from './Scrapbook';
import type { ScrapbookEntry, ScrapbookView } from '../api';

// Partial mock, Friends.test.tsx's shape: stub the fetcher, keep everything
// else real.
vi.mock('../api', async (importOriginal) => {
  const actual = await importOriginal<typeof import('../api')>();
  return {
    ...actual,
    adminScrapbook: vi.fn(),
  };
});
import { adminScrapbook } from '../api';

const emptyView: ScrapbookView = {
  entries: [],
  waiting: [],
  doors_open: [],
  orphan_claim_count: 0,
  stale_pending_count: 0,
};

function entry(over: Partial<ScrapbookEntry>): ScrapbookEntry {
  return {
    claimed_at: '2023-09-20T12:00:00Z',
    state: 'fulfilled',
    game: {
      id: 'g-x',
      title: 'some game',
      artwork_url: null,
      acquired_at: '2014-08-15T00:00:00Z',
    },
    recipient: 'sam',
    gift_note: null,
    tag: null,
    thank_note: null,
    thanked_at: null,
    link_token: 'tok1',
    link_label: 'label-tok1',
    ...over,
  };
}

// entries arrive SERVER-SORTED (claimed_at, game_id) — the page must preserve
// that order inside each year group.
const storyView: ScrapbookView = {
  ...emptyView,
  entries: [
    entry({
      claimed_at: '2019-05-10T10:00:00Z',
      game: {
        id: 'g-old',
        title: 'old treasure',
        artwork_url: 'https://art/old.png',
        acquired_at: '2014-08-15T00:00:00Z',
      },
      recipient: 'sam',
      gift_note: 'for the rainy days',
      tag: 'the sticker',
      thank_note: 'thank you ben!!',
      thanked_at: '2019-05-11T09:00:00Z',
    }),
    entry({
      claimed_at: '2023-09-20T12:00:00Z',
      game: { id: 'g-a', title: 'alpha game', artwork_url: null, acquired_at: null },
    }),
    entry({
      claimed_at: '2023-09-20T12:00:00Z',
      game: {
        id: 'g-b',
        title: 'beta game',
        artwork_url: null,
        // postdates the claim — bad data, the waited clause must be omitted
        acquired_at: '2024-01-01T00:00:00Z',
      },
    }),
    entry({
      claimed_at: '2026-09-13T12:00:00Z',
      state: 'pending',
      game: {
        id: 'g-p',
        title: 'mid unwrap',
        artwork_url: null,
        acquired_at: '2020-03-01T00:00:00Z',
      },
    }),
  ],
};

function renderPage() {
  return render(
    <MemoryRouter>
      <Scrapbook />
    </MemoryRouter>,
  );
}

beforeEach(() => {
  vi.mocked(adminScrapbook).mockReset();
});

describe('Scrapbook — the story', () => {
  it('groups by year of unwrap, oldest year first, with a jump-list', async () => {
    vi.mocked(adminScrapbook).mockResolvedValue(storyView);
    renderPage();
    await screen.findByText('old treasure');
    const headings = screen.getAllByRole('heading', { level: 2 });
    const years = headings.map((h) => h.textContent).filter((t) => /^\d{4}$/.test(t ?? ''));
    expect(years).toEqual(['2019', '2023', '2026']);
    // jump-list: one in-page link per year
    const nav = screen.getByLabelText('years');
    expect(within(nav).getByRole('link', { name: '2019' })).toHaveAttribute('href', '#y2019');
    expect(within(nav).getByRole('link', { name: '2026' })).toHaveAttribute('href', '#y2026');
  });

  it('preserves the server (claimed_at, game_id) order inside a year', async () => {
    vi.mocked(adminScrapbook).mockResolvedValue(storyView);
    renderPage();
    await screen.findByText('alpha game');
    const y2023 = document.getElementById('y2023');
    expect(y2023).not.toBeNull();
    const titles = within(y2023 as HTMLElement)
      .getAllByRole('heading', { level: 3 })
      .map((h) => h.textContent);
    expect(titles).toEqual(['alpha game', 'beta game']);
  });

  it('keepsake card shows recipient, notes, and thanks as one unit', async () => {
    vi.mocked(adminScrapbook).mockResolvedValue(storyView);
    renderPage();
    const card = (await screen.findByText('old treasure')).closest('article') as HTMLElement;
    expect(within(card).getByText(/for sam/)).toBeInTheDocument();
    expect(within(card).getByText('for the rainy days')).toBeInTheDocument();
    expect(within(card).getByText(/the sticker/)).toBeInTheDocument();
    // thanks render as ONE unit: note + date in the same line
    expect(within(card).getByText(/thank you ben!!.*may 2019/)).toBeInTheDocument();
  });

  it('postmark span ends at the unwrap: bought aug 2014 · opened may 2019 — waited 4 years', async () => {
    vi.mocked(adminScrapbook).mockResolvedValue(storyView);
    renderPage();
    const card = (await screen.findByText('old treasure')).closest('article') as HTMLElement;
    expect(
      within(card).getByText('bought aug 2014 · opened may 2019 — waited 4 years'),
    ).toBeInTheDocument();
  });

  it('span line omitted when acquired_at is null; waited clause omitted when negative', async () => {
    vi.mocked(adminScrapbook).mockResolvedValue(storyView);
    renderPage();
    const alpha = (await screen.findByText('alpha game')).closest('article') as HTMLElement;
    // no "bought" — but still shows opened
    expect(within(alpha).queryByText(/bought/)).toBeNull();
    expect(within(alpha).getByText(/opened sep 2023/)).toBeInTheDocument();
    const beta = (await screen.findByText('beta game')).closest('article') as HTMLElement;
    // acquired postdates claim: bought+opened shown, waited clause omitted
    expect(within(beta).getByText(/bought jan 2024 · opened sep 2023$/)).toBeInTheDocument();
  });

  it('pending entry wears the unwrapping… badge', async () => {
    vi.mocked(adminScrapbook).mockResolvedValue(storyView);
    renderPage();
    const card = (await screen.findByText('mid unwrap')).closest('article') as HTMLElement;
    expect(within(card).getByText('unwrapping…')).toBeInTheDocument();
  });

  it('the card deep-links to its owning links-tab row', async () => {
    vi.mocked(adminScrapbook).mockResolvedValue(storyView);
    renderPage();
    const card = (await screen.findByText('old treasure')).closest('article') as HTMLElement;
    const link = within(card).getByRole('link', { name: /for sam/ });
    expect(link.getAttribute('href')).toBe('/admin/links#link-tok1');
  });

  it('shows the soft empty state when there are no entries', async () => {
    vi.mocked(adminScrapbook).mockResolvedValue(emptyView);
    renderPage();
    expect(await screen.findByText(/no gifts opened yet/)).toBeInTheDocument();
  });
});

describe('Scrapbook — waiting, doors, summary, footnotes', () => {
  beforeEach(() => {
    vi.useFakeTimers({ toFake: ['Date'] });
    vi.setSystemTime(new Date('2026-09-14T12:00:00Z'));
  });
  afterEach(() => {
    vi.useRealTimers();
  });

  const richView: ScrapbookView = {
    entries: [
      entry({ claimed_at: '2023-04-01T10:00:00Z', recipient: 'sam' }),
      entry({
        claimed_at: '2023-05-01T10:00:00Z',
        recipient: 'sam',
        game: { id: 'g-2', title: 'second', artwork_url: null, acquired_at: null },
      }),
      entry({
        claimed_at: '2023-06-01T10:00:00Z',
        recipient: 'sarah bday',
        thank_note: 'ty!',
        thanked_at: '2023-06-02T10:00:00Z',
        game: { id: 'g-3', title: 'third', artwork_url: null, acquired_at: null },
      }),
    ],
    waiting: [
      {
        link_token: 'tok-w',
        link_label: 'label-w',
        recipient: 'sam',
        sealed_until: '2026-12-25T15:00:00Z',
        games: [
          {
            id: 'g-wait',
            title: 'patient treasure',
            artwork_url: null,
            acquired_at: '2020-03-01T00:00:00Z',
          },
        ],
      },
      {
        link_token: 'tok-w2',
        link_label: 'label-w2',
        recipient: 'alex',
        sealed_until: null,
        games: [
          { id: 'g-w2', title: 'quiet wait', artwork_url: null, acquired_at: null },
        ],
      },
    ],
    doors_open: [
      {
        link_token: 'tok-d',
        link_label: 'label-d',
        recipient: 'sam',
        claims_left: 2,
        created_at: '2024-09-01T00:00:00Z',
      },
      {
        link_token: 'tok-d2',
        link_label: 'label-d2',
        recipient: 'jo',
        claims_left: 1,
        created_at: '2026-09-01T00:00:00Z', // under a year — clause omitted
      },
    ],
    orphan_claim_count: 0,
    stale_pending_count: 0,
  };

  it('chosen and waiting: recipient named, sealed group says wrapped until', async () => {
    vi.mocked(adminScrapbook).mockResolvedValue(richView);
    renderPage();
    await screen.findByText('patient treasure');
    expect(screen.getByText(/chosen and waiting/)).toBeInTheDocument();
    const sealedGroup = screen.getByText(/for sam.*wrapped until dec 25/);
    expect(sealedGroup).toBeInTheDocument();
    // unsealed group carries no wrapped-until
    expect(screen.getByText(/^for alex$/)).toBeInTheDocument();
    // waiting span runs to now (frozen): bought mar 2020, waiting 6 years
    expect(screen.getByText(/waiting 6 years/)).toBeInTheDocument();
  });

  it('doors left open is its own heading — recipient named, one line each', async () => {
    vi.mocked(adminScrapbook).mockResolvedValue(richView);
    renderPage();
    await screen.findByText(/doors left open/);
    expect(
      screen.getByText('the door ben left open for sam · open 2 years · 2 claims left'),
    ).toBeInTheDocument();
    expect(
      screen.getByText('the door ben left open for jo · 1 claim left'),
    ).toBeInTheDocument();
  });

  it('summary sentence counts distinct recipients as people', async () => {
    vi.mocked(adminScrapbook).mockResolvedValue(richView);
    renderPage();
    expect(
      await screen.findByText('3 gifts opened by 2 people · 1 thank-you ♡'),
    ).toBeInTheDocument();
  });

  it('quiet footnotes render ONLY when counts are nonzero', async () => {
    vi.mocked(adminScrapbook).mockResolvedValue(richView);
    const first = renderPage();
    await first.findByText('patient treasure');
    expect(screen.queryByText(/can't find/)).toBeNull();
    expect(screen.queryByText(/stuck mid-unwrap/)).toBeNull();
    first.unmount();
    vi.mocked(adminScrapbook).mockResolvedValue({
      ...richView,
      orphan_claim_count: 1,
      stale_pending_count: 2,
    });
    renderPage();
    expect(
      await screen.findByText('1 claim references a link this page can\'t find'),
    ).toBeInTheDocument();
    expect(
      screen.getByText('2 gifts are stuck mid-unwrap — see ops'),
    ).toBeInTheDocument();
  });
});
