// Claim-STATE badge — one mapping for every view that renders a claim state.
//   fulfilled=green · pending=amber · compensated=slate · failed=rose
// compensated is SLATE, not give-violet, by design verdict (#135→#136): give-violet is
// the "a gift succeeded" signature, and gift-failure-made-right must not wear it in the
// one view where an admin scans for exactly that difference.
// Shared by admin Links (claims audit) and Catalog (self-claims list) — the two copies
// had diverged on exactly the compensated arm; now they can't drift.
// NOT shared by friend/ClaimsHistory.tsx, deliberately: that surface maps state to
// {label, className} with friend vocabulary ("gifted"/"processing"/"returned") and darker
// shades — and its fulfilled arm wears give-violet CORRECTLY (on that surface the gift
// succeeded, which is exactly the signature the verdict reserves the color for). Its
// compensated arm is already slate. Do not "unify" it into this module.
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
      // DELIBERATE RUNTIME NET — do not swap for an exhaustiveness assert. The state
      // unions in api.ts are bare `as` assertions over untrusted JSON (api.ts:442), so a
      // fifth backend state walks straight into this arm at runtime. Neutral-control is
      // the honest render: the old Catalog fallback was amber, which made an unknown
      // state impersonate "pending" in the exact view an admin scans for differences.
      return 'bg-control text-ink';
  }
}
