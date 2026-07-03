import { useEffect, useState } from 'react';
import { useNavigate, useOutletContext } from 'react-router-dom';
import { adminPasteCookie, adminSync } from '../api';
import { withAuth } from './withAuth';
import type { AdminOutletContext } from './AdminApp';

// Formats seconds-since-epoch as a human-readable relative time string.
// Called with Date.now() as the reference so callers can mock Date.now in tests.
function formatRelativeTime(epoch: number): string {
  const diffSeconds = Math.floor(Date.now() / 1000 - epoch);
  // Server/client clock skew can put epoch slightly in the future — never "-3s ago"
  if (diffSeconds < 0) return 'just now';
  if (diffSeconds < 60) return `${diffSeconds}s ago`;
  const diffMinutes = Math.floor(diffSeconds / 60);
  if (diffMinutes < 60) return `${diffMinutes}m ago`;
  const diffHours = Math.floor(diffMinutes / 60);
  if (diffHours < 24) return `${diffHours}h ago`;
  const diffDays = Math.floor(diffHours / 24);
  return `${diffDays}d ago`;
}

export function Ops() {
  const navigate = useNavigate();
  // status is owned by AdminApp (single copy of the server state); refreshStatus
  // triggers its re-fetch so the banner AND this card update immediately after
  // cookie paste or sync, without waiting for the next route navigation.
  const { status, refreshStatus } = useOutletContext<AdminOutletContext>();

  // Cookie panel
  const [cookieValue, setCookieValue] = useState('');
  const [cookieMsg, setCookieMsg] = useState<string | null>(null);
  const [cookieLoading, setCookieLoading] = useState(false);

  // Sync panel
  const [syncing, setSyncing] = useState(false);
  const [syncMsg, setSyncMsg] = useState<string | null>(null);

  const handleCookieSubmit = (e: React.FormEvent) => {
    e.preventDefault();
    const val = cookieValue;
    // Clear state before dispatching — value must not linger in component state
    // during the async operation. Same clear-before-await pattern as Login.tsx.
    setCookieValue('');
    setCookieMsg(null);
    setCookieLoading(true);
    withAuth(() => adminPasteCookie(val), navigate)
      .then((result) => {
        if (result.ok) {
          setCookieMsg('cookie validated ✓');
        } else if (result.restored_previous) {
          setCookieMsg('that cookie failed validation — kept your previous one');
        } else if (result.inconclusive) {
          setCookieMsg('humble unreachable — cookie state unknown, try again');
        } else {
          setCookieMsg('cookie failed validation');
        }
        refreshStatus();
      })
      .catch(() => {
        // withAuth handles 401 → login redirect; other errors swallowed
      })
      .finally(() => {
        setCookieLoading(false);
      });
  };

  // True while the server says a sync run is live. This is what disables the button for the
  // WHOLE backfill (the 202 lands ~1s after click; local `syncing` alone would re-enable it
  // and let a second concurrent walk be queued).
  const syncRunning = status?.sync_run?.running === true;

  const handleSync = () => {
    setSyncing(true);
    setSyncMsg(null);
    // Fire-and-forget: adminSync resolves when the backfill is QUEUED (202),
    // not when it finishes. Progress + final counts land on the status card
    // once the background run writes its SyncState.
    withAuth(() => adminSync(), navigate)
      .then(() => {
        setSyncMsg('sync started — watch the status card; a full backfill takes a few minutes');
        refreshStatus();
      })
      .catch((err: unknown) => {
        setSyncMsg(err instanceof Error ? err.message : 'couldn’t start sync — try again');
        setSyncing(false);
      });
  };

  // `syncing` bridges the gap between the 202 and the run marker appearing in status (the
  // fulfillment lambda may still be cold-starting): keep the button locked and poll briefly
  // until the marker shows up, then AdminApp's running-poll owns the cadence. If the marker
  // never appears (~30s), unlock — the status card will say whether a run ever reported.
  useEffect(() => {
    if (!syncing) return;
    if (syncRunning) {
      setSyncing(false);
      return;
    }
    let attempts = 0;
    const id = setInterval(() => {
      attempts += 1;
      if (attempts > 15) {
        setSyncing(false);
        return;
      }
      refreshStatus();
    }, 2000);
    return () => clearInterval(id);
  }, [syncing, syncRunning, refreshStatus]);

  return (
    <div className="flex flex-col gap-8">
      {/* ── Cookie panel ────────────────────────────────────────────────── */}
      <section className="flex flex-col gap-3 rounded bg-zinc-900 p-4">
        <h2 className="text-sm font-medium text-zinc-300">session cookie</h2>
        <form onSubmit={handleCookieSubmit} className="flex flex-col gap-3">
          <label className="text-xs text-zinc-400" htmlFor="cookie-input">
            humble session cookie
          </label>
          <input
            id="cookie-input"
            type="password"
            value={cookieValue}
            onChange={(e) => setCookieValue(e.target.value)}
            autoComplete="off"
            className="rounded border border-zinc-700 bg-zinc-800 px-3 py-2 text-sm text-zinc-100"
          />
          <button
            type="submit"
            disabled={cookieLoading}
            className="w-fit rounded bg-zinc-700 px-4 py-2 text-sm hover:bg-zinc-600 disabled:opacity-50"
          >
            {cookieLoading ? 'validating…' : 'submit'}
          </button>
          {cookieMsg !== null && (
            <p role="status" className="text-sm text-zinc-300">
              {cookieMsg}
            </p>
          )}
        </form>
      </section>

      {/* ── Sync panel ──────────────────────────────────────────────────── */}
      <section className="flex flex-col gap-3 rounded bg-zinc-900 p-4">
        <h2 className="text-sm font-medium text-zinc-300">sync</h2>
        <button
          type="button"
          onClick={handleSync}
          disabled={syncing || syncRunning}
          className="w-fit rounded bg-zinc-700 px-4 py-2 text-sm hover:bg-zinc-600 disabled:opacity-50"
        >
          {syncing || syncRunning ? 'syncing…' : 'sync now'}
        </button>
        {syncMsg !== null && (
          <p role="status" className="text-sm text-zinc-300">
            {syncMsg}
          </p>
        )}
      </section>

      {/* ── Status card ─────────────────────────────────────────────────── */}
      <section className="flex flex-col gap-3 rounded bg-zinc-900 p-4">
        <h2 className="text-sm font-medium text-zinc-300">status</h2>
        {status === null && <p className="text-xs text-zinc-500">loading…</p>}
        {status !== null && (
          <div className="flex flex-col gap-2">
            <p className="text-xs text-zinc-400">
              last run:{' '}
              {status.sync === null ? (
                <span>never</span>
              ) : (
                <span title={new Date(status.sync.last_run_epoch * 1000).toISOString()}>
                  {formatRelativeTime(status.sync.last_run_epoch)}
                </span>
              )}
            </p>

            {status.sync_run !== null && status.sync_run.running && (
              <p className="text-xs text-amber-300">
                sync running — started {formatRelativeTime(status.sync_run.started_epoch)}
              </p>
            )}
            {/* Marker present but not live: the run died before reporting (crash/timeout).
                Without this line a dropped backfill is indistinguishable from idle. */}
            {status.sync_run !== null && !status.sync_run.running && (
              <p className="text-xs text-red-300">
                a sync started {formatRelativeTime(status.sync_run.started_epoch)} but never
                reported — it likely failed; safe to retry
              </p>
            )}

            {status.sync !== null && (
              <>
                <div className="flex gap-2">
                  <span
                    className={`rounded px-2 py-0.5 text-xs ${
                      status.sync.ok ? 'bg-green-900 text-green-200' : 'bg-red-900 text-red-200'
                    }`}
                  >
                    {status.sync.ok ? 'ok ✓' : 'ok ✗'}
                  </span>
                  <span
                    className={`rounded px-2 py-0.5 text-xs ${
                      status.sync.cookie_ok
                        ? 'bg-green-900 text-green-200'
                        : 'bg-red-900 text-red-200'
                    }`}
                  >
                    {status.sync.cookie_ok ? 'cookie ✓' : 'cookie ✗'}
                  </span>
                </div>
                <p className="text-xs text-zinc-400">{status.sync.message}</p>
              </>
            )}

            {Object.keys(status.game_counts).length > 0 && (
              <div className="flex flex-wrap gap-2">
                {Object.entries(status.game_counts).map(([key, count]) => (
                  <span
                    key={key}
                    className="rounded bg-zinc-700 px-2 py-0.5 text-xs text-zinc-300"
                  >
                    {key}: {count}
                  </span>
                ))}
              </div>
            )}
          </div>
        )}
      </section>
    </div>
  );
}
