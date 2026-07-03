import { describe, it, expect, beforeEach, vi } from 'vitest';
import {
  fetchLink,
  claimGame,
  adminLogin,
  adminCatalog,
  adminLinkClaims,
  adminPasteCookie,
  adminStatus,
  NotFound,
  Unauthorized,
  type ClaimResult,
  type CookieResult,
  type StatusView,
  type ClaimView,
  type AdminGame,
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
      status: 200,
      json: vi.fn().mockResolvedValue({
        label: 'Test Link',
        claims_allowed: 5,
        claims_used: 2,
        active: true,
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
    expect(result.active).toBe(true);
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

  it('throws NotFound on 500 from fetchLink', async () => {
    const mockResponse = {
      status: 500,
      json: vi.fn().mockResolvedValue({ error: 'server error' }),
    };
    mockFetch.mockResolvedValueOnce(mockResponse);

    await expect(fetchLink('token')).rejects.toBeInstanceOf(NotFound);
  });
});

describe('claimGame', () => {
  it('returns {kind:"gifted", gift_url} on 200', async () => {
    const mockResponse = {
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
        keyindex: 0,
      },
    ] as AdminGame[];

    const mockResponse = {
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

  it('maps ClaimRecord array to ClaimView shape (no title)', async () => {
    const mockRecords = [
      {
        id: 'claim1',
        link_token: 'token',
        game_id: 'game1',
        state: 'fulfilled',
        gift_url: 'https://humble.example.com/gift1',
        created_at: '2026-07-03T12:00:00Z',
      },
      {
        id: 'claim2',
        link_token: 'token',
        game_id: 'game2',
        state: 'pending',
        gift_url: null,
        created_at: '2026-07-03T12:01:00Z',
      },
    ];

    const mockResponse = {
      status: 200,
      json: vi.fn().mockResolvedValue(mockRecords),
    };
    mockFetch.mockResolvedValueOnce(mockResponse);

    const result = (await adminLinkClaims('token')) as ClaimView[];

    expect(result).toHaveLength(2);
    expect(result[0]).toEqual({
      game_id: 'game1',
      state: 'fulfilled',
      gift_url: 'https://humble.example.com/gift1',
    });
    expect(result[1]).toEqual({
      game_id: 'game2',
      state: 'pending',
      gift_url: null,
    });
    expect(result[0]).not.toHaveProperty('title');
    expect(result[0]).not.toHaveProperty('id');
    expect(result[0]).not.toHaveProperty('created_at');
  });
});

describe('adminPasteCookie', () => {
  it('returns ok result passthrough', async () => {
    const mockResult: CookieResult = { ok: true, restored_previous: false };
    const mockResponse = {
      status: 200,
      json: vi.fn().mockResolvedValue(mockResult),
    };
    mockFetch.mockResolvedValueOnce(mockResponse);

    const result = await adminPasteCookie('cookie_value');

    expect(result).toEqual(mockResult);
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
