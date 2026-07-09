import { useEffect } from "react";

// ── The boot screen — the handheld powers on (ben's pick, 2026-07-09) ────────
// Plays once per page load on the gift link page: olive takeover, the
// logotype crawls in chunky steps, a burgundy ding, ©1984 small print, and a
// 12-block loading meter — then a hard cut to the page. It doubles as the
// loading screen while the link data fetches behind it. Only mounted when
// motion is affirmatively allowed (see motionOK); decorative throughout.
const BOOT_MS = 2800;

export function BootScreen({ onDone }: { onDone: () => void }) {
  useEffect(() => {
    const t = setTimeout(onDone, BOOT_MS);
    return () => clearTimeout(t);
    // mount-once: the boot plays exactly once per mount
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  return (
    <div className="cel-boot" aria-hidden="true">
      <div className="cel-boot-stack">
        <p className="cel-boot-logo">bendobundles™</p>
        <p className="cel-boot-sub">©1984 bendo co. all gifts reserved</p>
        <div className="cel-boot-meter">
          {Array.from({ length: 12 }, (_, i) => (
            <span
              key={i}
              className="cel-boot-seg"
              style={{ animationDelay: `${1550 + i * 80}ms` }}
            />
          ))}
        </div>
      </div>
    </div>
  );
}
