// Claim-STATE badge — one mapping for every view that renders a claim state.
//   fulfilled=green · pending=amber · compensated=slate · failed=rose
// compensated is SLATE, not give-violet, by design verdict (#135→#136): give-violet is
// the "a gift succeeded" signature, and gift-failure-made-right must not wear it in the
// one view where an admin scans for exactly that difference.
// Shared by admin Links (claims audit) and Catalog (self-claims list) — the two copies
// had diverged on exactly the compensated arm; now they can't drift.
export function stateBadgeClass(state: string): string {
  switch (state) {
    case 'fulfilled':
      return 'bg-green-700 text-green-100';
    case 'pending':
      return 'bg-amber-700 text-amber-100';
    case 'compensated':
      return 'bg-slate-600 text-slate-100';
    case 'failed':
      return 'bg-rose-950 text-rose-200';
    default:
      return 'bg-control text-ink';
  }
}
