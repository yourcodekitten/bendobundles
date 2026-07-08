import { memo, useEffect, useRef, useState, type JSX } from 'react';
import { prefersReducedMotion } from '../motion';

/* ── the cursor companion ──────────────────────────────────────────────────
   a small pixel friend that lazily trails the cursor around the gift page and
   darts off-screen the moment you open a game (it's shy about your business).
   pure delight: desktop + fine-pointer only, and it never mounts under
   prefers-reduced-motion. transform-only, one rAF loop, paused off-tab. */

export type CompanionVariant = 'firefly' | 'critter' | 'cart';

const SIZE = 28; // sprite box in px
const HALF = SIZE / 2;
const LEASH = 54; // px it hovers from the cursor — it drifts near, never lands on it
const STIFFNESS = 0.02; // spring pull toward the leash point (low = lazy)
const DAMPING = 0.9; // < 1 leaves a springy, elastic overshoot before it settles
const MAX_FOLLOW = 5; // px/frame cap while following → lazy catch-up, no snap on re-entry

const SPRITES: Record<CompanionVariant, JSX.Element> = {
  // a will-o'-wisp: a burgundy gift-spark with a breathing olive halo
  firefly: (
    <svg className="cc-sprite" width={SIZE} height={SIZE} viewBox="0 0 13 13" aria-hidden="true">
      <rect className="cc-halo" x="1" y="1" width="11" height="11" rx="5.5" fill="var(--color-give)" opacity="0.16" />
      <rect x="4" y="4" width="5" height="5" rx="2.5" fill="var(--color-give)" />
      <rect x="5" y="5" width="2" height="2" fill="var(--color-give-ink)" />
    </svg>
  ),
  // a little pixel ghost pet — dark olive, two bright eyes, a wavy hem
  critter: (
    <svg className="cc-sprite" width={SIZE} height={SIZE} viewBox="0 0 16 16" aria-hidden="true">
      <path
        d="M3 8a5 5 0 0 1 10 0v6l-2-1.4-1.6 1.4-1.4-1.4-1.4 1.4-1.6-1.4z"
        fill="var(--color-pixel)"
      />
      <rect className="cc-eye" x="5" y="7" width="2" height="2" rx="0.5" fill="var(--color-floor)" />
      <rect className="cc-eye" x="9" y="7" width="2" height="2" rx="0.5" fill="var(--color-floor)" />
    </svg>
  ),
  // a tiny game cartridge trailing you home
  cart: (
    <svg className="cc-sprite" width={SIZE} height={SIZE} viewBox="0 0 16 16" aria-hidden="true">
      <rect x="4" y="2" width="8" height="12" rx="1" fill="var(--color-pixel)" />
      <rect x="5" y="3" width="1" height="1" fill="var(--color-line)" />
      <rect x="7" y="3" width="1" height="1" fill="var(--color-line)" />
      <rect x="9" y="3" width="1" height="1" fill="var(--color-line)" />
      <rect x="5" y="6" width="6" height="6" fill="var(--color-floor)" />
      <rect x="6" y="8" width="4" height="2" fill="var(--color-give)" />
    </svg>
  ),
};

const CSS = `
.cc-root { position: fixed; top: 0; left: 0; z-index: 30; pointer-events: none; will-change: transform; opacity: 1; transition: opacity 1.1s ease; }
.cc-root.is-hidden { opacity: 0; }
.cc-body { transition: transform 1.5s cubic-bezier(0.25, 1, 0.5, 1), opacity 3s ease; transform-origin: center; }
.cc-root.is-away .cc-body { transform: scale(0.3) rotate(200deg); opacity: 0; }
.cc-sprite { display: block; image-rendering: pixelated; }
.cc-firefly .cc-halo { animation: cc-glow 1.8s ease-in-out infinite; transform-origin: center; transform-box: fill-box; }
@keyframes cc-glow { 0%, 100% { opacity: 0.14; transform: scale(1); } 50% { opacity: 0.36; transform: scale(1.35); } }
.cc-critter .cc-sprite { animation: cc-squash 1.5s ease-in-out infinite; transform-origin: bottom; }
@keyframes cc-squash { 0%, 100% { transform: scaleY(1) scaleX(1); } 50% { transform: scaleY(0.9) scaleX(1.06); } }
.cc-critter .cc-eye { animation: cc-blink 3.4s steps(1) infinite; transform-origin: center; transform-box: fill-box; }
@keyframes cc-blink { 0%, 93%, 100% { transform: scaleY(1); } 96% { transform: scaleY(0.1); } }
.cc-cart .cc-sprite { animation: cc-wobble 2.4s ease-in-out infinite; transform-origin: center; }
@keyframes cc-wobble { 0%, 100% { transform: rotate(-6deg); } 50% { transform: rotate(6deg); } }
`;

function canRun(): boolean {
  if (typeof window === 'undefined' || !window.matchMedia) return false;
  return window.matchMedia('(pointer: fine)').matches && !prefersReducedMotion();
}

function CursorCompanionImpl({ variant, away }: { variant: CompanionVariant; away: boolean }) {
  const [enabled] = useState(canRun);
  const rootRef = useRef<HTMLDivElement>(null);
  const awayRef = useRef(away);
  awayRef.current = away;

  useEffect(() => {
    if (!enabled) return;
    const el = rootRef.current;
    if (!el) return;

    // start off-screen so it drifts in on first move
    let cx = -SIZE;
    let cy = window.innerHeight / 2;
    let vx = 0;
    let vy = 0;
    let lastScrollX = window.scrollX;
    let lastScrollY = window.scrollY;
    let tx = window.innerWidth / 2;
    let ty = window.innerHeight / 2;
    let raf = 0;
    let hidden = false;
    let appeared = false;
    let idleFaded = false;
    let activeSince: number | null = null;
    let prevAway = awayRef.current;
    let lastMove = performance.now();
    // wait 15s after page load before the companion first shows up
    const showTimer = setTimeout(() => {
      appeared = true;
      lastMove = performance.now();
      idleFaded = false;
      activeSince = null;
    }, 15000);
    let returnTimer: ReturnType<typeof setTimeout> | undefined;

    const onMove = (e: PointerEvent) => {
      tx = e.clientX;
      ty = e.clientY;
      lastMove = performance.now();
    };
    const onVis = () => {
      hidden = document.hidden;
    };

    const loop = (now: number) => {
      raf = requestAnimationFrame(loop);
      // scroll with the page: displace by the scroll delta so a scroll drags the
      // companion along with the content, then the spring hurries it back to the cursor.
      const sx = window.scrollX;
      const sy = window.scrollY;
      const scrollDX = sx - lastScrollX;
      const scrollDY = sy - lastScrollY;
      lastScrollX = sx;
      lastScrollY = sy;
      if (hidden) return;
      cx -= scrollDX;
      cy -= scrollDY;

      // once a modal is dismissed, wait 5s before drifting back in
      if (prevAway && !awayRef.current) {
        appeared = false;
        clearTimeout(returnTimer);
        returnTimer = setTimeout(() => {
          appeared = true;
          lastMove = performance.now();
          idleFaded = false;
          activeSince = null;
        }, 5000);
      }
      prevAway = awayRef.current;
      // idle fade (opacity only — it keeps following underneath): hide after 5s of a
      // still cursor, fade back after the cursor has been moving again for 2s.
      if (awayRef.current) {
        lastMove = now; // don't accrue idle time while a modal is up
        idleFaded = false;
        activeSince = null;
      } else if (!idleFaded) {
        if (now - lastMove > 5000) idleFaded = true;
      } else if (now - lastMove < 350) {
        if (activeSince === null) activeSince = now;
        if (now - activeSince > 2000) {
          idleFaded = false;
          activeSince = null;
        }
      } else {
        activeSince = null;
      }
      el.classList.toggle('is-away', awayRef.current);
      el.classList.toggle('is-hidden', !appeared || idleFaded);

      let aimX: number;
      let aimY: number;
      let stiffness = STIFFNESS;
      let damping = DAMPING;
      let maxSpeed = MAX_FOLLOW;
      if (awayRef.current) {
        // dart out the nearest horizontal edge — snappier spring, less overshoot
        aimX = cx < window.innerWidth / 2 ? -SIZE * 3 : window.innerWidth + SIZE * 3;
        aimY = ty;
        stiffness = 0.14;
        damping = 0.82;
        maxSpeed = MAX_FOLLOW; // exit drifts off at the same lazy cap as the follow
      } else {
        // trail the cursor, but hold a leash-length standoff and add a slow wander so
        // it floats *around* the pointer instead of landing on it.
        const dx = tx - cx;
        const dy = ty - cy;
        const dist = Math.hypot(dx, dy) || 1;
        const wanderX = Math.cos(now / 900) * 9 + Math.cos(now / 1700) * 5;
        const wanderY = Math.sin(now / 1100) * 9 + Math.sin(now / 1500) * 5;
        aimX = tx - (dx / dist) * LEASH + wanderX;
        aimY = ty - (dy / dist) * LEASH + wanderY;
      }
      // spring: accelerate toward the aim, then damp — the < 1 damping is what gives
      // the elastic overshoot-and-settle instead of a flat ease.
      vx = (vx + (aimX - cx) * stiffness) * damping;
      vy = (vy + (aimY - cy) * stiffness) * damping;
      // cap speed so a cursor re-entering far away glides in lazily, never snaps
      const speed = Math.hypot(vx, vy);
      if (speed > maxSpeed) {
        vx = (vx / speed) * maxSpeed;
        vy = (vy / speed) * maxSpeed;
      }
      cx += vx;
      cy += vy;
      let rx = cx;
      let ry = cy;
      if (!awayRef.current) {
        // wobble the *path* while chasing: a sideways weave perpendicular to the
        // direction of the cursor, scaled by distance so it curves mid-chase and
        // flattens as it arrives — no dead-straight beeline.
        const ddx = tx - cx;
        const ddy = ty - cy;
        const dd = Math.hypot(ddx, ddy) || 1;
        const drift = (Math.sin(now / 520) + Math.sin(now / 300) * 0.4) * Math.min(dd, 260) * 0.22;
        rx += (-ddy / dd) * drift;
        ry += (ddx / dd) * drift;
      }
      el.style.transform = `translate3d(${rx - HALF}px, ${ry - HALF}px, 0)`;
    };

    window.addEventListener('pointermove', onMove, { passive: true });
    document.addEventListener('visibilitychange', onVis);
    raf = requestAnimationFrame(loop);
    return () => {
      cancelAnimationFrame(raf);
      window.removeEventListener('pointermove', onMove);
      document.removeEventListener('visibilitychange', onVis);
      clearTimeout(showTimer);
      clearTimeout(returnTimer);
    };
  }, [enabled]);

  if (!enabled) return null;
  return (
    <div ref={rootRef} aria-hidden="true" className={`cc-root cc-${variant}`}>
      <style>{CSS}</style>
      <div className="cc-body">{SPRITES[variant]}</div>
    </div>
  );
}

// memoized: the friend page re-renders ~70x/sec during the load typewriter;
// stable props mean the companion skips that storm entirely (its rAF is independent).
export const CursorCompanion = memo(CursorCompanionImpl);
