import { describe, it, expect, beforeEach, vi } from 'vitest';
import {
  fetchLink,
  claimGame,
  adminLogin,
  adminCatalog,
  adminLinkClaims,
  adminPasteCookie,
  adminStatus,
  adminSetHidden,
  adminCreateLink,
  adminLinks,
  adminRevoke,
  adminSync,
  NotFound,
  FetchFailed,
  Unauthorized,
  type ClaimResult,
  type CookieResult,
  type StatusView,
  type AdminGame,
  type AdminLink,
} from './api';

// eslint-disable-next-line @typescript-eslint/no-explicit-any
const mockFetch = vi.fn() as any;

beforeEach(() => {
  mockFetch.mockClear();
  vi.stubGlobal('fetch', mockFetch);
});

describe('fetchLink', () => {
  it('returns LinkView on 200', async () => {
    const mockResponse = {
      ok: true,
      status: 200,
      json: vi.fn().mockResolvedValue({
        label: 'Test Link',
        claims_allowed: 5,
        claims_used: 2,
        state: 'active',
        games: [
          {
            id: 'game1',
            title: 'Game 1',
            bundle: 'bundle1',
            key_type: 'key',
            artwork_url: 'https://example.com/art.png',
          },
        ],
        claims: [
          {
            game_id: 'game1',
            title: 'Game 1',
            state: 'fulfilled',
            gift_url: 'https://humble.example.com/gift1',
          },
        ],
      }),
    };
    mockFetch.mockResolvedValueOnce(mockResponse);

    const result = await fetchLink('token123');

    expect(result.label).toBe('Test Link');
    expect(result.claims_allowed).toBe(5);
    expect(result.claims_used).toBe(2);
    expect(result.state).toBe('active');
    expect(result.games).toHaveLength(1);
    expect(result.claims).toHaveLength(1);
    expect(mockFetch).toHaveBeenCalledWith('/api/l/token123');
  });

  it('throws NotFound on 404', async () => {
    const mockResponse = {
      status: 404,
      json: vi.fn().mockResolvedValue({ error: 'not found' }),
    };
    mockFetch.mockResolvedValueOnce(mockResponse);

    await expect(fetchLink('badtoken')).rejects.toBeInstanceOf(NotFound);
  });

  it('throws FetchFailed (not NotFound) on 500 — transient, retryable', async () => {
    const mockResponse = {
      status: 500,
      json: vi.fn().mockResolvedValue({ error: 'server error' }),
    };
    mockFetch.mockResolvedValueOnce(mockResponse);

    await expect(fetchLink('token')).rejects.toBeInstanceOf(FetchFailed);
  });

  it('throws FetchFailed on network error', async () => {
    mockFetch.mockRejectedValueOnce(new TypeError('failed to fetch'));

    await expect(fetchLink('token')).rejects.toBeInstanceOf(FetchFailed);
  });

  it('throws FetchFailed on malformed 200 body', async () => {
    const mockResponse = {
      ok: true,
      status: 200,
      json: vi.fn().mockRejectedValue(new SyntaxError('bad json')),
    };
    mockFetch.mockResolvedValueOnce(mockResponse);

    await expect(fetchLink('token')).rejects.toBeInstanceOf(FetchFailed);
  });
});

describe('claimGame', () => {
  it('returns {kind:"gifted", gift_url} on 200', async () => {
    const mockResponse = {
      ok: true,
      status: 200,
      json: vi.fn().mockResolvedValue({ gift_url: 'https://humble.example.com/gift' }),
    };
    mockFetch.mockResolvedValueOnce(mockResponse);

    const result = (await claimGame('token', 'game1')) as Extract<ClaimResult, { kind: 'gifted' }>;

    expect(result.kind).toBe('gifted');
    expect(result.gift_url).toBe('https://humble.example.com/gift');
    expect(mockFetch).toHaveBeenCalledWith('/api/l/token/claim', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ game_id: 'game1' }),
    });
  });

  it('returns {kind:"processing", message} on 202', async () => {
    const mockResponse = {
      status: 202,
      json: vi.fn().mockResolvedValue({ status: 'processing', message: 'your claim is being processed' }),
    };
    mockFetch.mockResolvedValueOnce(mockResponse);

    const result = (await claimGame('token', 'game1')) as Extract<ClaimResult, { kind: 'processing' }>;

    expect(result.kind).toBe('processing');
    expect(result.message).toBe('your claim is being processed');
  });

  it('returns {kind:"refused", message} on 409', async () => {
    const mockResponse = {
      status: 409,
      json: vi.fn().mockResolvedValue({ error: 'you have reached your claim limit' }),
    };
    mockFetch.mockResolvedValueOnce(mockResponse);

    const result = (await claimGame('token', 'game1')) as Extract<ClaimResult, { kind: 'refused' }>;

    expect(result.kind).toBe('refused');
    expect(result.message).toBe('you have reached your claim limit');
  });

  it('returns {kind:"refused", message} on 410', async () => {
    const mockResponse = {
      status: 410,
      json: vi.fn().mockResolvedValue({ error: 'this link has expired' }),
    };
    mockFetch.mockResolvedValueOnce(mockResponse);

    const result = (await claimGame('token', 'game1')) as Extract<ClaimResult, { kind: 'refused' }>;

    expect(result.kind).toBe('refused');
    expect(result.message).toBe('this link has expired');
  });

  it('returns {kind:"error", message} on fetch throw', async () => {
    mockFetch.mockRejectedValueOnce(new Error('network error'));

    const result = (await claimGame('token', 'game1')) as Extract<ClaimResult, { kind: 'error' }>;

    expect(result.kind).toBe('error');
    expect(result.message).toBe('something hiccuped — try again');
  });

  it('returns {kind:"error", message} on 500', async () => {
    const mockResponse = {
      status: 500,
      json: vi.fn().mockResolvedValue({ error: 'server error' }),
    };
    mockFetch.mockResolvedValueOnce(mockResponse);

    const result = (await claimGame('token', 'game1')) as Extract<ClaimResult, { kind: 'error' }>;

    expect(result.kind).toBe('error');
    expect(result.message).toBe('something hiccuped — try again');
  });

  it('returns {kind:"error", message} on JSON parse error', async () => {
    const mockResponse = {
      ok: true,
      status: 200,
      json: vi.fn().mockRejectedValue(new Error('invalid json')),
    };
    mockFetch.mockResolvedValueOnce(mockResponse);

    const result = (await claimGame('token', 'game1')) as Extract<ClaimResult, { kind: 'error' }>;

    expect(result.kind).toBe('error');
    expect(result.message).toBe('something hiccuped — try again');
  });
});

describe('adminLogin', () => {
  it('returns true on 200', async () => {
    const mockResponse = {
      ok: true,
      status: 200,
      json: vi.fn().mockResolvedValue({}),
    };
    mockFetch.mockResolvedValueOnce(mockResponse);

    const result = await adminLogin('secretpassword');

    expect(result).toBe(true);
    expect(mockFetch).toHaveBeenCalledWith('/admin/api/login', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ password: 'secretpassword' }),
      credentials: 'same-origin',
    });
  });

  it('returns false on 401', async () => {
    const mockResponse = {
      status: 401,
      json: vi.fn().mockResolvedValue({}),
    };
    mockFetch.mockResolvedValueOnce(mockResponse);

    const result = await adminLogin('wrongpassword');

    expect(result).toBe(false);
  });

  it('does not throw on 401', async () => {
    const mockResponse = {
      status: 401,
      json: vi.fn().mockResolvedValue({}),
    };
    mockFetch.mockResolvedValueOnce(mockResponse);

    await expect(adminLogin('wrongpassword')).resolves.toBe(false);
  });
});

describe('adminCatalog', () => {
  it('throws Unauthorized on 401', async () => {
    const mockResponse = {
      status: 401,
      json: vi.fn().mockResolvedValue({ error: 'unauthorized' }),
    };
    mockFetch.mockResolvedValueOnce(mockResponse);

    await expect(adminCatalog()).rejects.toBeInstanceOf(Unauthorized);
    expect(mockFetch).toHaveBeenCalledWith('/admin/api/catalog');
  });

  it('returns AdminGame[] on 200', async () => {
    const mockGames = [
      {
        id: 'game1',
        title: 'Game 1',
        bundle: 'bundle1',
        key_type: 'key',
        giftable: true,
        hidden: false,
        status: 'available',
        claim_id: null,
        artwork_url: 'https://example.com/art.png',
      },
    ] as AdminGame[];

    const mockResponse = {
      ok: true,
      status: 200,
      json: vi.fn().mockResolvedValue(mockGames),
    };
    mockFetch.mockResolvedValueOnce(mockResponse);

    const result = await adminCatalog();

    expect(result).toEqual(mockGames);
  });
});

describe('adminLinkClaims', () => {
  it('throws Unauthorized on 401', async () => {
    const mockResponse = {
      status: 401,
      json: vi.fn().mockResolvedValue({ error: 'unauthorized' }),
    };
    mockFetch.mockResolvedValueOnce(mockResponse);

    await expect(adminLinkClaims('token')).rejects.toBeInstanceOf(Unauthorized);
  });

  it('returns the redacted AdminClaimView shape (issued flag, NO gift_url)', async () => {
    const mockRecords = [
      { game_id: 'game1', state: 'fulfilled', issued: true },
      { game_id: 'game2', state: 'pending', issued: false },
    ];

    const mockResponse = {
      ok: true,
      status: 200,
      json: vi.fn().mockResolvedValue(mockRecords),
    };
    mockFetch.mockResolvedValueOnce(mockResponse);

    const result = await adminLinkClaims('token');

    expect(result).toHaveLength(2);
    expect(result[0]).toEqual({ game_id: 'game1', state: 'fulfilled', issued: true });
    expect(result[1]).toEqual({ game_id: 'game2', state: 'pending', issued: false });
    expect(result[0]).not.toHaveProperty('gift_url');
  });

  it('throws on non-401 error instead of passing the body into state', async () => {
    const mockResponse = {
      ok: false,
      status: 502,
      json: vi.fn().mockResolvedValue({ message: 'bad gateway' }),
    };
    mockFetch.mockResolvedValueOnce(mockResponse);

    await expect(adminLinkClaims('token')).rejects.toThrow(/failed to load claims/);
  });
});

describe('adminPasteCookie', () => {
  it('returns ok result passthrough', async () => {
    const mockResult: CookieResult = { ok: true };
    const mockResponse = {
      ok: true,
      status: 200,
      json: vi.fn().mockResolvedValue(mockResult),
    };
    mockFetch.mockResolvedValueOnce(mockResponse);

    const result = await adminPasteCookie('cookie_value');

    expect(result).toEqual(mockResult);
    expect(result).not.toHaveProperty('restored_previous');
    expect(mockFetch).toHaveBeenCalledWith('/admin/api/cookie', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ cookie: 'cookie_value' }),
    });
  });

  it('throws Unauthorized on 401', async () => {
    const mockResponse = {
      status: 401,
      json: vi.fn().mockResolvedValue({ error: 'unauthorized' }),
    };
    mockFetch.mockResolvedValueOnce(mockResponse);

    await expect(adminPasteCookie('cookie')).rejects.toBeInstanceOf(Unauthorized);
  });
});

describe('adminStatus', () => {
  it('returns StatusView shape passthrough', async () => {
    const mockStatus: StatusView = {
      sync: {
        last_run_epoch: 1688000000,
        ok: true,
        cookie_ok: true,
        games_written: 42,
        message: 'sync completed',
      },
      game_counts: { available: 10, pending: 2, gifted: 1 },
    };

    const mockResponse = {
      ok: true,
      status: 200,
      json: vi.fn().mockResolvedValue(mockStatus),
    };
    mockFetch.mockResolvedValueOnce(mockResponse);

    const result = await adminStatus();

    expect(result).toEqual(mockStatus);
    expect(mockFetch).toHaveBeenCalledWith('/admin/api/status');
  });

  it('throws Unauthorized on 401', async () => {
    const mockResponse = {
      status: 401,
      json: vi.fn().mockResolvedValue({ error: 'unauthorized' }),
    };
    mockFetch.mockResolvedValueOnce(mockResponse);

    await expect(adminStatus()).rejects.toBeInstanceOf(Unauthorized);
  });
});

describe('adminSetHidden', () => {
  it('returns {ok:true} on success', async () => {
    const mockResponse = {
      ok: true,
      status: 200,
      json: vi.fn().mockResolvedValue({}),
    };
    mockFetch.mockResolvedValueOnce(mockResponse);

    const result = await adminSetHidden('game1', true);

    expect(result).toEqual({ ok: true });
    expect(mockFetch).toHaveBeenCalledWith('/admin/api/games/game1/hidden', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ hidden: true }),
    });
  });

  it('returns {ok:false, message} on 409 with error', async () => {
    const mockResponse = {
      status: 409,
      ok: false,
      json: vi.fn().mockResolvedValue({ error: 'conflict error' }),
    };
    mockFetch.mockResolvedValueOnce(mockResponse);

    const result = await adminSetHidden('game1', false);

    expect(result).toEqual({ ok: false, message: 'conflict error' });
  });

  it('throws Unauthorized on 401', async () => {
    const mockResponse = {
      status: 401,
      json: vi.fn().mockResolvedValue({ error: 'unauthorized' }),
    };
    mockFetch.mockResolvedValueOnce(mockResponse);

    await expect(adminSetHidden('game1', true)).rejects.toBeInstanceOf(Unauthorized);
  });
});

describe('adminCreateLink', () => {
  it('returns {token, url_path} passthrough on success', async () => {
    const mockData = { token: 'abc123', url_path: '/gift/abc123' };
    const mockResponse = {
      ok: true,
      status: 200,
      json: vi.fn().mockResolvedValue(mockData),
    };
    mockFetch.mockResolvedValueOnce(mockResponse);

    const result = await adminCreateLink('My Link', 10, 30);

    expect(result).toEqual(mockData);
    expect(mockFetch).toHaveBeenCalledWith('/admin/api/links', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({
        label: 'My Link',
        claims_allowed: 10,
        expires_days: 30,
      }),
    });
  });

  it('asserts POST body when expires_days is undefined', async () => {
    const mockData = { token: 'xyz789', url_path: '/gift/xyz789' };
    const mockResponse = {
      ok: true,
      status: 200,
      json: vi.fn().mockResolvedValue(mockData),
    };
    mockFetch.mockResolvedValueOnce(mockResponse);

    await adminCreateLink('No Expiry', 5);

    expect(mockFetch).toHaveBeenCalledWith('/admin/api/links', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({
        label: 'No Expiry',
        claims_allowed: 5,
        expires_days: undefined,
      }),
    });
  });

  it('throws Unauthorized on 401', async () => {
    const mockResponse = {
      status: 401,
      json: vi.fn().mockResolvedValue({ error: 'unauthorized' }),
    };
    mockFetch.mockResolvedValueOnce(mockResponse);

    await expect(adminCreateLink('Link', 10)).rejects.toBeInstanceOf(Unauthorized);
  });
});

describe('adminLinks', () => {
  it('returns AdminLink[] array passthrough on success', async () => {
    const mockLinks: AdminLink[] = [
      {
        token: 'token1',
        label: 'Link 1',
        claims_allowed: 5,
        claims_used: 2,
        revoked: false,
        expires_at: '2026-08-03T00:00:00Z',
        created_at: '2026-07-03T00:00:00Z',
      },
      {
        token: 'token2',
        label: 'Link 2',
        claims_allowed: 10,
        claims_used: 0,
        revoked: false,
        expires_at: null,
        created_at: '2026-07-03T01:00:00Z',
      },
    ];
    const mockResponse = {
      ok: true,
      status: 200,
      json: vi.fn().mockResolvedValue(mockLinks),
    };
    mockFetch.mockResolvedValueOnce(mockResponse);

    const result = await adminLinks();

    expect(result).toEqual(mockLinks);
    expect(result).toHaveLength(2);
    expect(mockFetch).toHaveBeenCalledWith('/admin/api/links');
  });

  it('throws Unauthorized on 401', async () => {
    const mockResponse = {
      status: 401,
      json: vi.fn().mockResolvedValue({ error: 'unauthorized' }),
    };
    mockFetch.mockResolvedValueOnce(mockResponse);

    await expect(adminLinks()).rejects.toBeInstanceOf(Unauthorized);
  });
});

describe('adminRevoke', () => {
  it('completes successfully on 200', async () => {
    const mockResponse = {
      ok: true,
      status: 200,
      json: vi.fn().mockResolvedValue({}),
    };
    mockFetch.mockResolvedValueOnce(mockResponse);

    await adminRevoke('token123');

    expect(mockFetch).toHaveBeenCalledWith('/admin/api/links/token123/revoke', {
      method: 'POST',
    });
  });

  it('throws Unauthorized on 401', async () => {
    const mockResponse = {
      status: 401,
      json: vi.fn().mockResolvedValue({ error: 'unauthorized' }),
    };
    mockFetch.mockResolvedValueOnce(mockResponse);

    await expect(adminRevoke('token')).rejects.toBeInstanceOf(Unauthorized);
  });

  it('throws on 404 — a failed revoke must never look successful', async () => {
    const mockResponse = {
      ok: false,
      status: 404,
      json: vi.fn().mockResolvedValue({}),
    };
    mockFetch.mockResolvedValueOnce(mockResponse);

    await expect(adminRevoke('token')).rejects.toThrow(/revoke failed/);
  });

  it('throws on 500', async () => {
    const mockResponse = {
      ok: false,
      status: 500,
      json: vi.fn().mockResolvedValue({}),
    };
    mockFetch.mockResolvedValueOnce(mockResponse);

    await expect(adminRevoke('token')).rejects.toThrow(/revoke failed/);
  });
});

describe('adminSync', () => {
  it('returns {games_written, orders_failed} extraction on success', async () => {
    const mockData = { games_written: 42, orders_failed: 3 };
    const mockResponse = {
      ok: true,
      status: 200,
      json: vi.fn().mockResolvedValue(mockData),
    };
    mockFetch.mockResolvedValueOnce(mockResponse);

    const result = await adminSync();

    expect(result).toEqual({ games_written: 42, orders_failed: 3 });
    expect(mockFetch).toHaveBeenCalledWith('/admin/api/sync', {
      method: 'POST',
    });
  });

  it('throws Error with message on 500 with empty body', async () => {
    const mockResponse = {
      status: 500,
      ok: false,
      json: vi.fn().mockRejectedValue(new Error('invalid json')),
    };
    mockFetch.mockResolvedValueOnce(mockResponse);

    await expect(adminSync()).rejects.toThrow('sync failed — check status panel');
  });

  it('throws the same message on 200 with a malformed body (json catch branch)', async () => {
    const mockResponse = {
      ok: true,
      status: 200,
      json: vi.fn().mockRejectedValue(new SyntaxError('unexpected token')),
    };
    mockFetch.mockResolvedValueOnce(mockResponse);

    await expect(adminSync()).rejects.toThrow('sync failed — check status panel');
  });

  it('throws Error with message on non-ok status before attempting json', async () => {
    const mockResponse = {
      status: 502,
      ok: false,
      json: vi.fn(),
    };
    mockFetch.mockResolvedValueOnce(mockResponse);

    await expect(adminSync()).rejects.toThrow('sync failed — check status panel');
    // Verify json() was not called since we guard before it
    expect(mockResponse.json).not.toHaveBeenCalled();
  });

  it('throws Unauthorized on 401', async () => {
    const mockResponse = {
      status: 401,
      json: vi.fn().mockResolvedValue({ error: 'unauthorized' }),
    };
    mockFetch.mockResolvedValueOnce(mockResponse);

    await expect(adminSync()).rejects.toBeInstanceOf(Unauthorized);
  });
});
