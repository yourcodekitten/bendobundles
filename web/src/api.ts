// Types
export type GameView = {
  id: string;
  title: string;
  bundle: string;
  key_type: string;
  artwork_url: string | null;
};

export type ClaimView = {
  game_id: string;
  title?: string;
  state: 'pending' | 'fulfilled' | 'compensated';
  gift_url: string | null;
};

export type LinkView = {
  label: string;
  claims_allowed: number;
  claims_used: number;
  active: boolean;
  games: GameView[];
  claims: ClaimView[];
};

export type ClaimResult =
  | { kind: 'gifted'; gift_url: string }
  | { kind: 'processing'; message: string }
  | { kind: 'refused'; message: string }
  | { kind: 'error'; message: string };

export type AdminGame = {
  id: string;
  title: string;
  bundle: string;
  key_type: string;
  giftable: boolean;
  hidden: boolean;
  status: string;
  claim_id: string | null;
  artwork_url: string | null;
  keyindex: number;
};

export type AdminLink = {
  token: string;
  label: string;
  claims_allowed: number;
  claims_used: number;
  revoked: boolean;
  expires_at: string | null;
  created_at: string;
};

export type CookieResult = {
  ok: boolean;
  restored_previous?: boolean;
  inconclusive?: boolean;
};

export type StatusView = {
  sync:
    | {
        last_run_epoch: number;
        ok: boolean;
        cookie_ok: boolean;
        games_written: number;
        message: string;
      }
    | null;
  game_counts: Record<string, number>;
};

// Error classes
export class Unauthorized extends Error {
  constructor() {
    super('unauthorized');
    this.name = 'Unauthorized';
  }
}

export class NotFound extends Error {
  constructor() {
    super('not found');
    this.name = 'NotFound';
  }
}

// Transient failure (5xx, network drop, malformed body) — retryable, NOT a dead link.
export class FetchFailed extends Error {
  constructor() {
    super('fetch failed');
    this.name = 'FetchFailed';
  }
}

// Friend API
export async function fetchLink(token: string): Promise<LinkView> {
  let response: Response;
  try {
    response = await fetch(`/api/l/${token}`);
  } catch {
    throw new FetchFailed();
  }

  if (response.status === 404) {
    throw new NotFound();
  }

  if (response.status !== 200) {
    throw new FetchFailed();
  }

  try {
    return (await response.json()) as LinkView;
  } catch {
    throw new FetchFailed();
  }
}

export async function claimGame(token: string, gameId: string): Promise<ClaimResult> {
  try {
    const response = await fetch(`/api/l/${token}/claim`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ game_id: gameId }),
    });

    if (response.status === 200) {
      const data = (await response.json()) as { gift_url: string };
      return { kind: 'gifted', gift_url: data.gift_url };
    }

    if (response.status === 202) {
      const data = (await response.json()) as { status: string; message: string };
      return { kind: 'processing', message: data.message };
    }

    if (response.status === 409 || response.status === 410) {
      const data = (await response.json()) as { error: string };
      return { kind: 'refused', message: data.error };
    }

    return { kind: 'error', message: 'something hiccuped — try again' };
  } catch {
    return { kind: 'error', message: 'something hiccuped — try again' };
  }
}

// Admin API
export async function adminLogin(password: string): Promise<boolean> {
  try {
    const response = await fetch('/admin/api/login', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ password }),
      credentials: 'same-origin',
    });

    if (response.status === 401) {
      return false;
    }

    return response.status === 200;
  } catch {
    return false;
  }
}

async function checkUnauthorized(response: Response): Promise<void> {
  if (response.status === 401) {
    throw new Unauthorized();
  }
}

export async function adminCatalog(): Promise<AdminGame[]> {
  const response = await fetch('/admin/api/catalog');
  await checkUnauthorized(response);
  return await response.json();
}

export async function adminSetHidden(
  id: string,
  hidden: boolean,
): Promise<{ ok: true } | { ok: false; message: string }> {
  const response = await fetch(`/admin/api/games/${id}/hidden`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ hidden }),
  });

  await checkUnauthorized(response);

  if (response.ok) {
    return { ok: true };
  }

  if (response.status === 409) {
    const data = (await response.json()) as { error: string };
    return { ok: false, message: data.error };
  }

  if (response.status === 404) {
    return { ok: false, message: 'game not found' };
  }

  return { ok: false, message: 'unknown error' };
}

export async function adminCreateLink(
  label: string,
  claims: number,
  expiresDays?: number,
): Promise<{ token: string; url_path: string }> {
  const response = await fetch('/admin/api/links', {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({
      label,
      claims_allowed: claims,
      expires_days: expiresDays,
    }),
  });

  await checkUnauthorized(response);
  return await response.json();
}

export async function adminLinks(): Promise<AdminLink[]> {
  const response = await fetch('/admin/api/links');
  await checkUnauthorized(response);
  return await response.json();
}

export async function adminRevoke(token: string): Promise<void> {
  const response = await fetch(`/admin/api/links/${token}/revoke`, {
    method: 'POST',
  });
  await checkUnauthorized(response);
}

export async function adminLinkClaims(token: string): Promise<ClaimView[]> {
  const response = await fetch(`/admin/api/links/${token}/claims`);
  await checkUnauthorized(response);

  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  const records = (await response.json()) as any[];

  return records.map((record) => ({
    game_id: record.game_id,
    state: record.state,
    gift_url: record.gift_url,
  }));
}

export async function adminPasteCookie(cookie: string): Promise<CookieResult> {
  const response = await fetch('/admin/api/cookie', {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ cookie }),
  });

  await checkUnauthorized(response);
  return await response.json();
}

export async function adminSync(): Promise<{ games_written: number; orders_failed: number }> {
  const response = await fetch('/admin/api/sync', {
    method: 'POST',
  });

  await checkUnauthorized(response);

  if (!response.ok) {
    throw new Error('sync failed — check status panel');
  }

  let data;
  try {
    data = await response.json();
  } catch {
    throw new Error('sync failed — check status panel');
  }

  const typed = data as {
    games_written: number;
    orders_failed: number;
  };

  return {
    games_written: typed.games_written,
    orders_failed: typed.orders_failed,
  };
}

export async function adminStatus(): Promise<StatusView> {
  const response = await fetch('/admin/api/status');
  await checkUnauthorized(response);
  return await response.json();
}
