import { useState, useEffect, useRef } from "react";
import type {
  GameView,
  AdminGame,
  SteamDetailBlob,
  SelfClaimResult,
} from "./api";
import { MediaHeader } from "./MediaHeader";
import { ClaimChest } from "./ClaimChest";
import { titleColorClass, titleHueVar } from "./titleColor";
import { gameDetailCache } from "./gameDetailCache";

// ── Claim-chest tuning (friend mount; overdrive 2026-07-09) ───────────────────
// Generous fill so steady taps win without frantic mashing; the drain forces
// active commitment. Draining to zero IS the confirm/timeout (ben, 2026-07-09):
// the game opens with a seed of charge, and if you stop mashing it drains out
// and cancels back to the details — there's no separate clock.
const CLAIM_CHARGE_PER_MASH = 18;
const CLAIM_DRAIN_PER_SEC = 15;
const CLAIM_START_CHARGE = 30; // seed on open — a beat before the drain can cancel
const CLAIM_BURST_MS = 750;

// ── Focus trap (issue #61 acceptance) ─────────────────────────────────────────
// Computed at keydown time: the focusable set is dynamic (carousel slides are
// inert per-index, buttons appear/disappear), so a cached list would trap focus
// into hidden slides.

const FOCUSABLE_SELECTOR =
  'button, [href], input, select, textarea, video, [tabindex]:not([tabindex="-1"])';

function dialogFocusables(container: HTMLElement): HTMLElement[] {
  return Array.from(
    container.querySelectorAll<HTMLElement>(FOCUSABLE_SELECTOR),
  ).filter(
    (el) =>
      el.closest('[inert], [aria-hidden="true"]') === null &&
      !el.hasAttribute("disabled"),
  );
}

// ── Status badge — mirrors Catalog's mapping ──────────────────────────────────

function statusBadgeClass(status: string): string {
  switch (status) {
    case "available":
      return "bg-green-700 text-green-100";
    case "pending":
      return "bg-amber-700 text-amber-100";
    case "gifted":
      return "bg-give text-give-ink";
    case "ben_redeemed":
      return "bg-slate-600 text-slate-100";
    case "expired":
      return "bg-red-700 text-red-100";
    default:
      return "bg-control text-ink";
  }
}

// ── Review sentiment → color spectrum ─────────────────────────────────────────
// Steam's review descriptor placed on a clearly-positive → clearly-negative
// arc: green → yellow-green → amber → orange → red. Deep hues so they read on
// the light room (DESIGN.md, The Light Text Rule). One accent role (semantic
// status), not the room — this is a signal, not decoration.

const REVIEW_SPECTRUM: Record<string, string> = {
  "Overwhelmingly Positive": "oklch(52% 0.13 150)",
  "Very Positive": "oklch(54% 0.13 144)",
  Positive: "oklch(57% 0.12 138)",
  "Mostly Positive": "oklch(60% 0.12 118)",
  Mixed: "oklch(60% 0.12 85)",
  "Mostly Negative": "oklch(56% 0.14 55)",
  Negative: "oklch(53% 0.16 33)",
  "Very Negative": "oklch(52% 0.17 30)",
  "Overwhelmingly Negative": "oklch(48% 0.18 25)",
};

/** The sentiment's spectrum color (deep, readable on light). Falls back to
    Dust for any descriptor Steam adds that we don't map yet. */
function reviewHue(desc: string): string {
  return REVIEW_SPECTRUM[desc] ?? "oklch(43% 0.06 116)";
}

// ── Props (discriminated union) ───────────────────────────────────────────────

type FriendMountProps = {
  mount: "friend";
  /** Link token — used as part of the per-session cache key. */
  token: string;
  game: GameView;
  /** Honors the grid's disabled rules — disables the claim button when false. */
  active: boolean;
  onClaim: (game: GameView) => void;
};

type AdminMountProps = {
  mount: "admin";
  game: AdminGame;
  /** Passthrough from Catalog's arm/confirm state machine. */
  armedId: string | null;
  claiming: string | null;
  onSelfClaim: (game: AdminGame) => void;
  adminSteamId: string | null;
  selfClaimResult: { gameId: string; r: SelfClaimResult } | null;
};

export type GameDetailModalProps = (FriendMountProps | AdminMountProps) & {
  onClose: () => void;
  /**
   * Caller-supplied fetch function for the Steam detail blob.
   * Friend mount: `(id) => fetchGameDetail(token, id)`
   * Admin mount: `(id) => withAuth(() => adminGameDetail(id), navigate)`
   *
   * withAuth returns a forever-pending promise on 401 (navigation is already
   * in flight) — the modal stays in the "loading" phase and unmounts naturally
   * when the router transitions. Never shows an error on the 401 path.
   */
  loadDetail: (gameId: string) => Promise<{ steam: SteamDetailBlob | null }>;
};

// ── Load state ────────────────────────────────────────────────────────────────

type LoadState =
  | { phase: "loading" }
  | { phase: "error" }
  | { phase: "loaded"; steam: SteamDetailBlob | null };

// ── Component ─────────────────────────────────────────────────────────────────

export function GameDetailModal(props: GameDetailModalProps) {
  const { onClose, loadDetail } = props;
  const game = props.game;
  const mount = props.mount;
  // Extract token early so the useEffect dep array is stable
  const token = props.mount === "friend" ? props.token : null;

  const [loadState, setLoadState] = useState<LoadState>({ phase: "loading" });
  const [retryKey, setRetryKey] = useState(0);

  // ── Claim-chest mini-game (friend mount) ──────────────────────────────────
  const [claimPhase, setClaimPhase] = useState<
    "idle" | "charging" | "bursting"
  >("idle");
  const [claimCharge, setClaimCharge] = useState(0);
  const [claimPulse, setClaimPulse] = useState(0);
  const claimChargeRef = useRef(0);
  const claimRafRef = useRef<number | null>(null);
  const claimDoneRef = useRef(false);

  const containerRef = useRef<HTMLDivElement>(null);

  // ── Focus on open (a11y) ──────────────────────────────────────────────────

  useEffect(() => {
    containerRef.current?.focus();
  }, []);

  // ── Load detail data ──────────────────────────────────────────────────────

  useEffect(() => {
    let cancelled = false;
    const gameId = game.id;
    const cKey =
      token !== null ? `friend:${token}:${gameId}` : `admin:${gameId}`;

    async function doLoad() {
      // Cache hit — serve immediately (skip on retry so stale data is overwritten)
      if (retryKey === 0 && gameDetailCache.has(cKey)) {
        if (!cancelled) {
          setLoadState({
            phase: "loaded",
            steam: gameDetailCache.get(cKey) ?? null,
          });
        }
        return;
      }
      if (!cancelled) setLoadState({ phase: "loading" });
      try {
        const res = await loadDetail(gameId);
        if (!cancelled) {
          gameDetailCache.set(cKey, res.steam);
          setLoadState({ phase: "loaded", steam: res.steam });
        }
      } catch {
        if (!cancelled) setLoadState({ phase: "error" });
      }
    }

    void doLoad();
    return () => {
      cancelled = true;
    };
    // retryKey bumps a refetch without changing the other deps.
    // loadDetail is the caller-supplied loader — changes when scope changes.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [game.id, mount, token, loadDetail, retryKey]);

  // ── Escape key ────────────────────────────────────────────────────────────

  useEffect(() => {
    const handleKeyDown = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
    };
    document.addEventListener("keydown", handleKeyDown);
    return () => document.removeEventListener("keydown", handleKeyDown);
  }, [onClose]);

  // ── Focus trap: Tab/Shift+Tab wrap inside the dialog ──────────────────────
  // (Escape stays the document listener above; the carousel's Arrow handling
  // lives on the carousel region — Tab is the only key this handler owns.)

  const handleTrapKeyDown = (e: React.KeyboardEvent) => {
    if (e.key !== "Tab") return;
    const container = containerRef.current;
    if (container === null) return;
    const els = dialogFocusables(container);
    if (els.length === 0) {
      e.preventDefault(); // nowhere to go — focus stays on the container
      return;
    }
    const first = els[0];
    const last = els[els.length - 1];
    if (first === undefined || last === undefined) return;
    const active = document.activeElement;
    if (e.shiftKey) {
      if (active === first || active === container) {
        e.preventDefault();
        last.focus();
      }
    } else if (active === last || active === container) {
      e.preventDefault();
      first.focus();
    }
  };

  // ── Claim-chest: start / mash / cancel ────────────────────────────────────

  function startClaim() {
    if (claimPhase !== "idle") return;
    claimChargeRef.current = CLAIM_START_CHARGE;
    claimDoneRef.current = false;
    setClaimCharge(CLAIM_START_CHARGE);
    setClaimPulse(0);
    setClaimPhase("charging");
  }

  function mashClaim() {
    if (claimPhase !== "charging" || claimDoneRef.current) return;
    const next = Math.min(100, claimChargeRef.current + CLAIM_CHARGE_PER_MASH);
    claimChargeRef.current = next;
    setClaimCharge(next);
    setClaimPulse((p) => p + 1);
    if (next >= 100) {
      claimDoneRef.current = true;
      setClaimPhase("bursting");
    }
  }

  function cancelClaim() {
    claimDoneRef.current = true;
    claimChargeRef.current = 0;
    setClaimCharge(0);
    setClaimPhase("idle");
  }

  // Charge drains while charging — stop mashing and you lose ground. Draining
  // all the way to zero IS the timeout: the game gives up and fades back.
  useEffect(() => {
    if (claimPhase !== "charging") return;
    let last: number | null = null;
    function tick(ts: number) {
      if (last === null) last = ts;
      const dt = (ts - last) / 1000;
      last = ts;
      const next = Math.max(
        0,
        claimChargeRef.current - CLAIM_DRAIN_PER_SEC * dt,
      );
      claimChargeRef.current = next;
      setClaimCharge(next);
      if (next <= 0) {
        // drained out — that's the cancel; don't schedule another frame
        cancelClaim();
        return;
      }
      claimRafRef.current = requestAnimationFrame(tick);
    }
    claimRafRef.current = requestAnimationFrame(tick);
    return () => {
      if (claimRafRef.current !== null)
        cancelAnimationFrame(claimRafRef.current);
    };
    // cancelClaim is stable for this component instance.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [claimPhase]);

  // Burst → fire the real claim (the parent runs the reveal ceremony).
  useEffect(() => {
    if (claimPhase !== "bursting") return;
    const t = setTimeout(() => {
      claimChargeRef.current = 0;
      setClaimCharge(0);
      setClaimPhase("idle");
      if (props.mount === "friend") props.onClaim(props.game);
    }, CLAIM_BURST_MS);
    return () => clearTimeout(t);
    // props.onClaim is stable from the parent for the modal's lifetime.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [claimPhase]);

  // ── Render ────────────────────────────────────────────────────────────────

  return (
    <>
      {/* Backdrop */}
      <div className="fixed inset-0 z-40 bg-black/60" aria-hidden="true" />

      {/* Dialog container — backdrop click closes; tabIndex=-1 + ref for focus management */}
      <div
        ref={containerRef}
        tabIndex={-1}
        role="dialog"
        aria-modal="true"
        aria-label={game.title}
        className="fixed inset-0 z-50 flex items-center justify-center p-4 outline-none"
        onClick={(e) => {
          if (e.target === e.currentTarget) onClose();
        }}
        onKeyDown={handleTrapKeyDown}
      >
        <div className="handheld-shell flex max-h-[90vh] w-full max-w-2xl flex-col overflow-hidden">
          <div className="handheld-bezel">
            <div className="handheld-bezel-top">
              <span className="handheld-led" aria-hidden="true" />
              <span>battery</span>
              <span className="handheld-bezel-label">
                dot matrix with stereo sound
              </span>
              <button
                type="button"
                onClick={onClose}
                aria-label="close"
                className="handheld-close"
              >
                ✕
              </button>
            </div>
            <div className="handheld-screenwrap">
              <div
                className={`handheld-screen${props.mount === "friend" && claimPhase !== "idle" ? " is-dimmed" : ""}`}
              >
                {loadState.phase === "loading" && (
                  <p className="px-6 py-8 text-center text-dust">loading...</p>
                )}

                {loadState.phase === "error" && (
                  <div className="px-6 py-8 text-center">
                    <p className="text-dust">couldn&apos;t load details</p>
                    <button
                      type="button"
                      onClick={() => setRetryKey((k) => k + 1)}
                      className="mt-2 text-sm text-give-soft hover:text-give"
                    >
                      retry
                    </button>
                  </div>
                )}

                {loadState.phase === "loaded" && loadState.steam === null && (
                  <div className="space-y-4 px-6 py-6">
                    {game.artwork_url !== null ? (
                      <img
                        src={game.artwork_url}
                        alt={game.title}
                        className="w-full rounded object-cover"
                      />
                    ) : (
                      <div
                        className={`aspect-video w-full rounded ${titleColorClass(game.title)}`}
                        aria-hidden="true"
                      />
                    )}
                    <div className="flex flex-wrap gap-2">
                      <span className="rounded bg-shelf px-2 py-0.5 text-xs text-ink-soft">
                        {game.bundle}
                      </span>
                      <span className="rounded bg-shelf px-2 py-0.5 text-xs text-ink-soft">
                        {game.key_type}
                      </span>
                    </div>
                    <p className="text-sm text-dust">
                      no steam page for this one.
                    </p>
                  </div>
                )}

                {loadState.phase === "loaded" &&
                  loadState.steam !== null &&
                  (() => {
                    const { detail, overall, recent } = loadState.steam;
                    // all-time positive share — drives both the meter label and its bar width
                    const overallPct =
                      overall !== null
                        ? Math.round(
                            (overall.total_positive /
                              Math.max(1, overall.total_reviews)) *
                              100,
                          )
                        : 0;
                    return (
                      <div className="space-y-4 pb-2">
                        <MediaHeader
                          key={game.id}
                          title={game.title}
                          artworkUrl={game.artwork_url}
                          detail={detail}
                        />
                        {detail !== null && (
                          <div className="space-y-3 px-6">
                            <div className="flex items-baseline justify-between gap-3">
                              <p className="text-sm text-ink-soft">
                                {detail.developers.join(", ")}
                                {detail.publishers.length > 0 &&
                                  detail.publishers.join(",") !==
                                    detail.developers.join(",") && (
                                    <> · {detail.publishers.join(", ")}</>
                                  )}
                                {detail.release_date !== null && (
                                  <> · {detail.release_date}</>
                                )}
                              </p>
                              {game.steam_app_id !== null && (
                                <a
                                  href={`https://store.steampowered.com/app/${game.steam_app_id}`}
                                  target="_blank"
                                  rel="noreferrer"
                                  className="shrink-0 text-xs text-dust transition-colors hover:text-ink-soft focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-pixel focus-visible:ring-offset-2 focus-visible:ring-offset-room"
                                >
                                  steam ↗
                                </a>
                              )}
                            </div>
                            {detail.genres.length > 0 && (
                              <div className="flex flex-wrap gap-1.5">
                                {detail.genres.map((genre) => (
                                  <span
                                    key={genre}
                                    className="rounded px-2 py-0.5 text-xs"
                                    style={{
                                      background: `color-mix(in oklch, ${titleHueVar(genre)}, var(--color-floor) 86%)`,
                                      color: `color-mix(in oklch, ${titleHueVar(genre)}, oklch(20% 0.02 110) 30%)`,
                                      boxShadow: `inset 0 0 0 1px color-mix(in oklch, ${titleHueVar(genre)}, transparent 55%)`,
                                    }}
                                  >
                                    {genre}
                                  </span>
                                ))}
                              </div>
                            )}
                            <p className="text-sm leading-relaxed text-ink-soft">
                              {detail.short_description}
                            </p>
                          </div>
                        )}
                        {(overall !== null || recent !== null) && (
                          <div className="px-6">
                            {overall !== null ? (
                              <>
                                {/* Stats frame the bar — recent (left) vs total (right),
                                    both in the body face; the pixel rating sits below. */}
                                <div
                                  className="flex items-baseline justify-between text-xs text-dust"
                                  style={{ fontFamily: "var(--font-sans)" }}
                                >
                                  <span>
                                    {recent !== null
                                      ? `${recent.percent_positive}% of ${recent.count.toLocaleString()} recent`
                                      : ""}
                                  </span>
                                  <span>
                                    {overallPct}% of{" "}
                                    {overall.total_reviews.toLocaleString()}{" "}
                                    total
                                  </span>
                                </div>
                                <div
                                  className="mt-1.5 overflow-hidden rounded-full bg-shelf"
                                  style={{ height: "6px" }}
                                  role="img"
                                  aria-label={`${overall.desc}, ${overall.total_reviews.toLocaleString()} reviews`}
                                >
                                  <div
                                    style={{
                                      width: `${overallPct}%`,
                                      height: "100%",
                                      background: reviewHue(overall.desc),
                                    }}
                                  />
                                </div>
                                <p
                                  className="mt-1.5 text-xs font-medium"
                                  style={{
                                    color: `color-mix(in oklch, ${reviewHue(overall.desc)}, oklch(22% 0.03 110) 32%)`,
                                  }}
                                >
                                  {overall.desc}
                                </p>
                              </>
                            ) : (
                              recent !== null && (
                                <p
                                  className="text-xs text-dust"
                                  style={{ fontFamily: "var(--font-sans)" }}
                                >
                                  {recent.percent_positive}% of{" "}
                                  {recent.count.toLocaleString()} recent
                                </p>
                              )
                            )}
                          </div>
                        )}
                      </div>
                    );
                  })()}
              </div>
              <div className="handheld-grid" aria-hidden="true" />
              {props.mount === "friend" && claimPhase !== "idle" && (
                <ClaimChest
                  charge={claimCharge}
                  phase={claimPhase}
                  pulse={claimPulse}
                  onMash={mashClaim}
                  onCancel={cancelClaim}
                />
              )}
            </div>
          </div>
          <div className="handheld-controls flex items-center gap-4">
            <h2 className="handheld-title">{game.title}</h2>
            {props.mount === "friend" ? (
              <button
                type="button"
                disabled={!props.active || claimPhase === "bursting"}
                onClick={claimPhase === "idle" ? startClaim : mashClaim}
                className={`handheld-a${claimPhase === "charging" ? " is-mashing" : ""}`}
                aria-label={claimPhase === "idle" ? "claim" : "mash to claim"}
              >
                {claimPhase === "idle"
                  ? "claim"
                  : claimPhase === "bursting"
                    ? "♡"
                    : "mash!"}
              </button>
            ) : (
              (() => {
                const ap = props;
                const g = ap.game;
                const isArmed = ap.armedId === g.id;
                const isClaiming = ap.claiming === g.id;
                const r =
                  ap.selfClaimResult?.gameId === g.id
                    ? ap.selfClaimResult.r
                    : null;
                return (
                  <div className="flex min-w-0 flex-wrap items-center gap-3">
                    <span
                      className={`rounded px-2 py-0.5 text-xs font-medium ${statusBadgeClass(g.status)}`}
                    >
                      {g.status}
                    </span>
                    {g.status === "available" && (
                      <button
                        type="button"
                        disabled={isClaiming}
                        onClick={() => ap.onSelfClaim(g)}
                        className={`rounded px-3 py-1 text-xs ${
                          isArmed
                            ? "bg-emerald-700 text-emerald-100 hover:bg-emerald-600"
                            : "bg-control hover:bg-control-bright"
                        } disabled:opacity-50`}
                      >
                        {isArmed
                          ? g.owned_by_ben && ap.adminSteamId !== null
                            ? g.requires_choice
                              ? "you already own this on steam — spends 1 pick, sure?"
                              : "you already own this on steam — sure?"
                            : g.requires_choice
                              ? "confirm? spends 1 pick"
                              : "confirm?"
                          : isClaiming
                            ? "claiming…"
                            : "claim for me"}
                      </button>
                    )}
                    {r?.kind === "revealed" && (
                      <div className="flex items-center gap-2 text-sm">
                        <span className="select-all font-mono">{r.key}</span>
                        <button
                          type="button"
                          onClick={() =>
                            void navigator.clipboard.writeText(r.key)
                          }
                          className="rounded bg-control px-2 py-1 text-xs"
                        >
                          copy
                        </button>
                        {r.keyType === "steam" && (
                          <a
                            href={`https://store.steampowered.com/account/registerkey?key=${encodeURIComponent(r.key)}`}
                            target="_blank"
                            rel="noreferrer"
                            className="rounded bg-blue-700 px-2 py-1 text-xs text-blue-100"
                          >
                            redeem on steam
                          </a>
                        )}
                      </div>
                    )}
                    {r?.kind === "processing" && (
                      <p className="text-xs text-amber-800">
                        processing — check self-claims below
                      </p>
                    )}
                    {r?.kind === "refused" && (
                      <p className="text-xs text-red-700">{r.message}</p>
                    )}
                  </div>
                );
              })()
            )}
          </div>
          <div className="handheld-speaker" aria-hidden="true" />
        </div>
      </div>
    </>
  );
}
