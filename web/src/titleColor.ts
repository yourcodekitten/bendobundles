// Deterministic fallback color from a game title — the SAME game must render
// the same color everywhere it appears (friend grid, admin catalog). Shared so
// a palette or hash tweak can never make the two surfaces disagree.
const PALETTE = [
  'bg-violet-800',
  'bg-blue-800',
  'bg-green-800',
  'bg-amber-800',
  'bg-red-800',
  'bg-pink-800',
  'bg-teal-800',
  'bg-indigo-800',
] as const;

export function titleColorClass(title: string): string {
  let hash = 0;
  for (let i = 0; i < title.length; i++) {
    hash = ((hash << 5) - hash + title.charCodeAt(i)) | 0;
  }
  return PALETTE[Math.abs(hash) % PALETTE.length] ?? 'bg-zinc-700';
}
