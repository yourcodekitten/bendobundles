import {
  IDLE_TOOLKIT,
  type GroupKey,
  type RatingFloor,
  type SortKey,
  type ToolkitState,
} from './catalogToolkit';

// Shared control classes — same visual family as the catalog search input.
const controlClass =
  'rounded border border-line bg-floor px-3 py-1.5 text-sm text-ink focus:border-pixel focus:outline-none';

const RATING_OPTIONS: { value: RatingFloor; label: string }[] = [
  { value: 'any', label: 'any' },
  { value: 'mixed', label: 'at least Mixed' },
  { value: 'mostly-positive', label: 'at least Mostly Positive' },
  { value: 'very-positive', label: 'at least Very Positive' },
  { value: 'overwhelmingly-positive', label: 'at least Overwhelmingly Positive' },
];
const SORT_OPTIONS: { value: SortKey; label: string }[] = [
  { value: 'title', label: 'title a–z' },
  { value: 'rating', label: 'rating' },
  { value: 'date-new', label: 'newest' },
  { value: 'date-old', label: 'oldest' },
];
const GROUP_OPTIONS: { value: GroupKey; label: string }[] = [
  { value: 'none', label: 'none' },
  { value: 'publisher', label: 'publisher' },
  { value: 'studio', label: 'studio' },
  { value: 'bundle', label: 'bundle month' },
];

/** Controlled toolkit controls for the catalog: tag chips, rating floor,
 * sort, group, and an honest visibility summary. All state lives with the
 * caller (Catalog keeps it in the URL). */
export function ToolkitBar({
  state,
  tagOptions,
  shown,
  total,
  excludedNoData,
  onChange,
}: {
  state: ToolkitState;
  tagOptions: { tag: string; count: number }[];
  shown: number;
  total: number;
  excludedNoData: number;
  onChange: (next: ToolkitState) => void;
}) {
  const filtersActive =
    state.q !== '' || state.tags.length > 0 || state.rating !== 'any';

  const toggleTag = (tag: string) =>
    onChange({
      ...state,
      tags: state.tags.includes(tag)
        ? state.tags.filter((t) => t !== tag)
        : [...state.tags, tag],
    });

  return (
    <div className="mb-4 flex flex-wrap items-start gap-4">
      <details open={state.tags.length > 0} className="min-w-0">
        <summary className="cursor-pointer text-sm text-dust hover:text-ink-soft">
          tags{state.tags.length > 0 ? ` (${state.tags.length})` : ''}
        </summary>
        <div className="mt-2 flex max-w-2xl flex-wrap gap-1.5">
          {tagOptions.map(({ tag, count }) => {
            const selected = state.tags.includes(tag);
            return (
              <button
                key={tag}
                type="button"
                onClick={() => toggleTag(tag)}
                className={
                  selected
                    ? 'rounded border border-pixel bg-pixel/20 px-2 py-0.5 text-xs text-ink'
                    : 'rounded border border-line bg-floor px-2 py-0.5 text-xs text-dust hover:text-ink-soft'
                }
              >
                {tag} ({count})
              </button>
            );
          })}
        </div>
      </details>

      <label className="flex items-center gap-2 text-sm text-dust">
        rating
        <select
          aria-label="rating"
          value={state.rating}
          onChange={(e) => onChange({ ...state, rating: e.target.value as RatingFloor })}
          className={controlClass}
        >
          {RATING_OPTIONS.map((o) => (
            <option key={o.value} value={o.value}>
              {o.label}
            </option>
          ))}
        </select>
      </label>

      <label className="flex items-center gap-2 text-sm text-dust">
        sort
        <select
          aria-label="sort"
          value={state.sort}
          onChange={(e) => onChange({ ...state, sort: e.target.value as SortKey })}
          className={controlClass}
        >
          {SORT_OPTIONS.map((o) => (
            <option key={o.value} value={o.value}>
              {o.label}
            </option>
          ))}
        </select>
      </label>

      <label className="flex items-center gap-2 text-sm text-dust">
        group
        <select
          aria-label="group"
          value={state.group}
          onChange={(e) => onChange({ ...state, group: e.target.value as GroupKey })}
          className={controlClass}
        >
          {GROUP_OPTIONS.map((o) => (
            <option key={o.value} value={o.value}>
              {o.label}
            </option>
          ))}
        </select>
      </label>

      <div className="ml-auto flex items-center gap-3 text-sm text-dust-faint">
        {filtersActive ? (
          <>
            <span>
              showing {shown} of {total}
              {/* counts games dropped for LACKING the filtered field (unmapped,
                  no genres, or unrated/low-count review desc) — not "no steam
                  data": a mapped-but-unrated game lands here too. */}
              {excludedNoData > 0 ? ` · ${excludedNoData} missing tag or rating data hidden` : ''}
            </span>
            <button
              type="button"
              onClick={() =>
                onChange({ ...IDLE_TOOLKIT, sort: state.sort, group: state.group })
              }
              className="rounded border border-line bg-floor px-2 py-0.5 text-xs text-dust hover:text-ink-soft"
            >
              clear filters
            </button>
          </>
        ) : (
          <span>{total} games</span>
        )}
      </div>
    </div>
  );
}
