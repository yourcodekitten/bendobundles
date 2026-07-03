import type { ClaimView } from '../api';

const STATE_CHIP: Record<ClaimView['state'], { label: string; className: string }> = {
  fulfilled: { label: 'gifted', className: 'bg-violet-900 text-violet-200' },
  pending: { label: 'processing', className: 'bg-amber-900 text-amber-200' },
  compensated: { label: 'compensated', className: 'bg-slate-800 text-slate-300' },
};

interface ClaimsHistoryProps {
  claims: ClaimView[];
}

export function ClaimsHistory({ claims }: ClaimsHistoryProps) {
  if (claims.length === 0) return null;

  return (
    <section className="px-6 py-4">
      <h2 className="text-sm font-semibold text-zinc-400 uppercase tracking-wider mb-3">
        your gifts
      </h2>
      <ul className="space-y-2">
        {claims.map((claim, index) => {
          const chip = STATE_CHIP[claim.state] ?? {
            label: claim.state,
            className: 'bg-zinc-800 text-zinc-300',
          };

          return (
            <li
              key={`${claim.game_id}-${index}`}
              className="flex items-center gap-3 rounded bg-zinc-900 px-4 py-3 text-sm"
            >
              <span className={`shrink-0 rounded px-2 py-0.5 text-xs font-medium ${chip.className}`}>
                {chip.label}
              </span>
              <span className="flex-1 truncate text-zinc-300">
                {claim.title ?? claim.game_id}
              </span>
              {claim.state === 'fulfilled' && claim.gift_url !== null ? (
                <a
                  href={claim.gift_url}
                  target="_blank"
                  rel="noreferrer"
                  className="shrink-0 text-violet-400 hover:text-violet-300 underline text-xs"
                >
                  lost the tab? it&apos;s right here
                </a>
              ) : claim.state === 'pending' ? (
                <span className="shrink-0 text-xs text-zinc-500">processing</span>
              ) : null}
            </li>
          );
        })}
      </ul>
    </section>
  );
}
