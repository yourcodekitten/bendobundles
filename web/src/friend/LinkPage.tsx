import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useParams } from "react-router-dom";
import {
  fetchLink,
  fetchGameDetail,
  steamOwnedForLink,
  NotFound,
  type GameView,
  type LinkView,
} from "../api";
import {
  consumeReturnFragment,
  loadIdentity,
  saveIdentity,
  clearIdentity,
  beginConnect,
  type SteamIdentity,
} from "../steamIdentity";
import { ClaimDialog } from "./ClaimDialog";
import { ClaimsHistory } from "./ClaimsHistory";
import { GameGrid } from "./GameGrid";
import { GameDetailModal } from "../GameDetailModal";
import { CursorCompanion } from "./CursorCompanion";
import { prefersReducedMotion, motionOK } from "../motion";
import { graphemes } from "../text";
import { BootScreen } from "./BootScreen";

type ViewState =
  | { kind: "loading" }
  | { kind: "not-found" }
  | { kind: "error" }
  | { kind: "loaded"; data: LinkView };

// The gift page powers on like the handheld (ben's pick, 2026-07-09): the
// boot screen plays over the initial load — it doubles as the loading screen
// while the link data fetches behind it. Skipped under reduced motion.
export function LinkPage() {
  const [booting, setBooting] = useState(motionOK);
  return (
    <>
      {booting && <BootScreen onDone={() => setBooting(false)} />}
      <LinkPageBody bootDone={!booting} />
    </>
  );
}

/** The dialog box's blinking block cursor — one definition for all beats. */
function TwCursor() {
  return (
    <span aria-hidden="true" className="tw-cursor">
      &#9646;
    </span>
  );
}

function LinkPageBody({ bootDone }: { bootDone: boolean }) {
  const { token } = useParams<{ token: string }>();
  const [view, setView] = useState<ViewState>({ kind: "loading" });
  const [claimingGame, setClaimingGame] = useState<GameView | null>(null);
  const viewLoaded = view.kind === "loaded";
  // ── dialog-box typewriter (the page's one entrance; see DESIGN.md motion) ──
  const DIALOG_BODY =
    "games from ben's humble stash, picked for you \u2661 open one for details, claim it, and the key is yours.";
  const typedLabel = view.kind === "loaded" ? view.data.label : "";
  // ben's personal note types as a third beat after the standard body —
  // absent on most links, so everything is length-0-safe
  const giftNote = view.kind === "loaded" ? (view.data.gift_note ?? "") : "";
  // Beats are measured and sliced in GRAPHEME CLUSTERS — .slice on the string
  // cuts UTF-16 units (splits surrogate pairs into U+FFFD), and code points
  // still split ZWJ sequences (a family emoji assembles member by member).
  // Graphemes are the unit the eye sees; the gift note is exactly where emoji
  // show up. (The server's 500 bound stays code points — that's an input
  // limit, enforced where input happens.) Memoized: this component re-renders
  // ~70×/s while typing, and re-deriving per tick is pure allocation churn.
  const labelGr = useMemo(() => graphemes(typedLabel), [typedLabel]);
  const bodyGr = useMemo(() => graphemes(DIALOG_BODY), [DIALOG_BODY]);
  const noteGr = useMemo(() => graphemes(giftNote), [giftNote]);
  // cumulative beat offsets — every slice and cursor handoff below reads from
  // these, so the boundaries live in exactly one place
  const bodyStart = labelGr.length;
  const noteStart = bodyStart + bodyGr.length;
  const typeTotal = noteStart + noteGr.length;
  const [typeChars, setTypeChars] = useState(0);
  const [typeKey, setTypeKey] = useState(0);
  // the typeKey whose entrance has fully played, stamped ONLY at the explicit
  // completion sites (interval clamp, skip, reduced-motion/already-played
  // snap) — never derived from a render: a replay click's commit still sees
  // the old completed typeChars, and a derived stamp would mark the new key
  // played before it types a character. The note is editable post-creation,
  // so a background refetch can change typeTotal mid-session: once the
  // entrance has played, snap to the new text instead of replaying (a retype
  // would re-disable the typeDone-gated controls under the friend for
  // seconds). The replay button bumps typeKey, which re-arms the animation.
  const playedKeyRef = useRef<number | null>(null);
  useEffect(() => {
    // The entrance belongs to the loaded page. Without this gate the effect
    // types DIALOG_BODY invisibly behind the loading/error views (typeTotal
    // is never 0 — the body is a constant) and stamps itself played, so a
    // slow first fetch or an error→retry would render the box pre-typed and
    // the entrance would never animate.
    if (!viewLoaded) return;
    if (prefersReducedMotion() || playedKeyRef.current === typeKey) {
      playedKeyRef.current = typeKey;
      setTypeChars(typeTotal);
      return;
    }
    // the entrance waits for the boot screen to clear (the data loads BEHIND
    // the boot, so without this gate the typing is already done when the
    // boot cuts away — ben caught it, 2026-07-09)
    if (!bootDone) return;
    setTypeChars(0);
    // ...then a 1s beat before the first character — the box sits with its
    // blinking cursor for a moment, like the game is thinking
    let iv: ReturnType<typeof setInterval> | undefined;
    const delay = setTimeout(() => {
      iv = setInterval(() => {
        setTypeChars((c) => {
          if (c >= typeTotal) {
            if (iv !== undefined) clearInterval(iv);
            return c;
          }
          const next = c + 1;
          // completion observed at the source (idempotent ref write)
          if (next >= typeTotal) playedKeyRef.current = typeKey;
          return next;
        });
      }, 14);
    }, 1000);
    return () => {
      clearTimeout(delay);
      if (iv !== undefined) clearInterval(iv);
    };
  }, [typeKey, typeTotal, bootDone, viewLoaded]);
  const typeDone = typeChars >= typeTotal;
  // skip-to-end — click/tap on the dialog box, or Enter/Space anywhere (the
  // JRPG "press A" this page is homaging; keyboard users get the same out).
  // Skipping counts as played: a later refetch must snap, not retype.
  const skipTyping = useCallback(() => {
    playedKeyRef.current = typeKey;
    setTypeChars(typeTotal);
  }, [typeKey, typeTotal]);
  useEffect(() => {
    if (!viewLoaded || typeDone) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key !== "Enter" && e.key !== " ") return;
      const t = e.target;
      if (
        t instanceof HTMLInputElement ||
        t instanceof HTMLTextAreaElement ||
        t instanceof HTMLButtonElement ||
        (t instanceof HTMLElement && t.isContentEditable)
      ) {
        return;
      }
      e.preventDefault();
      skipTyping();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [viewLoaded, typeDone, skipTyping]);
  const [detailGame, setDetailGame] = useState<GameView | null>(null);
  const [refreshTick, setRefreshTick] = useState(0);
  const prevTokenRef = useRef<string | undefined>(undefined);

  // ── steam identity state ────────────────────────────────────────────────────
  const [steamIdentity, setSteamIdentity] = useState<SteamIdentity | null>(
    null,
  );
  const [steamPrivate, setSteamPrivate] = useState(false);
  const [steamError, setSteamError] = useState<string | null>(null);

  const refresh = useCallback(() => setRefreshTick((t) => t + 1), []);

  // ── steam identity effect — runs once on mount (per token) ──────────────────
  useEffect(() => {
    let cancelled = false;

    const fragment = consumeReturnFragment();

    if (fragment === null) {
      // No return fragment — restore from localStorage
      const stored = loadIdentity();
      if (!cancelled) setSteamIdentity(stored);
      return;
    }

    if ("error" in fragment) {
      if (!cancelled) setSteamError(fragment.error);
      return;
    }

    // Steam OpenID return with steamid + persona
    const { steamid, persona } = fragment;

    async function fetchOwned() {
      try {
        const result = await steamOwnedForLink(token!, steamid);
        if (cancelled) return;
        const owned = result === "private" ? [] : result;
        const id: SteamIdentity = {
          steamid,
          persona,
          owned,
          fetched_at: Date.now(),
        };
        saveIdentity(id);
        setSteamIdentity(id);
        if (result === "private") setSteamPrivate(true);
      } catch {
        if (!cancelled) setSteamError("steam_unreachable");
      }
    }

    void fetchOwned();
    return () => {
      cancelled = true;
    };
  }, [token]); // eslint-disable-line react-hooks/exhaustive-deps

  // ── link load effect ────────────────────────────────────────────────────────
  useEffect(() => {
    let cancelled = false;

    async function load() {
      if (!token) {
        setView({ kind: "not-found" });
        return;
      }
      // Hard reset to the spinner only on token change (initial load / navigation).
      // refreshTick bumps refetch behind the current view — no blank flash mid-claim.
      // (This branch ONLY — a reset on every refreshTick refetch would replay the
      // entrance after each claim, the exact churn playedKeyRef exists to prevent.)
      if (prevTokenRef.current !== token) {
        prevTokenRef.current = token;
        setView({ kind: "loading" });
        // a different link is a different page: its entrance hasn't played
        playedKeyRef.current = null;
        setTypeChars(0);
      }
      try {
        const data = await fetchLink(token);
        if (!cancelled) setView({ kind: "loaded", data });
      } catch (error) {
        if (cancelled) return;
        if (error instanceof NotFound) {
          setView({ kind: "not-found" });
        } else {
          // Transient failure — keep stale loaded data if we have it
          setView((v) => (v.kind === "loaded" ? v : { kind: "error" }));
        }
      }
    }

    void load();
    return () => {
      cancelled = true;
    };
  }, [token, refreshTick]);

  // Derived owned set for GameGrid
  const ownedSet = useMemo(
    () => new Set<number>(steamIdentity?.owned ?? []),
    [steamIdentity],
  );

  // ── The shelf shuffle (ben, 2026-07-09) ─────────────────────────────────────
  // Games render in a random order so each visit rummages the trove afresh —
  // but the order is locked per visit (ranks assigned once, then reused), so
  // a claim-refresh never rearranges the shelf someone is standing in front
  // of. New ids appearing mid-visit sort after the shuffled ones.
  const shuffleRanksRef = useRef<Map<string, number> | null>(null);
  const shelfGames = useMemo(() => {
    if (view.kind !== "loaded") return [];
    const games = view.data.games;
    if (shuffleRanksRef.current === null) {
      const ids = games.map((g) => g.id);
      for (let i = ids.length - 1; i > 0; i--) {
        const j = Math.floor(Math.random() * (i + 1));
        const tmp = ids[i]!;
        ids[i] = ids[j]!;
        ids[j] = tmp;
      }
      shuffleRanksRef.current = new Map(ids.map((id, pos) => [id, pos]));
    }
    const ranks = shuffleRanksRef.current;
    return [...games].sort(
      (a, b) =>
        (ranks.get(a.id) ?? Number.MAX_SAFE_INTEGER) -
        (ranks.get(b.id) ?? Number.MAX_SAFE_INTEGER),
    );
  }, [view]);

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
          <h1 className="text-2xl font-bold">couldn&apos;t load this page</h1>
          <p className="mt-2 text-dust">
            something hiccuped on our end — the link is fine
          </p>
          <button
            type="button"
            onClick={refresh}
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
          <h1 className="text-2xl font-bold">link not found</h1>
          <p className="mt-2 text-dust">ask your friend for a new link ♡</p>
        </main>
      </div>
    );
  }

  const { data } = view;
  // Explicit server state — never inferred from side signals like games.length
  const exhausted = data.state === "exhausted";
  const dead = data.state === "revoked" || data.state === "expired";

  return (
    <div className="min-h-screen bg-room text-ink">
      <header className="border-b border-line">
        <div className="relative">
          <div
            aria-hidden="true"
            className="h-60 w-full"
            style={{
              backgroundImage: "url(/art/banner.png)",
              backgroundRepeat: "repeat-x",
              /* pin the banner's center (the chest) 200px from the right edge;
                 the scene tiles horizontally for wide viewports */
              backgroundPosition: "calc(100% + 824px) 62%",
              backgroundColor: "rgb(197,198,125)",
            }}
          />
          <div className="absolute inset-x-0 top-0 flex items-center justify-between px-6 py-3">
            <h1 className="font-logo wordmark-outline text-xl uppercase tracking-[0.03em]">
              bendobundles
            </h1>
            <span
              className="inline-flex items-center gap-2 font-pixel text-[0.8125rem] text-give-soft"
              aria-label={`${data.claims_used} of ${data.claims_allowed} claims used`}
            >
              {data.claims_allowed - data.claims_used > 0 ? (
                <>
                  <span className="claim-beacon" aria-hidden="true" />
                  {data.claims_allowed - data.claims_used} gift
                  {data.claims_allowed - data.claims_used === 1 ? "" : "s"}{" "}
                  waiting
                </>
              ) : (
                <span className="text-dust">all claimed</span>
              )}
            </span>
          </div>
          {/* ≤800px the box leaves the banner and drops into flow, still
              straddling the banner's bottom edge like a JRPG text box */}
          {/* onClick: tap-to-complete, the JRPG-dialog convention this box is
              homaging — a long note otherwise keeps the typeDone-gated steam
              controls inert for seconds with nothing to do but watch */}
          <div
            onClick={() => {
              if (!typeDone) skipTyping();
            }}
            className="absolute bottom-4 left-6 w-[34rem] max-w-[calc(100%-3rem)] rounded-xl border-[3px] border-pixel bg-floor px-5 py-3.5 [box-shadow:inset_0_0_0_3px_var(--color-floor),inset_0_0_0_5px_var(--color-pixel)] max-[800px]:relative max-[800px]:bottom-auto max-[800px]:left-auto max-[800px]:w-auto max-[800px]:max-w-none max-[800px]:-mt-8 max-[800px]:mx-7"
          >
            <button
              type="button"
              onClick={(e) => {
                // don't bubble into the box's tap-to-skip — one click must
                // not mean both "skip" and "replay" (the skip half would
                // paint a full-text frame before the restart)
                e.stopPropagation();
                setTypeKey((k) => k + 1);
              }}
              aria-label="replay the text"
              title="replay"
              className="font-pixel absolute top-2.5 right-3 text-sm text-dust-faint hover:text-ink"
            >
              &#8635;
            </button>
            <h2 className="min-h-7 text-xl leading-tight text-give-soft">
              {labelGr.slice(0, typeChars).join("")}
              {typeChars < bodyStart && <TwCursor />}
            </h2>
            <p className="mt-1.5 min-h-10 max-w-[60ch] text-sm text-ink-soft">
              {bodyGr.slice(0, Math.max(0, typeChars - bodyStart)).join("")}
              {typeChars >= bodyStart && typeChars < noteStart && <TwCursor />}
            </p>
            {/* ben's note — the personal beat. The container renders whenever a
                note exists (empty while the earlier beats type) so the box
                never grows mid-monologue. The cursor moves in at the boundary
                (>= noteStart, like the body's handoff) so it never blips out
                for a tick between beats. */}
            {giftNote !== "" && (
              <p className="mt-1.5 min-h-5 max-w-[60ch] text-sm italic text-give-soft">
                {typeChars > noteStart && (
                  <>&ldquo;{noteGr.slice(0, typeChars - noteStart).join("")}</>
                )}
                {typeChars >= noteStart && !typeDone && <TwCursor />}
                {typeDone && (
                  <>
                    &rdquo;{" "}
                    <span className="font-pixel not-italic text-xs text-dust">
                      &mdash; ben
                    </span>
                  </>
                )}
              </p>
            )}
            {steamIdentity !== null ? (
              <div
                className={`mt-2 flex items-center gap-2 transition-opacity duration-300 ${typeDone ? "opacity-100" : "pointer-events-none opacity-0"}`}
              >
                <span className="rounded bg-shelf px-2 py-1 text-xs text-ink-soft">
                  {steamIdentity.persona}
                </span>
                <button
                  type="button"
                  onClick={() => {
                    clearIdentity();
                    setSteamIdentity(null);
                    setSteamPrivate(false);
                    setSteamError(null);
                  }}
                  className="text-xs text-dust-faint hover:text-ink-soft"
                >
                  disconnect
                </button>
              </div>
            ) : (
              <button
                type="button"
                onClick={() => beginConnect(`/l/${token}`)}
                className={`font-pixel group mt-2 -mx-1 flex items-center gap-1.5 rounded px-1 py-0.5 text-sm text-ink hover:bg-shelf transition-opacity duration-300 ${typeDone ? "opacity-100" : "pointer-events-none opacity-0"}`}
              >
                <span aria-hidden="true" className="menu-cursor text-give">
                  &#9656;
                </span>
                connect to steam
                <span className="font-sans text-xs text-dust-faint">
                  — flags the games you already own
                </span>
              </button>
            )}
          </div>
        </div>
      </header>

      {/* your gifts — moved up under the banner (ben, 2026-07-09): the friend's
          claimed games sit right below the scene, not buried at the page bottom */}
      <ClaimsHistory claims={data.claims} />

      {/* Steam privacy notice — spec §4 wording verbatim */}
      {steamPrivate && (
        <p className="mx-6 mt-4 text-sm text-dust">
          couldn&apos;t read your library — check Steam&apos;s{" "}
          <em>game details</em> privacy setting
        </p>
      )}

      {/* Steam connect error */}
      {steamError !== null && (
        <p className="mx-6 mt-4 text-sm text-dust">
          {steamError === "verify_failed"
            ? "we couldn't verify your Steam account — try again"
            : "Steam is currently unavailable — try again later"}
        </p>
      )}

      {exhausted && (
        <div
          role="alert"
          className="mx-6 mt-4 rounded border border-amber-800 bg-amber-950 px-4 py-3 text-amber-200"
        >
          you&apos;ve used all your claims
        </div>
      )}

      {dead && (
        <div
          role="alert"
          className="mx-6 mt-4 rounded border border-red-800 bg-red-950 px-4 py-3 text-red-200"
        >
          this invite isn&apos;t active anymore — bug ben
        </div>
      )}

      {/* Grid: shown for exhausted or active (claiming lives in the detail modal,
          which respects link state); hidden for revoked/expired */}
      {!dead && (
        <GameGrid
          games={shelfGames}
          owned={ownedSet}
          onDetail={setDetailGame}
        />
      )}

      {detailGame !== null && token !== undefined && (
        <GameDetailModal
          mount="friend"
          token={token}
          game={detailGame}
          active={data.state === "active"}
          loadDetail={(gameId) => fetchGameDetail(token, gameId)}
          onClaim={(g) => {
            setDetailGame(null);
            setClaimingGame(g);
          }}
          onClose={() => setDetailGame(null)}
        />
      )}

      {claimingGame !== null && token !== undefined && (
        <ClaimDialog
          token={token}
          game={claimingGame}
          onClose={() => setClaimingGame(null)}
          onRefresh={refresh}
        />
      )}

      <CursorCompanion
        variant="critter"
        away={detailGame !== null || claimingGame !== null}
      />
    </div>
  );
}
