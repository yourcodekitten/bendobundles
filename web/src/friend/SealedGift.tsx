import { useEffect, useRef, useState } from "react";
import { motionOK } from "../motion";

/** d/h/m/s, omitting leading units that are zero (90061 → "1d 1h 1m 1s"). */
function formatRemaining(total: number): string {
  const s = Math.max(0, total);
  const d = Math.floor(s / 86400);
  const h = Math.floor((s % 86400) / 3600);
  const m = Math.floor((s % 3600) / 60);
  const sec = s % 60;
  const parts: string[] = [];
  if (d > 0) parts.push(`${d}d`);
  if (d > 0 || h > 0) parts.push(`${h}h`);
  if (d > 0 || h > 0 || m > 0) parts.push(`${m}m`);
  parts.push(`${sec}s`);
  return parts.join(" ");
}

/**
 * The wrapped present: pre-unlock state of a sealed link (spec 2026-08-05 §3).
 *
 * The countdown runs from REMAINING seconds the server computed — never from
 * comparing wall clocks, so a skewed device can't fake midnight. At zero it
 * calls `onRefetch` and keeps rendering the sealed view: the component holds
 * no payload and structurally cannot self-unseal; whatever the next fetch
 * says decides what the friend sees. A still-sealed refetch reseeds
 * `unlocksInSeconds` and the clock simply continues — that IS the quiet
 * backoff, no error state exists. Tab-visibility refetches resync a tab
 * left open all day (family review).
 */
export function SealedGift({
  label,
  unlocksInSeconds,
  unlocksAt,
  onRefetch,
}: {
  label: string;
  unlocksInSeconds: number;
  unlocksAt: string;
  onRefetch: () => void;
}) {
  const [remaining, setRemaining] = useState(unlocksInSeconds);
  // One refetch per elapse; re-armed when a fresh remaining arrives.
  const firedRef = useRef(false);
  const onRefetchRef = useRef(onRefetch);
  onRefetchRef.current = onRefetch;

  useEffect(() => {
    setRemaining(unlocksInSeconds);
    firedRef.current = false;
  }, [unlocksInSeconds]);

  useEffect(() => {
    const iv = setInterval(() => {
      setRemaining((r) => Math.max(0, r - 1));
    }, 1000);
    return () => clearInterval(iv);
  }, []);

  // The zero-crossing fires the refetch from an EFFECT, not from inside the
  // state updater — updaters must stay pure (StrictMode replays them), and
  // the ref guard keeps it to one refetch per elapse (re-armed on reseed).
  useEffect(() => {
    if (remaining === 0 && !firedRef.current) {
      firedRef.current = true;
      onRefetchRef.current();
    }
  }, [remaining]);

  useEffect(() => {
    const onVisible = () => {
      if (document.visibilityState === "visible") onRefetchRef.current();
    };
    document.addEventListener("visibilitychange", onVisible);
    return () => document.removeEventListener("visibilitychange", onVisible);
  }, []);

  const opensAt = new Date(unlocksAt);
  const opensText = Number.isNaN(opensAt.getTime())
    ? ""
    : opensAt.toLocaleString(undefined, {
        weekday: "long",
        month: "long",
        day: "numeric",
        hour: "numeric",
        minute: "2-digit",
      });

  return (
    <div className="min-h-screen bg-room text-ink flex items-center justify-center p-6">
      <main className="text-center">
        {/* the present — inline pixel SVG in the four-shade palette, like the
            landing key charm (no asset pipeline). paper = floor/shelf shades,
            ribbon = the give pink, outline = pixel ink. */}
        <div
          className={motionOK() ? "gift-sway inline-block" : "inline-block"}
          aria-hidden="true"
        >
          <svg
            viewBox="0 0 32 30"
            className="mx-auto h-40 w-40 [image-rendering:pixelated]"
            shapeRendering="crispEdges"
          >
            {/* box */}
            <rect x="4" y="12" width="24" height="16" className="fill-floor" />
            {/* lid, one pixel proud on each side */}
            <rect x="3" y="8" width="26" height="5" className="fill-shelf" />
            {/* ribbon vertical + horizontal */}
            <rect x="14" y="8" width="4" height="20" className="fill-give" />
            <rect x="4" y="18" width="24" height="3" className="fill-give-soft" />
            {/* bow */}
            <rect x="10" y="4" width="5" height="4" className="fill-give" />
            <rect x="17" y="4" width="5" height="4" className="fill-give" />
            <rect x="14" y="5" width="4" height="3" className="fill-give-bright" />
            {/* outline (pixel ink) */}
            <rect x="3" y="7" width="26" height="1" className="fill-pixel" />
            <rect x="2" y="8" width="1" height="5" className="fill-pixel" />
            <rect x="29" y="8" width="1" height="5" className="fill-pixel" />
            <rect x="3" y="13" width="1" height="15" className="fill-pixel" />
            <rect x="28" y="13" width="1" height="15" className="fill-pixel" />
            <rect x="4" y="28" width="24" height="1" className="fill-pixel" />
          </svg>
        </div>

        {/* the gift tag — label in the JRPG box style */}
        <div className="mx-auto mt-6 w-fit max-w-[26rem] rounded-xl border-[3px] border-pixel bg-floor px-5 py-3.5 [box-shadow:inset_0_0_0_3px_var(--color-floor),inset_0_0_0_5px_var(--color-pixel)]">
          <h1 className="font-pixel text-xl leading-tight text-give-soft">{label}</h1>
          <p className="mt-1.5 text-sm text-ink-soft">
            a gift is waiting for you <span aria-hidden="true">♡</span>
          </p>
          <p className="mt-0.5 text-sm text-ink-soft">
            ben wrapped this one — it opens itself when the moment comes
          </p>
        </div>

        {/* the countdown */}
        <p
          className="mt-5 font-pixel text-2xl text-ink"
          role="timer"
          aria-live="off"
        >
          {formatRemaining(remaining)}
        </p>
        {opensText !== "" && (
          <p className="mt-1 text-sm text-dust">opens {opensText}</p>
        )}
      </main>
    </div>
  );
}
