import { describe, it, expect } from 'vitest';
import type { AdminGame, SteamSummary } from '../api';
import {
  IDLE_TOOLKIT,
  applyToolkit,
  collectTagOptions,
  type ToolkitState,
} from './catalogToolkit';

// ── Fixtures ──────────────────────────────────────────────────────────────────

const base: AdminGame = {
  id: 'base',
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

const steamBase: SteamSummary = {
  genres: [],
  developers: [],
  publishers: [],
  release_date: null,
  release_date_iso: null,
  review_desc: null,
  review_percent: null,
  review_count: null,
  recent_percent: null,
};

function g(id: string, over: Partial<AdminGame>, steam?: Partial<SteamSummary>): AdminGame {
  return {
    ...base,
    id,
    title: id,
    ...over,
    steam: steam === undefined ? (over.steam ?? null) : { ...steamBase, ...steam },
  };
}

const ids = (r: ReturnType<typeof applyToolkit>) =>
  r.groups.flatMap((grp) => grp.games.map((x) => x.id));

const state = (over: Partial<ToolkitState>): ToolkitState => ({ ...IDLE_TOOLKIT, ...over });

// ── collectTagOptions ─────────────────────────────────────────────────────────

describe('collectTagOptions', () => {
  it('unions genres with counts, sorted count-desc then name-asc; steam-null contributes nothing', () => {
    const games = [
      g('a', {}, { genres: ['Action', 'Co-op'] }),
      g('b', {}, { genres: ['Action'] }),
      g('c', {}, { genres: ['Indie'] }),
      g('d', {}), // mapped, no genres
      { ...base, id: 'e' }, // unmapped
    ];
    expect(collectTagOptions(games)).toEqual([
      { tag: 'Action', count: 2 },
      { tag: 'Co-op', count: 1 },
      { tag: 'Indie', count: 1 },
    ]);
  });
});

// ── filter ────────────────────────────────────────────────────────────────────

describe('applyToolkit filtering', () => {
  it('search matches title OR bundle, case-insensitive (existing semantics)', () => {
    const games = [
      g('zork', { title: 'Zork Prime' }),
      g('bun', { title: 'Other', bundle: 'Zorky Bundle' }),
      g('nope', { title: 'Unrelated' }),
    ];
    expect(ids(applyToolkit(games, state({ q: 'zork' })))).toEqual(['bun', 'zork']);
  });

  it('tags are AND; steam-less games are excluded and counted while a tag filter is active', () => {
    const games = [
      g('both', {}, { genres: ['Action', 'Co-op'] }),
      g('one', {}, { genres: ['Action'] }),
      { ...base, id: 'unmapped' },
    ];
    const r = applyToolkit(games, state({ tags: ['Action', 'Co-op'] }));
    expect(ids(r)).toEqual(['both']);
    expect(r.excludedNoData).toBe(1);
    expect(r.shown).toBe(1);
  });

  it('rating floor keeps rank >= floor; unrated and unknown descs excluded + counted', () => {
    const games = [
      g('op', {}, { review_desc: 'Overwhelmingly Positive' }),
      g('vp', {}, { review_desc: 'Very Positive' }),
      g('pos', {}, { review_desc: 'Positive' }),
      g('mixed', {}, { review_desc: 'Mixed' }),
      g('weird', {}, { review_desc: '3 user reviews' }),
      g('unrated', {}),
      { ...base, id: 'unmapped' },
    ];
    const vp = applyToolkit(games, state({ rating: 'very-positive' }));
    expect(ids(vp)).toEqual(['op', 'vp']);
    expect(vp.excludedNoData).toBe(3); // weird + unrated + unmapped

    const mixed = applyToolkit(games, state({ rating: 'mixed' }));
    expect(ids(mixed)).toEqual(['mixed', 'op', 'pos', 'vp']);
  });

  it("rating 'any' keeps everything including steam-null", () => {
    const games = [g('a', {}, { review_desc: 'Mixed' }), { ...base, id: 'unmapped' }];
    const r = applyToolkit(games, state({}));
    expect(r.shown).toBe(2);
    expect(r.excludedNoData).toBe(0);
  });
});

// ── sort ──────────────────────────────────────────────────────────────────────

describe('applyToolkit sorting', () => {
  it('rating: percent desc, count tiebreak, unrated last, title tiebreak', () => {
    const games = [
      g('b90small', {}, { review_percent: 90, review_count: 10 }),
      g('a90big', {}, { review_percent: 90, review_count: 100 }),
      g('c95', {}, { review_percent: 95, review_count: 5 }),
      g('unrated', {}),
    ];
    expect(ids(applyToolkit(games, state({ sort: 'rating' })))).toEqual([
      'c95',
      'a90big',
      'b90small',
      'unrated',
    ]);
  });

  it('date-new / date-old: lexicographic on iso; null iso sinks in BOTH directions', () => {
    const games = [
      g('old', {}, { release_date_iso: '2001-01-01' }),
      g('new', {}, { release_date_iso: '2024-06-30' }),
      g('nodate', {}),
    ];
    expect(ids(applyToolkit(games, state({ sort: 'date-new' })))).toEqual([
      'new',
      'old',
      'nodate',
    ]);
    expect(ids(applyToolkit(games, state({ sort: 'date-old' })))).toEqual([
      'old',
      'new',
      'nodate',
    ]);
  });

  it('title: locale compare (default)', () => {
    const games = [g('b', { title: 'beta' }), g('a', { title: 'Alpha' })];
    expect(ids(applyToolkit(games, state({})))).toEqual(['a', 'b']);
  });
});

// ── group ─────────────────────────────────────────────────────────────────────

describe('applyToolkit grouping', () => {
  it('publisher: multi-publisher duplicates honestly; count-desc order; empty + unmapped buckets last', () => {
    const games = [
      g('multi', {}, { publishers: ['Big Pub', 'Small Pub'] }),
      g('big1', {}, { publishers: ['Big Pub'] }),
      g('empty', {}, { publishers: [] }),
      { ...base, id: 'unmapped' },
    ];
    const r = applyToolkit(games, state({ group: 'publisher' }));
    expect(r.groups.map((grp) => [grp.label, grp.games.map((x) => x.id)])).toEqual([
      ['Big Pub', ['big1', 'multi']],
      ['Small Pub', ['multi']],
      ['no publisher listed', ['empty']],
      ['unmapped', ['unmapped']],
    ]);
  });

  it('studio groups over developers; bundle groups over the bundle field', () => {
    const games = [
      g('x', { bundle: 'June 2026' }, { developers: ['DevA'] }),
      g('y', { bundle: 'June 2026' }, { developers: ['DevA'] }),
      g('z', { bundle: 'March 2025' }, { developers: [] }),
    ];
    const byStudio = applyToolkit(games, state({ group: 'studio' }));
    expect(byStudio.groups.map((grp) => grp.label)).toEqual(['DevA', 'no studio listed']);
    const byBundle = applyToolkit(games, state({ group: 'bundle' }));
    expect(byBundle.groups.map((grp) => [grp.label, grp.games.length])).toEqual([
      ['June 2026', 2],
      ['March 2025', 1],
    ]);
  });

  it("group 'none' yields one anonymous group", () => {
    const r = applyToolkit([g('a', {})], state({}));
    expect(r.groups).toHaveLength(1);
    expect(r.groups[0]!.label).toBeNull();
  });

  it('filter runs before grouping; sort applies within each group', () => {
    const games = [
      g('kept-b', { title: 'b' }, { genres: ['Action'], publishers: ['P'] }),
      g('kept-a', { title: 'a' }, { genres: ['Action'], publishers: ['P'] }),
      g('dropped', { title: 'c' }, { genres: ['Indie'], publishers: ['P'] }),
    ];
    const r = applyToolkit(games, state({ tags: ['Action'], group: 'publisher' }));
    expect(r.groups).toHaveLength(1);
    expect(r.groups[0]!.games.map((x) => x.id)).toEqual(['kept-a', 'kept-b']);
  });
});
