import type { GameView } from '../api';
import { titleColorClass } from '../titleColor';

interface GameGridProps {
  games: GameView[];
  active: boolean;
  onClaim: (game: GameView) => void;
  /** Set of Steam appids the viewer already owns — shows "you own this" pill. */
  owned?: Set<number>;
}

export function GameGrid({ games, active, onClaim, owned }: GameGridProps) {
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
          <div key={game.title} className="rounded-lg bg-zinc-900 overflow-hidden">
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
              <h3 className="font-semibold text-sm leading-tight">{game.title}</h3>
              <p className="mt-1 text-xs text-zinc-400 truncate">{game.bundle}</p>
              <div className="mt-2 flex flex-wrap gap-1.5">
                <span className="rounded bg-zinc-800 px-2 py-0.5 text-xs text-zinc-300">
                  {game.key_type}
                </span>
                {count > 1 && (
                  <span className="rounded bg-zinc-700 px-2 py-0.5 text-xs text-zinc-300">
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
                disabled={!active}
                onClick={() => onClaim(game)}
                className="mt-3 w-full rounded bg-violet-700 px-3 py-1.5 text-sm font-medium hover:bg-violet-600 disabled:cursor-not-allowed disabled:opacity-40"
              >
                claim
              </button>
            </div>
          </div>
        );
      })}
    </section>
  );
}
