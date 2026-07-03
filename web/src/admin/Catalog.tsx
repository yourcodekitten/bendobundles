import { useState, useEffect, useCallback, useMemo } from 'react';
import { useNavigate } from 'react-router-dom';
import { adminCatalog, adminSetHidden, type AdminGame } from '../api';
import { withAuth } from './withAuth';
import { titleColorClass } from '../titleColor';

// Status badge — exact color mapping from plan (snake_case serde values)
//   available=green, pending=amber, gifted=violet, ben_redeemed=slate, expired=red
function statusBadgeClass(status: string): string {
  switch (status) {
    case 'available':
      return 'bg-green-700 text-green-100';
    case 'pending':
      return 'bg-amber-700 text-amber-100';
    case 'gifted':
      return 'bg-violet-700 text-violet-100';
    case 'ben_redeemed':
      return 'bg-slate-600 text-slate-100';
    case 'expired':
      return 'bg-red-700 text-red-100';
    default:
      return 'bg-zinc-700 text-zinc-100';
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
  const [search, setSearch] = useState('');
  // Per-row inline error for toggle refusals (mid-claim 409 from server)
  const [rowErrors, setRowErrors] = useState<Record<string, string>>({});

  const load = useCallback(() => {
    setState({ phase: 'loading' });
    // withAuth re-throws non-Unauthorized errors → .catch sets error state
    withAuth(() => adminCatalog(), navigate)
      .then((games) => setState({ phase: 'loaded', games }))
      .catch(() => setState({ phase: 'error' }));
  }, [navigate]);

  useEffect(() => {
    load();
  }, [load]);

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
  // recompute per search keystroke; filtered recomputes only on games/query.
  const games = state.phase === 'loaded' ? state.games : NO_GAMES;
  const q = search.toLowerCase();

  const filtered = useMemo(
    () =>
      q === ''
        ? games
        : games.filter(
            (g) =>
              g.title.toLowerCase().includes(q) || g.bundle.toLowerCase().includes(q),
          ),
    [games, q],
  );

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
    return <p className="text-zinc-400">loading…</p>;
  }

  if (state.phase === 'error') {
    return (
      <div className="flex flex-col gap-4">
        <p className="text-zinc-400">couldn't load the catalog — try again</p>
        <button
          type="button"
          onClick={load}
          className="w-fit rounded bg-zinc-700 px-4 py-2 text-sm hover:bg-zinc-600"
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
          value={search}
          onChange={(e) => setSearch(e.target.value)}
          className="rounded border border-zinc-700 bg-zinc-900 px-3 py-1.5 text-sm text-zinc-100 placeholder-zinc-500 focus:border-zinc-500 focus:outline-none"
        />
        <p className="text-sm text-zinc-500">{summary}</p>
      </div>

      <div className="space-y-1">
        {filtered.map((game) => {
          const rowErr = rowErrors[game.id];
          return (
            <div
              key={game.id}
              className="flex flex-wrap items-center gap-3 rounded bg-zinc-900 px-4 py-3"
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

              {/* Title + bundle */}
              <div className="min-w-0 flex-1">
                <p className="truncate text-sm font-medium">{game.title}</p>
                <p className="truncate text-xs text-zinc-400">{game.bundle}</p>
              </div>

              {/* key_type */}
              <span className="rounded bg-zinc-800 px-2 py-0.5 text-xs text-zinc-300">
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
                <span className="rounded bg-violet-900 px-2 py-0.5 text-xs text-violet-200">
                  giftable
                </span>
              )}

              {/* Hidden toggle switch */}
              <label className="flex cursor-pointer items-center gap-1.5">
                <input
                  type="checkbox"
                  role="switch"
                  aria-label={`hide ${game.title}`}
                  checked={game.hidden}
                  onChange={() => handleToggle(game)}
                  className="h-4 w-4 cursor-pointer accent-zinc-500"
                />
                <span className="text-xs text-zinc-400">hidden</span>
              </label>

              {/* Inline toggle error — shown when server refuses (e.g. mid-claim) */}
              {rowErr !== undefined && (
                <p className="w-full text-xs text-red-400">{rowErr}</p>
              )}
            </div>
          );
        })}
      </div>
    </div>
  );
}
