// Shared steam identity module — one localStorage key, both admin + friend surfaces.
// Spec §3: same origin, same key.

const KEY = 'steam_identity';

export type SteamIdentity = {
  steamid: string;
  persona: string;
  owned: number[];
  fetched_at: number;
};

export function loadIdentity(): SteamIdentity | null {
  try {
    const raw = localStorage.getItem(KEY);
    if (raw === null) return null;
    return JSON.parse(raw) as SteamIdentity;
  } catch {
    return null;
  }
}

export function saveIdentity(i: SteamIdentity): void {
  localStorage.setItem(KEY, JSON.stringify(i));
}

/** "not you? disconnect" */
export function clearIdentity(): void {
  localStorage.removeItem(KEY);
}

/**
 * Parses #steam=<id64>&persona=<enc> or #steam_error=<reason> off location.hash,
 * clears the hash, and returns the parsed result. Returns null if hash is irrelevant.
 */
export function consumeReturnFragment():
  | { steamid: string; persona: string }
  | { error: string }
  | null {
  const hash = location.hash;
  if (!hash || hash === '#') return null;

  const params = new URLSearchParams(hash.slice(1));

  if (params.has('steam_error')) {
    history.replaceState(null, '', location.pathname + location.search);
    return { error: params.get('steam_error')! };
  }

  if (params.has('steam')) {
    const steamid = params.get('steam')!;
    // URLSearchParams.get() already decodes percent-encoded values
    const persona = params.get('persona') ?? '';
    history.replaceState(null, '', location.pathname + location.search);
    return { steamid, persona };
  }

  return null;
}

export function beginConnect(ctx: string): void {
  location.href = `/api/steam/login?ctx=${encodeURIComponent(ctx)}`;
}
