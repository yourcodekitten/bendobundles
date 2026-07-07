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
      <div className="flex min-h-screen items-center justify-center bg-room text-ink">
        <p className="text-dust">loading...</p>
      </div>
    );
  }

  if (view.kind === 'error') {
    return (
      <div className="flex min-h-screen items-center justify-center bg-room text-ink">
        <main className="text-center">
          <h1 className="text-2xl font-bold">couldn&apos;t load this page</h1>
          <p className="mt-2 text-dust">something hiccuped on our end — the link is fine</p>
          <button
            type="button"
            onClick={refresh}
            className="mt-4 rounded bg-control px-4 py-2 text-sm hover:bg-control-bright"
          >
            retry
          </button>
        </main>
      </div>
    );
  }

  if (view.kind === 'not-found') {
    return (
      <div className="flex min-h-screen items-center justify-center bg-room text-ink">
        <main className="text-center">
          <h1 className="text-2xl font-bold">link not found</h1>
          <p className="mt-2 text-dust">ask your friend for a new link ♡</p>
        </main>
      </div>
    );
  }

  const { data } = view;
  // Explicit server state — never inferred from side signals like games.length
  const exhausted = data.state === 'exhausted';
  const dead = data.state === 'revoked' || data.state === 'expired';

  return (
    <div className="min-h-screen bg-room text-ink">
      <header className="border-b border-line">
        <div className="relative">
          <div
            aria-hidden="true"
            className="h-60 w-full"
            style={{
              backgroundImage: 'url(/art/banner.png)',
              backgroundRepeat: 'repeat-x',
              /* pin the banner's center (the chest) 200px from the right edge;
                 the scene tiles horizontally for wide viewports */
              backgroundPosition: 'calc(100% + 824px) 62%',
              backgroundColor: 'rgb(197,198,125)',
            }}
          />
          <div className="absolute inset-x-0 top-0 flex items-center justify-between px-6 py-3">
            <h1 className="font-logo wordmark-outline text-xl uppercase tracking-[0.03em]">bendobundles</h1>
            <span className="text-sm font-medium text-ink">
              {data.claims_used}/{data.claims_allowed} claims used
            </span>
          </div>
          <div className="absolute bottom-4 left-6 max-w-xl rounded-xl border-[3px] border-pixel bg-floor px-5 py-3.5 [box-shadow:inset_0_0_0_3px_var(--color-floor),inset_0_0_0_5px_var(--color-pixel)]">
              <h2 className="text-xl leading-tight text-give-soft">{data.label}</h2>
              <p className="mt-1.5 max-w-[60ch] text-sm text-ink-soft">
                games from ben&apos;s humble stash, picked for you ♡ open one for details, claim it, and the key is yours.
              </p>
              {steamIdentity !== null ? (
                <div className="mt-2 flex items-center gap-2">
                  <span className="rounded bg-shelf px-2 py-1 text-xs text-ink-soft">{steamIdentity.persona}</span>
                  <button type="button" onClick={() => { clearIdentity(); setSteamIdentity(null); setSteamPrivate(false); setSteamError(null); }} className="text-xs text-dust-faint hover:text-ink-soft">disconnect</button>
                </div>
              ) : (
                <button
                  type="button"
                  onClick={() => beginConnect(`/l/${token}`)}
                  className="font-pixel group mt-2 -mx-1 flex items-center gap-1.5 rounded px-1 py-0.5 text-sm text-ink hover:bg-shelf"
                >
                  <span aria-hidden="true" className="text-give transition-transform group-hover:translate-x-0.5">▸</span>
                  connect to steam
                  <span className="font-sans text-xs text-dust-faint">— flags the games you already own</span>
                </button>
              )}
          </div>
        </div>
      </header>

      {/* Steam privacy notice — spec §4 wording verbatim */}
      {steamPrivate && (
        <p className="mx-6 mt-4 text-sm text-dust">
          couldn&apos;t read your library — check Steam&apos;s <em>game details</em> privacy
          setting
        </p>
      )}

      {/* Steam connect error */}
      {steamError !== null && (
        <p className="mx-6 mt-4 text-sm text-dust">
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

      {/* Grid: shown for exhausted or active (claiming lives in the detail modal,
          which respects link state); hidden for revoked/expired */}
      {!dead && (
        <GameGrid games={data.games} owned={ownedSet} onDetail={setDetailGame} />
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
