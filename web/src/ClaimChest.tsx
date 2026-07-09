import { useEffect, useRef } from "react";

// ── The claim chest — a Game Boy game (overdrive, ben's live pick 2026-07-09) ──
// The gift-unwrap made into a mini-game that takes over the LCD: opaque Floor
// screen, olive-LCD palette, pixel font, a segmented block meter, and a chunky
// pixel treasure chest that shakes as you mash and bursts open at full charge.
// Purely presentational — charge/phase/timers live in GameDetailModal (the footer
// claim button is the masher). The look is the arcade skin — scanlines, tall
// gauge, big burgundy prompt — ben's pick (2026-07-09) over dungeon and cozy.
// Every motion has a reduced-motion bypass.

const prefersReducedMotion = (): boolean =>
  typeof window !== "undefined" &&
  window.matchMedia?.("(prefers-reduced-motion: reduce)").matches === true;

const SEGMENTS = 12;

type ClaimChestProps = {
  /** 0–100 charge, drained in the parent, filled by mashing. */
  charge: number;
  phase: "charging" | "bursting";
  /** Bumped on every mash so the shake retriggers. */
  pulse: number;
  onMash: () => void;
  onCancel: () => void;
};

export function ClaimChest({
  charge,
  phase,
  pulse,
  onMash,
  onCancel,
}: ClaimChestProps) {
  const artRef = useRef<HTMLDivElement>(null);

  // A quick shake on each mash — retriggers reliably via the Web Animations API
  // (re-running the same CSS animation name would not). Skipped on first render
  // (pulse 0) and under reduced motion. Amplitude grows as the chest fills.
  useEffect(() => {
    if (pulse === 0 || phase !== "charging") return;
    if (prefersReducedMotion()) return;
    const el = artRef.current;
    if (el === null) return;
    const amp = 2 + (charge / 100) * 5;
    // Optional call: feature-detects the Web Animations API (absent in jsdom).
    el.animate?.(
      [
        { transform: "translate3d(0,0,0) rotate(0deg)" },
        { transform: `translate3d(-${amp}px, 1px, 0) rotate(-${amp / 2}deg)` },
        { transform: `translate3d(${amp}px, -1px, 0) rotate(${amp / 2}deg)` },
        { transform: "translate3d(0,0,0) rotate(0deg)" },
      ],
      { duration: 150, easing: "ease-out" },
    );
    // charge omitted: a shake per mash (pulse) reading latest charge for amplitude,
    // without re-firing on drain ticks.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [pulse, phase]);

  const pct = Math.round(charge);
  const bursting = phase === "bursting";
  const litSegments = Math.round((charge / 100) * SEGMENTS);

  return (
    <div className="claim-game">
      <div className="cg-frame">
        <p className="cg-title">{bursting ? "opened!" : "open the chest"}</p>

        <div
          ref={artRef}
          className={`cg-chest${bursting ? " is-burst" : ""}`}
          style={{ ["--cc-charge" as string]: pct / 100 }}
          onClick={bursting ? undefined : onMash}
          role="button"
          tabIndex={-1}
          aria-hidden="true"
        >
          <span className="cc-glow" />
          <span className="cc-lid" />
          <span className="cc-body" />
          <span className="cc-band" />
          <span className="cc-lock" />
          <span className="cc-spark cc-spark-1" />
          <span className="cc-spark cc-spark-2" />
          <span className="cc-spark cc-spark-3" />
          <span className="cc-spark cc-spark-4" />
        </div>

        {bursting ? (
          <p className="cg-win">it&apos;s yours ♡</p>
        ) : (
          <>
            <div
              className="cg-meter"
              role="progressbar"
              aria-valuenow={pct}
              aria-valuemin={0}
              aria-valuemax={100}
              aria-label="claim charge — mash the claim button"
            >
              {Array.from({ length: SEGMENTS }, (_, i) => (
                <span
                  key={i}
                  className={`cg-seg${i < litSegments ? " is-on" : ""}`}
                />
              ))}
            </div>
            <p className="cg-prompt">
              mash <span className="cg-a">Ⓐ</span> to claim
            </p>
            <button type="button" className="cg-cancel" onClick={onCancel}>
              never mind
            </button>
          </>
        )}
      </div>
    </div>
  );
}
