import { useEffect, useRef, useState } from "react";
import { claimGame, type ClaimResult, type GameView } from "../api";
import { motionOK } from "../motion";

// ── The claim celebration — THE CHAIN (ben's pick, 2026-07-09) ───────────────
// The unwrap is the product (PRODUCT.md #1), so a successful claim plays the
// full combo: the key unlocks the padlock (clunk, shackle pops, fragments) →
// the "✦ you are amazing ✦" fanfare slams in → the gifted panel reveals UNDER
// a full-viewport confetti rain (the confetti is the reveal's weather, so the
// key is never gated by it). Only runs when motion is AFFIRMATIVELY allowed
// (see motionOK): reduced-motion users — and jsdom tests — skip straight to
// the key. The boot concept graduated to the page-load BootScreen.
const CHAIN_KEY_MS = 2200;
const CHAIN_FANFARE_MS = 1800;
const CONFETTI_TAIL_MS = 2200;

interface ClaimDialogProps {
  token: string;
  game: GameView;
  onClose: () => void;
  onRefresh: () => void;
}

type Step =
  | "confirm"
  | "loading"
  | "celebrating"
  | "gifted"
  | "processing"
  | "refused"
  | "error";

// The per-step CASUAL-dismiss policy (Escape, backdrop click) — one place, so
// the surfaces can never drift: null = not dismissible (gifted protects the
// one-time URL; loading has a claim in flight), 'refresh' = a claim was
// consumed so dismissal must refetch, 'close' = plain close. The explicit
// close BUTTONS are not dismissal: gifted deliberately allows its button
// while blocking stray Escapes/clicks.
function dismissKindFor(step: Step): "close" | "refresh" | null {
  // celebrating already holds a gifted result — dismissal would lose the
  // one-time URL, so it's protected exactly like gifted.
  if (step === "gifted" || step === "loading" || step === "celebrating")
    return null;
  if (step === "processing" || step === "refused") return "refresh";
  return "close";
}

export function ClaimDialog({
  token,
  game,
  onClose,
  onRefresh,
}: ClaimDialogProps) {
  const [step, setStep] = useState<Step>("confirm");
  const [result, setResult] = useState<ClaimResult | null>(null);
  const containerRef = useRef<HTMLDivElement>(null);
  const [copied, setCopied] = useState(false);
  // Hard re-entry guard for the one-shot claim POST. Unmounting the confirm
  // button on setStep('loading') is an implementation detail, and checking
  // `step` in the handler doesn't help either — a double-click / Enter-repeat /
  // AT-synthesized second activation can land in the same tick, before React
  // re-renders, so both closures still see step === 'confirm'. A ref flips
  // synchronously; the second activation sees it and bails. Never reset: the
  // dialog never returns to 'confirm', so one claim per mount is the contract.
  const claimFiredRef = useRef(false);

  // The confetti rain persists OVER the revealed gifted panel (the chain's
  // final stage); it self-clears after its fall.
  const [confettiTail, setConfettiTail] = useState(false);

  // Focus the dialog on open
  useEffect(() => {
    containerRef.current?.focus();
  }, []);

  // Escape key — same policy as the backdrop, via dismissKindFor
  useEffect(() => {
    const handleKeyDown = (e: KeyboardEvent) => {
      if (e.key !== "Escape") return;
      const kind = dismissKindFor(step);
      if (kind === null) return;
      if (kind === "refresh") onRefresh();
      onClose();
    };
    document.addEventListener("keydown", handleKeyDown);
    return () => document.removeEventListener("keydown", handleKeyDown);
  }, [step, onClose, onRefresh]);

  const handleConfirm = async () => {
    if (claimFiredRef.current) return;
    claimFiredRef.current = true;
    setStep("loading");
    const r = await claimGame(token, game.id);
    setResult(r);
    if (r.kind === "gifted") setStep(motionOK() ? "celebrating" : "gifted");
    else if (r.kind === "processing") setStep("processing");
    else if (r.kind === "refused") setStep("refused");
    else setStep("error");
  };

  const handleCloseWithRefresh = () => {
    onRefresh();
    onClose();
  };

  const handleCopy = async (url: string) => {
    try {
      await navigator.clipboard.writeText(url);
      setCopied(true);
      setTimeout(() => setCopied(false), 2000);
    } catch {
      // clipboard unavailable — text is still selectable
    }
  };

  return (
    <>
      {/* Backdrop — pure dim layer. The dialog container below is a full-viewport
          sibling stacked ABOVE it, so clicks on the dimmed area land on the
          container, never here — the click-outside handler lives there. */}
      <div className="fixed inset-0 z-40 bg-black/60" aria-hidden="true" />

      {/* Dialog panel — click-outside-to-dismiss: a click whose target is the
          container itself (not the panel or its children) is a backdrop click,
          routed through the same dismissKindFor policy as Escape */}
      <div
        ref={containerRef}
        role="dialog"
        aria-modal="true"
        aria-label={`claim ${game.title}`}
        tabIndex={-1}
        className="fixed inset-0 z-50 flex items-center justify-center p-4 outline-none"
        onClick={(e) => {
          if (e.target !== e.currentTarget) return;
          const kind = dismissKindFor(step);
          if (kind === null) return;
          if (kind === "refresh") onRefresh();
          onClose();
        }}
      >
        <div className="dialog-bezel w-full max-w-md rounded-xl bg-floor p-6">
          {step === "confirm" && (
            <>
              <h2 className="text-lg font-semibold">
                claim <span className="text-give-soft">{game.title}</span>?
              </h2>
              <p className="mt-2 text-sm text-dust">
                this uses 1 of your claims
              </p>
              <div className="mt-6 flex gap-3 justify-end">
                <button
                  type="button"
                  onClick={onClose}
                  className="rounded px-4 py-2 text-sm text-dust transition-colors hover:text-ink-soft focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-pixel focus-visible:ring-offset-2 focus-visible:ring-offset-floor"
                >
                  cancel
                </button>
                <button
                  type="button"
                  onClick={() => {
                    void handleConfirm();
                  }}
                  className="rounded bg-give px-4 py-2 text-sm font-medium text-give-ink transition hover:bg-give-bright focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-pixel focus-visible:ring-offset-2 focus-visible:ring-offset-floor active:scale-[0.98]"
                >
                  confirm
                </button>
              </div>
            </>
          )}

          {step === "loading" && (
            <p className="text-center text-dust py-4">claiming...</p>
          )}

          {step === "celebrating" && (
            <ClaimCelebration
              onDone={() => {
                setStep("gifted");
                setConfettiTail(true);
              }}
            />
          )}

          {step === "gifted" && result?.kind === "gifted" && (
            <>
              <h2 className="text-lg font-semibold text-give-soft">
                it&apos;s yours! ♡
              </h2>
              <p className="mt-1 text-xs text-dust-faint">
                this link is one-time — redeem it to YOUR humble account
              </p>
              <div className="mt-4 rounded bg-shelf p-3">
                <a
                  href={result.gift_url}
                  target="_blank"
                  rel="noreferrer"
                  className="block break-all text-sm text-give-soft underline hover:text-give"
                >
                  {result.gift_url}
                </a>
              </div>
              <div className="mt-3 flex gap-2">
                <button
                  type="button"
                  onClick={() => {
                    void handleCopy(result.gift_url);
                  }}
                  className="flex-1 rounded bg-shelf px-3 py-2 text-sm transition-colors hover:bg-control focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-pixel focus-visible:ring-offset-2 focus-visible:ring-offset-floor"
                >
                  {copied ? "copied ✓" : "copy link"}
                </button>
                <a
                  href={result.gift_url}
                  target="_blank"
                  rel="noreferrer"
                  className="flex-1 rounded bg-give px-3 py-2 text-sm text-center text-give-ink transition-colors hover:bg-give-bright focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-pixel focus-visible:ring-offset-2 focus-visible:ring-offset-floor"
                >
                  open on humble
                </a>
              </div>
              <p className="mt-4 text-xs text-dust-faint">
                keys may be region-locked
              </p>
              <div className="mt-4 flex justify-end">
                <button
                  type="button"
                  onClick={handleCloseWithRefresh}
                  className="rounded px-4 py-2 text-sm text-dust transition-colors hover:text-ink-soft focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-pixel focus-visible:ring-offset-2 focus-visible:ring-offset-floor"
                >
                  close
                </button>
              </div>
            </>
          )}

          {step === "processing" && result?.kind === "processing" && (
            <>
              <h2 className="text-lg font-semibold text-amber-800">
                processing
              </h2>
              <p className="mt-2 text-sm text-ink-soft">{result.message}</p>
              <p className="mt-1 text-sm text-dust-faint">
                check this page later
              </p>
              <div className="mt-6 flex justify-end">
                <button
                  type="button"
                  onClick={handleCloseWithRefresh}
                  className="rounded px-4 py-2 text-sm text-dust transition-colors hover:text-ink-soft focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-pixel focus-visible:ring-offset-2 focus-visible:ring-offset-floor"
                >
                  close
                </button>
              </div>
            </>
          )}

          {step === "refused" && result?.kind === "refused" && (
            <>
              <h2 className="text-lg font-semibold text-red-700">whoops</h2>
              <p className="mt-2 text-sm text-ink-soft">{result.message}</p>
              <div className="mt-6 flex justify-end">
                <button
                  type="button"
                  onClick={handleCloseWithRefresh}
                  className="rounded px-4 py-2 text-sm text-dust transition-colors hover:text-ink-soft focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-pixel focus-visible:ring-offset-2 focus-visible:ring-offset-floor"
                >
                  close
                </button>
              </div>
            </>
          )}

          {step === "error" && result?.kind === "error" && (
            <>
              <h2 className="text-lg font-semibold text-red-700">uh oh</h2>
              <p className="mt-2 text-sm text-ink-soft">{result.message}</p>
              <div className="mt-6 flex justify-end">
                <button
                  type="button"
                  onClick={onClose}
                  className="rounded px-4 py-2 text-sm text-dust transition-colors hover:text-ink-soft focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-pixel focus-visible:ring-offset-2 focus-visible:ring-offset-floor"
                >
                  close
                </button>
              </div>
            </>
          )}
        </div>
        {confettiTail && <ConfettiSky onDone={() => setConfettiTail(false)} />}
      </div>
    </>
  );
}

// ── ClaimCelebration — THE CHAIN: key → fanfare. The confetti tail is
// rendered by the dialog itself so the gifted panel reveals UNDER the rain.
// Decorative throughout (aria-hidden) — the reveal that matters is the
// gifted step.

const CONFETTI_COLORS = [
  "--color-give",
  "--color-give-bright",
  "--color-hash-mustard",
  "--color-hash-moss",
  "--color-hash-rust",
  "--color-hash-heather",
] as const;

function ClaimCelebration({ onDone }: { onDone: () => void }) {
  const [stage, setStage] = useState<"key" | "fanfare">("key");
  useEffect(() => {
    const t1 = setTimeout(() => setStage("fanfare"), CHAIN_KEY_MS);
    const t2 = setTimeout(onDone, CHAIN_KEY_MS + CHAIN_FANFARE_MS);
    return () => {
      clearTimeout(t1);
      clearTimeout(t2);
    };
    // mount-once: the chain runs exactly once per celebration
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  if (stage === "fanfare") {
    const stars = [
      { top: 10, left: 14, delay: 150 },
      { top: 10, right: 14, delay: 320 },
      { bottom: 40, left: 22, delay: 480 },
      { bottom: 40, right: 22, delay: 640 },
    ];
    return (
      <div className="celebrate cel-fanfare" aria-hidden="true">
        <span className="cel-flash" />
        {stars.map((s, i) => (
          <span
            key={i}
            className="cel-star"
            style={{ ...s, animationDelay: `${s.delay}ms` }}
          >
            ✦
          </span>
        ))}
        <p className="cel-banner">✦ you are amazing ✦</p>
        <p className="cel-afterline">+1 treasure obtained</p>
        {Array.from({ length: 24 }, (_, i) => (
          <span
            key={`s${i}`}
            className="cel-sparkrain"
            style={{
              left: `${4 + ((i * 71) % 92)}%`,
              animationDelay: `${((i * 97) % 420) + (i >= 12 ? 500 : 0)}ms`,
            }}
          />
        ))}
      </div>
    );
  }

  // stage === "key" — the chain opens with the unlocking
  return (
    <div className="celebrate cel-keyturn" aria-hidden="true">
      <span className="cel-white-blink" />
      <span className="cel-ring-flash" />
      <span className="cel-lock">
        <span className="cel-shackle" />
        <span className="cel-lock-hole" />
        {Array.from({ length: 8 }, (_, i) => {
          // deterministic burst — padlock fragments fly on the clunk
          const a = ((i * 137.5) % 360) * (Math.PI / 180);
          const dist = 44 + ((i * 53) % 30);
          return (
            <span
              key={i}
              className="cel-frag"
              style={{
                ["--dx" as string]: `${Math.round(Math.cos(a) * dist)}px`,
                ["--dy" as string]: `${Math.round(Math.sin(a) * dist) - 12}px`,
                ["--rot" as string]: `${(i * 77) % 360}deg`,
              }}
            />
          );
        })}
      </span>
      <span className="cel-key">
        <span className="cel-key-bow" />
        <span className="cel-key-teeth" />
        {Array.from({ length: 3 }, (_, i) => (
          <span
            key={i}
            className="cel-key-spark"
            style={{
              left: `${-30 - i * 12}px`,
              animationDelay: `${i * 90}ms`,
            }}
          />
        ))}
      </span>
    </div>
  );
}

// ── ConfettiSky — the chain's final stage: a full-viewport pixel rain that
// falls OVER the revealed gifted panel, then clears itself. ──────────────────
function ConfettiSky({ onDone }: { onDone: () => void }) {
  useEffect(() => {
    const t = setTimeout(onDone, CONFETTI_TAIL_MS);
    return () => clearTimeout(t);
    // mount-once: one rain per reveal
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  return (
    <span className="cel-sky" aria-hidden="true">
      {Array.from({ length: 64 }, (_, i) => {
        // deterministic golden-angle spread, two waves, viewport-scale
        const a = ((i * 137.5) % 360) * (Math.PI / 180);
        const dist = 120 + ((i * 53) % 300);
        return (
          <span
            key={i}
            className={`cel-piece${i % 7 === 0 ? " cel-piece--streamer" : ""}`}
            style={{
              ["--dx" as string]: `${Math.round(Math.cos(a) * dist)}px`,
              ["--dy" as string]: `${Math.round(Math.sin(a) * dist * 0.7) - 60}px`,
              ["--rot" as string]: `${(i * 77) % 360}deg`,
              animationDelay: `${((i * 23) % 180) + (i >= 40 ? 380 : 0)}ms`,
              background: `var(${CONFETTI_COLORS[i % CONFETTI_COLORS.length] ?? "--color-give"})`,
            }}
          />
        );
      })}
    </span>
  );
}
