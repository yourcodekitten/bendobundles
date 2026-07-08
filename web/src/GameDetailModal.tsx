import { useState, useEffect, useRef } from 'react';
import type { GameView, AdminGame, SteamDetailBlob, SelfClaimResult } from './api';
import { MediaHeader } from './MediaHeader';
import { titleColorClass } from './titleColor';

// ── Per-session module-level detail cache ─────────────────────────────────────
// Keyed by scope:token:gameId (friend) or admin:gameId so different links and
// the admin surface never collide. Survives close/reopen since the Map lives
// outside the component — unmounting does not destroy it.

const gameDetailCache = new Map<string, SteamDetailBlob | null>();

export function clearGameDetailCache(): void {
  gameDetailCache.clear();
}

// ── Focus trap (issue #61 acceptance) ─────────────────────────────────────────
// Computed at keydown time: the focusable set is dynamic (carousel slides are
// inert per-index, buttons appear/disappear), so a cached list would trap focus
// into hidden slides.

const FOCUSABLE_SELECTOR =
  'button, [href], input, select, textarea, video, [tabindex]:not([tabindex="-1"])';

function dialogFocusables(container: HTMLElement): HTMLElement[] {
  return Array.from(container.querySelectorAll<HTMLElement>(FOCUSABLE_SELECTOR)).filter(
    (el) =>
      el.closest('[inert], [aria-hidden="true"]') === null &&
      !el.hasAttribute('disabled'),
  );
}

// ── Status badge — mirrors Catalog's mapping ──────────────────────────────────

function statusBadgeClass(status: string): string {
  switch (status) {
    case 'available':
      return 'bg-green-700 text-green-100';
    case 'pending':
      return 'bg-amber-700 text-amber-100';
    case 'gifted':
      return 'bg-give text-give-ink';
    case 'ben_redeemed':
      return 'bg-slate-600 text-slate-100';
    case 'expired':
      return 'bg-red-700 text-red-100';
    default:
      return 'bg-control text-ink';
  }
}

// ── Props (discriminated union) ───────────────────────────────────────────────

type FriendMountProps = {
  mount: 'friend';
  /** Link token — used as part of the per-session cache key. */
  token: string;
  game: GameView;
  /** Honors the grid's disabled rules — disables the claim button when false. */
  active: boolean;
  onClaim: (game: GameView) => void;
};

type AdminMountProps = {
  mount: 'admin';
  game: AdminGame;
  /** Passthrough from Catalog's arm/confirm state machine. */
  armedId: string | null;
  claiming: string | null;
  onSelfClaim: (game: AdminGame) => void;
  adminSteamId: string | null;
  selfClaimResult: { gameId: string; r: SelfClaimResult } | null;
};

export type GameDetailModalProps = (FriendMountProps | AdminMountProps) & {
  onClose: () => void;
  /**
   * Caller-supplied fetch function for the Steam detail blob.
   * Friend mount: `(id) => fetchGameDetail(token, id)`
   * Admin mount: `(id) => withAuth(() => adminGameDetail(id), navigate)`
   *
   * withAuth returns a forever-pending promise on 401 (navigation is already
   * in flight) — the modal stays in the "loading" phase and unmounts naturally
   * when the router transitions. Never shows an error on the 401 path.
   */
  loadDetail: (gameId: string) => Promise<{ steam: SteamDetailBlob | null }>;
};

// ── Load state ────────────────────────────────────────────────────────────────

type LoadState =
  | { phase: 'loading' }
  | { phase: 'error' }
  | { phase: 'loaded'; steam: SteamDetailBlob | null };

// ── Component ─────────────────────────────────────────────────────────────────

export function GameDetailModal(props: GameDetailModalProps) {
  const { onClose, loadDetail } = props;
  const game = props.game;
  const mount = props.mount;
  // Extract token early so the useEffect dep array is stable
  const token = props.mount === 'friend' ? props.token : null;

  const [loadState, setLoadState] = useState<LoadState>({ phase: 'loading' });
  const [retryKey, setRetryKey] = useState(0);

  const containerRef = useRef<HTMLDivElement>(null);

  // ── Focus on open (a11y) ──────────────────────────────────────────────────

  useEffect(() => {
    containerRef.current?.focus();
  }, []);

  // ── Load detail data ──────────────────────────────────────────────────────

  useEffect(() => {
    let cancelled = false;
    const gameId = game.id;
    const cKey = token !== null ? `friend:${token}:${gameId}` : `admin:${gameId}`;

    async function doLoad() {
      // Cache hit — serve immediately (skip on retry so stale data is overwritten)
      if (retryKey === 0 && gameDetailCache.has(cKey)) {
        if (!cancelled) {
          setLoadState({ phase: 'loaded', steam: gameDetailCache.get(cKey) ?? null });
        }
        return;
      }
      if (!cancelled) setLoadState({ phase: 'loading' });
      try {
        const res = await loadDetail(gameId);
        if (!cancelled) {
          gameDetailCache.set(cKey, res.steam);
          setLoadState({ phase: 'loaded', steam: res.steam });
        }
      } catch {
        if (!cancelled) setLoadState({ phase: 'error' });
      }
    }

    void doLoad();
    return () => {
      cancelled = true;
    };
    // retryKey bumps a refetch without changing the other deps.
    // loadDetail is the caller-supplied loader — changes when scope changes.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [game.id, mount, token, loadDetail, retryKey]);

  // ── Escape key ────────────────────────────────────────────────────────────

  useEffect(() => {
    const handleKeyDown = (e: KeyboardEvent) => {
      if (e.key === 'Escape') onClose();
    };
    document.addEventListener('keydown', handleKeyDown);
    return () => document.removeEventListener('keydown', handleKeyDown);
  }, [onClose]);

  // ── Focus trap: Tab/Shift+Tab wrap inside the dialog ──────────────────────
  // (Escape stays the document listener above; the carousel's Arrow handling
  // lives on the carousel region — Tab is the only key this handler owns.)

  const handleTrapKeyDown = (e: React.KeyboardEvent) => {
    if (e.key !== 'Tab') return;
    const container = containerRef.current;
    if (container === null) return;
    const els = dialogFocusables(container);
    if (els.length === 0) {
      e.preventDefault(); // nowhere to go — focus stays on the container
      return;
    }
    const first = els[0];
    const last = els[els.length - 1];
    if (first === undefined || last === undefined) return;
    const active = document.activeElement;
    if (e.shiftKey) {
      if (active === first || active === container) {
        e.preventDefault();
        last.focus();
      }
    } else if (active === last || active === container) {
      e.preventDefault();
      first.focus();
    }
  };

  // ── Render ────────────────────────────────────────────────────────────────

  return (
    <>
      {/* Backdrop */}
      <div className="fixed inset-0 z-40 bg-black/60" aria-hidden="true" />

      {/* Dialog container — backdrop click closes; tabIndex=-1 + ref for focus management */}
      <div
        ref={containerRef}
        tabIndex={-1}
        role="dialog"
        aria-modal="true"
        aria-label={game.title}
        className="fixed inset-0 z-50 flex items-center justify-center p-4 outline-none"
        onClick={(e) => {
          if (e.target === e.currentTarget) onClose();
        }}
        onKeyDown={handleTrapKeyDown}
      >
        <div className="flex max-h-[90vh] w-full max-w-2xl flex-col overflow-hidden rounded-xl bg-floor shadow-2xl ring-1 ring-pixel">
          {/* Header */}
          <div className="flex items-center justify-between border-b border-line px-6 py-4">
            <h2 className="text-lg font-semibold">{game.title}</h2>
            <button
              type="button"
              onClick={onClose}
              aria-label="close"
              className="text-dust hover:text-ink-soft"
            >
              ✕
            </button>
          </div>

          {/* Body */}
          <div className="flex-1 overflow-y-auto">
            {loadState.phase === 'loading' && (
              <p className="px-6 py-8 text-center text-dust">loading...</p>
            )}

            {loadState.phase === 'error' && (
              <div className="px-6 py-8 text-center">
                <p className="text-dust">couldn&apos;t load details</p>
                <button
                  type="button"
                  onClick={() => setRetryKey((k) => k + 1)}
                  className="mt-2 text-sm text-give-soft hover:text-give"
                >
                  retry
                </button>
              </div>
            )}

            {/* Thin fallback — steam: null */}
            {loadState.phase === 'loaded' && loadState.steam === null && (
              <div className="space-y-4 px-6 py-6">
                {game.artwork_url !== null ? (
                  <img
                    src={game.artwork_url}
                    alt={game.title}
                    className="w-full rounded object-cover"
                  />
                ) : (
                  <div
                    className={`aspect-video w-full rounded ${titleColorClass(game.title)}`}
                    aria-hidden="true"
                  />
                )}
                <div className="flex flex-wrap gap-2">
                  <span className="rounded bg-shelf px-2 py-0.5 text-xs text-ink-soft">
                    {game.bundle}
                  </span>
                  <span className="rounded bg-shelf px-2 py-0.5 text-xs text-ink-soft">
                    {game.key_type}
                  </span>
                </div>
                <p className="text-sm text-dust">no steam page for this one.</p>
              </div>
            )}

            {/* Full detail — steam non-null */}
            {loadState.phase === 'loaded' && loadState.steam !== null && (() => {
              const { detail, overall, recent } = loadState.steam;

              return (
                <div className="space-y-4 pb-2">
                  {/* Media header — trailer + screenshots carousel (#61).
                      Keyed by game so carousel/video state can never survive
                      an in-place game swap if one is ever added. */}
                  <MediaHeader
                    key={game.id}
                    title={game.title}
                    artworkUrl={game.artwork_url}
                    detail={detail}
                  />

                  {/* Text detail — only when detail is non-null */}
                  {detail !== null && (
                    <div className="space-y-3 px-6">
                      {/* Dev · Pub · Release */}
                      <p className="text-sm text-ink-soft">
                        {detail.developers.join(', ')}
                        {detail.publishers.length > 0 &&
                          detail.publishers.join(',') !== detail.developers.join(',') && (
                            <> · {detail.publishers.join(', ')}</>
                          )}
                        {detail.release_date !== null && <> · {detail.release_date}</>}
                      </p>

                      {/* Genre chips */}
                      {detail.genres.length > 0 && (
                        <div className="flex flex-wrap gap-1.5">
                          {detail.genres.map((genre) => (
                            <span
                              key={genre}
                              className="rounded bg-shelf px-2 py-0.5 text-xs text-dust"
                            >
                              {genre}
                            </span>
                          ))}
                        </div>
                      )}

                      {/* Short description */}
                      <p className="text-sm leading-relaxed text-ink-soft">
                        {detail.short_description}
                      </p>
                    </div>
                  )}

                  {/* Review badges */}
                  {(overall !== null || recent !== null) && (
                    <div className="flex flex-wrap gap-2 px-6">
                      {overall !== null && (
                        <span
                          className="rounded bg-shelf px-2 py-1 text-xs text-ink-soft"
                          title={`${overall.total_positive.toLocaleString()} positive · ${overall.total_negative.toLocaleString()} negative`}
                        >
                          {overall.desc} · {overall.total_reviews.toLocaleString()} reviews
                        </span>
                      )}
                      {recent !== null && (
                        <span className="rounded bg-shelf px-2 py-1 text-xs text-ink-soft">
                          {recent.percent_positive}% positive (
                          {recent.count.toLocaleString()} recent)
                        </span>
                      )}
                    </div>
                  )}
                </div>
              );
            })()}
          </div>

          {/* Footer */}
          <div className="flex items-center gap-3 border-t border-line px-6 py-4">
            {props.mount === 'friend' ? (
              <button
                type="button"
                disabled={!props.active}
                onClick={() => props.onClaim(props.game)}
                className="rounded bg-give px-4 py-2 text-sm font-medium text-give-ink hover:bg-give-bright disabled:cursor-not-allowed disabled:opacity-40"
              >
                claim
              </button>
            ) : (
              (() => {
                const ap = props;
                const g = ap.game;
                const isArmed = ap.armedId === g.id;
                const isClaiming = ap.claiming === g.id;
                const r =
                  ap.selfClaimResult?.gameId === g.id ? ap.selfClaimResult.r : null;
                return (
                  <>
                    {/* Status badge */}
                    <span
                      className={`rounded px-2 py-0.5 text-xs font-medium ${statusBadgeClass(g.status)}`}
                    >
                      {g.status}
                    </span>

                    {/* Self-claim arm/confirm — routes through Catalog's state machine */}
                    {g.status === 'available' && (
                      <button
                        type="button"
                        disabled={isClaiming}
                        onClick={() => ap.onSelfClaim(g)}
                        className={`rounded px-3 py-1 text-xs ${
                          isArmed
                            ? 'bg-emerald-700 text-emerald-100 hover:bg-emerald-600'
                            : 'bg-control hover:bg-control-bright'
                        } disabled:opacity-50`}
                      >
                        {isArmed
                          ? g.owned_by_ben && ap.adminSteamId !== null
                            ? g.requires_choice
                              ? 'you already own this on steam — spends 1 pick, sure?'
                              : 'you already own this on steam — sure?'
                            : g.requires_choice
                              ? 'confirm? spends 1 pick'
                              : 'confirm?'
                          : isClaiming
                            ? 'claiming…'
                            : 'claim for me'}
                      </button>
                    )}

                    {/* Reveal result */}
                    {r?.kind === 'revealed' && (
                      <div className="flex items-center gap-2 text-sm">
                        <span className="select-all font-mono">{r.key}</span>
                        <button
                          type="button"
                          onClick={() => void navigator.clipboard.writeText(r.key)}
                          className="rounded bg-control px-2 py-1 text-xs"
                        >
                          copy
                        </button>
                        {r.keyType === 'steam' && (
                          <a
                            href={`https://store.steampowered.com/account/registerkey?key=${encodeURIComponent(r.key)}`}
                            target="_blank"
                            rel="noreferrer"
                            className="rounded bg-blue-700 px-2 py-1 text-xs text-blue-100"
                          >
                            redeem on steam
                          </a>
                        )}
                      </div>
                    )}
                    {r?.kind === 'processing' && (
                      <p className="text-xs text-amber-800">processing — check self-claims below</p>
                    )}
                    {r?.kind === 'refused' && (
                      <p className="text-xs text-red-700">{r.message}</p>
                    )}
                  </>
                );
              })()
            )}
          </div>
        </div>
      </div>
    </>
  );
}
