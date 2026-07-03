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
