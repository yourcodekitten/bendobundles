import type { SteamDetailBlob } from './api';

// ── Per-session module-level detail cache ─────────────────────────────────────
// Keyed by scope:token:gameId (friend) or admin:gameId so different links and
// the admin surface never collide. Survives close/reopen since the Map lives
// outside the component — unmounting does not destroy it.
//
// Lives in its own module (not GameDetailModal.tsx) so the component file has
// only component exports and stays React Fast Refresh compatible.

export const gameDetailCache = new Map<string, SteamDetailBlob | null>();

export function clearGameDetailCache(): void {
  gameDetailCache.clear();
}
