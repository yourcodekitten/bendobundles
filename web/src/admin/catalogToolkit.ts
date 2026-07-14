import type { AdminGame } from '../api';
import { displayTags, isMature } from '../tags';

export type RatingFloor =
  | 'any'
  | 'mixed'
  | 'mostly-positive'
  | 'very-positive'
  | 'overwhelmingly-positive';
export type SortKey = 'title' | 'rating' | 'date-new' | 'date-old';
export type MatureFilter = 'all' | 'hide' | 'only';
export type GroupKey = 'none' | 'publisher' | 'studio' | 'bundle';

export type ToolkitState = {
  q: string;
  tags: string[];
  rating: RatingFloor;
  sort: SortKey;
  group: GroupKey;
  /** 🔞 policy over content_descriptor_ids (#71): show all / hide flagged / only flagged. */
  mature: MatureFilter;
};

export const IDLE_TOOLKIT: ToolkitState = {
  q: '',
  tags: [],
  rating: 'any',
  sort: 'title',
  group: 'none',
  mature: 'all',
};

// Steam's review ladder, worst→best. Rank = index. An unknown desc (Steam's
// "3 user reviews" placeholder, future wording changes) ranks as unrated:
// excluded while a floor is active, never silently promoted.
const LADDER = [
  'Overwhelmingly Negative',
  'Very Negative',
  'Negative',
  'Mostly Negative',
  'Mixed',
  'Mostly Positive',
  'Positive',
  'Very Positive',
  'Overwhelmingly Positive',
] as const;
const FLOOR_RANK: Record<Exclude<RatingFloor, 'any'>, number> = {
  mixed: LADDER.indexOf('Mixed'),
  'mostly-positive': LADDER.indexOf('Mostly Positive'),
  'very-positive': LADDER.indexOf('Very Positive'),
  'overwhelmingly-positive': LADDER.indexOf('Overwhelmingly Positive'),
};

export function collectTagOptions(games: AdminGame[]): { tag: string; count: number }[] {
  const counts = new Map<string, number>();
  for (const g of games)
    // Same chips you see = same chips you filter: community tags, genre fallback (#71).
    for (const t of displayTags(g.steam ?? {})) counts.set(t, (counts.get(t) ?? 0) + 1);
  return [...counts]
    .map(([tag, count]) => ({ tag, count }))
    .sort((a, b) => b.count - a.count || a.tag.localeCompare(b.tag));
}

function matchesSearch(g: AdminGame, q: string): boolean {
  if (q === '') return true;
  return g.title.toLowerCase().includes(q) || g.bundle.toLowerCase().includes(q);
}

/** Filter → sort → group. `excludedNoData` counts games a steam-field filter
 * (tags / rating floor) dropped solely for having no data — the UI surfaces
 * this so the trove never silently shrinks. */
export function applyToolkit(
  games: AdminGame[],
  state: ToolkitState,
): {
  groups: { label: string | null; games: AdminGame[] }[];
  shown: number;
  excludedNoData: number;
} {
  const q = state.q.toLowerCase();
  let excludedNoData = 0;
  const filtered = games.filter((g) => {
    if (!matchesSearch(g, q)) return false;
    if (state.tags.length > 0) {
      const tags = displayTags(g.steam ?? {});
      if (tags.length === 0) {
        excludedNoData++;
        return false;
      }
      if (!state.tags.every((t) => tags.includes(t))) return false;
    }
    if (state.rating !== 'any') {
      const desc = g.steam?.review_desc;
      const rank = desc ? LADDER.indexOf(desc as (typeof LADDER)[number]) : -1;
      if (rank === -1) {
        excludedNoData++;
        return false;
      }
      if (rank < FLOOR_RANK[state.rating]) return false;
    }
    if (state.mature !== 'all') {
      const flagged = isMature(g.steam?.content_descriptor_ids);
      if (state.mature === 'hide' && flagged) return false;
      if (state.mature === 'only' && !flagged) {
        // unmapped rows aren't provably mature — under 'only' they're no-data exclusions
        if (g.steam === null) excludedNoData++;
        return false;
      }
    }
    return true;
  });

  const sorted = [...filtered].sort(comparator(state.sort));
  return { groups: groupGames(sorted, state.group), shown: filtered.length, excludedNoData };
}

function comparator(key: SortKey): (a: AdminGame, b: AdminGame) => number {
  const byTitle = (a: AdminGame, b: AdminGame) => a.title.localeCompare(b.title);
  switch (key) {
    case 'title':
      return byTitle;
    case 'rating':
      return (a, b) => {
        const ap = a.steam?.review_percent ?? -1;
        const bp = b.steam?.review_percent ?? -1;
        if (ap !== bp) return bp - ap; // unrated (-1) sinks
        const ac = a.steam?.review_count ?? 0;
        const bc = b.steam?.review_count ?? 0;
        if (ac !== bc) return bc - ac;
        return byTitle(a, b);
      };
    case 'date-new':
    case 'date-old': {
      const dir = key === 'date-new' ? -1 : 1;
      return (a, b) => {
        const ai = a.steam?.release_date_iso ?? null;
        const bi = b.steam?.release_date_iso ?? null;
        if (ai === null && bi === null) return byTitle(a, b);
        if (ai === null) return 1; // no-date sinks in both directions
        if (bi === null) return -1;
        if (ai !== bi) return dir * ai.localeCompare(bi);
        return byTitle(a, b);
      };
    }
  }
}

const UNMAPPED = 'unmapped';

function groupGames(
  games: AdminGame[],
  key: GroupKey,
): { label: string | null; games: AdminGame[] }[] {
  if (key === 'none') return [{ label: null, games }];
  const emptyLabel = key === 'publisher' ? 'no publisher listed' : 'no studio listed';
  const buckets = new Map<string, AdminGame[]>();
  const push = (label: string, g: AdminGame) => {
    const b = buckets.get(label);
    if (b) b.push(g);
    else buckets.set(label, [g]);
  };
  for (const g of games) {
    if (key === 'bundle') {
      push(g.bundle, g); // bundle is always present — no unmapped bucket
      continue;
    }
    if (!g.steam) {
      push(UNMAPPED, g);
      continue;
    }
    const vals = key === 'publisher' ? g.steam.publishers : g.steam.developers;
    if (vals.length === 0) push(emptyLabel, g);
    else for (const v of vals) push(v, g); // multi-valued: honest duplication
  }
  // Missing-data buckets pin to the tail (unmapped after empty), everything
  // else sorts by size desc then label.
  const tail = (label: string) => (label === UNMAPPED ? 2 : label === emptyLabel ? 1 : 0);
  return [...buckets]
    .map(([label, gs]) => ({ label: label as string | null, games: gs }))
    .sort(
      (a, b) =>
        tail(a.label as string) - tail(b.label as string) ||
        b.games.length - a.games.length ||
        (a.label as string).localeCompare(b.label as string),
    );
}
