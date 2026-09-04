import { useEffect, useState } from "react";
import { useParams } from "react-router-dom";
import { fetchShelf, NotFound, type ShelfView } from "../api";
import { titleColorClass } from "../titleColor";

type ViewState =
  | { kind: "loading" }
  | { kind: "not-found" }
  | { kind: "error" }
  | { kind: "loaded"; data: ShelfView };

/** America/New_York, not UTC and not browser-local — unwrapped_at is a server
 * instant in rfc3339, but the year is the keepsake's headline and the
 * audience (ben + the friend) reads it in ben's own timezone. A gift
 * unwrapped Dec 31 late-EST is still `getUTCFullYear()`-January in UTC, so
 * a naive UTC read shows the friend the wrong year for their own memory.
 * `Intl`/`toLocaleString` with an explicit `timeZone` is DST-aware, so this
 * stays correct across the EST/EDT boundary without hand-rolled offsets.
 * This is the shelf's single computation point for the year (the admin list
 * formats its dates browser-local and shows no unwrap year). Returns "" for
 * an unparseable instant — the caller skips the label entirely rather than
 * rendering a dangling "unwrapped ". */
function unwrappedYear(rfc3339: string): string {
  const d = new Date(rfc3339);
  if (Number.isNaN(d.getTime())) return "";
  return d.toLocaleString("en-US", {
    timeZone: "America/New_York",
    year: "numeric",
  });
}

/**
 * The friend's persistent shelf — every game ben ever gave them, at
 * `/s/{shelf_token}`. Read-only keepsake: no claim buttons, no links out, no
 * actions of any kind (spec-gift-shelf.md). Gifts render in exactly the
 * order the server delivers them (oldest-first) — this page never re-sorts;
 * the scroll itself IS the timeline.
 */
export function ShelfPage() {
  const { token } = useParams<{ token: string }>();
  const [view, setView] = useState<ViewState>({ kind: "loading" });
  const [refreshTick, setRefreshTick] = useState(0);
  const retry = () => setRefreshTick((t) => t + 1);

  useEffect(() => {
    let cancelled = false;

    async function load() {
      if (!token) {
        setView({ kind: "not-found" });
        return;
      }
      setView({ kind: "loading" });
      try {
        const data = await fetchShelf(token);
        if (!cancelled) setView({ kind: "loaded", data });
      } catch (error) {
        if (cancelled) return;
        if (error instanceof NotFound) {
          setView({ kind: "not-found" });
        } else {
          // Transient failure (public-api's "partial failure is whole
          // failure" 500) — soft retry, never the not-found copy: a live
          // shelf that hiccuped is not a dead link.
          setView({ kind: "error" });
        }
      }
    }

    void load();
    return () => {
      cancelled = true;
    };
  }, [token, refreshTick]);

  if (view.kind === "loading") {
    return (
      <div className="flex min-h-screen items-center justify-center bg-room text-ink">
        <p className="text-dust">loading...</p>
      </div>
    );
  }

  if (view.kind === "error") {
    return (
      <div className="flex min-h-screen items-center justify-center bg-room text-ink">
        <main className="text-center">
          <h1 className="text-2xl font-bold">couldn&apos;t load this shelf</h1>
          <p className="mt-2 text-dust">
            something hiccuped on our end — the shelf is fine
          </p>
          <button
            type="button"
            onClick={retry}
            className="mt-4 rounded bg-control px-4 py-2 text-sm hover:bg-control-bright"
          >
            retry
          </button>
        </main>
      </div>
    );
  }

  if (view.kind === "not-found") {
    return (
      <div className="flex min-h-screen items-center justify-center bg-room text-ink">
        <main className="text-center">
          <h1 className="text-2xl font-bold">shelf not found</h1>
          <p className="mt-2 text-dust">ask ben for the link ♡</p>
        </main>
      </div>
    );
  }

  const { data } = view;

  return (
    <div className="min-h-screen bg-room text-ink">
      <header className="border-b border-line px-6 py-8">
        <h1 className="font-pixel text-2xl leading-tight text-give-soft">
          ben&apos;s shelf for {data.name} <span aria-hidden="true">♡</span>
        </h1>
      </header>

      {data.gifts.length === 0 ? (
        <p className="px-6 py-16 text-center text-dust">
          this shelf is waiting for its first story
        </p>
      ) : (
        <ol className="mx-auto flex max-w-2xl flex-col gap-6 px-6 py-8">
          {data.gifts.map((gift) => {
            const year = unwrappedYear(gift.unwrapped_at);
            return (
              <li
                key={gift.game_id}
                className="flex gap-4 rounded-[6px_6px_20px_6px] bg-floor p-4"
              >
                {/* cover art, with the GameGrid fallback treatment: no
                    artwork_url falls back to the shared title-hash color —
                    there's no steam_app_id on a shelf gift to fall further
                    back to a steam capsule image. */}
                <div className="h-20 w-32 shrink-0 overflow-hidden rounded">
                  {gift.artwork_url !== null ? (
                    <img
                      src={gift.artwork_url}
                      alt={gift.title}
                      className="h-full w-full object-cover"
                    />
                  ) : (
                    <div
                      className={`h-full w-full ${titleColorClass(gift.title)}`}
                    />
                  )}
                </div>
                <div className="min-w-0 flex-1">
                  <h2 className="font-pixel text-lg leading-tight">
                    {gift.title}
                  </h2>
                  {year !== "" && (
                    <p className="mt-0.5 text-xs text-dust">unwrapped {year}</p>
                  )}
                  {/* ben's voice — same italic-quote-plus-attribution register
                      as the gift note on LinkPage's dialog box. */}
                  {gift.gift_note !== null && (
                    <p className="mt-2 max-w-[60ch] text-sm italic text-give-soft">
                      &ldquo;{gift.gift_note}&rdquo;{" "}
                      <span className="font-pixel not-italic text-xs text-dust">
                        &mdash; ben
                      </span>
                    </p>
                  )}
                  {/* the friend's reply — same register as ThanksCard's
                      SentNote, since this IS the friend's own page. */}
                  {gift.thank_note !== null && (
                    <p className="mt-1.5 max-w-[60ch] text-sm italic text-give-soft">
                      &ldquo;{gift.thank_note}&rdquo;{" "}
                      <span className="font-pixel not-italic text-xs text-dust">
                        &mdash; you, delivered to ben ♡
                      </span>
                    </p>
                  )}
                </div>
              </li>
            );
          })}
        </ol>
      )}
    </div>
  );
}
