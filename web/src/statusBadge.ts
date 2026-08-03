// Status badge — exact color mapping from plan (snake_case serde values)
//   available=green, pending=amber, gifted=violet, ben_redeemed=slate, expired=red
// Shared by the admin catalog rows and the game-detail modal (#51 PR D dedup) —
// the two copies had already agreed byte-for-byte; now they can't drift.
export function statusBadgeClass(status: string): string {
  switch (status) {
    case 'available':
      return 'bg-green-700 text-green-100';
    case 'pending':
      return 'bg-amber-700 text-amber-100';
    case 'gifted':
      return 'bg-give text-give-ink';
    case 'ben_redeemed':
      return 'bg-slate-600 text-slate-100';
    case 'expired':
      return 'bg-red-700 text-red-100';
    default:
      return 'bg-control text-ink';
  }
}
