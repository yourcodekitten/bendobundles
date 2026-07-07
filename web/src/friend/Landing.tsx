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
        <h1 className="mt-8 text-4xl font-bold tracking-tight text-balance">bendobundles</h1>
        <p className="mt-4 text-dust">
          a friend has to hand you a{' '}
          <span className="font-semibold text-give-soft">key</span> for this treasure ♡
        </p>
      </main>
    </div>
  );
}
