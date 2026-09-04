import { useState, useEffect, useCallback } from 'react';
import { useNavigate } from 'react-router-dom';
import {
  adminFriends,
  adminCreateFriend,
  adminReissueFriendToken,
  adminRevokeFriendToken,
  CreateLinkValidationError,
  type AdminFriend,
} from '../api';
import { withAuth } from './withAuth';
import { shelfUrl } from '../inviteUrl';

// Page-level state machine — mirrors Links.tsx's PageState shape.
type PageState =
  | { phase: 'loading' }
  | { phase: 'error' }
  | { phase: 'loaded'; friends: AdminFriend[] };

/** Immutably drop one key from a record (Links.tsx's omitKey, same reason:
 * the destructuring-rest omit idiom trips this repo's no-unused-vars). */
function omitKey<T>(map: Record<string, T>, key: string): Record<string, T> {
  const next = { ...map };
  delete next[key];
  return next;
}

export function Friends() {
  const navigate = useNavigate();
  const [state, setState] = useState<PageState>({ phase: 'loading' });

  // Create form state
  const [formName, setFormName] = useState('');
  const [creating, setCreating] = useState(false);
  const [createdInfo, setCreatedInfo] = useState<{ fullUrl: string; name: string } | null>(
    null,
  );
  // A create failure must be loud — minting a shelf is a spend-adjacent action
  // (Links.tsx's createError pattern, same reasoning).
  const [createError, setCreateError] = useState<string | null>(null);

  // Two-step reissue: id → armed. Confirming shows the in-voice warning BEFORE
  // the request fires — the old token dies the moment reissue succeeds.
  const [reissueArmed, setReissueArmed] = useState<Set<string>>(new Set());
  const [reissueErrors, setReissueErrors] = useState<Record<string, string>>({});

  // Two-step revoke — mirrors Links.tsx's revokeArmed/revokeErrors exactly.
  const [revokeArmed, setRevokeArmed] = useState<Set<string>>(new Set());
  const [revokeErrors, setRevokeErrors] = useState<Record<string, string>>({});

  // In-flight guard for reissue AND revoke — one Set serves both (they're
  // mutually exclusive per row and both rotate/kill the same token). Kindred
  // to Links.tsx's reassignBusy: set before the request, cleared in finally,
  // confirm and cancel disabled while set. Without it a double-click fires
  // two POSTs — two token rotations, the first painted token already dead.
  const [actionBusy, setActionBusy] = useState<Set<string>>(new Set());

  const actionBusyOn = (id: string) =>
    setActionBusy((prev) => new Set(prev).add(id));
  const actionBusyOff = (id: string) =>
    setActionBusy((prev) => {
      const next = new Set(prev);
      next.delete(id);
      return next;
    });

  const load = useCallback(() => {
    setState({ phase: 'loading' });
    withAuth(() => adminFriends(), navigate)
      .then((friends) => setState({ phase: 'loaded', friends }))
      .catch(() => setState({ phase: 'error' }));
  }, [navigate]);

  useEffect(() => {
    load();
  }, [load]);

  const handleCreate = (e: React.FormEvent) => {
    e.preventDefault();
    const trimmed = formName.trim();
    if (!trimmed) return;
    setCreating(true);
    setCreateError(null);
    withAuth(() => adminCreateFriend(trimmed), navigate)
      .then((result) => {
        setCreatedInfo({ fullUrl: shelfUrl(result.shelf_token), name: trimmed });
        setCreateError(null);
        setFormName('');
        load();
      })
      .catch((err: unknown) => {
        // A previous success callout with no visible label reads as "this new
        // friend was added" next to a fresh failure — drop it (Links.tsx's
        // createdInfo-vs-createError rule).
        setCreatedInfo(null);
        setCreateError(
          err instanceof CreateLinkValidationError
            ? err.message
            : "couldn't add the friend — check the list below before retrying.",
        );
      })
      .finally(() => setCreating(false));
  };

  const handleReissueArm = (id: string) => {
    setReissueArmed((prev) => new Set(prev).add(id));
  };

  const handleReissueCancel = (id: string) => {
    setReissueArmed((prev) => {
      const next = new Set(prev);
      next.delete(id);
      return next;
    });
    setReissueErrors((prev) => omitKey(prev, id));
  };

  const handleReissueConfirm = (friend: AdminFriend) => {
    // Re-entrancy guard on top of the disabled button — a queued second
    // click must never fire a second rotation.
    if (actionBusy.has(friend.id)) return;
    // Reissue also serves a REVOKED friend (backend accepts it since the
    // pass-2 pairing): no old link dies there, so the failure copy differs.
    const live = friend.shelf_token !== '';
    actionBusyOn(friend.id);
    setReissueErrors((prev) => omitKey(prev, friend.id));
    withAuth(() => adminReissueFriendToken(friend.id), navigate)
      .then(() => {
        setReissueArmed((prev) => {
          const next = new Set(prev);
          next.delete(friend.id);
          return next;
        });
        load();
      })
      .catch(() => {
        // "may" on purpose — on a timeout after a server-side commit the
        // client cannot know whether the rotation landed (revoke copy below
        // already hedges the same way).
        setReissueErrors((prev) => ({
          ...prev,
          [friend.id]: live
            ? "couldn't reissue — the old link may still be live. try again."
            : "couldn't issue the new link — try again.",
        }));
      })
      .finally(() => actionBusyOff(friend.id));
  };

  const handleRevoke = (friend: AdminFriend) => {
    if (!revokeArmed.has(friend.id)) {
      setRevokeArmed((prev) => new Set(prev).add(friend.id));
      return;
    }
    if (actionBusy.has(friend.id)) return;
    actionBusyOn(friend.id);
    withAuth(() => adminRevokeFriendToken(friend.id), navigate)
      .then(() => {
        setRevokeArmed((prev) => {
          const next = new Set(prev);
          next.delete(friend.id);
          return next;
        });
        setRevokeErrors((prev) => omitKey(prev, friend.id));
        load();
      })
      .catch(() => {
        setRevokeErrors((prev) => ({
          ...prev,
          [friend.id]: 'revoke failed — the shelf link may still be live. try again.',
        }));
      })
      .finally(() => actionBusyOff(friend.id));
  };

  const copyToClipboard = (text: string) => {
    void navigator.clipboard.writeText(text);
  };

  if (state.phase === 'loading') {
    return <p className="text-dust">loading…</p>;
  }

  if (state.phase === 'error') {
    return (
      <div className="flex flex-col gap-4">
        <p className="text-dust">couldn't load friends — try again</p>
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

  const info = createdInfo;

  return (
    <div className="flex flex-col gap-6">
      {/* ── Create form ────────────────────────────────────────────────── */}
      <form onSubmit={handleCreate} className="flex flex-col gap-3 rounded bg-floor p-4">
        <h2 className="text-sm font-medium text-ink-soft">new friend</h2>
        <div className="flex flex-wrap items-end gap-3">
          <label className="flex flex-col gap-1 text-xs text-dust">
            name
            <input
              type="text"
              required
              aria-label="name"
              value={formName}
              onChange={(e) => setFormName(e.target.value)}
              className="rounded border border-line bg-shelf px-2 py-1 text-sm text-ink"
            />
          </label>
          <button
            type="submit"
            disabled={creating}
            className="w-fit rounded bg-control px-4 py-2 text-sm hover:bg-control-bright disabled:opacity-50"
          >
            add friend
          </button>
        </div>

        {createError !== null && (
          <p role="alert" className="text-xs text-red-700">
            {createError}
          </p>
        )}
      </form>

      {/* ── Created friend callout — the shelf URL ben hands them ─────── */}
      {info !== null && (
        <div className="rounded border border-line bg-floor p-4">
          <p className="mb-2 text-sm text-ink-soft">
            friend added — send them their shelf:
          </p>
          <div className="flex items-center gap-3">
            <code className="flex-1 break-all rounded bg-shelf px-3 py-2 text-sm text-ink">
              {info.fullUrl}
            </code>
            <button
              type="button"
              onClick={() => copyToClipboard(info.fullUrl)}
              aria-label={`copy shelf link for ${info.name}`}
              className="rounded bg-control px-3 py-2 text-xs hover:bg-control-bright"
            >
              copy
            </button>
          </div>
        </div>
      )}

      {/* ── Friends list ───────────────────────────────────────────────── */}
      <div className="space-y-2">
        {state.friends.map((friend) => {
          const live = friend.shelf_token !== '';
          const url = live ? shelfUrl(friend.shelf_token) : null;
          const armedReissue = reissueArmed.has(friend.id);
          const reissueErr = reissueErrors[friend.id];
          const armedRevoke = revokeArmed.has(friend.id);
          const revokeErr = revokeErrors[friend.id];
          const busy = actionBusy.has(friend.id);

          return (
            <div key={friend.id} className="rounded bg-floor p-4">
              <div className="flex flex-wrap items-center gap-3">
                <span className="font-medium text-ink">{friend.name}</span>

                {!live && (
                  <span className="rounded bg-red-900 px-2 py-0.5 text-xs text-red-200">
                    revoked
                  </span>
                )}

                {/* A revoked friend is not a dead-end: the reissue endpoint
                    now accepts a revoked friend (backend-paired change), so
                    offer the way back — same confirm+busy flow, different
                    arming copy since no old link dies here. */}
                {!live && (
                  <div className="ml-auto flex items-center gap-2">
                    <button
                      type="button"
                      disabled={armedReissue}
                      onClick={() => handleReissueArm(friend.id)}
                      aria-label={`issue a new link for ${friend.name}`}
                      className="rounded bg-control px-3 py-1.5 text-xs hover:bg-control-bright disabled:opacity-50"
                    >
                      issue a new link
                    </button>
                  </div>
                )}

                {live && (
                  <div className="ml-auto flex items-center gap-2">
                    <button
                      type="button"
                      onClick={() => copyToClipboard(url!)}
                      aria-label={`copy shelf link for ${friend.name}`}
                      className="rounded bg-control px-3 py-1.5 text-xs hover:bg-control-bright"
                    >
                      copy URL
                    </button>
                    <button
                      type="button"
                      disabled={armedReissue}
                      onClick={() => handleReissueArm(friend.id)}
                      aria-label={`reissue shelf link for ${friend.name}`}
                      className="rounded bg-control px-3 py-1.5 text-xs hover:bg-control-bright disabled:opacity-50"
                    >
                      reissue
                    </button>
                    <button
                      type="button"
                      disabled={busy}
                      onClick={() => handleRevoke(friend)}
                      aria-label={armedRevoke ? `confirm revoke ${friend.name}` : `revoke ${friend.name}`}
                      className={`rounded px-3 py-1.5 text-xs disabled:opacity-50 ${
                        armedRevoke
                          ? 'bg-red-700 text-red-100 hover:bg-red-600'
                          : 'bg-control hover:bg-control-bright'
                      }`}
                    >
                      {armedRevoke ? 'confirm?' : 'revoke'}
                    </button>
                  </div>
                )}
              </div>

              {/* Current shelf URL, or the revoked-empty state — this is what
                  reissue/revoke update once they resolve. */}
              {live ? (
                <p className="mt-2 break-all text-xs text-dust">{url}</p>
              ) : (
                <p className="mt-2 text-xs text-dust">no shelf link</p>
              )}

              {/* Reissue confirm — the in-voice warning renders BEFORE the
                  request fires; nothing here is a window.confirm(). */}
              {armedReissue && (
                <div className="mt-2 flex flex-wrap items-center gap-3 rounded bg-shelf p-2">
                  <p className="text-xs text-dust">
                    {live
                      ? 'the old link stops working — hand them the new one'
                      : 'this hands them a brand-new link — their shelf wakes back up'}
                  </p>
                  <button
                    type="button"
                    disabled={busy}
                    onClick={() => handleReissueConfirm(friend)}
                    aria-label={`confirm reissue for ${friend.name}`}
                    className="rounded bg-control px-3 py-1.5 text-xs hover:bg-control-bright disabled:opacity-50"
                  >
                    {busy ? 'issuing…' : 'confirm reissue'}
                  </button>
                  <button
                    type="button"
                    disabled={busy}
                    onClick={() => handleReissueCancel(friend.id)}
                    aria-label={`cancel reissue for ${friend.name}`}
                    className="rounded px-3 py-1.5 text-xs text-dust hover:text-ink disabled:opacity-50"
                  >
                    cancel
                  </button>
                </div>
              )}

              {reissueErr !== undefined && (
                <p role="alert" className="mt-2 text-xs text-red-700">
                  {reissueErr}
                </p>
              )}

              {revokeErr !== undefined && (
                <p role="alert" className="mt-2 text-xs text-red-700">
                  {revokeErr}
                </p>
              )}
            </div>
          );
        })}
      </div>
    </div>
  );
}
