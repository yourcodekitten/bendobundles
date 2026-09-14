import { useState, useEffect, useCallback } from 'react';
import { useNavigate, Link as RouterLink } from 'react-router-dom';
import { adminScrapbook, type ScrapbookView, type ScrapbookEntry } from '../api';
import { withAuth } from './withAuth';
import { postmark, waitedYears } from '../postmark';

// The scrapbook 📖 — the story of ben's giving, told back to him
// (docs/spec-scrapbook.md). Read-only: every filter arrived server-side; this
// page only groups, orders-preserves, and keeps the voice warm.

type PageState =
  | { phase: 'loading' }
  | { phase: 'loaded'; view: ScrapbookView }
  | { phase: 'error' };

/** bought/opened/waited line for a keepsake card. The span ENDS AT THE UNWRAP —
 *  `waitedYears` must be fed the claim moment, never its default `now` (the
 *  default-now call drifts +1 every January; postmark.ts's own doc names this
 *  caller). A negative span (acquired postdates the claim — bad data) omits
 *  the waited clause rather than rendering "waited −2 years". */
function spanLine(e: ScrapbookEntry): string | null {
  const opened = postmark(e.claimed_at);
  if (opened === null) return null;
  const bought = postmark(e.game.acquired_at ?? undefined);
  if (bought === null) return `opened ${opened}`;
  const claimedMs = Date.parse(e.claimed_at);
  const waited =
    Date.parse(e.game.acquired_at as string) <= claimedMs
      ? waitedYears(e.game.acquired_at ?? undefined, claimedMs)
      : null;
  const base = `bought ${bought} · opened ${opened}`;
  return waited !== null ? `${base} — waited ${waited} years` : base;
}

function KeepsakeCard({ e }: { e: ScrapbookEntry }) {
  const span = spanLine(e);
  return (
    <article className="flex gap-4 rounded bg-floor p-4">
      {e.game.artwork_url ? (
        <img
          src={e.game.artwork_url}
          alt=""
          className="h-20 w-20 rounded object-cover"
        />
      ) : (
        <div
          aria-hidden="true"
          className="flex h-20 w-20 items-center justify-center rounded bg-shelf text-2xl"
        >
          🎁
        </div>
      )}
      <div className="flex min-w-0 flex-col gap-1">
        <h3 className="font-medium text-ink">{e.game.title}</h3>
        <RouterLink
          to={{ pathname: '/admin/links', hash: `#link-${e.link_token}` }}
          className="w-fit text-sm text-dust hover:text-ink"
        >
          for {e.recipient} ♡
        </RouterLink>
        {e.state === 'pending' && (
          <span className="w-fit rounded bg-amber-900 px-2 py-0.5 text-xs text-amber-200">
            unwrapping…
          </span>
        )}
        {e.gift_note && <p className="text-sm text-ink-soft">{e.gift_note}</p>}
        {e.tag && <p className="text-sm text-ink-soft">✍ {e.tag}</p>}
        {e.thank_note && (
          <p className="text-sm text-ink-soft">
            “{e.thank_note}” — {postmark(e.thanked_at ?? undefined) ?? 'their thanks'}
          </p>
        )}
        {span && <p className="text-xs text-dust">{span}</p>}
      </div>
    </article>
  );
}

function yearOf(iso: string): number {
  return new Date(Date.parse(iso)).getUTCFullYear();
}

// lowercase month list matching postmark's style; day-level because a seal has
// a DATE, unlike the month-grain postmark. null/junk → caller omits the clause.
const MONTHS_SHORT = [
  'jan', 'feb', 'mar', 'apr', 'may', 'jun',
  'jul', 'aug', 'sep', 'oct', 'nov', 'dec',
];
function sealDate(iso: string): string | null {
  const t = Date.parse(iso);
  if (Number.isNaN(t)) return null;
  const d = new Date(t);
  return `${MONTHS_SHORT[d.getUTCMonth()]} ${d.getUTCDate()}`;
}

/** The summary is derived CLIENT-side from the same payload the cards render
 *  from — a correctness property, not a convenience: the numbers cannot
 *  disagree with the cards because they share a derivation. Don't "optimise"
 *  it server-side into a second source of truth. */
function summarySentence(view: ScrapbookView): string | null {
  const n = view.entries.length;
  if (n === 0) return null;
  const people = new Set(view.entries.map((e) => e.recipient)).size;
  const thanks = view.entries.filter((e) => e.thank_note !== null).length;
  const years = view.entries.map((e) => yearOf(e.claimed_at));
  const span = Math.max(...years) - Math.min(...years);
  let s = `${n} ${n === 1 ? 'gift' : 'gifts'} opened by ${people} ${people === 1 ? 'person' : 'people'}`;
  if (span > 0) s += ` across ${span} years`;
  if (thanks > 0) s += ` · ${thanks} ${thanks === 1 ? 'thank-you' : 'thank-yous'}`;
  return `${s} ♡`;
}

export function Scrapbook() {
  const navigate = useNavigate();
  const [state, setState] = useState<PageState>({ phase: 'loading' });

  const load = useCallback(() => {
    setState({ phase: 'loading' });
    withAuth(() => adminScrapbook(), navigate)
      .then((view) => setState({ phase: 'loaded', view }))
      .catch(() => setState({ phase: 'error' }));
  }, [navigate]);

  useEffect(() => {
    load();
  }, [load]);

  if (state.phase === 'loading') {
    return <p className="text-dust">loading…</p>;
  }
  if (state.phase === 'error') {
    return (
      <div className="flex flex-col gap-4">
        <p className="text-dust">couldn't open the scrapbook — try again</p>
        <button
          onClick={load}
          className="w-fit rounded bg-control px-4 py-2 text-sm hover:bg-control-bright"
        >
          retry
        </button>
      </div>
    );
  }

  const { view } = state;

  // group entries by year of unwrap, oldest first; order INSIDE a year is the
  // server's (claimed_at, game_id) sort, preserved — never re-derived here.
  const byYear = new Map<number, ScrapbookEntry[]>();
  for (const e of view.entries) {
    const y = yearOf(e.claimed_at);
    const bucket = byYear.get(y);
    if (bucket) {
      bucket.push(e);
    } else {
      byYear.set(y, [e]);
    }
  }
  const years = [...byYear.keys()].sort((a, b) => a - b);

  const summary = summarySentence(view);

  return (
    <div className="flex flex-col gap-6">
      {summary && <p className="text-sm text-ink-soft">{summary}</p>}
      {years.length > 1 && (
        <nav aria-label="years" className="flex flex-wrap gap-2 text-sm">
          {years.map((y) => (
            <a
              key={y}
              href={`#y${y}`}
              className="rounded bg-shelf px-2 py-0.5 text-dust hover:text-ink"
            >
              {y}
            </a>
          ))}
        </nav>
      )}

      {view.entries.length === 0 && (
        <p className="text-dust">no gifts opened yet — the story starts with the first unwrap ♡</p>
      )}

      {years.map((y) => (
        <section key={y} id={`y${y}`} className="flex flex-col gap-3">
          <h2 className="text-sm font-medium text-ink-soft">{y}</h2>
          {(byYear.get(y) ?? []).map((e) => (
            <KeepsakeCard key={`${e.link_token}-${e.game.id}-${e.claimed_at}`} e={e} />
          ))}
        </section>
      ))}

      {view.waiting.length > 0 && (
        <section className="flex flex-col gap-3">
          <h2 className="text-sm font-medium text-ink-soft">chosen and waiting</h2>
          {view.waiting.map((w) => {
            const sealed = w.sealed_until ? sealDate(w.sealed_until) : null;
            return (
              <div key={w.link_token} className="rounded bg-floor p-4">
                <p className="text-sm text-ink">
                  {sealed ? `for ${w.recipient} · wrapped until ${sealed}` : `for ${w.recipient}`}
                </p>
                <ul className="mt-2 flex flex-col gap-1">
                  {w.games.map((g) => {
                    const bought = postmark(g.acquired_at ?? undefined);
                    // still waiting — the span genuinely runs to now, so the
                    // default is correct HERE (and only here + doors).
                    const waitingYears = waitedYears(g.acquired_at ?? undefined);
                    return (
                      <li key={g.id} className="text-sm text-ink-soft">
                        {g.title}
                        {bought && (
                          <span className="text-xs text-dust">
                            {' '}
                            · bought {bought}
                            {waitingYears !== null && ` · waiting ${waitingYears} years`}
                          </span>
                        )}
                      </li>
                    );
                  })}
                </ul>
              </div>
            );
          })}
        </section>
      )}

      {view.doors_open.length > 0 && (
        <section className="flex flex-col gap-2">
          <h2 className="text-sm font-medium text-ink-soft">doors left open</h2>
          {view.doors_open.map((d) => {
            // a door's span feeds link.created_at — a DIFFERENT field from the
            // cards' game.acquired_at; never symmetry-copied (spec).
            const openYears = waitedYears(d.created_at);
            const clauses = [
              `the door ben left open for ${d.recipient}`,
              ...(openYears !== null ? [`open ${openYears} years`] : []),
              `${d.claims_left} ${d.claims_left === 1 ? 'claim' : 'claims'} left`,
            ];
            return (
              <p key={d.link_token} className="text-sm text-ink-soft">
                {clauses.join(' · ')}
              </p>
            );
          })}
        </section>
      )}

      {(view.orphan_claim_count > 0 || view.stale_pending_count > 0) && (
        <footer className="flex flex-col gap-1 text-xs text-dust">
          {view.orphan_claim_count > 0 && (
            <p>
              {view.orphan_claim_count === 1
                ? "1 claim references a link this page can't find"
                : `${view.orphan_claim_count} claims reference links this page can't find`}
            </p>
          )}
          {view.stale_pending_count > 0 && (
            <p>
              {view.stale_pending_count === 1
                ? '1 gift is stuck mid-unwrap — see ops'
                : `${view.stale_pending_count} gifts are stuck mid-unwrap — see ops`}
            </p>
          )}
        </footer>
      )}
    </div>
  );
}
