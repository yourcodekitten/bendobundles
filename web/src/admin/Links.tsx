import { useState, useEffect, useCallback } from 'react';
import { useNavigate } from 'react-router-dom';
import {
  adminLinks,
  adminCreateLink,
  adminRevoke,
  adminLinkClaims,
  CreateLinkValidationError,
  type AdminLink,
  type AdminClaimView,
} from '../api';
import { withAuth } from './withAuth';
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

function stateBadgeClass(state: string): string {
  switch (state) {
    case 'fulfilled':
      return 'bg-green-700 text-green-100';
    case 'pending':
      return 'bg-amber-700 text-amber-100';
    case 'compensated':
      return 'bg-give text-give-ink';
    default:
      return 'bg-control text-ink';
  }
}

function formatDate(iso: string): string {
  return new Date(iso).toLocaleDateString();
}

export function Links() {
  const navigate = useNavigate();
  const [state, setState] = useState<PageState>({ phase: 'loading' });

  // Create form state
  const [formLabel, setFormLabel] = useState('');
  const [claimsAllowed, setClaimsAllowed] = useState(1);
  const [expiresDays, setExpiresDays] = useState('');
  const [creating, setCreating] = useState(false);
  // Stored after successful create — separate from page state so reload doesn't clear it
  const [createdInfo, setCreatedInfo] = useState<{ fullUrl: string; label: string } | null>(null);
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
    withAuth(() => adminCreateLink(trimmedLabel, claimsAllowed, expires), navigate)
      .then((result) => {
        setCreatedInfo({ fullUrl: inviteUrl(result.token), label: trimmedLabel });
        setCreateError(null);
        setFormLabel('');
        setClaimsAllowed(1);
        setExpiresDays('');
        // Reload to prepend the new link into the list
        load();
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
        setRevokeErrors((prev) => {
          const next = { ...prev };
          delete next[link.token];
          return next;
        });
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
      setAuditMap((prev) => {
        const next = { ...prev };
        delete next[token];
        return next;
      });
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
              value={claimsAllowed}
              onChange={(e) => {
                const n = parseInt(e.target.value, 10);
                setClaimsAllowed(isNaN(n) ? 1 : Math.max(1, n));
              }}
              className="w-24 rounded border border-line bg-shelf px-2 py-1 text-sm text-ink"
            />
          </label>
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
        </div>
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
        </div>
      )}

      {/* ── Links list ─────────────────────────────────────────────────── */}
      <div className="space-y-2">
        {state.links.map((link) => {
          const linkUrl = inviteUrl(link.token);
          const auditState = auditMap[link.token];
          const armed = revokeArmed.has(link.token);
          const revokeErr = revokeErrors[link.token];

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
