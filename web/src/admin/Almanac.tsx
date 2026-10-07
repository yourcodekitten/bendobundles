import { useState, useEffect, useCallback, useMemo } from 'react';
import { useNavigate } from 'react-router-dom';
import { adminCatalog, type AdminGame } from '../api';
import { withAuth } from './withAuth';
import { titleColorClass } from '../titleColor';
import {
  MONTH_NAMES,
  buildAlmanac,
  countLine,
  headline,
  subtitle,
  thumbSrcs,
  type AlmanacEntry,
} from '../almanac';

// The almanac 📜 — ben's attic as a calendar of months (docs/spec-almanac.md).
// Read-only; every grouping decision lives in ../almanac.ts. This page renders.

type PageState =
  | { phase: 'loading' }
  | { phase: 'loaded'; games: AdminGame[] }
  | { phase: 'error' };

const STRIP_MAX = 8;
const GLYPH: Record<AlmanacEntry['source'], string> = { name: '🗓️', postmark: '📮', undated: '📦' };
// the glyph is the ONLY signal of where a date came from — say it to a screen reader too
const SOURCE_SR: Record<AlmanacEntry['source'], string> = {
  name: 'dated by its name',
  postmark: 'dated by its postmark',
  undated: 'no date known',
};

function Thumb({ g }: { g: AdminGame }) {
  // walk the ladder on each load error (a steam capsule can 404: 11 of 858 in the real
  // render), then the colour block — never the browser's broken-image icon
  const srcs = thumbSrcs(g);
  const [rung, setRung] = useState(0);
  const src = srcs[rung] ?? null;
  return src !== null ? (
    <img
      src={src}
      alt=""
      loading="lazy"
      onError={() => setRung((r) => r + 1)}
      className="h-10 w-16 flex-shrink-0 rounded object-cover"
    />
  ) : (
    <div aria-hidden="true" className={`h-10 w-16 flex-shrink-0 rounded ${titleColorClass(g.title)}`} />
  );
}

// heading level follows where the entry sits — h4 under a month's h3, h3 directly on the
// undated shelf's h2 — so the outline never skips a level (review 2)
function Entry({ e, onWrap, level }: { e: AlmanacEntry; onWrap: (e: AlmanacEntry) => void; level: 3 | 4 }) {
  const Heading = level === 3 ? 'h3' : 'h4';
  const lit = e.waiting.length > 0;
  const shown = e.games.slice(0, STRIP_MAX);
  const more = e.games.length - shown.length;
  return (
    <article
      aria-label={e.label}
      // lit = raised shelf; quiet = flat + faded. No burgundy: DESIGN.md's Button Burgundy Rule
      className={`flex flex-col gap-2 rounded p-4 ${lit ? 'bg-shelf' : 'bg-floor opacity-80'}`}
    >
      <div className="flex flex-wrap items-baseline gap-2">
        <span aria-hidden="true">{GLYPH[e.source]}</span>
        <span className="sr-only">{SOURCE_SR[e.source]}</span>
        <Heading className="font-medium text-ink">{e.label}</Heading>
        <span className="text-sm text-dust">{countLine(e)}</span>
        {lit && (
          <button
            type="button"
            onClick={() => onWrap(e)}
            // label-in-name (WCAG 2.5.3): the accessible name starts with the visible text
            aria-label={`wrap these: ${e.waiting.length} waiting from ${e.label}`}
            className="ml-auto rounded bg-control px-3 py-1 text-sm hover:bg-control-bright"
          >
            wrap these →
          </button>
        )}
      </div>
      <div className="flex flex-wrap items-center gap-2">
        {shown.map((g) => (
          <span key={g.id} title={g.title} className={g.hidden ? 'opacity-40' : undefined}>
            {/* keyed on the ladder itself: a changed source list restarts at rung 0 */}
            <Thumb key={thumbSrcs(g).join('|')} g={g} />
          </span>
        ))}
        {more > 0 && <span className="text-sm text-dust">+{more}</span>}
      </div>
    </article>
  );
}

export function Almanac({ currentYear = new Date().getUTCFullYear() }: { currentYear?: number }) {
  const navigate = useNavigate();
  const [state, setState] = useState<PageState>({ phase: 'loading' });

  const load = useCallback(() => {
    setState({ phase: 'loading' });
    withAuth(() => adminCatalog(), navigate)
      .then((games) => setState({ phase: 'loaded', games }))
      .catch(() => setState({ phase: 'error' }));
  }, [navigate]);

  useEffect(() => {
    load();
  }, [load]);

  const almanac = useMemo(
    () => (state.phase === 'loaded' ? buildAlmanac(state.games, currentYear) : null),
    [state, currentYear],
  );

  // the catalog's exact handoff contract (Catalog.tsx → Links.tsx location.state.picked)
  const wrap = (e: AlmanacEntry) =>
    navigate('/admin/links', {
      state: { picked: e.waiting.map((g) => ({ id: g.id, title: g.title, requiresChoice: g.requires_choice })) },
    });

  if (state.phase === 'loading') return <p className="text-dust">loading…</p>;
  if (state.phase === 'error' || almanac === null) {
    return (
      <div className="flex flex-col gap-4">
        <p className="text-dust">couldn't open the almanac — try again</p>
        <button onClick={load} className="w-fit rounded bg-control px-4 py-2 text-sm hover:bg-control-bright">
          retry
        </button>
      </div>
    );
  }

  const line = headline(almanac);
  const { unparsed } = almanac.picks;
  return (
    <div className="flex flex-col gap-8">
      <header className="flex flex-col gap-1">
        <h1 className="text-xl font-medium text-ink">📜 the almanac</h1>
        <p className="text-ink-soft">{subtitle(almanac)}</p>
        {line && <p className="font-medium text-ink">{line}</p>}
        {/* beside N, which counts these picks — an unparsed pick may still be DATED by a
            postmark, so it is never the undated shelf's preface (reviews 1 + 2) */}
        {unparsed > 0 && (
          <p className="text-sm text-ink">
            {`⚠️ ${unparsed} choice ${unparsed === 1 ? 'pick' : 'picks'} whose month we couldn't read`}
          </p>
        )}
      </header>

      {almanac.years.map((y) => (
        <section key={y.year} className="flex flex-col gap-4">
          <h2 className="border-b border-line pb-1 text-lg text-ink">{y.year}</h2>
          {y.months.map((m) => (
            <div key={m.month} className="flex flex-col gap-2 sm:flex-row sm:gap-4">
              <h3 className="w-24 flex-shrink-0 text-sm font-normal text-dust">{MONTH_NAMES[m.month]}</h3>
              <div className="flex min-w-0 flex-1 flex-col gap-2">
                {m.entries.map((e) => (
                  <Entry key={e.key} e={e} onWrap={wrap} level={4} />
                ))}
              </div>
            </div>
          ))}
        </section>
      ))}

      {almanac.undated.length > 0 && (
        <section className="flex flex-col gap-3">
          <h2 className="border-b border-line pb-1 text-lg text-ink">undated</h2>
          <p className="text-sm text-dust">the attic doesn't know when these arrived.</p>
          {almanac.undated.map((e) => (
            <Entry key={e.key} e={e} onWrap={wrap} level={3} />
          ))}
        </section>
      )}
    </div>
  );
}
