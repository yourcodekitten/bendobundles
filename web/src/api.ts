// Types
export type GameView = {
  id: string;
  title: string;
  bundle: string;
  key_type: string;
  artwork_url: string | null;
  steam_app_id: number | null;
  /** First ~5 steam genres from the server's enrichment cache; absent when unknown. */
  genres?: string[];
  /** Top community tags (popularity order, ≤10); absent when unknown/empty — fall back to genres (#71). */
  tags?: string[];
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
  /** Ben's personal note to the friend; absent when he didn't leave one. */
  gift_note?: string;
  /** The friend's own thank-you, echoed back; absent when never sent. The
   * say-thanks card gates on presence — absent = compose, present = sent. */
  thank_note?: string;
  claims_allowed: number;
  claims_used: number;
  state: LinkState;
  games: GameView[];
  claims: ClaimView[];
};

export type ThanksResult =
  | { kind: 'sent'; thank_note: string }
  | { kind: 'refused'; message: string }
  | { kind: 'error'; message: string };

export type ClaimResult =
  | { kind: 'gifted'; gift_url: string }
  | { kind: 'processing'; message: string }
  | { kind: 'refused'; message: string }
  | { kind: 'error'; message: string };

/** Compact steam projection on catalog rows — the toolkit's filter/sort/group
 * data. Mirrors admin-api's SteamSummaryView exactly. */
export type SteamSummary = {
  genres: string[];
  /** Top community tags (popularity order, ≤10) — the toolkit's chips + tag filter (#71).
   * Optional like SteamAppDetail's mirror fields: an OLD lambda racing this bundle during
   * deploy omits the key. Read via displayTags(). */
  tags?: string[];
  /** Raw content descriptor ids — badge/mature-filter policy lives in tags.ts (#71).
   * Optional for the same deploy-window reason; isMature() tolerates undefined. */
  content_descriptor_ids?: number[];
  developers: string[];
  publishers: string[];
  release_date: string | null;
  /** "YYYY-MM-DD", parsed server-side; null when Steam's string isn't a date. */
  release_date_iso: string | null;
  review_desc: string | null;
  review_percent: number | null;
  review_count: number | null;
  recent_percent: number | null;
};

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
  requires_choice: boolean;
  steam_app_id: number | null;
  owned_by_ben: boolean;
  /** Who last set hidden — 'sync' rows are auto-hides (#71). Optional for the
   * deploy window (old lambda omits it); absent reads as unknown = no label. */
  hidden_source?: 'admin' | 'sync' | null;
  steam: SteamSummary | null;
};

export type SelfClaimResult =
  | { kind: 'revealed'; key: string; keyType: string }
  | { kind: 'processing' }
  | { kind: 'refused'; message: string }
  | { kind: 'error' };

export type SelfClaimView = {
  game_id: string;
  state: 'pending' | 'fulfilled' | 'compensated';
  revealed_key: string | null;
  created_at: string;
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
  /** Ben's note to the friend; absent when unset (list serializes domain::Link). */
  gift_note?: string;
  /** The friend's thank-you back; absent when never sent. Read-only — ben
   * receives their words, he doesn't edit them. */
  thank_note?: string;
  /** RFC3339; present iff thank_note is. */
  thanked_at?: string;
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

/** Send the friend's one thank-you note. 409/422 surface the server's message
 * (already-sent, dead link, validation); everything else degrades to the same
 * soft retry line the claim flow uses. */
export async function sendThanks(token: string, note: string): Promise<ThanksResult> {
  try {
    const response = await fetch(`/api/l/${token}/thanks`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ note }),
    });

    if (response.status === 200) {
      const data = (await response.json()) as { thank_note: string };
      return { kind: 'sent', thank_note: data.thank_note };
    }

    if (response.status === 409 || response.status === 422) {
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

/// The server rejected a link-field INPUT (422) — thrown by create-link and
/// note-edit alike; the message names the violated bound, safe to show
/// verbatim in the form. (On the note path the link itself exists — only the
/// field was refused.)
export class CreateLinkValidationError extends Error {}

/// Client mirror of the server's GIFT_NOTE_MAX_CHARS (admin-api). The server
/// stays the authority — this only sizes textareas/counters so the UI and the
/// 422 bound can't drift apart one literal at a time.
export const GIFT_NOTE_MAX = 500;

// Shared 422 contract: the body is {"error": msg} naming the violated bound.
// Parse-and-throw lives here once so every 422-capable endpoint surfaces the
// server's message identically (a fix to the parsing lands everywhere).
async function throwIfValidation422(response: Response, fallback: string): Promise<void> {
  if (response.status !== 422) return;
  let message = fallback;
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

export async function adminCreateLink(
  label: string,
  claims: number,
  expiresDays?: number,
  giftNote?: string,
): Promise<{ token: string; url_path: string }> {
  const response = await fetch('/admin/api/links', {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({
      label,
      claims_allowed: claims,
      expires_days: expiresDays,
      gift_note: giftNote,
    }),
  });

  await checkUnauthorized(response);

  // Creating a link mints the artifact ben hands a friend — a 5xx from API
  // Gateway still carries a JSON body, which would parse "fine", leave token
  // undefined, and render an /l/undefined invite for a link that was never
  // created. Non-ok must throw, never fake success. A 422 carries
  // {"error": msg} naming the violated bound — surface it so the form says WHY.
  if (!response.ok) {
    await throwIfValidation422(response, 'invalid link parameters');
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

/** Set, replace, or clear (blank note) a link's gift note after creation. */
export async function adminSetLinkNote(token: string, note: string): Promise<void> {
  const response = await fetch(`/admin/api/links/${token}/note`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ gift_note: note }),
  });
  await checkUnauthorized(response);

  if (!response.ok) {
    await throwIfValidation422(response, 'invalid note');
    throw new Error("couldn't save the note — it may not have changed");
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

export async function adminSelfClaim(gameId: string): Promise<SelfClaimResult> {
  let response: Response;
  try {
    response = await fetch(`/admin/api/games/${encodeURIComponent(gameId)}/self-claim`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: '{}',
    });
  } catch {
    return { kind: 'error' };
  }
  if (response.status === 401) throw new Unauthorized();
  try {
    if (response.status === 200) {
      const b = await response.json();
      return { kind: 'revealed', key: b.revealed_key, keyType: b.key_type };
    }
    if (response.status === 202) return { kind: 'processing' };
    if (response.status === 409 || response.status === 410) {
      const b = await response.json();
      return { kind: 'refused', message: b.error ?? 'refused' };
    }
  } catch {
    return { kind: 'error' };
  }
  return { kind: 'error' };
}

export async function adminSelfClaims(): Promise<SelfClaimView[]> {
  const response = await fetch('/admin/api/claims/self');
  if (response.status === 401) throw new Unauthorized();
  if (!response.ok) throw new FetchFailed();
  return response.json();
}

// ── Steam API ────────────────────────────────────────────────────────────────

/**
 * Friend-surface: fetch the owned appids for a steam user via a link token.
 * Returns 'private' when the user's game-details privacy is locked.
 * Throws FetchFailed on 404 (dead link) / 409 (inactive link) / 5xx.
 */
export async function steamOwnedForLink(
  token: string,
  steamid: string,
): Promise<number[] | 'private'> {
  let response: Response;
  try {
    response = await fetch(`/api/l/${token}/steam/owned/${encodeURIComponent(steamid)}`);
  } catch {
    throw new FetchFailed();
  }
  if (response.status === 404 || response.status === 409) throw new FetchFailed();
  if (!response.ok) throw new FetchFailed();
  const data = (await response.json()) as { appids?: number[]; private?: true };
  if (data.private) return 'private';
  return data.appids ?? [];
}

/**
 * Admin-surface: fetch owned appids for the admin steam identity.
 * Returns 'private' when the library is locked down.
 */
export async function adminSteamOwned(steamid: string): Promise<number[] | 'private'> {
  const response = await fetch(`/admin/api/steam/owned/${encodeURIComponent(steamid)}`);
  await checkUnauthorized(response);
  if (!response.ok) throw new FetchFailed();
  const data = (await response.json()) as { appids?: number[]; private?: true };
  if (data.private) return 'private';
  return data.appids ?? [];
}

/** Returns the admin's configured Steam steamid, or null if not set. */
export async function adminSteamIdentity(): Promise<string | null> {
  const response = await fetch('/admin/api/steam/identity');
  await checkUnauthorized(response);
  if (!response.ok) throw new FetchFailed();
  const data = (await response.json()) as { steamid: string | null };
  return data.steamid;
}

/** Persists the admin's Steam identity on the server. */
export async function adminSetSteamIdentity(steamid: string): Promise<void> {
  const response = await fetch('/admin/api/steam/identity', {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ steamid }),
  });
  await checkUnauthorized(response);
  if (!response.ok) throw new FetchFailed();
}

/** Removes the admin's Steam identity from the server. */
export async function adminClearSteamIdentity(): Promise<void> {
  const response = await fetch('/admin/api/steam/identity', { method: 'DELETE' });
  await checkUnauthorized(response);
  if (!response.ok) throw new FetchFailed();
}

// ── Steam detail types (mirrors Rust steam-client structs, snake_case serde) ──

/** One store screenshot — mirrors Rust steam_client::Screenshot. */
export type Screenshot = {
  thumbnail: string;
  full: string;
};

export type SteamAppDetail = {
  app_id: number;
  name: string;
  developers: string[];
  publishers: string[];
  genres: string[];
  release_date: string | null;
  short_description: string;
  header_image: string | null;
  video_hls_url: string | null;
  video_thumbnail: string | null;
  /**
   * Optional, not `Screenshot[] | null`: an OLD lambda racing this bundle during deploy
   * omits the key entirely. Read as `detail.screenshots ?? []`.
   */
  screenshots?: Screenshot[];
  /** Optional like screenshots: an OLD lambda racing this bundle during deploy omits
   * these keys. Read as `detail.tags ?? []` etc. (#71). */
  tags?: string[];
  content_descriptor_ids?: number[];
  content_notes?: string | null;
};

export type ReviewSummary = {
  desc: string;
  total_positive: number;
  total_negative: number;
  total_reviews: number;
};

export type RecentReviews = {
  percent_positive: number;
  count: number;
};

/** One steam blob from the detail endpoint — all three halves can be null independently. */
export type SteamDetailBlob = {
  detail: SteamAppDetail | null;
  overall: ReviewSummary | null;
  recent: RecentReviews | null;
};

/** Friend-surface game detail response. `steam: null` = unmapped / no cache item. */
export type GameDetailResponse = {
  game: GameView;
  steam: SteamDetailBlob | null;
};

/** Admin-surface game detail response — same steam blob, full AdminGame view. */
export type AdminGameDetailResponse = {
  game: AdminGame;
  steam: SteamDetailBlob | null;
};

/**
 * Friend-surface: fetch full Steam detail for a game via a link token.
 * Throws NotFound on 404 (dead link or unknown game), FetchFailed on other errors.
 */
export async function fetchGameDetail(
  token: string,
  gameId: string,
): Promise<GameDetailResponse> {
  let response: Response;
  try {
    response = await fetch(`/api/l/${token}/games/${encodeURIComponent(gameId)}/detail`);
  } catch {
    throw new FetchFailed();
  }
  if (response.status === 404) throw new NotFound();
  if (!response.ok) throw new FetchFailed();
  try {
    return (await response.json()) as GameDetailResponse;
  } catch {
    throw new FetchFailed();
  }
}

/**
 * Admin-surface: fetch full Steam detail for a catalog game.
 * Throws Unauthorized when session missing, FetchFailed on other errors.
 */
export async function adminGameDetail(gameId: string): Promise<AdminGameDetailResponse> {
  const response = await fetch(`/admin/api/games/${encodeURIComponent(gameId)}/detail`);
  await checkUnauthorized(response);
  if (!response.ok) throw new FetchFailed();
  return (await response.json()) as AdminGameDetailResponse;
}

/**
 * Associates (or clears) a Steam app ID with a catalog game.
 * Passing null removes the association.
 */
export async function adminSetAppId(gameId: string, appId: number | null): Promise<void> {
  const response = await fetch(`/admin/api/games/${encodeURIComponent(gameId)}/steam-app-id`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ app_id: appId }),
  });
  await checkUnauthorized(response);
  if (!response.ok) throw new FetchFailed();
}
