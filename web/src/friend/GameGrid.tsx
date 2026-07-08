import { type GameView } from '../api';
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
        // the game's shell hue for the 'clear' variant — same shared hash
        const shellHue = `var(${titleColorClass(game.title).replace('bg-', '--color-')})`;

        const art =
          game.artwork_url !== null ? (
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
          );

        // genres ride the list payload now (issue #55) — no per-card fetch.
        // max 4 chips on the card; absent/empty falls back to the key_type chip.
        const genres =
          game.genres !== undefined && game.genres.length > 0
            ? game.genres.slice(0, 4)
            : null;

        const titleBlock = (
          <>
            <h3 className="font-pixel text-xl font-semibold leading-tight">{game.title}</h3>
            <p className="mt-1 text-xs text-ink-soft truncate">{game.bundle}</p>
          </>
        );

        const chipsRow = (
          <div className="mt-2 flex flex-wrap gap-1.5">
            {/* genre chips replace the key_type chip when the payload carries
                genres; tag colors ride the shared title-hash palette
                (The Title-Hash Rule) tinted toward floor for chip duty */}
            {genres === null ? (
              /* floor chip — the shelf chip vanishes on the shelf card */
              <span className="rounded bg-floor px-2 py-0.5 text-xs text-ink-soft">
                {game.key_type}
              </span>
            ) : (
              genres.map((genre) => {
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
              })
            )}
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
        );

        return (
          /* the game card is a clear-shell DMG cartridge (ben's pick, live
             session 2026-07-07): see-through plastic tinted by the game's
             title-hash hue, grip ridges, label art, asymmetric corner. The
             whole cart IS the details control — no separate button. */
          <button
            key={game.title}
            type="button"
            aria-label={`${game.title} — details`}
            onClick={() => onDetail(game)}
            className="block w-full rounded-[6px_6px_20px_6px] overflow-hidden text-left cursor-pointer focus-visible:outline-[3px] focus-visible:outline-pixel focus-visible:outline-offset-2"
            style={{ background: `color-mix(in oklch, ${shellHue}, var(--color-shelf) 80%)` }}
          >
            <div
              aria-hidden="true"
              className="h-2.5"
              style={{
                background: `repeating-linear-gradient(90deg, color-mix(in oklch, ${shellHue}, var(--color-control) 72%) 0 10px, color-mix(in oklch, ${shellHue}, var(--color-shelf) 80%) 10px 20px)`,
              }}
            />
            <div className="px-3 pt-2">{art}</div>
            <div className="p-4">
              {titleBlock}
              {chipsRow}
            </div>
          </button>
        );
      })}
    </section>
  );
}
