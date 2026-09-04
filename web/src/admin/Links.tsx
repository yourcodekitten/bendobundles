import { useState, useEffect, useCallback } from 'react';
import { useNavigate, useLocation } from 'react-router-dom';
import {
  adminLinks,
  adminCreateLink,
  adminRevoke,
  adminLinkClaims,
  adminSetLinkNote,
  adminSetLinkUnlock,
  adminDeleteLinkUnlock,
  adminFriends,
  adminSetLinkFriend,
  CreateLinkValidationError,
  GIFT_NOTE_MAX,
  type AdminLink,
  type AdminClaimView,
  type AdminFriend,
} from '../api';
import { withAuth } from './withAuth';
import { exposureLabel } from './pickExposure';
import { stateBadgeClass } from '../stateBadge';
import { clampCodePoints, codePointCount } from '../text';
import { inviteUrl } from '../inviteUrl';

// Page-level state machine
type PageState =
  | { phase: 'loading' }
  | { phase: 'error' }
  | { phase: 'loaded'; links: AdminLink[] };

// Per-token audit expansion state
type AuditData =
  | { phase: 'loading' }
  | { phase: 'error' }
  | { phase: 'loaded'; claims: AdminClaimView[] };

/** Immutably drop one key from a record — the destructuring-rest omit idiom
 * trips this repo's no-unused-vars (no ignoreRestSiblings). */
function omitKey<T>(map: Record<string, T>, key: string): Record<string, T> {
  const next = { ...map };
  delete next[key];
  return next;
}

// Show the note character counter only near the bound — both note textareas
// (create form + row editor) share this threshold so they can't drift.
// Counted in CODE POINTS (the server's chars() unit) — maxLength/.length are
// UTF-16 units and would cap an emoji-heavy note at half the real limit while
// the counter lied about it (OMBB, #69 review). Input is clamped to the bound
// via clampCodePoints instead of maxLength.
const NOTE_COUNTER_AT = GIFT_NOTE_MAX - 100;

function formatDate(iso: string): string {
  return new Date(iso).toLocaleDateString();
}

function formatDateTime(iso: string): string {
  return new Date(iso).toLocaleString();
}

/** Sealed = has an unlock moment that hasn't arrived. Past unlock_at is just open. */
function isSealed(link: AdminLink): boolean {
  return link.unlock_at !== undefined && new Date(link.unlock_at).getTime() > Date.now();
}

export function Links() {
  const navigate = useNavigate();
  const location = useLocation();
  const [state, setState] = useState<PageState>({ phase: 'loading' });

  // Friend roster (Task 6) — loaded independently of the links list: it only
  // feeds a picker/name-lookup, so a friends-fetch failure must not blank the
  // links page. `friends` stays [] until it resolves; every lookup already
  // tolerates "not found".
  const [friends, setFriends] = useState<AdminFriend[]>([]);
  useEffect(() => {
    withAuth(() => adminFriends(), navigate)
      .then(setFriends)
      .catch(() => {});
  }, [navigate]);
  const friendName = (id: string | undefined): string | undefined =>
    id === undefined ? undefined : friends.find((f) => f.id === id)?.name;

  // Create form state
  const [formLabel, setFormLabel] = useState('');
  // Friend picker on create — '' means open shelf (no assignment). The link
  // API itself has no friend_id field (Task 4's CreateLinkBody), so a pick
  // here fires a follow-up POST /admin/api/links/{token}/friend right after
  // creation succeeds.
  const [pickedFriendId, setPickedFriendId] = useState('');
  const [claimsAllowed, setClaimsAllowed] = useState(1);
  const [expiresDays, setExpiresDays] = useState('');
  const [unlockAt, setUnlockAt] = useState('');
  const [giftNote, setGiftNote] = useState('');
  const [creating, setCreating] = useState(false);
  // picks arrive from the catalog's "wrap these into a link" (router state) —
  // order is ben's pick order and is the order sent to the api.
  // `requiresChoice` is OPTIONAL on purpose: router state is not persisted, so a
  // reload already yields []. ABSENT must mean UNKNOWN, never false — the exposure
  // readout depends on telling those apart, and a required boolean would erase the
  // distinction at the type level.
  const [picked, setPicked] = useState<
    { id: string; title: string; requiresChoice?: boolean }[]
  >(
    () =>
      (
        location.state as {
          picked?: { id: string; title: string; requiresChoice?: boolean }[];
        } | null
      )?.picked ?? [],
  );
  // Stored after successful create — separate from page state so reload doesn't clear it
  const [createdInfo, setCreatedInfo] = useState<
    { fullUrl: string; label: string; chosen?: string[] } | null
  >(null);
  // Create failure — creating a link is a spend-adjacent action; a silent catch
  // leaves the admin with zero signal whether a link now exists (mirrors the
  // revoke-error pattern below). A 422 carries the violated bound verbatim.
  const [createError, setCreateError] = useState<string | null>(null);

  // Two-step revoke: set of armed token strings
  const [revokeArmed, setRevokeArmed] = useState<Set<string>>(new Set());
  // Per-token revoke failure — revoking a leaked invite is a security action,
  // a failure must never look like success
  const [revokeErrors, setRevokeErrors] = useState<Record<string, string>>({});

  // Audit expansions: token → AuditData (noUncheckedIndexedAccess → AuditData | undefined)
  const [auditMap, setAuditMap] = useState<Record<string, AuditData>>({});

  // Note editing: token → draft text (key presence = editor open). Saving a
  // blank draft clears the note server-side.
  // Per-row seal editor (mirrors the note editor's draft/error/saving shape).
  const [sealDrafts, setSealDrafts] = useState<Record<string, string>>({});
  const [sealErrors, setSealErrors] = useState<Record<string, string>>({});
  const [sealBusy, setSealBusy] = useState<Set<string>>(new Set());
  const [noteDrafts, setNoteDrafts] = useState<Record<string, string>>({});
  const [noteSaving, setNoteSaving] = useState<Set<string>>(new Set());
  // Per-token note-save failure — a silent catch would leave ben unsure what
  // the friend's page now says (mirrors the revoke-error pattern)
  const [noteErrors, setNoteErrors] = useState<Record<string, string>>({});

  // Friend reassignment: token → pending friend id ('' = clear). Key presence
  // is the "armed" signal (noteDrafts' shape) — the in-voice warning and the
  // request both wait for a SEPARATE confirm click, never fire on selection
  // alone. Reassigning MOVES the link's already-claimed gifts to the other
  // shelf (set_link_friend moves gsi3pk atomically), so a stray click must
  // never be the trigger.
  const [reassignDrafts, setReassignDrafts] = useState<Record<string, string>>({});
  const [reassignErrors, setReassignErrors] = useState<Record<string, string>>({});
  const [reassignBusy, setReassignBusy] = useState<Set<string>>(new Set());

  const load = useCallback(() => {
    setState({ phase: 'loading' });
    // withAuth re-throws non-Unauthorized errors → .catch sets error state
    withAuth(() => adminLinks(), navigate)
      .then((links) => setState({ phase: 'loaded', links }))
      .catch(() => setState({ phase: 'error' }));
  }, [navigate]);

  useEffect(() => {
    load();
  }, [load]);

  const handleCreate = (e: React.FormEvent) => {
    e.preventDefault();
    const trimmedLabel = formLabel.trim();
    if (!trimmedLabel) return;
    setCreating(true);
    setCreateError(null);
    const expires = expiresDays !== '' ? parseInt(expiresDays, 10) : undefined;
    // Blank/whitespace note → omit the field; the server also normalizes, this
    // just keeps the wire clean.
    const note = giftNote.trim() !== '' ? giftNote.trim() : undefined;
    // datetime-local is ben's LOCAL wall-clock pick; the browser resolves it to
    // an absolute instant here, at write time (family review: a bare date is
    // how a gift opens at 7pm).
    const unlock = unlockAt !== '' ? new Date(unlockAt).toISOString() : undefined;
    const gameIds = picked.length > 0 ? picked.map((p) => p.id) : undefined;
    withAuth(
      () => adminCreateLink(trimmedLabel, claimsAllowed, expires, note, unlock, gameIds),
      navigate,
    )
      .then((result) => {
        setCreatedInfo({
          fullUrl: inviteUrl(result.token),
          label: trimmedLabel,
          chosen: picked.map((p) => p.title),
        });
        setCreateError(null);
        setFormLabel('');
        setClaimsAllowed(1);
        setExpiresDays('');
        setUnlockAt('');
        setGiftNote('');
        setPicked([]);
        const friendId = pickedFriendId;
        setPickedFriendId('');
        // The link API itself has no friend_id field — a pick fires the
        // separate assignment endpoint right after creation. The link exists
        // either way; a failed assignment must say so without erasing the
        // just-created invite URL above (it's still copyable from the list).
        if (friendId !== '') {
          withAuth(() => adminSetLinkFriend(result.token, friendId), navigate)
            .catch(() => {
              setCreateError(
                'link created, but assigning the friend failed — assign it from the list below.',
              );
            })
            .finally(() => load());
        } else {
          // Reload to prepend the new link into the list
          load();
        }
      })
      .catch((err: unknown) => {
        // withAuth handles 401. A 422 means the server rejected the INPUT —
        // no link exists; show the violated bound verbatim so it can be
        // corrected (inputs stay put). Anything else (5xx, network) means we
        // DON'T KNOW whether the link exists — say so. Either way drop any
        // PREVIOUS success callout: it has no visible label, so next to a
        // fresh failure it reads as "your link was created" and the admin can
        // hand a friend the wrong URL. (The old link's URL stays copyable
        // from its list row.)
        setCreatedInfo(null);
        setCreateError(
          err instanceof CreateLinkValidationError
            ? err.message
            : "couldn't create the link — check the list below before retrying.",
        );
      })
      .finally(() => {
        setCreating(false);
      });
  };

  const handleSaveNote = (link: AdminLink) => {
    const draft = noteDrafts[link.token];
    if (draft === undefined) return;
    // No-op save (unchanged text, e.g. open-then-save): just close the editor —
    // no request, no server write, no list refetch.
    if (draft.trim() === (link.gift_note ?? '')) {
      setNoteDrafts((prev) => omitKey(prev, link.token));
      return;
    }
    setNoteSaving((prev) => new Set(prev).add(link.token));
    setNoteErrors((prev) => omitKey(prev, link.token));
    withAuth(() => adminSetLinkNote(link.token, draft.trim()), navigate)
      .then(() => {
        // Only close the editor if the draft is still what we saved — the
        // textarea stays editable during a slow save, and an unconditional
        // delete here would silently discard anything typed since.
        setNoteDrafts((prev) =>
          prev[link.token] === draft ? omitKey(prev, link.token) : prev,
        );
        // Reload so the row reflects what the friend's page now says
        load();
      })
      .catch((err: unknown) => {
        setNoteErrors((prev) => ({
          ...prev,
          [link.token]:
            err instanceof CreateLinkValidationError
              ? err.message
              : "couldn't save the note — the friend's page is unchanged.",
        }));
      })
      .finally(() => {
        setNoteSaving((prev) => {
          const next = new Set(prev);
          next.delete(link.token);
          return next;
        });
      });
  };

  const sealBusyOn = (token: string) =>
    setSealBusy((prev) => new Set(prev).add(token));
  const sealBusyOff = (token: string) =>
    setSealBusy((prev) => {
      const next = new Set(prev);
      next.delete(token);
      return next;
    });

  const handleSaveUnlock = (link: AdminLink) => {
    const draft = sealDrafts[link.token];
    if (draft === undefined || draft === '') return;
    sealBusyOn(link.token);
    setSealErrors((prev) => omitKey(prev, link.token));
    // datetime-local → absolute instant, resolved here in ben's browser.
    const iso = new Date(draft).toISOString();
    withAuth(() => adminSetLinkUnlock(link.token, iso), navigate)
      .then(() => {
        setSealDrafts((prev) => omitKey(prev, link.token));
        load();
      })
      .catch((err: unknown) => {
        setSealErrors((prev) => ({
          ...prev,
          [link.token]: err instanceof Error ? err.message : "couldn't move the moment",
        }));
        // A 409 means the seal state changed under us (it opened) — reload so
        // the row stops offering controls the server will refuse.
        load();
      })
      .finally(() => sealBusyOff(link.token));
  };

  const handleUnseal = (link: AdminLink) => {
    sealBusyOn(link.token);
    setSealErrors((prev) => omitKey(prev, link.token));
    withAuth(() => adminDeleteLinkUnlock(link.token), navigate)
      .then(() => load())
      .catch((err: unknown) => {
        setSealErrors((prev) => ({
          ...prev,
          [link.token]: err instanceof Error ? err.message : "couldn't unseal",
        }));
        load();
      })
      .finally(() => sealBusyOff(link.token));
  };

  const handleReassignCancel = (token: string) => {
    setReassignDrafts((prev) => omitKey(prev, token));
    setReassignErrors((prev) => omitKey(prev, token));
  };

  const handleReassignConfirm = (link: AdminLink) => {
    const draft = reassignDrafts[link.token];
    if (draft === undefined) return;
    setReassignBusy((prev) => new Set(prev).add(link.token));
    setReassignErrors((prev) => omitKey(prev, link.token));
    withAuth(() => adminSetLinkFriend(link.token, draft === '' ? null : draft), navigate)
      .then(() => {
        setReassignDrafts((prev) => omitKey(prev, link.token));
        load();
      })
      .catch((err: unknown) => {
        setReassignErrors((prev) => ({
          ...prev,
          [link.token]:
            err instanceof CreateLinkValidationError
              ? err.message
              : "couldn't reassign — the link's friend is unchanged. try again.",
        }));
      })
      .finally(() => {
        setReassignBusy((prev) => {
          const next = new Set(prev);
          next.delete(link.token);
          return next;
        });
      });
  };

  const handleRevoke = (link: AdminLink) => {
    if (!revokeArmed.has(link.token)) {
      // First click: arm the button
      setRevokeArmed((prev) => new Set(prev).add(link.token));
      return;
    }
    // Second click: execute
    withAuth(() => adminRevoke(link.token), navigate)
      .then(() => {
        setRevokeArmed((prev) => {
          const next = new Set(prev);
          next.delete(link.token);
          return next;
        });
        setRevokeErrors((prev) => omitKey(prev, link.token));
        load();
      })
      .catch(() => {
        // withAuth handles 401. Anything else (adminRevoke throws on !ok, or
        // network) means the link may STILL BE LIVE — say so, keep the button
        // armed so the next click retries immediately.
        setRevokeErrors((prev) => ({
          ...prev,
          [link.token]: 'revoke failed — the link may still be live. try again.',
        }));
      });
  };

  const handleAuditToggle = (token: string) => {
    const current = auditMap[token];
    if (current !== undefined) {
      // Already open — collapse
      setAuditMap((prev) => omitKey(prev, token));
      return;
    }
    // Open: start loading
    setAuditMap((prev) => ({ ...prev, [token]: { phase: 'loading' } }));
    withAuth(() => adminLinkClaims(token), navigate)
      .then((claims) => {
        setAuditMap((prev) => ({ ...prev, [token]: { phase: 'loaded', claims } }));
      })
      .catch(() => {
        setAuditMap((prev) => ({ ...prev, [token]: { phase: 'error' } }));
      });
  };

  const copyToClipboard = (text: string) => {
    void navigator.clipboard.writeText(text);
  };

  // Loading / error early returns (mirror Catalog.tsx style)
  if (state.phase === 'loading') {
    return <p className="text-dust">loading…</p>;
  }

  if (state.phase === 'error') {
    return (
      <div className="flex flex-col gap-4">
        <p className="text-dust">couldn't load links — try again</p>
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

  // Capture as const so TypeScript narrows through closures below
  const info = createdInfo;

  // Unassigned indicator (Lilith's finding): derived CLIENT-SIDE from the
  // already-fetched links — friend_id rides the read-override, so an absent
  // key means unassigned. Renders nothing at 0; the workbench carries the
  // doubt the shelf structurally can't (a link with no friend has nowhere to
  // show its gifts once claimed).
  const unassignedCount = state.links.filter((l) => l.friend_id === undefined).length;

  return (
    <div className="flex flex-col gap-6">
      {/* ── Create form ────────────────────────────────────────────────── */}
      <form onSubmit={handleCreate} className="flex flex-col gap-3 rounded bg-floor p-4">
        <h2 className="text-sm font-medium text-ink-soft">new invite link</h2>
        <div className="flex flex-wrap gap-3">
          <label className="flex flex-col gap-1 text-xs text-dust">
            label
            <input
              type="text"
              required
              aria-label="label"
              value={formLabel}
              onChange={(e) => setFormLabel(e.target.value)}
              className="rounded border border-line bg-shelf px-2 py-1 text-sm text-ink"
            />
          </label>
          <label className="flex flex-col gap-1 text-xs text-dust">
            claims allowed
            <input
              type="number"
              min={1}
              aria-label="claims allowed"
              aria-describedby="pick-exposure"
              value={claimsAllowed}
              onChange={(e) => {
                const n = parseInt(e.target.value, 10);
                setClaimsAllowed(isNaN(n) ? 1 : Math.max(1, n));
              }}
              className="w-24 rounded border border-line bg-shelf px-2 py-1 text-sm text-ink"
            />
          </label>
          {/* 🎟️ What this link exposes of ben's monthly picks. ONE LINE OF TEXT, not a
              card — PRODUCT.md: the admin is a workbench, not a dashboard.
              🔴 RENDERS UNCONDITIONALLY. Do NOT wrap this in `picked.length > 0 &&`.
              The empty case IS the finding: every link ben has ever sent is an open
              shelf exposing his whole allowance, and nothing has ever said so.
              ♿ `id` + `aria-describedby` on the allowance input, NOT a data-testid.
              This repo queries by role/label/text everywhere and had zero test ids in
              production markup — and the accessible route is the point, not the style:
              the line DESCRIBES the control, so a screen reader reads it out when ben
              focuses the field. A PR about telling the person who is spending must not
              ship the telling as pixels only.
              🔊 `aria-live="polite"` because aria-describedby is announced ON FOCUS and is then
              SILENT: ben changes the allowance while the field is focused, so the number moves
              with nothing spoken — the exact case the readout exists for. Polite, not assertive:
              it updates on deliberate acts (a pick removed, an allowance retyped), never in a loop.
              ⚠️ EPISTEMIC STATUS, stated because it matters: this is reasoned from the ARIA spec
              and has NOT been observed with a real screen reader by me or by the reviewer who
              raised it. The known risk is double-announcement on ATs that speak both the live
              region and the description. If anyone ever tests it and it doubles, drop aria-live
              and move the text into an aria-live sibling instead. */}
          <p className="self-end text-xs text-dust" id="pick-exposure" aria-live="polite">
            {exposureLabel(picked, claimsAllowed)}
          </p>
          <label className="flex flex-col gap-1 text-xs text-dust">
            expires in days (optional)
            <input
              type="number"
              min={1}
              aria-label="expires in days"
              value={expiresDays}
              onChange={(e) => setExpiresDays(e.target.value)}
              placeholder="never"
              className="w-24 rounded border border-line bg-shelf px-2 py-1 text-sm text-ink"
            />
          </label>
          <label className="flex flex-col gap-1 text-xs text-dust">
            unlocks at (optional — wraps the gift until then)
            <input
              type="datetime-local"
              aria-label="unlocks at"
              value={unlockAt}
              onChange={(e) => setUnlockAt(e.target.value)}
              className="rounded border border-line bg-shelf px-2 py-1 text-sm text-ink"
            />
          </label>
          <label className="flex flex-col gap-1 text-xs text-dust">
            friend (optional — puts this link on their shelf)
            <select
              aria-label="friend (optional — puts this link on their shelf)"
              value={pickedFriendId}
              onChange={(e) => setPickedFriendId(e.target.value)}
              className="rounded border border-line bg-shelf px-2 py-1 text-sm text-ink"
            >
              <option value="">open shelf — no friend</option>
              {friends.map((f) => (
                <option key={f.id} value={f.id}>
                  {f.name}
                </option>
              ))}
            </select>
          </label>
        </div>
        <label className="flex flex-col gap-1 text-xs text-dust">
          note to your friend (optional — greets them on their page)
          <textarea
            aria-label="note to your friend"
            value={giftNote}
            onChange={(e) =>
              setGiftNote(clampCodePoints(e.target.value, GIFT_NOTE_MAX))
            }
            rows={2}
            placeholder="picked these with you in mind…"
            className="rounded border border-line bg-shelf px-2 py-1 text-sm text-ink"
          />
          {codePointCount(giftNote) > NOTE_COUNTER_AT && (
            <span className="text-right text-dust">
              {codePointCount(giftNote)}/{GIFT_NOTE_MAX}
            </span>
          )}
        </label>
        {picked.length > 0 && (
          <div className="flex flex-col gap-1 text-xs text-dust">
            <span>chosen for this gift ({picked.length})</span>
            <ul className="flex flex-wrap gap-1.5">
              {picked.map((p, i) => (
                <li key={p.id}
                  className="flex items-center gap-1 rounded bg-shelf px-2 py-0.5 text-xs text-ink-soft">
                  {p.title}
                  <button type="button" aria-label={`move ${p.title} earlier`}
                    disabled={i === 0} className="disabled:opacity-40"
                    onClick={() => setPicked((cur) => {
                      const next = [...cur];
                      // safe: the button is disabled at i===0, so i-1 is in bounds here
                      const tmp = next[i - 1]!;
                      next[i - 1] = next[i]!;
                      next[i] = tmp;
                      return next;
                    })}>↑</button>
                  <button type="button" aria-label={`move ${p.title} later`}
                    disabled={i === picked.length - 1} className="disabled:opacity-40"
                    onClick={() => setPicked((cur) => {
                      const next = [...cur];
                      // safe: the button is disabled at the last index, so i+1 is in bounds here
                      const tmp = next[i + 1]!;
                      next[i + 1] = next[i]!;
                      next[i] = tmp;
                      return next;
                    })}>↓</button>
                  <button type="button" aria-label={`remove ${p.title} from this gift`}
                    onClick={() => setPicked((cur) => cur.filter((q) => q.id !== p.id))}>×</button>
                </li>
              ))}
            </ul>
          </div>
        )}
        <button
          type="submit"
          disabled={creating}
          className="w-fit rounded bg-control px-4 py-2 text-sm hover:bg-control-bright disabled:opacity-50"
        >
          create invite link
        </button>

        {/* Create failure — must be loud; without it the admin can't tell
            whether an invite link exists */}
        {createError !== null && (
          <p role="alert" className="text-xs text-red-700">
            {createError}
          </p>
        )}
      </form>

      {/* ── Created link callout — the artifact ben hands a friend ───── */}
      {info !== null && (
        <div className="rounded border border-line bg-floor p-4">
          <p className="mb-2 text-sm text-ink-soft">
            invite link created — send this to your friend:
          </p>
          <div className="flex items-center gap-3">
            <code className="flex-1 break-all rounded bg-shelf px-3 py-2 text-sm text-ink">
              {info.fullUrl}
            </code>
            <button
              type="button"
              onClick={() => copyToClipboard(info.fullUrl)}
              aria-label={`copy invite for ${info.label}`}
              className="rounded bg-control px-3 py-2 text-xs hover:bg-control-bright"
            >
              copy
            </button>
          </div>
          {info.chosen !== undefined && info.chosen.length > 0 && (
            <p className="text-xs text-dust">wrapped: {info.chosen.join(', ')}</p>
          )}
        </div>
      )}

      {/* ── Links list ─────────────────────────────────────────────────── */}
      {unassignedCount > 0 && (
        <p className="text-xs text-dust">
          {unassignedCount === 1
            ? "1 gift isn't on a shelf yet"
            : `${unassignedCount} gifts aren't on a shelf yet`}
        </p>
      )}
      <div className="space-y-2">
        {state.links.map((link) => {
          const linkUrl = inviteUrl(link.token);
          const auditState = auditMap[link.token];
          const armed = revokeArmed.has(link.token);
          const revokeErr = revokeErrors[link.token];
          const noteDraft = noteDrafts[link.token];
          const noteErr = noteErrors[link.token];
          const savingNote = noteSaving.has(link.token);
          const sealDraft = sealDrafts[link.token];
          const sealErr = sealErrors[link.token];
          const currentFriendId = link.friend_id ?? '';
          const reassignDraft = reassignDrafts[link.token];
          const reassignErr = reassignErrors[link.token];
          const reassignIsBusy = reassignBusy.has(link.token);
          const assignedFriendName = friendName(link.friend_id);

          return (
            <div key={link.token} className="rounded bg-floor p-4">
              {/* Row: label, meta, actions */}
              <div className="flex flex-wrap items-center gap-3">
                <span className="font-medium text-ink">{link.label}</span>

                {link.revoked && (
                  <span className="rounded bg-red-900 px-2 py-0.5 text-xs text-red-200">
                    revoked
                  </span>
                )}

                <span className="text-sm text-dust">
                  {link.claims_used}/{link.claims_allowed} used
                </span>

                <span className="text-xs text-dust-faint">
                  created {formatDate(link.created_at)}
                </span>

                <span className="text-xs text-dust-faint">
                  expires{' '}
                  {link.expires_at !== null ? formatDate(link.expires_at) : 'never'}
                </span>

                {isSealed(link) && link.unlock_at !== undefined && (
                  <span className="rounded bg-give-soft px-2 py-0.5 text-xs text-give-ink">
                    🎁 sealed until {formatDateTime(link.unlock_at)}
                  </span>
                )}

                {link.curated_game_ids !== undefined && (
                  <span className="rounded bg-shelf px-2 py-0.5 text-xs text-ink-soft">
                    {link.curated_game_ids.length} chosen
                  </span>
                )}

                {/* Assigned friend — the row's own name display (Task 6);
                    absent = unassigned, counted in the header above. */}
                {assignedFriendName !== undefined && (
                  <span className="rounded bg-shelf px-2 py-0.5 text-xs text-ink-soft">
                    🎀 {assignedFriendName}
                  </span>
                )}

                {/* Actions — all accessible-named with the link's label */}
                <div className="ml-auto flex items-center gap-2">
                  <button
                    type="button"
                    onClick={() => copyToClipboard(linkUrl)}
                    aria-label={`copy invite for ${link.label}`}
                    className="rounded bg-control px-3 py-1.5 text-xs hover:bg-control-bright"
                  >
                    copy URL
                  </button>

                  {/* Friend reassignment — selecting a NEW value only arms the
                      confirm below; the request never fires on selection alone
                      (reassign moves the link's already-claimed gifts — AND
                      any thank-you note on them: a private reply-to-ben
                      becomes visible on the new friend's shelf, so the
                      warning below says so). */}
                  <select
                    aria-label={`reassign friend for ${link.label}`}
                    disabled={reassignIsBusy}
                    value={reassignDraft ?? currentFriendId}
                    onChange={(e) =>
                      setReassignDrafts((prev) => ({ ...prev, [link.token]: e.target.value }))
                    }
                    className="rounded border border-line bg-shelf px-2 py-1 text-xs text-ink disabled:opacity-50"
                  >
                    <option value="">no friend</option>
                    {friends.map((f) => (
                      <option key={f.id} value={f.id}>
                        {f.name}
                      </option>
                    ))}
                  </select>

                  {/* Revoke: two-step, not window.confirm */}
                  {!link.revoked && (
                    <button
                      type="button"
                      onClick={() => handleRevoke(link)}
                      aria-label={
                        armed
                          ? `confirm revoke ${link.label}`
                          : `revoke ${link.label}`
                      }
                      className={`rounded px-3 py-1.5 text-xs ${
                        armed
                          ? 'bg-red-700 text-red-100 hover:bg-red-600'
                          : 'bg-control hover:bg-control-bright'
                      }`}
                    >
                      {armed ? 'confirm?' : 'revoke'}
                    </button>
                  )}

                  {isSealed(link) && (
                    <>
                      <button
                        type="button"
                        disabled={sealBusy.has(link.token)}
                        onClick={() => {
                          // Toggle the seal editor (note-editor shape): closing
                          // abandons the draft and its stale error.
                          setSealErrors((prev) => omitKey(prev, link.token));
                          setSealDrafts((prev) =>
                            prev[link.token] !== undefined
                              ? omitKey(prev, link.token)
                              : { ...prev, [link.token]: '' },
                          );
                        }}
                        aria-label={`move the moment for ${link.label}`}
                        className="rounded bg-control px-3 py-1.5 text-xs hover:bg-control-bright disabled:opacity-50"
                      >
                        move the moment
                      </button>
                      <button
                        type="button"
                        disabled={sealBusy.has(link.token)}
                        onClick={() => handleUnseal(link)}
                        aria-label={`unseal ${link.label}`}
                        className="rounded bg-control px-3 py-1.5 text-xs hover:bg-control-bright disabled:opacity-50"
                      >
                        unseal
                      </button>
                    </>
                  )}

                  <button
                    type="button"
                    disabled={savingNote}
                    onClick={() => {
                      // Closing the editor abandons the edit — drop its stale
                      // failure message along with the draft.
                      setNoteErrors((prev) => omitKey(prev, link.token));
                      setNoteDrafts((prev) =>
                        prev[link.token] !== undefined
                          ? omitKey(prev, link.token)
                          : { ...prev, [link.token]: link.gift_note ?? '' },
                      );
                    }}
                    aria-label={
                      link.gift_note !== undefined
                        ? `edit note for ${link.label}`
                        : `add note for ${link.label}`
                    }
                    className="rounded bg-control px-3 py-1.5 text-xs hover:bg-control-bright disabled:opacity-50"
                  >
                    {link.gift_note !== undefined ? 'edit note' : 'add note'}
                  </button>

                  <button
                    type="button"
                    onClick={() => handleAuditToggle(link.token)}
                    aria-label={
                      auditState !== undefined
                        ? `collapse audit for ${link.label}`
                        : `expand audit for ${link.label}`
                    }
                    className="rounded bg-control px-3 py-1.5 text-xs hover:bg-control-bright"
                  >
                    {auditState !== undefined ? 'collapse' : 'audit'}
                  </button>
                </div>
              </div>

              {/* Current note — what the friend's dialog says today */}
              {link.gift_note !== undefined && noteDraft === undefined && (
                <p className="mt-2 text-xs italic text-dust">
                  &ldquo;{link.gift_note}&rdquo;
                </p>
              )}

              {/* The friend's thank-you — read-only, ben receives their words.
                  Signed with the link's label: "♡ …" — Dave, 7/15/2026 */}
              {link.thank_note !== undefined && (
                <p className="mt-2 text-xs italic text-give-soft">
                  <span aria-hidden="true">♡ </span>
                  &ldquo;{link.thank_note}&rdquo;{' '}
                  <span className="not-italic text-dust-faint">
                    &mdash; {link.label}
                    {link.thanked_at !== undefined
                      ? `, ${formatDate(link.thanked_at)}`
                      : ''}
                  </span>
                </p>
              )}

              {/* Reassign confirm — the in-voice warning renders BEFORE the
                  request fires; only a picked value DIFFERENT from the
                  current assignment arms it (re-picking the same friend is a
                  no-op, mirrors the note editor's unchanged-draft check). */}
              {reassignDraft !== undefined && reassignDraft !== currentFriendId && (
                <div className="mt-2 flex flex-wrap items-center gap-3 rounded bg-shelf p-2">
                  <p className="text-xs text-dust">
                    this moves its gifts — thank-you note included — to the other shelf
                  </p>
                  <button
                    type="button"
                    disabled={reassignIsBusy}
                    onClick={() => handleReassignConfirm(link)}
                    aria-label={`confirm reassign for ${link.label}`}
                    className="rounded bg-control px-3 py-1.5 text-xs hover:bg-control-bright disabled:opacity-50"
                  >
                    {reassignIsBusy ? 'saving…' : 'confirm reassign'}
                  </button>
                  <button
                    type="button"
                    disabled={reassignIsBusy}
                    onClick={() => handleReassignCancel(link.token)}
                    aria-label={`cancel reassign for ${link.label}`}
                    className="rounded px-3 py-1.5 text-xs text-dust hover:text-ink disabled:opacity-50"
                  >
                    cancel
                  </button>
                </div>
              )}
              {reassignErr !== undefined && (
                <p role="alert" className="mt-1 text-xs text-red-700">
                  {reassignErr}
                </p>
              )}

              {/* Note editor — save persists; a blank save clears the note */}
              {sealDraft !== undefined && (
                <div className="mt-2 flex items-end gap-2">
                  <label className="flex flex-col gap-1 text-xs text-dust">
                    new unlock moment
                    <input
                      type="datetime-local"
                      aria-label="new unlock moment"
                      value={sealDraft}
                      disabled={sealBusy.has(link.token)}
                      onChange={(e) =>
                        setSealDrafts((prev) => ({
                          ...prev,
                          [link.token]: e.target.value,
                        }))
                      }
                      className="rounded border border-line bg-shelf px-2 py-1 text-sm text-ink disabled:opacity-50"
                    />
                  </label>
                  <button
                    type="button"
                    disabled={sealBusy.has(link.token) || sealDraft === ''}
                    onClick={() => handleSaveUnlock(link)}
                    aria-label={`save unlock for ${link.label}`}
                    className="rounded bg-control px-3 py-1.5 text-xs hover:bg-control-bright disabled:opacity-50"
                  >
                    {sealBusy.has(link.token) ? 'saving…' : 'save'}
                  </button>
                </div>
              )}
              {sealErr !== undefined && (
                <p role="alert" className="mt-1 text-xs text-red-700">
                  {sealErr}
                </p>
              )}

              {noteDraft !== undefined && (
                <div className="mt-2 flex flex-col gap-2">
                  <textarea
                    aria-label={`note for ${link.label}`}
                    value={noteDraft}
                    disabled={savingNote}
                    onChange={(e) =>
                      setNoteDrafts((prev) => ({
                        ...prev,
                        [link.token]: clampCodePoints(
                          e.target.value,
                          GIFT_NOTE_MAX,
                        ),
                      }))
                    }
                    rows={2}
                    placeholder="leave blank to remove the note"
                    className="rounded border border-line bg-shelf px-2 py-1 text-sm text-ink disabled:opacity-50"
                  />
                  {codePointCount(noteDraft) > NOTE_COUNTER_AT && (
                    <span className="text-right text-xs text-dust">
                      {codePointCount(noteDraft)}/{GIFT_NOTE_MAX}
                    </span>
                  )}
                  <div className="flex items-center gap-2">
                    <button
                      type="button"
                      disabled={savingNote}
                      onClick={() => handleSaveNote(link)}
                      aria-label={`save note for ${link.label}`}
                      className="rounded bg-control px-3 py-1.5 text-xs hover:bg-control-bright disabled:opacity-50"
                    >
                      {savingNote ? 'saving…' : 'save note'}
                    </button>
                    <button
                      type="button"
                      disabled={savingNote}
                      onClick={() => {
                        // Cancel abandons the edit — a lingering failure alert
                        // for a save nobody is retrying would just read as
                        // "something is still wrong".
                        setNoteErrors((prev) => omitKey(prev, link.token));
                        setNoteDrafts((prev) => omitKey(prev, link.token));
                      }}
                      aria-label={`cancel note for ${link.label}`}
                      className="rounded px-3 py-1.5 text-xs text-dust hover:text-ink"
                    >
                      cancel
                    </button>
                  </div>
                </div>
              )}

              {/* Note-save failure — loud, per row */}
              {noteErr !== undefined && (
                <p role="alert" className="mt-2 text-xs text-red-700">
                  {noteErr}
                </p>
              )}

              {/* Revoke failure — must be loud; the link may still be claimable */}
              {revokeErr !== undefined && (
                <p role="alert" className="mt-2 text-xs text-red-700">
                  {revokeErr}
                </p>
              )}

              {/* Audit panel — the gift URL is the friend's bearer secret and is
                  redacted SERVER-side (AdminClaimView sends only issued:bool);
                  it never even reaches this browser's network tab. */}
              {auditState !== undefined && (
                <div className="mt-3 border-t border-line pt-3">
                  {auditState.phase === 'loading' && (
                    <p className="text-xs text-dust-faint">loading claims…</p>
                  )}
                  {auditState.phase === 'error' && (
                    <p className="text-xs text-red-700">couldn't load claims</p>
                  )}
                  {auditState.phase === 'loaded' && auditState.claims.length === 0 && (
                    <p className="text-xs text-dust-faint">no claims yet</p>
                  )}
                  {auditState.phase === 'loaded' && auditState.claims.length > 0 && (
                    <div className="space-y-1">
                      {auditState.claims.map((claim, i) => (
                        <div key={i} className="flex items-center gap-3 text-xs">
                          <span className="text-dust">{claim.game_id}</span>
                          <span
                            className={`rounded px-2 py-0.5 ${stateBadgeClass(claim.state)}`}
                          >
                            {claim.state}
                          </span>
                          {claim.issued && <span className="text-green-700">issued ✓</span>}
                        </div>
                      ))}
                    </div>
                  )}
                </div>
              )}
            </div>
          );
        })}
      </div>
    </div>
  );
}
