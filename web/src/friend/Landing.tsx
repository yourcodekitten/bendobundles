export function Landing() {
  return (
    <div className="min-h-screen bg-room text-ink flex items-center justify-center p-6">
      <main className="text-center landing-rise">
        <img
          src="/art/landing.png"
          alt="a tiny pixel adventurer walking toward a treasure chest, drawn in four shades of pea green"
          width={640}
          height={640}
          className="mx-auto w-[min(72vw,300px)] rounded-[14px_14px_44px_14px] border-8 border-pixel"
        />
        {/* the logotype: Silkscreen, full caps — the one place caps exist (DESIGN.md carve-out) */}
        <h1 className="mt-8 font-logo text-4xl font-normal uppercase tracking-[0.03em] text-balance">
          bendobundles
        </h1>
        <p className="mt-4 text-dust">
          a friend has to hand you a{' '}
          <span className="font-pixel text-[1.0625rem] font-semibold text-give-soft">key</span> for
          this treasure ♡
        </p>
      </main>
    </div>
  );
}
