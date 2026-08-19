import { memo } from 'react';
import { type GameView } from '../api';
import { displayTags, fitTags } from '../tags';
import { titleColorClass, titleHueVar } from '../titleColor';

interface GameGridProps {
  games: GameView[];
  /** curated link: server order is ben's pick order — no shuffle upstream, no dedupe here. */
  curated?: boolean;
  /** Set of Steam appids the viewer already owns — shows "you own this" pill. */
  owned?: Set<number>;
  /** Opens the detail modal — via the details button or the card body. Claiming
      happens inside the modal (the grid never claims directly; see DESIGN.md,
      The Button Burgundy Rule). */
  onDetail: (game: GameView) => void;
}

// Group by title; preserve server order — first occurrence wins the card.
// The open-shelf storefront affordance only — curated links skip this (see below).
function dedupedByTitle(games: GameView[]): { game: GameView; count: number }[] {
  const seen = new Map<string, { game: GameView; count: number }>();
  for (const game of games) {
    const entry = seen.get(game.title);
    if (entry !== undefined) {
      entry.count += 1;
    } else {
      seen.set(game.title, { game, count: 1 });
    }
  }
  return Array.from(seen.values());
}

function GameGridImpl({ games, curated, owned, onDetail }: GameGridProps) {
  // Curated: ben picked ids, not titles — two copies of one title are two
  // gifts (spec §5). Dedupe is the open-shelf storefront affordance only.
  const entries = curated
    ? games.map((game) => ({ game, count: 1 }))
    : dedupedByTitle(games);

  return (
    <section className={curated
      ? `grid grid-cols-1 gap-4 p-6 ${entries.length >= 2 ? 'sm:grid-cols-2' : ''} ${entries.length >= 3 ? 'lg:grid-cols-3' : ''}`
      : 'grid grid-cols-1 gap-4 p-6 sm:grid-cols-2 lg:grid-cols-3'}>
      {entries.map(({ game, count }) => {
        const youOwnThis =
          game.steam_app_id !== null &&
          owned !== undefined &&
          owned.has(game.steam_app_id);
        // the game's shell hue for the 'clear' variant — same shared hash
        const shellHue = titleHueVar(game.title);

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

        // community tags replace genres on the chips (#71); genres remain the fallback
        // for gated/delisted/pre-backfill apps. width-budget fit mirrors steam's store
        // box (short tags ⇒ more chips). absent/empty falls back to the key_type chip.
        const chipTags = fitTags(displayTags(game));
        const genres = chipTags.length ? chipTags : null;

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
                const hue = titleHueVar(genre);
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

        const cardKey = curated ? game.id : game.title;

        // A ghost is an acknowledgment, not a control: dimmed art, neutral
        // copy, no button — the gate 404s it anyway (the surface mirrors
        // the boundary). The wire carries `gone: true` and never the
        // cause — the ghost never invents or implies a reason.
        if (game.gone === true) {
          return (
            <div
              key={cardKey}
              className="block w-full rounded-[6px_6px_20px_6px] overflow-hidden text-left opacity-50 grayscale"
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
                <div className="mt-2 flex flex-wrap gap-1.5">
                  <span className="rounded bg-floor px-2 py-0.5 text-xs text-dust">
                    this one&apos;s spoken for
                  </span>
                </div>
              </div>
            </div>
          );
        }

        return (
          /* the game card is a clear-shell DMG cartridge (ben's pick, live
             session 2026-07-07): see-through plastic tinted by the game's
             title-hash hue, grip ridges, label art, asymmetric corner. The
             whole cart IS the details control — no separate button. */
          <button
            key={cardKey}
            type="button"
            aria-label={`${game.title} — details`}
            onClick={() => onDetail(game)}
            className="block w-full rounded-[6px_6px_20px_6px] overflow-hidden text-left cursor-pointer transition duration-200 ease-[cubic-bezier(0.25,1,0.5,1)] hover:brightness-[1.05] active:brightness-[0.98] motion-safe:hover:-translate-y-[3px] motion-safe:active:-translate-y-px focus-visible:outline-[3px] focus-visible:outline-pixel focus-visible:outline-offset-2"
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

// memoized so the friend page's per-character typewriter re-render doesn't
// reconcile the whole card grid ~70x/sec; props are stable during typing.
export const GameGrid = memo(GameGridImpl);
