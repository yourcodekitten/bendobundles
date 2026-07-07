// Deterministic fallback color from a game title — the SAME game must render
// the same color everywhere it appears (friend grid, admin catalog). Shared so
// a palette or hash tweak can never make the two surfaces disagree.
// Muted-earth palette (tokens in index.css @theme; DESIGN.md "The Title-Hash
// Rule"). Order is a hue-matched migration from the original tailwind -800 set
// (violet→heather, blue→slate, …) so each title's color FEELS continuous.
const PALETTE = [
  'bg-hash-heather',
  'bg-hash-slate',
  'bg-hash-moss',
  'bg-hash-mustard',
  'bg-hash-rust',
  'bg-hash-mauve',
  'bg-hash-pine',
  'bg-hash-clay',
] as const;

export function titleColorClass(title: string): string {
  let hash = 0;
  for (let i = 0; i < title.length; i++) {
    hash = ((hash << 5) - hash + title.charCodeAt(i)) | 0;
  }
  return PALETTE[Math.abs(hash) % PALETTE.length] ?? 'bg-shelf';
}
