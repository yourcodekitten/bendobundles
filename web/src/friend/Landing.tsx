export function Landing() {
  return (
    <div className="min-h-screen bg-room text-ink flex items-center justify-center p-6">
      <main className="landing-diorama landing-rise">
        <div className="landing-stage">
          <img
            className="landing-stage-art"
            src="/art/landing.png"
            alt="a tiny pixel adventurer walking toward a treasure chest, drawn in four shades of pea green"
            width={640}
            height={640}
          />
        </div>
        {/* the wordmark + line, docked in a floating JRPG text box (The Bezel Rule) */}
        <div className="landing-box">
          {/* the logotype: Silkscreen, full caps — the one place caps exist (DESIGN.md carve-out) */}
          <h1 className="landing-box-word">bendobundles</h1>
          <p className="landing-box-line">
            a friend has to hand you a{' '}
            <span className="font-pixel font-semibold text-give-soft">
              key
              {/* a tiny pixel treasure key that swings gently off the word */}
              <svg className="landing-key-charm" viewBox="0 0 16 8" aria-hidden="true">
                <rect x="0" y="0" width="5" height="1" />
                <rect x="0" y="5" width="5" height="1" />
                <rect x="0" y="0" width="1" height="6" />
                <rect x="4" y="0" width="1" height="6" />
                <rect x="5" y="2" width="10" height="2" />
                <rect x="11" y="4" width="1" height="2" />
                <rect x="13" y="4" width="1" height="2" />
              </svg>
            </span>{' '}
            {/* hover the treasure art and the heart lifts + sparkles (see .landing-heart) */}
            for this treasure <span className="landing-heart">♡</span>
          </p>
        </div>
      </main>
    </div>
  );
}
