import { describe, expect, it } from 'vitest';
import type { AdminGame } from './api';
import {
  buildAlmanac,
  countLine,
  headline,
  isWaiting,
  nameMonth,
  orderKey,
  subtitle,
  thumbSrc,
  type AlmanacEntry,
} from './almanac';

const Y = 2026; // injected current year — never the real clock

function game(over: Partial<AdminGame>): AdminGame {
  return {
    id: 'gk1:mn1',
    title: 'a game',
    bundle: 'Some Bundle',
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
    ...over,
  };
}

describe('nameMonth', () => {
  it('reads both measured spellings to the same (year, month)', () => {
    expect(nameMonth('October 2021', Y)).toEqual({ year: 2021, month: 9 });
    expect(nameMonth('October 2021 Humble Choice', Y)).toEqual({ year: 2021, month: 9 });
  });
  it('is strict: anything else falls through (null)', () => {
    expect(nameMonth('october 2021', Y)).toBeNull(); // case: measured names are capitalised
    expect(nameMonth('Humble Monthly — March 2017', Y)).toBeNull(); // pre-Choice era string
    expect(nameMonth('October 2021 Bundle', Y)).toBeNull();
    expect(nameMonth('Sept 2021', Y)).toBeNull();
  });
  it('bounds the year to [2010, currentYear + 1]', () => {
    expect(nameMonth('March 0000', Y)).toBeNull();
    expect(nameMonth('March 2009', Y)).toBeNull();
    expect(nameMonth('March 2010', Y)).toEqual({ year: 2010, month: 2 });
    expect(nameMonth('January 2027', Y)).toEqual({ year: 2027, month: 0 });
    expect(nameMonth('January 2028', Y)).toBeNull();
  });
});

describe('isWaiting — exactly Game::is_listable', () => {
  it('needs available AND giftable AND not hidden', () => {
    expect(isWaiting(game({}))).toBe(true);
    expect(isWaiting(game({ status: 'ben_redeemed' }))).toBe(false);
    expect(isWaiting(game({ giftable: false }))).toBe(false);
    expect(isWaiting(game({ hidden: true }))).toBe(false);
  });
});

describe('orderKey / thumbSrc', () => {
  it('orderKey is the gamekey prefix of id', () => {
    expect(orderKey(game({ id: 'zApDsS:wingspan_steam' }))).toBe('zApDsS');
    expect(orderKey(game({ id: 'no-colon' }))).toBe('no-colon');
  });
  it('thumbSrc prefers artwork, then the steam capsule, else null', () => {
    expect(thumbSrc(game({ artwork_url: 'https://hb.imgix.net/x.png', steam_app_id: 1 }))).toBe(
      'https://hb.imgix.net/x.png',
    );
    expect(thumbSrc(game({ steam_app_id: 413150 }))).toBe(
      'https://shared.akamai.steamstatic.com/store_item_assets/steam/apps/413150/capsule_616x353.jpg',
    );
    expect(thumbSrc(game({}))).toBeNull();
  });
});

describe('buildAlmanac', () => {
  it('merges both spellings of one Choice month into ONE entry, and the name beats the postmark', () => {
    const a = buildAlmanac(
      [
        game({ id: 'p1:a', bundle: 'November 2021', requires_choice: true }),
        // order key redeemed on the evening of nov 30 EST ⇒ postmark says DEC; the name wins
        game({ id: 'zApDsS:b', bundle: 'November 2021 Humble Choice', status: 'ben_redeemed', acquired_at: '2021-12-01T01:45:48.153511Z' }),
      ],
      Y,
    );
    expect(a.years).toHaveLength(1);
    expect(a.years[0]!.year).toBe(2021);
    expect(a.years[0]!.months).toEqual([
      expect.objectContaining({ month: 10, entries: [expect.objectContaining({ source: 'name', label: 'november 2021' })] }),
    ]);
    expect(a.years[0]!.months[0]!.entries[0]!.games).toHaveLength(2);
  });

  it('groups the postmark path by ORDER, never by name (9 orders share one name in prod)', () => {
    const a = buildAlmanac(
      [
        game({ id: 'k1:a', bundle: 'A very special gift just for you', acquired_at: '2014-05-10T12:00:00Z' }),
        game({ id: 'k2:b', bundle: 'A very special gift just for you', acquired_at: '2019-02-01T12:00:00Z' }),
      ],
      Y,
    );
    expect(a.years.map((y) => y.year)).toEqual([2019, 2014]);
    expect(a.years.every((y) => y.months[0]!.entries[0]!.source === 'postmark')).toBe(true);
  });

  it('merges same-label orders inside ONE month (4 identical gift cards in jul 2026, real render), never across months', () => {
    const a = buildAlmanac(
      [
        game({ id: 'k1:a', bundle: 'A very special gift just for you', acquired_at: '2026-07-02T12:00:00Z' }),
        game({ id: 'k2:b', bundle: 'A very special gift just for you', acquired_at: '2026-07-20T12:00:00Z' }),
        game({ id: 'k3:c', bundle: 'A very special gift just for you', acquired_at: '2026-06-20T12:00:00Z' }),
        game({ id: 'k4:d', bundle: 'Other', acquired_at: '2026-07-05T12:00:00Z' }),
      ],
      Y,
    );
    const jul = a.years[0]!.months.find((m) => m.month === 6)!.entries;
    expect(jul.map((e) => [e.label, e.games.length])).toEqual([
      ['A very special gift just for you', 2],
      ['Other', 1],
    ]);
    expect(a.years[0]!.months.find((m) => m.month === 5)!.entries).toHaveLength(1); // june stays its own
  });

  it('dates an order by its EARLIEST valid acquired_at and keeps its undated siblings with it', () => {
    const a = buildAlmanac(
      [
        game({ id: 'k1:a', bundle: 'B', acquired_at: '2015-06-02T00:00:00Z' }),
        game({ id: 'k1:b', bundle: 'B', acquired_at: '2015-04-02T00:00:00Z' }),
        game({ id: 'k1:c', bundle: 'B' }),
        game({ id: 'k1:d', bundle: 'B', acquired_at: 'junk' }),
      ],
      Y,
    );
    expect(a.years[0]!.months[0]!.month).toBe(3); // april, the earliest
    expect(a.years[0]!.months[0]!.entries[0]!.games).toHaveLength(4);
    expect(a.undated).toEqual([]);
  });

  it('a pre-Choice name with a postmark lands on its ORDER date, not undated (Lilith Q3)', () => {
    const a = buildAlmanac(
      [game({ id: 'hm:a', bundle: 'Humble Monthly — March 2017', acquired_at: '2017-03-03T00:00:00Z' })],
      Y,
    );
    expect(a.years[0]!.year).toBe(2017);
    expect(a.years[0]!.months[0]!.entries[0]!.source).toBe('postmark');
  });

  it('never invents a date: no name-month and no postmark ⇒ the undated shelf', () => {
    const a = buildAlmanac([game({ id: 'u:a', bundle: 'Mystery Box' })], Y);
    expect(a.years).toEqual([]);
    expect(a.undated).toEqual([expect.objectContaining({ source: 'undated', label: 'Mystery Box' })]);
    expect(a.span).toBeNull();
  });

  it('orders years and months newest first, entries by label then key', () => {
    const a = buildAlmanac(
      [
        game({ id: 'a:1', bundle: 'March 2020' }),
        game({ id: 'b:1', bundle: 'Zeta', acquired_at: '2020-07-09T00:00:00Z' }),
        game({ id: 'c:1', bundle: 'Alpha', acquired_at: '2020-07-01T00:00:00Z' }),
        game({ id: 'd:1', bundle: 'June 2023' }),
      ],
      Y,
    );
    expect(a.years.map((y) => y.year)).toEqual([2023, 2020]);
    expect(a.years[1]!.months.map((m) => m.month)).toEqual([6, 2]);
    expect(a.years[1]!.months[0]!.entries.map((e) => e.label)).toEqual(['Alpha', 'Zeta']);
    expect(a.span).toBe(4); // 2023 − 2020 + 1, derived
  });

  it('counts waiting and tucked per entry', () => {
    const a = buildAlmanac(
      [
        game({ id: 'p:1', bundle: 'May 2022' }),
        game({ id: 'p:2', bundle: 'May 2022', hidden: true }),
        game({ id: 'p:3', bundle: 'May 2022', status: 'gifted' }),
      ],
      Y,
    );
    const e = a.years[0]!.months[0]!.entries[0]!;
    expect(e.waiting.map((g) => g.id)).toEqual(['p:1']);
    expect(e.tucked).toBe(1);
  });

  it('counts N over EVERY waiting choice pick, whatever its date source (OMBB D5)', () => {
    const a = buildAlmanac(
      [
        game({ id: 'p:1', bundle: 'May 2022', requires_choice: true }),
        game({ id: 'p:2', bundle: 'May 2022', requires_choice: true }),
        game({ id: 'p:3', bundle: 'June 2022', requires_choice: true }),
        game({ id: 'p:4', bundle: 'June 2022', requires_choice: true, status: 'gifted' }), // not waiting
        game({ id: 'r:1', bundle: 'Choice: Renamed Month', requires_choice: true }), // a rename
      ],
      Y,
    );
    expect(a.picks).toEqual({ waiting: 4, months: 2, unparsed: 1 });
  });
});

describe('copy', () => {
  function entry(over: Partial<AlmanacEntry>): AlmanacEntry {
    return { key: 'k', source: 'name', label: 'may 2022', games: [], waiting: [], tucked: 0, ...over };
  }
  const g = game({});

  it('countLine: all waiting / some waiting / none / single / tucked', () => {
    expect(countLine(entry({ games: [g, g, g], waiting: [g, g, g] }))).toBe('3 treasures · all 3 still waiting');
    expect(countLine(entry({ games: [g, g, g], waiting: [g] }))).toBe('3 treasures · 1 still waiting');
    expect(countLine(entry({ games: [g, g] }))).toBe('2 treasures · all given or kept ♡');
    expect(countLine(entry({ games: [g], waiting: [g] }))).toBe('1 treasure · still waiting');
    expect(countLine(entry({ games: [g, g], waiting: [g], tucked: 1 }))).toBe('2 treasures · 1 still waiting · 1 tucked away');
    expect(countLine(entry({ games: [g, g], tucked: 2 }))).toBe('2 treasures · all tucked away');
    expect(countLine(entry({ games: [g, g, g], tucked: 1 }))).toBe('3 treasures · the rest given or kept ♡ · 1 tucked away');
  });

  it('headline: conditional, pluralised, never "0 picks"', () => {
    const base = { years: [], undated: [], span: null };
    expect(headline({ ...base, picks: { waiting: 0, months: 0, unparsed: 0 } })).toBeNull();
    expect(headline({ ...base, picks: { waiting: 589, months: 75, unparsed: 0 } })).toBe(
      '589 choice picks still waiting, across 75 months — never spent.',
    );
    expect(headline({ ...base, picks: { waiting: 1, months: 1, unparsed: 0 } })).toBe(
      '1 choice pick still waiting, across 1 month — never spent.',
    );
    expect(headline({ ...base, picks: { waiting: 2, months: 0, unparsed: 2 } })).toBe(
      '2 choice picks still waiting — never spent.',
    );
  });

  it('subtitle derives the span and drops it when unknown', () => {
    const base = { years: [], undated: [], picks: { waiting: 0, months: 0, unparsed: 0 } };
    expect(subtitle({ ...base, span: 15 })).toBe('15 years of the attic, month by month.');
    expect(subtitle({ ...base, span: 1 })).toBe('1 year of the attic, month by month.');
    expect(subtitle({ ...base, span: null })).toBe('the attic, month by month.');
  });
});
