import { useEffect, useSyncExternalStore, type ReactNode } from 'react';
import { useParams } from 'react-router-dom';
import { fetchGameDetail, type GameView } from '../api';
import { titleColorClass } from '../titleColor';

// Steam genres per game, fetched once and cached for the page's lifetime.
// Chips mount once per card, so the cache + external store keeps every
// instance in sync off a single fetch per game.
const genreCache = new Map<string, string[]>();
let genreVersion = 0;
const genreListeners = new Set<() => void>();
const genreSubscribe = (fn: () => void) => {
  genreListeners.add(fn);
  return () => genreListeners.delete(fn);
};
const genreSnapshot = () => genreVersion;
const genreNotify = () => {
  genreVersion += 1;
  genreListeners.forEach((fn) => fn());
};

function GenreChips({
  game,
  children,
}: {
  game: GameView;
  /** Called with the first 4 steam genres, or null when unavailable (no
      appid, not yet fetched, or no enrichment) — render the thin fallback. */
  children: (genres: string[] | null) => ReactNode;
}) {
  const { token } = useParams<{ token: string }>();
  useSyncExternalStore(genreSubscribe, genreSnapshot, genreSnapshot);
  useEffect(() => {
    if (game.steam_app_id === null || token === undefined || genreCache.has(game.id)) return;
    genreCache.set(game.id, []); // pending sentinel — dedupes concurrent mounts
    fetchGameDetail(token, game.id)
      .then((d) => {
        genreCache.set(game.id, d.steam?.detail?.genres ?? []);
        genreNotify();
      })
      .catch(() => {
        genreCache.set(game.id, []);
        genreNotify();
      });
  }, [game.id, game.steam_app_id, token]);
  const cached = genreCache.get(game.id);
  return children(cached !== undefined && cached.length > 0 ? cached.slice(0, 4) : null);
}

interface GameGridProps {
  games: GameView[];
  /** Set of Steam appids the viewer already owns — shows "you own this" pill. */
  owned?: Set<number>;
  /** Opens the detail modal — via the details button or the card body. Claiming
      happens inside the modal (the grid never claims directly; see DESIGN.md,
      The Button Burgundy Rule). */
  onDetail: (game: GameView) => void;
}

export function GameGrid({ games, owned, onDetail }: GameGridProps) {
  // Group by title; preserve server order — first occurrence wins the card
  const seen = new Map<string, { game: GameView; count: number }>();
  for (const game of games) {
    const entry = seen.get(game.title);
    if (entry !== undefined) {
      entry.count += 1;
    } else {
      seen.set(game.title, { game, count: 1 });
    }
  }

  return (
    <section className="grid grid-cols-1 gap-4 p-6 sm:grid-cols-2 lg:grid-cols-3">
      {Array.from(seen.values()).map(({ game, count }) => {
        const youOwnThis =
          game.steam_app_id !== null &&
          owned !== undefined &&
          owned.has(game.steam_app_id);

        return (
          /* the game card is a DMG cartridge: grip ridges, shelf plastic,
             label-sticker art, the asymmetric bottom corner (DESIGN.md §5) */
          <div
            key={game.title}
            className="rounded-[6px_6px_20px_6px] bg-shelf overflow-hidden cursor-pointer"
            onClick={() => onDetail(game)}
          >
            <div
              aria-hidden="true"
              className="h-2.5 [background:repeating-linear-gradient(90deg,var(--color-control)_0_10px,var(--color-shelf)_10px_20px)]"
            />
            <div className="px-3 pt-2">
              {game.artwork_url !== null ? (
                <img
                  src={game.artwork_url}
                  alt={game.title}
                  className="w-full aspect-video object-cover"
                />
              ) : (
                <div className={`w-full aspect-video ${titleColorClass(game.title)}`}>
                  {game.steam_app_id !== null && (
                    <img
                      src={`https://shared.akamai.steamstatic.com/store_item_assets/steam/apps/${game.steam_app_id}/capsule_616x353.jpg`}
                      alt={game.title}
                      loading="lazy"
                      className="h-full w-full object-cover"
                      onError={(e) => {
                        e.currentTarget.style.display = 'none';
                      }}
                    />
                  )}
                </div>
              )}
            </div>
            <div className="p-4">
              <h3 className="font-pixel text-xl font-semibold leading-tight">{game.title}</h3>
              <p className="mt-1 text-xs text-ink-soft truncate">{game.bundle}</p>
              <div className="mt-2 flex flex-wrap gap-1.5">
                {/* genre chips replace the key_type chip when steam genres are
                    cached; tag colors ride the shared title-hash palette
                    (The Title-Hash Rule) tinted toward floor for chip duty */}
                <GenreChips game={game}>
                  {(genres) =>
                    genres === null ? (
                      /* floor chip — the shelf chip vanishes on the shelf card */
                      <span className="rounded bg-floor px-2 py-0.5 text-xs text-ink-soft">
                        {game.key_type}
                      </span>
                    ) : (
                      <>
                        {genres.map((genre) => {
                          const hue = `var(${titleColorClass(genre).replace('bg-', '--color-')})`;
                          return (
                            <span
                              key={genre}
                              className="rounded px-2 py-0.5 text-xs"
                              style={{
                                background: `color-mix(in oklch, ${hue}, var(--color-floor) 70%)`,
                                color: `color-mix(in oklch, ${hue}, oklch(15% 0.02 110) 35%)`,
                              }}
                            >
                              {genre}
                            </span>
                          );
                        })}
                      </>
                    )
                  }
                </GenreChips>
                {count > 1 && (
                  <span className="rounded bg-control px-2 py-0.5 text-xs text-ink-soft">
                    ×{count} copies
                  </span>
                )}
                {youOwnThis && (
                  <span className="rounded bg-blue-900 px-2 py-0.5 text-xs text-blue-200">
                    you own this
                  </span>
                )}
              </div>
              <button
                type="button"
                onClick={(e) => { e.stopPropagation(); onDetail(game); }}
                className="mt-3 w-full rounded bg-control px-3 py-1.5 text-sm font-medium text-ink hover:bg-control-bright"
              >
                details
              </button>
            </div>
          </div>
        );
      })}
    </section>
  );
}
