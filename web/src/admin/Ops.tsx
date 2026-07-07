import { useEffect, useState } from 'react';
import { useNavigate, useOutletContext } from 'react-router-dom';
import { adminSync, adminSteamIdentity, adminSetSteamIdentity, adminClearSteamIdentity, adminSteamOwned } from '../api';
import {
  consumeReturnFragment,
  loadIdentity,
  saveIdentity,
  clearIdentity,
  beginConnect,
} from '../steamIdentity';
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
  // sync, without waiting for the next route navigation.
  const { status, refreshStatus } = useOutletContext<AdminOutletContext>();

  // Sync panel
  const [syncing, setSyncing] = useState(false);
  const [syncMsg, setSyncMsg] = useState<string | null>(null);

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
        setSyncMsg(err instanceof Error ? err.message : "couldn't start sync — try again");
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

  // ── steam connect panel state ───────────────────────────────────────────────
  // undefined = still loading from server; null = not connected; string = steamid
  const [steamIdState, setSteamIdState] = useState<string | null | undefined>(undefined);
  const [steamPersona, setSteamPersona] = useState<string | null>(null);
  const [steamConnecting, setSteamConnecting] = useState(false);
  const [steamMsg, setSteamMsg] = useState<string | null>(null);

  // Load steam identity on mount + consume any return fragment from Steam OpenID
  useEffect(() => {
    let cancelled = false;

    const fragment = consumeReturnFragment();

    if (fragment !== null && 'error' in fragment) {
      // Steam returned an error fragment (verify_failed or steam_unreachable).
      // Show the message but do NOT return — fall through to identity load so
      // steamIdState resolves to null and the connect button appears for retry.
      setSteamMsg(
        fragment.error === 'verify_failed'
          ? "we couldn't verify your Steam account — try again"
          : 'Steam is currently unavailable — try again later',
      );
    }

    if (fragment !== null && 'steamid' in fragment) {
      // Steam OpenID returned — the admin extra step: persist on server then save locally
      const { steamid, persona } = fragment;
      setSteamConnecting(true);

      withAuth(() => adminSetSteamIdentity(steamid), navigate)
        .then(() => withAuth(() => adminSteamOwned(steamid), navigate))
        .then((ownedResult) => {
          if (cancelled) return;
          const owned = ownedResult === 'private' ? [] : ownedResult;
          saveIdentity({ steamid, persona, owned, fetched_at: Date.now() });
          setSteamIdState(steamid);
          setSteamPersona(persona);
          setSteamConnecting(false);
        })
        .catch(() => {
          if (!cancelled) {
            setSteamConnecting(false);
            setSteamMsg('connect failed — try again');
            setSteamIdState(null);
          }
        });
      return () => {
        cancelled = true;
      };
    }

    // No fragment — load current identity from server
    withAuth(() => adminSteamIdentity(), navigate)
      .then((id) => {
        if (cancelled) return;
        setSteamIdState(id);
        if (id !== null) {
          const local = loadIdentity();
          if (local?.steamid === id) {
            setSteamPersona(local.persona);
          }
        }
      })
      .catch(() => {
        if (!cancelled) setSteamIdState(null);
      });

    return () => {
      cancelled = true;
    };
  }, [navigate]); // eslint-disable-line react-hooks/exhaustive-deps

  const handleDisconnect = () => {
    withAuth(() => adminClearSteamIdentity(), navigate)
      .then(() => {
        clearIdentity();
        setSteamIdState(null);
        setSteamPersona(null);
      })
      .catch((err: unknown) => {
        setSteamMsg(err instanceof Error ? err.message : 'disconnect failed — try again');
      });
  };

  return (
    <div className="flex flex-col gap-8">
      {/* ── Sync panel ──────────────────────────────────────────────────── */}
      <section className="flex flex-col gap-3 rounded bg-floor p-4">
        <h2 className="text-sm font-medium text-ink-soft">sync</h2>
        <button
          type="button"
          onClick={handleSync}
          disabled={syncing || syncRunning}
          className="w-fit rounded bg-control px-4 py-2 text-sm hover:bg-control-bright disabled:opacity-50"
        >
          {syncing || syncRunning ? 'syncing…' : 'sync now'}
        </button>
        {syncMsg !== null && (
          <p role="status" className="text-sm text-ink-soft">
            {syncMsg}
          </p>
        )}
      </section>

      {/* ── Steam connect panel ──────────────────────────────────────────── */}
      <section className="flex flex-col gap-3 rounded bg-floor p-4">
        <h2 className="text-sm font-medium text-ink-soft">steam identity</h2>
        {steamIdState === undefined ? (
          <p className="text-xs text-dust-faint">loading…</p>
        ) : steamConnecting ? (
          <p className="text-xs text-dust-faint">connecting…</p>
        ) : steamIdState !== null ? (
          <div className="flex items-center gap-3">
            <span className="rounded bg-shelf px-2 py-1 text-xs text-ink-soft">
              {steamPersona ?? steamIdState}
            </span>
            <button
              type="button"
              onClick={handleDisconnect}
              className="rounded bg-control px-3 py-1.5 text-xs hover:bg-control-bright"
            >
              disconnect
            </button>
          </div>
        ) : (
          <button
            type="button"
            onClick={() => beginConnect('/admin/ops')}
            className="w-fit rounded bg-control px-4 py-2 text-sm hover:bg-control-bright"
          >
            connect steam
          </button>
        )}
        {steamMsg !== null && (
          <p role="status" className="text-sm text-ink-soft">
            {steamMsg}
          </p>
        )}
      </section>

      {/* ── Status card ─────────────────────────────────────────────────── */}
      <section className="flex flex-col gap-3 rounded bg-floor p-4">
        <h2 className="text-sm font-medium text-ink-soft">status</h2>
        {status === null && <p className="text-xs text-dust-faint">loading…</p>}
        {status !== null && (
          <div className="flex flex-col gap-2">
            <p className="text-xs text-dust">
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
              <p className="text-xs text-amber-800">
                sync running — started {formatRelativeTime(status.sync_run.started_epoch)}
              </p>
            )}
            {/* Marker present but not live: the run died before reporting (crash/timeout).
                Without this line a dropped backfill is indistinguishable from idle. */}
            {status.sync_run !== null && !status.sync_run.running && (
              <p className="text-xs text-red-700">
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
                <p className="text-xs text-dust">{status.sync.message}</p>
              </>
            )}

            {Object.keys(status.game_counts).length > 0 && (
              <div className="flex flex-wrap gap-2">
                {Object.entries(status.game_counts).map(([key, count]) => (
                  <span
                    key={key}
                    className="rounded bg-control px-2 py-0.5 text-xs text-ink-soft"
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
