import { useState, useEffect, useCallback, useMemo } from 'react';
import { useNavigate, useSearchParams } from 'react-router-dom';
import {
  adminCatalog,
  adminGameDetail,
  adminSetHidden,
  adminSelfClaim,
  adminSelfClaims,
  adminSteamIdentity,
  type AdminGame,
  type SelfClaimResult,
  type SelfClaimView,
} from '../api';
import { withAuth } from './withAuth';
import { titleColorClass } from '../titleColor';
import { GameDetailModal } from '../GameDetailModal';
import {
  applyToolkit,
  collectTagOptions,
  type GroupKey,
  type RatingFloor,
  type SortKey,
  type ToolkitState,
} from './catalogToolkit';
import { ToolkitBar } from './ToolkitBar';

// URL param values are attacker-writable (it's a URL) — anything outside the
// known key sets falls back to the idle value rather than reaching a cast.
const RATING_KEYS: readonly RatingFloor[] = [
  'any',
  'mixed',
  'mostly-positive',
  'very-positive',
  'overwhelmingly-positive',
];
const SORT_KEYS: readonly SortKey[] = ['title', 'rating', 'date-new', 'date-old'];
const GROUP_KEYS: readonly GroupKey[] = ['none', 'publisher', 'studio', 'bundle'];

function keyOf<T extends string>(raw: string | null, known: readonly T[], idle: T): T {
  return raw !== null && (known as readonly string[]).includes(raw) ? (raw as T) : idle;
}

// Status badge — exact color mapping from plan (snake_case serde values)
//   available=green, pending=amber, gifted=violet, ben_redeemed=slate, expired=red
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

type PageState =
  | { phase: 'loading' }
  | { phase: 'error' }
  | { phase: 'loaded'; games: AdminGame[] };

// Stable empty list so the memos below don't recompute across loading renders.
const NO_GAMES: AdminGame[] = [];

export function Catalog() {
  const navigate = useNavigate();
  const [state, setState] = useState<PageState>({ phase: 'loading' });
  // Toolkit state (search included) lives in the URL so refresh/back/forward
  // preserve the dig. Params are omitted at their idle values.
  const [params, setParams] = useSearchParams();
  const toolkit: ToolkitState = useMemo(
    () => ({
      q: params.get('q') ?? '',
      tags: params.get('tags')?.split(',').filter(Boolean) ?? [],
      rating: keyOf(params.get('rating'), RATING_KEYS, 'any'),
      sort: keyOf(params.get('sort'), SORT_KEYS, 'title'),
      group: keyOf(params.get('group'), GROUP_KEYS, 'none'),
    }),
    [params],
  );
  const setToolkit = (next: ToolkitState) => {
    const p = new URLSearchParams();
    if (next.q) p.set('q', next.q);
    if (next.tags.length > 0) p.set('tags', next.tags.join(','));
    if (next.rating !== 'any') p.set('rating', next.rating);
    if (next.sort !== 'title') p.set('sort', next.sort);
    if (next.group !== 'none') p.set('group', next.group);
    setParams(p, { replace: true });
  };
  // Per-row inline error for toggle refusals (mid-claim 409 from server)
  const [rowErrors, setRowErrors] = useState<Record<string, string>>({});

  // Self-claim state
  const [armedId, setArmedId] = useState<string | null>(null);
  const [claiming, setClaiming] = useState<string | null>(null);
  const [result, setResult] = useState<{ gameId: string; r: SelfClaimResult } | null>(null);
  const [selfClaims, setSelfClaims] = useState<SelfClaimView[]>([]);

  // Admin steam identity — controls owned_by_ben badge visibility (frozen-stamps caveat)
  const [adminSteamId, setAdminSteamId] = useState<string | null>(null);

  // Detail modal — opens on row click
  const [detailGame, setDetailGame] = useState<AdminGame | null>(null);

  const load = useCallback(() => {
    setState({ phase: 'loading' });
    // withAuth re-throws non-Unauthorized errors → .catch sets error state
    withAuth(() => adminCatalog(), navigate)
      .then((games) => setState({ phase: 'loaded', games }))
      .catch(() => setState({ phase: 'error' }));
  }, [navigate]);

  const loadSelfClaims = useCallback(() => {
    withAuth(() => adminSelfClaims(), navigate)
      .then((claims) => setSelfClaims(claims))
      .catch(() => {
        // non-critical — fail silently, list stays stale
      });
  }, [navigate]);

  useEffect(() => {
    load();
    loadSelfClaims();
    // Load admin steam identity — non-critical; if it fails, badges just stay hidden
    withAuth(() => adminSteamIdentity(), navigate)
      .then((id) => setAdminSteamId(id))
      .catch(() => {});
  }, [load, loadSelfClaims, navigate]);

  const handleSelfClaim = async (g: AdminGame) => {
    if (armedId !== g.id) {
      setArmedId(g.id);
      return;
    }
    setArmedId(null);
    setClaiming(g.id);
    const r = await withAuth(() => adminSelfClaim(g.id), navigate);
    setClaiming(null);
    setResult({ gameId: g.id, r });
    load();
    loadSelfClaims();
  };

  const handleToggle = (game: AdminGame) => {
    if (state.phase !== 'loaded') return;
    const newHidden = !game.hidden;

    // Functional updates throughout: concurrent toggles must never revert
    // through a stale whole-list snapshot (that would clobber other rows).
    const setRowHidden = (hidden: boolean) => {
      setState((s) =>
        s.phase === 'loaded'
          ? {
              phase: 'loaded',
              games: s.games.map((g) => (g.id === game.id ? { ...g, hidden } : g)),
            }
          : s,
      );
    };

    // Optimistic flip
    setRowHidden(newHidden);

    withAuth(() => adminSetHidden(game.id, newHidden), navigate)
      .then((result) => {
        if (!result.ok) {
          // Server refused (e.g. mid-claim 409) — revert this row + show message
          setRowHidden(game.hidden);
          setRowErrors((prev) => ({ ...prev, [game.id]: result.message }));
        } else {
          // Clear any previous row error on success
          setRowErrors((prev) => {
            const next = { ...prev };
            delete next[game.id];
            return next;
          });
        }
      })
      .catch(() => {
        // Unexpected error — revert this row silently (withAuth already redirected on 401)
        setRowHidden(game.hidden);
      });
  };

  // Memos live above the early returns (hooks must run unconditionally).
  // summary derives only from the full unfiltered list — it must not
  // recompute per search keystroke; the pipeline recomputes only on
  // games/toolkit changes.
  const games = state.phase === 'loaded' ? state.games : NO_GAMES;

  const tagOptions = useMemo(() => collectTagOptions(games), [games]);
  const visible = useMemo(() => applyToolkit(games, toolkit), [games, toolkit]);

  const summary = useMemo(() => {
    const counts: Record<string, number> = {};
    for (const g of games) {
      counts[g.status] = (counts[g.status] ?? 0) + 1;
    }
    return Object.entries(counts)
      .map(([s, n]) => `${s}: ${n}`)
      .join(' · ');
  }, [games]);

  if (state.phase === 'loading') {
    return <p className="text-dust">loading…</p>;
  }

  if (state.phase === 'error') {
    return (
      <div className="flex flex-col gap-4">
        <p className="text-dust">couldn't load the catalog — try again</p>
        <button
          type="button"
          onClick={load}
          className="w-fit rounded bg-control px-4 py-2 text-sm hover:bg-control-bright"
        >
          retry
        </button>
      </div>
    );
  }

  return (
    <div>
      <div className="mb-4 flex flex-wrap items-center gap-4">
        <input
          type="search"
          aria-label="search games"
          placeholder="search title or bundle…"
          value={toolkit.q}
          onChange={(e) => setToolkit({ ...toolkit, q: e.target.value })}
          className="rounded border border-line bg-floor px-3 py-1.5 text-sm text-ink placeholder-dust focus:border-pixel focus:outline-none"
        />
        <p className="text-sm text-dust-faint">{summary}</p>
      </div>

      <ToolkitBar
        state={toolkit}
        tagOptions={tagOptions}
        shown={visible.shown}
        total={games.length}
        excludedNoData={visible.excludedNoData}
        onChange={setToolkit}
      />

      <div className="space-y-4">
        {visible.groups.map((group) => {
          const rows = group.games.map((game) => renderRow(game));
          if (group.label === null) {
            return (
              <div key="__all" className="space-y-1">
                {rows}
              </div>
            );
          }
          return (
            <details key={group.label} open>
              <summary className="cursor-pointer text-sm font-medium text-ink-soft">
                {group.label} ({group.games.length})
              </summary>
              <div className="mt-2 space-y-1">{rows}</div>
            </details>
          );
        })}
      </div>

      {/* Game detail modal — opens on row click */}
      {detailGame !== null && (
        <GameDetailModal
          mount="admin"
          game={detailGame}
          loadDetail={(gameId) => withAuth(() => adminGameDetail(gameId), navigate)}
          onClose={() => setDetailGame(null)}
          armedId={armedId}
          claiming={claiming}
          onSelfClaim={(g) => void handleSelfClaim(g)}
          adminSteamId={adminSteamId}
          selfClaimResult={result}
        />
      )}

      {/* Self-claims section */}
      {selfClaims.length > 0 && (
        <div className="mt-8">
          <h2 className="mb-3 text-sm font-medium text-ink-soft">your self-claims</h2>
          <div className="space-y-2">
            {selfClaims.map((sc) => (
              <div
                key={sc.game_id}
                className="flex flex-wrap items-center gap-3 rounded bg-floor px-4 py-3 text-sm"
              >
                <span className="font-mono text-xs text-dust">{sc.game_id}</span>
                <span
                  className={`rounded px-2 py-0.5 text-xs font-medium ${
                    sc.state === 'fulfilled'
                      ? 'bg-green-700 text-green-100'
                      : sc.state === 'compensated'
                        ? 'bg-slate-600 text-slate-100'
                        : 'bg-amber-700 text-amber-100'
                  }`}
                >
                  {sc.state}
                </span>
                {sc.revealed_key !== null && (
                  <>
                    <span className="select-all font-mono text-xs">{sc.revealed_key}</span>
                    <button
                      type="button"
                      onClick={() => void navigator.clipboard.writeText(sc.revealed_key!)}
                      className="rounded bg-control px-2 py-1 text-xs"
                    >
                      copy
                    </button>
                  </>
                )}
              </div>
            ))}
          </div>
        </div>
      )}
    </div>
  );

  // Row renderer — the pre-toolkit map body, unchanged except the steam
  // readout under the bundle line. Hoisted so grouped and ungrouped views
  // render identical rows.
  function renderRow(game: AdminGame) {
          const rowErr = rowErrors[game.id];
          const isArmed = armedId === game.id;
          const isClaiming = claiming === game.id;
          const rowResult = result?.gameId === game.id ? result.r : null;
          // TS narrowing caveat: alias r before branching
          const r = rowResult;
          return (
            <div key={game.id} className="space-y-1">
              <div
                className="flex flex-wrap items-center gap-3 rounded bg-floor px-4 py-3 cursor-pointer"
                onClick={() => setDetailGame(game)}
              >
                {/* Artwork thumbnail — colored fallback when url absent */}
                {game.artwork_url !== null ? (
                  <img
                    src={game.artwork_url}
                    alt={game.title}
                    className="h-10 w-16 flex-shrink-0 rounded object-cover"
                  />
                ) : (
                  <div
                    className={`h-10 w-16 flex-shrink-0 rounded ${titleColorClass(game.title)}`}
                    aria-hidden="true"
                  />
                )}

                {/* Title + bundle + compact steam readout (rating · % · year) */}
                <div className="min-w-0 flex-1">
                  <p className="truncate text-sm font-medium">{game.title}</p>
                  <p className="truncate text-xs text-dust">{game.bundle}</p>
                  {game.steam !== null &&
                    (game.steam.review_desc !== null ||
                      game.steam.review_percent !== null ||
                      game.steam.release_date_iso !== null) && (
                      <p className="truncate text-xs text-dust-faint">
                        {[
                          game.steam.review_desc,
                          game.steam.review_percent !== null
                            ? `${game.steam.review_percent}%`
                            : null,
                          game.steam.release_date_iso?.slice(0, 4),
                        ]
                          .filter(Boolean)
                          .join(' · ')}
                      </p>
                    )}
                </div>

                {/* key_type */}
                <span className="rounded bg-shelf px-2 py-0.5 text-xs text-ink-soft">
                  {game.key_type}
                </span>

                {/* Status badge with exact plan color mapping */}
                <span
                  className={`rounded px-2 py-0.5 text-xs font-medium ${statusBadgeClass(game.status)}`}
                >
                  {game.status}
                </span>

                {/* Giftable chip — only shown when true */}
                {game.giftable && (
                  <span className="rounded bg-give px-2 py-0.5 text-xs text-give-ink">
                    giftable
                  </span>
                )}

                {/* owned_by_ben badge — hidden when adminSteamIdentity is null (frozen-stamps caveat) */}
                {game.owned_by_ben && adminSteamId !== null && (
                  <span className="rounded bg-blue-900 px-2 py-0.5 text-xs text-blue-200">
                    already own on steam
                  </span>
                )}

                {/* Self-claim button — available games only, arm/confirm two-step */}
                {game.status === 'available' && (
                  <button
                    type="button"
                    disabled={isClaiming}
                    onClick={(e) => { e.stopPropagation(); void handleSelfClaim(game); }}
                    className={`rounded px-3 py-1 text-xs ${
                      isArmed
                        ? 'bg-emerald-700 text-emerald-100 hover:bg-emerald-600'
                        : 'bg-control hover:bg-control-bright'
                    } disabled:opacity-50`}
                  >
                    {isArmed
                      ? game.owned_by_ben && adminSteamId !== null
                        ? game.requires_choice
                          ? 'you already own this on steam — spends 1 pick, sure?'
                          : 'you already own this on steam — sure?'
                        : game.requires_choice
                          ? 'confirm? spends 1 pick'
                          : 'confirm?'
                      : isClaiming
                        ? 'claiming…'
                        : 'claim for me'}
                  </button>
                )}

                {/* Hidden toggle switch — stopPropagation prevents row click from opening modal */}
                <label
                  className="flex cursor-pointer items-center gap-1.5"
                  onClick={(e) => e.stopPropagation()}
                >
                  <input
                    type="checkbox"
                    role="switch"
                    aria-label={`hide ${game.title}`}
                    checked={game.hidden}
                    onChange={() => handleToggle(game)}
                    className="h-4 w-4 cursor-pointer accent-give"
                  />
                  <span className="text-xs text-dust">hidden</span>
                </label>

                {/* Inline toggle error — shown when server refuses (e.g. mid-claim) */}
                {rowErr !== undefined && (
                  <p className="w-full text-xs text-red-700">{rowErr}</p>
                )}
              </div>

              {/* Result panel — dismissible, per-row */}
              {r?.kind === 'revealed' && (
                <div className="rounded bg-emerald-950 p-3 text-sm">
                  <span className="select-all font-mono">{r.key}</span>
                  <button
                    type="button"
                    onClick={() => void navigator.clipboard.writeText(r.key)}
                    className="ml-2 rounded bg-control px-2 py-1 text-xs"
                  >
                    copy
                  </button>
                  {r.keyType === 'steam' && (
                    <a
                      href={`https://store.steampowered.com/account/registerkey?key=${encodeURIComponent(r.key)}`}
                      target="_blank"
                      rel="noreferrer"
                      className="ml-2 rounded bg-blue-700 px-2 py-1 text-xs text-blue-100"
                    >
                      redeem on steam
                    </a>
                  )}
                  <button
                    type="button"
                    onClick={() => setResult(null)}
                    className="ml-2 text-xs text-dust"
                  >
                    dismiss
                  </button>
                </div>
              )}
              {r?.kind === 'processing' && (
                <div className="rounded bg-amber-950 p-3 text-sm">
                  reveal is processing — the key will appear under self-claims below.
                  <button
                    type="button"
                    onClick={() => setResult(null)}
                    className="ml-2 text-xs"
                  >
                    dismiss
                  </button>
                </div>
              )}
              {r?.kind === 'refused' && (
                <div className="rounded bg-red-950 p-3 text-sm">
                  {r.message}
                  <button
                    type="button"
                    onClick={() => setResult(null)}
                    className="ml-2 text-xs"
                  >
                    dismiss
                  </button>
                </div>
              )}
            </div>
          );
  }
}
