// Single source of truth for the friend-facing invite URL convention.
// The App.tsx route pattern and every rendered invite URL derive from here —
// change the route shape in this file and nothing else drifts.
export const LINK_ROUTE_PATTERN = '/l/:token';

export function inviteUrlPath(token: string): string {
  return `/l/${token}`;
}

export function inviteUrl(token: string): string {
  return `${window.location.origin}${inviteUrlPath(token)}`;
}

// Single source of truth for the friend-facing shelf URL convention (the
// gift shelf, /s/{shelf_token}) — same shape as the invite-link convention
// above, one file so the route pattern and every rendered shelf URL can
// never drift apart.
export const SHELF_ROUTE_PATTERN = '/s/:token';

export function shelfUrlPath(token: string): string {
  return `/s/${token}`;
}

export function shelfUrl(token: string): string {
  return `${window.location.origin}${shelfUrlPath(token)}`;
}
