import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import {
  loadIdentity,
  saveIdentity,
  clearIdentity,
  consumeReturnFragment,
  type SteamIdentity,
} from './steamIdentity';

describe('steamIdentity', () => {
  beforeEach(() => {
    localStorage.clear();
    // Reset URL to a clean state before each test
    history.replaceState(null, '', '/');
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  // ── localStorage round-trip ──────────────────────────────────────────────────

  describe('round-trip (save/load/clear)', () => {
    it('loadIdentity returns null when nothing is stored', () => {
      expect(loadIdentity()).toBeNull();
    });

    it('saveIdentity + loadIdentity preserves all fields', () => {
      const id: SteamIdentity = {
        steamid: '76561198000000001',
        persona: 'TestUser',
        owned: [420, 730],
        fetched_at: 1_000_000,
      };
      saveIdentity(id);
      expect(loadIdentity()).toEqual(id);
    });

    it('clearIdentity removes the stored value', () => {
      saveIdentity({ steamid: '1', persona: 'x', owned: [], fetched_at: 0 });
      clearIdentity();
      expect(loadIdentity()).toBeNull();
    });

    it('loadIdentity returns null (not throws) on corrupt localStorage', () => {
      localStorage.setItem('steam_identity', '{not-json}');
      expect(loadIdentity()).toBeNull();
    });
  });

  // ── consumeReturnFragment ───────────────────────────────────────────────────

  describe('consumeReturnFragment', () => {
    it('returns null when hash is empty', () => {
      expect(consumeReturnFragment()).toBeNull();
    });

    it('returns null for an irrelevant hash', () => {
      history.replaceState(null, '', '/#otherthing=1');
      expect(consumeReturnFragment()).toBeNull();
    });

    it('parses #steam=…&persona=… and returns {steamid, persona}', () => {
      history.replaceState(null, '', '/#steam=76561198000000001&persona=Alice');
      expect(consumeReturnFragment()).toEqual({
        steamid: '76561198000000001',
        persona: 'Alice',
      });
    });

    it('decodes percent-encoded persona', () => {
      history.replaceState(null, '', '/#steam=123&persona=Test%20User');
      expect(consumeReturnFragment()).toEqual({ steamid: '123', persona: 'Test User' });
    });

    it('clears the hash after consuming a steam fragment', () => {
      history.replaceState(null, '', '/#steam=123&persona=Alice');
      consumeReturnFragment();
      expect(location.hash).toBe('');
    });

    it('parses #steam_error=verify_failed', () => {
      history.replaceState(null, '', '/#steam_error=verify_failed');
      expect(consumeReturnFragment()).toEqual({ error: 'verify_failed' });
    });

    it('parses #steam_error=steam_unreachable', () => {
      history.replaceState(null, '', '/#steam_error=steam_unreachable');
      expect(consumeReturnFragment()).toEqual({ error: 'steam_unreachable' });
    });

    it('clears the hash after consuming an error fragment', () => {
      history.replaceState(null, '', '/#steam_error=verify_failed');
      consumeReturnFragment();
      expect(location.hash).toBe('');
    });

    it('returns null on second call (hash was cleared by first call)', () => {
      history.replaceState(null, '', '/#steam=123&persona=x');
      consumeReturnFragment();
      expect(consumeReturnFragment()).toBeNull();
    });
  });
});
