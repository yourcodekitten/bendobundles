import { useEffect, useRef, useState } from 'react';
import { claimGame, type ClaimResult, type GameView } from '../api';

interface ClaimDialogProps {
  token: string;
  game: GameView;
  onClose: () => void;
  onRefresh: () => void;
}

type Step = 'confirm' | 'loading' | 'gifted' | 'processing' | 'refused' | 'error';

// The per-step CASUAL-dismiss policy (Escape, backdrop click) — one place, so
// the surfaces can never drift: null = not dismissible (gifted protects the
// one-time URL; loading has a claim in flight), 'refresh' = a claim was
// consumed so dismissal must refetch, 'close' = plain close. The explicit
// close BUTTONS are not dismissal: gifted deliberately allows its button
// while blocking stray Escapes/clicks.
function dismissKindFor(step: Step): 'close' | 'refresh' | null {
  if (step === 'gifted' || step === 'loading') return null;
  if (step === 'processing' || step === 'refused') return 'refresh';
  return 'close';
}

export function ClaimDialog({ token, game, onClose, onRefresh }: ClaimDialogProps) {
  const [step, setStep] = useState<Step>('confirm');
  const [result, setResult] = useState<ClaimResult | null>(null);
  const containerRef = useRef<HTMLDivElement>(null);
  const [copied, setCopied] = useState(false);

  // Focus the dialog on open
  useEffect(() => {
    containerRef.current?.focus();
  }, []);

  // Escape key — same policy as the backdrop, via dismissKindFor
  useEffect(() => {
    const handleKeyDown = (e: KeyboardEvent) => {
      if (e.key !== 'Escape') return;
      const kind = dismissKindFor(step);
      if (kind === null) return;
      if (kind === 'refresh') onRefresh();
      onClose();
    };
    document.addEventListener('keydown', handleKeyDown);
    return () => document.removeEventListener('keydown', handleKeyDown);
  }, [step, onClose, onRefresh]);

  const handleConfirm = async () => {
    setStep('loading');
    const r = await claimGame(token, game.id);
    setResult(r);
    if (r.kind === 'gifted') setStep('gifted');
    else if (r.kind === 'processing') setStep('processing');
    else if (r.kind === 'refused') setStep('refused');
    else setStep('error');
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
      {/* Backdrop — same policy as Escape, via dismissKindFor */}
      <div
        className="fixed inset-0 z-40 bg-black/60"
        onClick={
          dismissKindFor(step) === null
            ? undefined
            : dismissKindFor(step) === 'refresh'
              ? handleCloseWithRefresh
              : onClose
        }
        aria-hidden="true"
      />

      {/* Dialog panel */}
      <div
        ref={containerRef}
        role="dialog"
        aria-modal="true"
        aria-label={`claim ${game.title}`}
        tabIndex={-1}
        className="fixed inset-0 z-50 flex items-center justify-center p-4 outline-none"
      >
        <div className="w-full max-w-md rounded-xl bg-zinc-900 p-6 shadow-2xl ring-1 ring-zinc-700">
          {step === 'confirm' && (
            <>
              <h2 className="text-lg font-semibold">
                claim <span className="text-violet-300">{game.title}</span>?
              </h2>
              <p className="mt-2 text-sm text-zinc-400">this uses 1 of your claims</p>
              <div className="mt-6 flex gap-3 justify-end">
                <button
                  type="button"
                  onClick={onClose}
                  className="rounded px-4 py-2 text-sm text-zinc-400 hover:text-zinc-200"
                >
                  cancel
                </button>
                <button
                  type="button"
                  onClick={() => { void handleConfirm(); }}
                  className="rounded bg-violet-700 px-4 py-2 text-sm font-medium hover:bg-violet-600"
                >
                  confirm
                </button>
              </div>
            </>
          )}

          {step === 'loading' && (
            <p className="text-center text-zinc-400 py-4">claiming...</p>
          )}

          {step === 'gifted' && result?.kind === 'gifted' && (
            <>
              <h2 className="text-lg font-semibold text-violet-300">it&apos;s yours! ♡</h2>
              <p className="mt-1 text-xs text-zinc-500">
                this link is one-time — redeem it to YOUR humble account
              </p>
              <div className="mt-4 rounded bg-zinc-800 p-3">
                <a
                  href={result.gift_url}
                  target="_blank"
                  rel="noreferrer"
                  className="block break-all text-sm text-violet-400 underline hover:text-violet-300"
                >
                  {result.gift_url}
                </a>
              </div>
              <div className="mt-3 flex gap-2">
                <button
                  type="button"
                  onClick={() => { void handleCopy(result.gift_url); }}
                  className="flex-1 rounded bg-zinc-800 px-3 py-2 text-sm hover:bg-zinc-700"
                >
                  {copied ? 'copied ✓' : 'copy link'}
                </button>
                <a
                  href={result.gift_url}
                  target="_blank"
                  rel="noreferrer"
                  className="flex-1 rounded bg-violet-700 px-3 py-2 text-sm text-center hover:bg-violet-600"
                >
                  open on humble
                </a>
              </div>
              <p className="mt-4 text-xs text-zinc-500">keys may be region-locked</p>
              <div className="mt-4 flex justify-end">
                <button
                  type="button"
                  onClick={handleCloseWithRefresh}
                  className="rounded px-4 py-2 text-sm text-zinc-400 hover:text-zinc-200"
                >
                  close
                </button>
              </div>
            </>
          )}

          {step === 'processing' && result?.kind === 'processing' && (
            <>
              <h2 className="text-lg font-semibold text-amber-300">processing</h2>
              <p className="mt-2 text-sm text-zinc-300">{result.message}</p>
              <p className="mt-1 text-sm text-zinc-500">check this page later</p>
              <div className="mt-6 flex justify-end">
                <button
                  type="button"
                  onClick={handleCloseWithRefresh}
                  className="rounded px-4 py-2 text-sm text-zinc-400 hover:text-zinc-200"
                >
                  close
                </button>
              </div>
            </>
          )}

          {step === 'refused' && result?.kind === 'refused' && (
            <>
              <h2 className="text-lg font-semibold text-red-400">whoops</h2>
              <p className="mt-2 text-sm text-zinc-300">{result.message}</p>
              <div className="mt-6 flex justify-end">
                <button
                  type="button"
                  onClick={handleCloseWithRefresh}
                  className="rounded px-4 py-2 text-sm text-zinc-400 hover:text-zinc-200"
                >
                  close
                </button>
              </div>
            </>
          )}

          {step === 'error' && result?.kind === 'error' && (
            <>
              <h2 className="text-lg font-semibold text-red-400">uh oh</h2>
              <p className="mt-2 text-sm text-zinc-300">{result.message}</p>
              <div className="mt-6 flex justify-end">
                <button
                  type="button"
                  onClick={onClose}
                  className="rounded px-4 py-2 text-sm text-zinc-400 hover:text-zinc-200"
                >
                  close
                </button>
              </div>
            </>
          )}
        </div>
      </div>
    </>
  );
}
