import { useCallback, useEffect, useState } from 'react';
import { useParams } from 'react-router-dom';
import { fetchLink, type GameView, type LinkView } from '../api';
import { ClaimDialog } from './ClaimDialog';
import { ClaimsHistory } from './ClaimsHistory';
import { GameGrid } from './GameGrid';

type ViewState =
  | { kind: 'loading' }
  | { kind: 'not-found' }
  | { kind: 'loaded'; data: LinkView };

export function LinkPage() {
  const { token } = useParams<{ token: string }>();
  const [view, setView] = useState<ViewState>({ kind: 'loading' });
  const [claimingGame, setClaimingGame] = useState<GameView | null>(null);
  const [refreshTick, setRefreshTick] = useState(0);

  const refresh = useCallback(() => setRefreshTick((t) => t + 1), []);

  useEffect(() => {
    let cancelled = false;

    async function load() {
      if (!token) {
        setView({ kind: 'not-found' });
        return;
      }
      setView({ kind: 'loading' });
      try {
        const data = await fetchLink(token);
        if (!cancelled) setView({ kind: 'loaded', data });
      } catch {
        if (!cancelled) setView({ kind: 'not-found' });
      }
    }

    void load();
    return () => {
      cancelled = true;
    };
  }, [token, refreshTick]);

  if (view.kind === 'loading') {
    return (
      <div className="flex min-h-screen items-center justify-center bg-zinc-950 text-zinc-100">
        <p className="text-zinc-400">loading...</p>
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
  // active:false + games present = exhausted all claims
  const exhausted = !data.active && data.games.length > 0;
  // active:false + games empty = revoked or expired
  const revoked = !data.active && data.games.length === 0;

  return (
    <div className="min-h-screen bg-zinc-950 text-zinc-100">
      <header className="flex items-center justify-between border-b border-zinc-800 px-6 py-4">
        <h1 className="text-lg font-semibold">{data.label}</h1>
        <span className="text-sm text-zinc-400">
          {data.claims_used}/{data.claims_allowed} claims used
        </span>
      </header>

      {exhausted && (
        <div
          role="alert"
          className="mx-6 mt-4 rounded border border-amber-800 bg-amber-950 px-4 py-3 text-amber-200"
        >
          you&apos;ve used all your claims
        </div>
      )}

      {revoked && (
        <div
          role="alert"
          className="mx-6 mt-4 rounded border border-red-800 bg-red-950 px-4 py-3 text-red-200"
        >
          this invite isn&apos;t active anymore — bug ben
        </div>
      )}

      {/* Grid: shown for exhausted (disabled buttons) or active; hidden for revoked */}
      {!revoked && (
        <GameGrid games={data.games} active={data.active} onClaim={setClaimingGame} />
      )}

      <ClaimsHistory claims={data.claims} />

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
