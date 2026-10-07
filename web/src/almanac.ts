// 📜 the almanac (docs/spec-almanac.md) — ben's attic as a calendar of months.
// Pure and deterministic: no Date.now(), no locale/ICU (postmark.ts's rule); the
// current year is a parameter. Helper module per web convention (postmark.ts,
// tags.ts): the page component only renders what this returns.
import type { AdminGame } from './api';
import { postmarkMonth, type YearMonth } from './postmark';

export const MONTH_NAMES = [
  'january', 'february', 'march', 'april', 'may', 'june',
  'july', 'august', 'september', 'october', 'november', 'december',
] as const;
const CAPITALISED = MONTH_NAMES.map((m) => m.charAt(0).toUpperCase() + m.slice(1));
// D1: strict on purpose — a miss falls through to the postmark/undated, never to a
// wrong month. Measured over prod 2026-10-07: 590/590 discovery picks and 22/22
// order-key stragglers match (21 steam + Diablo IV, blizzard_keyless).
const NAME_MONTH_RE = new RegExp(`^(${CAPITALISED.join('|')}) (\\d{4})( Humble Choice)?$`);
const MIN_YEAR = 2010;

/** D1 ①: the month a bundle's NAME says it is, or null. Year bounded to
 *  [2010, currentYear + 1] so a typo can never draw a `0000` divider. */
export function nameMonth(bundle: string, currentYear: number): YearMonth | null {
  const m = NAME_MONTH_RE.exec(bundle);
  if (m === null) return null;
  const year = Number(m[2]);
  if (year < MIN_YEAR || year > currentYear + 1) return null;
  return { year, month: CAPITALISED.indexOf(m[1] as string) };
}

/** Exactly `Game::is_listable` (domain/src/lib.rs). One definition of "waiting". */
export function isWaiting(g: AdminGame): boolean {
  return g.status === 'available' && g.giftable && !g.hidden;
}

/** The order a game came from: the gamekey prefix of `id` ("{gamekey}:{machine_name}").
 *  D1 ②: a NAME is not a bundle — 9 prod orders share "A very special gift just for you". */
export function orderKey(g: AdminGame): string {
  const i = g.id.indexOf(':');
  return i === -1 ? g.id : g.id.slice(0, i);
}

/** The thumb fallback LADDER, in order: humble artwork → the SMALL steam capsule. The page
 *  walks it on each load error, then falls to the colour block (review 1: one boolean skipped
 *  the capsule whenever the artwork failed). 231x87 not GameGrid's 616x353: same availability
 *  and ~7.8x fewer bytes over a 12-app sample (measured 2026-10-07), drawn at 64x40 anyway. */
export function thumbSrcs(g: AdminGame): string[] {
  const out: string[] = [];
  if (g.artwork_url !== null) out.push(g.artwork_url);
  if (g.steam_app_id !== null) {
    out.push(`https://shared.akamai.steamstatic.com/store_item_assets/steam/apps/${g.steam_app_id}/capsule_231x87.jpg`);
  }
  return out;
}

/** The first rung of the ladder, or null. */
export function thumbSrc(g: AdminGame): string | null {
  return thumbSrcs(g)[0] ?? null;
}

export type EntrySource = 'name' | 'postmark' | 'undated';
export type AlmanacEntry = {
  key: string;
  source: EntrySource;
  label: string;
  games: AdminGame[];
  waiting: AdminGame[];
  tucked: number;
};
export type AlmanacMonth = { month: number; entries: AlmanacEntry[] };
export type AlmanacYear = { year: number; months: AlmanacMonth[] };
export type Almanac = {
  years: AlmanacYear[];
  undated: AlmanacEntry[];
  /** newest − oldest dated year + 1; null when nothing is dated. Derived, never typed. */
  span: number | null;
  picks: { waiting: number; months: number; unparsed: number };
};

function makeEntry(key: string, source: EntrySource, label: string, games: AdminGame[]): AlmanacEntry {
  return { key, source, label, games, waiting: games.filter(isWaiting), tucked: games.filter((g) => g.hidden).length };
}

// plain code-unit comparison — deterministic, no ICU (localeCompare is locale-dependent)
function cmp(a: string, b: string): number {
  return a < b ? -1 : a > b ? 1 : 0;
}
/** The month's own name entry (its Choice shelf) leads; then labels case-insensitively;
 *  key breaks ties. Raw code units sorted every Capitalised bundle above 'november 2021'
 *  and buried the lit picks (review 1). toLowerCase is locale-free (toLocaleLowerCase is not). */
function byLabelThenKey(a: AlmanacEntry, b: AlmanacEntry): number {
  const nameFirst = Number(b.source === 'name') - Number(a.source === 'name');
  return nameFirst || cmp(a.label.toLowerCase(), b.label.toLowerCase()) || cmp(a.key, b.key);
}

/** Inside ONE month, postmark entries that share a label read as one shelf: the date is
 *  identical at month grain, so nothing D1 protects is lost (the 9-orders case was ACROSS
 *  years and stays split). Found in the real render: 4 identical one-treasure gift cards
 *  under jul 2026. The merged key is the smallest member key — deterministic. */
function mergeSameLabelOrders(entries: AlmanacEntry[]): AlmanacEntry[] {
  const out: AlmanacEntry[] = [];
  const byLabel = new Map<string, number>();
  for (const e of entries) {
    if (e.source !== 'postmark') {
      out.push(e);
      continue;
    }
    const at = byLabel.get(e.label);
    if (at === undefined) {
      byLabel.set(e.label, out.length);
      out.push(e);
      continue;
    }
    const prev = out[at]!;
    out[at] = makeEntry(prev.key < e.key ? prev.key : e.key, 'postmark', e.label, [...prev.games, ...e.games]);
  }
  return out;
}

export function buildAlmanac(games: AdminGame[], currentYear: number): Almanac {
  const named = new Map<string, { ym: YearMonth; games: AdminGame[] }>();
  const orders = new Map<string, { label: string; games: AdminGame[] }>();
  for (const g of games) {
    const nm = nameMonth(g.bundle, currentYear);
    if (nm !== null) {
      const k = `${nm.year}-${String(nm.month).padStart(2, '0')}`;
      const slot = named.get(k) ?? { ym: nm, games: [] };
      slot.games.push(g);
      named.set(k, slot);
    } else {
      const k = orderKey(g);
      const slot = orders.get(k) ?? { label: g.bundle, games: [] };
      slot.games.push(g);
      orders.set(k, slot);
    }
  }

  const dated: { ym: YearMonth; entry: AlmanacEntry }[] = [];
  const undated: AlmanacEntry[] = [];
  for (const [k, { ym, games: gs }] of named) {
    dated.push({ ym, entry: makeEntry(`name:${k}`, 'name', `${MONTH_NAMES[ym.month]} ${ym.year}`, gs) });
  }
  for (const [k, { label, games: gs }] of orders) {
    // D1 ②: the order's EARLIEST valid instant, bucketed by the chip's own function (D2)
    let earliest: string | undefined;
    for (const g of gs) {
      if (g.acquired_at === undefined || Number.isNaN(Date.parse(g.acquired_at))) continue;
      if (earliest === undefined || Date.parse(g.acquired_at) < Date.parse(earliest)) earliest = g.acquired_at;
    }
    const ym = postmarkMonth(earliest);
    if (ym === null) undated.push(makeEntry(`order:${k}`, 'undated', label, gs));
    else dated.push({ ym, entry: makeEntry(`order:${k}`, 'postmark', label, gs) });
  }

  const byYear = new Map<number, Map<number, AlmanacEntry[]>>();
  for (const { ym, entry } of dated) {
    const months = byYear.get(ym.year) ?? new Map<number, AlmanacEntry[]>();
    const list = months.get(ym.month) ?? [];
    list.push(entry);
    months.set(ym.month, list);
    byYear.set(ym.year, months);
  }
  const years: AlmanacYear[] = [...byYear.entries()]
    .sort(([a], [b]) => b - a)
    .map(([year, months]) => ({
      year,
      months: [...months.entries()]
        .sort(([a], [b]) => b - a)
        .map(([month, entries]) => ({ month, entries: mergeSameLabelOrders(entries).sort(byLabelThenKey) })),
    }));
  undated.sort(byLabelThenKey);

  // D5: N over EVERY waiting choice pick — a rename must show as `unparsed`, never shrink N
  const picks = games.filter((g) => g.requires_choice && isWaiting(g));
  const pickMonths = new Set<string>();
  let unparsed = 0;
  for (const g of picks) {
    const nm = nameMonth(g.bundle, currentYear);
    if (nm === null) unparsed += 1;
    else pickMonths.add(`${nm.year}-${nm.month}`);
  }

  const span = years.length === 0 ? null : years[0]!.year - years[years.length - 1]!.year + 1;
  return { years, undated, span, picks: { waiting: picks.length, months: pickMonths.size, unparsed } };
}

export function countLine(e: AlmanacEntry): string {
  const n = e.games.length;
  const w = e.waiting.length;
  const noun = n === 1 ? 'treasure' : 'treasures';
  if (w === 0 && e.tucked === n && n > 0) return `${n} ${noun} · all tucked away`;
  let s: string;
  // with some tucked away, "all given or kept" would be false — say "the rest" (plan review)
  if (w === 0) s = e.tucked > 0 ? `${n} ${noun} · the rest given or kept ♡` : `${n} ${noun} · all given or kept ♡`;
  else if (w === n) s = n === 1 ? '1 treasure · still waiting' : `${n} ${noun} · all ${n} still waiting`;
  else s = `${n} ${noun} · ${w} still waiting`;
  if (e.tucked > 0) s += ` · ${e.tucked} tucked away`;
  return s;
}

/** D5: one sentence, never a card; null when nothing is waiting (no "0 picks"). */
export function headline(a: Almanac): string | null {
  const { waiting, months } = a.picks;
  if (waiting === 0) return null;
  const picks = `${waiting} choice ${waiting === 1 ? 'pick' : 'picks'} still waiting`;
  if (months === 0) return `${picks} — never spent.`;
  return `${picks}, across ${months} ${months === 1 ? 'month' : 'months'} — never spent.`;
}

export function subtitle(a: Almanac): string {
  if (a.span === null) return 'the attic, month by month.';
  return `${a.span} ${a.span === 1 ? 'year' : 'years'} of the attic, month by month.`;
}
