import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { useParams } from 'react-router-dom';
import { fetchLink, fetchGameDetail, steamOwnedForLink, NotFound, type GameView, type LinkView } from '../api';
import {
  consumeReturnFragment,
  loadIdentity,
  saveIdentity,
  clearIdentity,
  beginConnect,
  type SteamIdentity,
} from '../steamIdentity';
import { ClaimDialog } from './ClaimDialog';
import { ClaimsHistory } from './ClaimsHistory';
import { GameGrid } from './GameGrid';
import { GameDetailModal } from '../GameDetailModal';

type ViewState =
  | { kind: 'loading' }
  | { kind: 'not-found' }
  | { kind: 'error' }
  | { kind: 'loaded'; data: LinkView };

export function LinkPage() {
  const { token } = useParams<{ token: string }>();
  const [view, setView] = useState<ViewState>({ kind: 'loading' });
  const [claimingGame, setClaimingGame] = useState<GameView | null>(null);
  const [detailGame, setDetailGame] = useState<GameView | null>(null);
  const [refreshTick, setRefreshTick] = useState(0);
  const prevTokenRef = useRef<string | undefined>(undefined);

  // ── steam identity state ────────────────────────────────────────────────────
  const [steamIdentity, setSteamIdentity] = useState<SteamIdentity | null>(null);
  const [steamPrivate, setSteamPrivate] = useState(false);
  const [steamError, setSteamError] = useState<string | null>(null);

  const refresh = useCallback(() => setRefreshTick((t) => t + 1), []);

  // ── steam identity effect — runs once on mount (per token) ──────────────────
  useEffect(() => {
    let cancelled = false;

    const fragment = consumeReturnFragment();

    if (fragment === null) {
      // No return fragment — restore from localStorage
      const stored = loadIdentity();
      if (!cancelled) setSteamIdentity(stored);
      return;
    }

    if ('error' in fragment) {
      if (!cancelled) setSteamError(fragment.error);
      return;
    }

    // Steam OpenID return with steamid + persona
    const { steamid, persona } = fragment;

    async function fetchOwned() {
      try {
        const result = await steamOwnedForLink(token!, steamid);
        if (cancelled) return;
        const owned = result === 'private' ? [] : result;
        const id: SteamIdentity = { steamid, persona, owned, fetched_at: Date.now() };
        saveIdentity(id);
        setSteamIdentity(id);
        if (result === 'private') setSteamPrivate(true);
      } catch {
        if (!cancelled) setSteamError('steam_unreachable');
      }
    }

    void fetchOwned();
    return () => {
      cancelled = true;
    };
  }, [token]); // eslint-disable-line react-hooks/exhaustive-deps

  // ── link load effect ────────────────────────────────────────────────────────
  useEffect(() => {
    let cancelled = false;

    async function load() {
      if (!token) {
        setView({ kind: 'not-found' });
        return;
      }
      // Hard reset to the spinner only on token change (initial load / navigation).
      // refreshTick bumps refetch behind the current view — no blank flash mid-claim.
      if (prevTokenRef.current !== token) {
        prevTokenRef.current = token;
        setView({ kind: 'loading' });
      }
      try {
        const data = await fetchLink(token);
        if (!cancelled) setView({ kind: 'loaded', data });
      } catch (error) {
        if (cancelled) return;
        if (error instanceof NotFound) {
          setView({ kind: 'not-found' });
        } else {
          // Transient failure — keep stale loaded data if we have it
          setView((v) => (v.kind === 'loaded' ? v : { kind: 'error' }));
        }
      }
    }

    void load();
    return () => {
      cancelled = true;
    };
  }, [token, refreshTick]);

  // Derived owned set for GameGrid
  const ownedSet = useMemo(
    () => new Set<number>(steamIdentity?.owned ?? []),
    [steamIdentity],
  );

  if (view.kind === 'loading') {
    return (
      <div className="flex min-h-screen items-center justify-center bg-zinc-950 text-zinc-100">
        <p className="text-zinc-400">loading...</p>
      </div>
    );
  }

  if (view.kind === 'error') {
    return (
      <div className="flex min-h-screen items-center justify-center bg-zinc-950 text-zinc-100">
        <main className="text-center">
          <h1 className="text-2xl font-bold">couldn&apos;t load this page</h1>
          <p className="mt-2 text-zinc-400">something hiccuped on our end — the link is fine</p>
          <button
            type="button"
            onClick={refresh}
            className="mt-4 rounded bg-zinc-700 px-4 py-2 text-sm hover:bg-zinc-600"
          >
            retry
          </button>
        </main>
      </div>
    );
  }

  if (view.kind === 'not-found') {
    return (
      <div className="flex min-h-screen items-center justify-center bg-zinc-950 text-zinc-100">
        <main className="text-center">
          <h1 className="text-2xl font-bold">link not found</h1>
          <p className="mt-2 text-zinc-400">ask your friend for a new link ♡</p>
        </main>
      </div>
    );
  }

  const { data } = view;
  // Explicit server state — never inferred from side signals like games.length
  const exhausted = data.state === 'exhausted';
  const dead = data.state === 'revoked' || data.state === 'expired';

  return (
    <div className="min-h-screen bg-zinc-950 text-zinc-100">
      <header className="flex items-center justify-between border-b border-zinc-800 px-6 py-4">
        <h1 className="text-lg font-semibold">{data.label}</h1>
        <div className="flex items-center gap-3">
          <span className="text-sm text-zinc-400">
            {data.claims_used}/{data.claims_allowed} claims used
          </span>
          {steamIdentity !== null ? (
            <div className="flex items-center gap-2">
              <span className="rounded bg-zinc-800 px-2 py-1 text-xs text-zinc-200">
                {steamIdentity.persona}
              </span>
              <button
                type="button"
                onClick={() => {
                  clearIdentity();
                  setSteamIdentity(null);
                  setSteamPrivate(false);
                  setSteamError(null);
                }}
                className="text-xs text-zinc-500 hover:text-zinc-300"
              >
                disconnect
              </button>
            </div>
          ) : (
            <button
              type="button"
              onClick={() => beginConnect(`/l/${token}`)}
              className="rounded bg-zinc-700 px-3 py-1.5 text-xs hover:bg-zinc-600"
            >
              connect steam
            </button>
          )}
        </div>
      </header>

      {/* Steam privacy notice — spec §4 wording verbatim */}
      {steamPrivate && (
        <p className="mx-6 mt-4 text-sm text-zinc-400">
          couldn&apos;t read your library — check Steam&apos;s <em>game details</em> privacy
          setting
        </p>
      )}

      {/* Steam connect error */}
      {steamError !== null && (
        <p className="mx-6 mt-4 text-sm text-zinc-400">
          {steamError === 'verify_failed'
            ? "we couldn't verify your Steam account — try again"
            : 'Steam is currently unavailable — try again later'}
        </p>
      )}

      {exhausted && (
        <div
          role="alert"
          className="mx-6 mt-4 rounded border border-amber-800 bg-amber-950 px-4 py-3 text-amber-200"
        >
          you&apos;ve used all your claims
        </div>
      )}

      {dead && (
        <div
          role="alert"
          className="mx-6 mt-4 rounded border border-red-800 bg-red-950 px-4 py-3 text-red-200"
        >
          this invite isn&apos;t active anymore — bug ben
        </div>
      )}

      {/* Grid: shown for exhausted (disabled buttons) or active; hidden for revoked/expired */}
      {!dead && (
        <GameGrid
          games={data.games}
          active={data.state === 'active'}
          onClaim={setClaimingGame}
          owned={ownedSet}
          onDetail={setDetailGame}
        />
      )}

      <ClaimsHistory claims={data.claims} />

      {detailGame !== null && token !== undefined && (
        <GameDetailModal
          mount="friend"
          token={token}
          game={detailGame}
          active={data.state === 'active'}
          loadDetail={(gameId) => fetchGameDetail(token, gameId)}
          onClaim={(g) => {
            setDetailGame(null);
            setClaimingGame(g);
          }}
          onClose={() => setDetailGame(null)}
        />
      )}

      {claimingGame !== null && token !== undefined && (
        <ClaimDialog
          token={token}
          game={claimingGame}
          onClose={() => setClaimingGame(null)}
          onRefresh={refresh}
        />
      )}
    </div>
  );
}
