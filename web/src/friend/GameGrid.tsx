import type { GameView } from '../api';
import { titleColorClass } from '../titleColor';

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
          <div
            key={game.title}
            className="rounded-lg bg-floor overflow-hidden cursor-pointer"
            onClick={() => onDetail(game)}
          >
            {game.artwork_url !== null ? (
              <img
                src={game.artwork_url}
                alt={game.title}
                className="w-full aspect-video object-cover"
              />
            ) : (
              <div
                className={`w-full aspect-video ${titleColorClass(game.title)}`}
                aria-hidden="true"
              />
            )}
            <div className="p-4">
              <h3 className="text-xl font-medium leading-tight">{game.title}</h3>
              <p className="mt-1 text-xs text-ink-soft truncate">{game.bundle}</p>
              <div className="mt-2 flex flex-wrap gap-1.5">
                <span className="rounded bg-shelf px-2 py-0.5 text-xs text-ink-soft">
                  {game.key_type}
                </span>
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
