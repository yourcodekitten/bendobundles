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

export type LinkState = 'active' | 'revoked' | 'expired' | 'exhausted';

export type LinkView = {
  label: string;
  claims_allowed: number;
  claims_used: number;
  state: LinkState;
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
};

// Redacted admin view of a claim — the friend's one-time gift URL is a bearer
// secret and NEVER crosses to the admin surface; only the fact it was issued.
export type AdminClaimView = {
  game_id: string;
  state: 'pending' | 'fulfilled' | 'compensated';
  issued: boolean;
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
  // Present while a sync-run marker exists; completion deletes the marker, so null = idle.
  // `running` is computed server-side (the browser clock can't judge staleness against
  // server-written epochs): true = a run is live; false = a run began but never reported
  // (crash/timeout) — likely failed, safe to retry.
  sync_run: { started_epoch: number; running: boolean } | null;
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

// 401 → Unauthorized (login redirect); any other non-2xx → throw so the page
// shows its error/retry state. Without this, a 403/502/503 JSON body (e.g.
// from API Gateway) flows into component state and TypeErrors the render.
async function checkOk(response: Response, what: string): Promise<void> {
  await checkUnauthorized(response);
  if (!response.ok) {
    throw new Error(`failed to load ${what}`);
  }
}

export async function adminCatalog(): Promise<AdminGame[]> {
  const response = await fetch('/admin/api/catalog');
  await checkOk(response, 'catalog');
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

/// The server rejected the create-link INPUT (422) — no link exists and the
/// message names the violated bound, safe to show verbatim in the form.
export class CreateLinkValidationError extends Error {}

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

  // Creating a link mints the artifact ben hands a friend — a 5xx from API
  // Gateway still carries a JSON body, which would parse "fine", leave token
  // undefined, and render an /l/undefined invite for a link that was never
  // created. Non-ok must throw, never fake success. A 422 carries
  // {"error": msg} naming the violated bound — surface it so the form says WHY.
  if (!response.ok) {
    if (response.status === 422) {
      let message = 'invalid link parameters';
      try {
        const errBody = (await response.json()) as { error?: unknown };
        if (typeof errBody.error === 'string') {
          message = errBody.error;
        }
      } catch {
        // non-JSON body — keep the generic message
      }
      throw new CreateLinkValidationError(message);
    }
    throw new Error('failed to create link');
  }

  // A 200 whose body isn't the link contract (proxy error page, API drift)
  // is the same /l/undefined trap wearing a success status — validate the
  // shape, not just the status code.
  const data = await response.json();
  if (typeof data?.token !== 'string' || typeof data?.url_path !== 'string') {
    throw new Error('create link returned an unexpected response shape');
  }
  return data;
}

export async function adminLinks(): Promise<AdminLink[]> {
  const response = await fetch('/admin/api/links');
  await checkOk(response, 'links');
  return await response.json();
}

export async function adminRevoke(token: string): Promise<void> {
  const response = await fetch(`/admin/api/links/${token}/revoke`, {
    method: 'POST',
  });
  await checkUnauthorized(response);

  // Revoking a leaked invite is a security action — a 404/500 must surface,
  // never resolve as if the link were dead.
  if (!response.ok) {
    throw new Error('revoke failed — the link may still be live');
  }
}

export async function adminLinkClaims(token: string): Promise<AdminClaimView[]> {
  const response = await fetch(`/admin/api/links/${token}/claims`);
  await checkOk(response, 'claims');
  return (await response.json()) as AdminClaimView[];
}

// Sync-now is fire-and-forget: the server returns 202 the moment the backfill
// is queued (a full backfill runs for minutes, past any HTTP timeout). There
// are no counts to return — the status card reflects progress once the
// background run writes its SyncState.
export async function adminSync(): Promise<void> {
  const response = await fetch('/admin/api/sync', {
    method: 'POST',
  });

  await checkUnauthorized(response);

  // 409 = a live run already holds the sync-run marker; distinct copy so the admin knows
  // waiting (not retrying) is the move.
  if (response.status === 409) {
    throw new Error('a sync is already running — watch the status card');
  }
  if (!response.ok) {
    throw new Error('couldn’t start sync — try again');
  }
}

export async function adminStatus(): Promise<StatusView> {
  const response = await fetch('/admin/api/status');
  await checkOk(response, 'status');
  return await response.json();
}
