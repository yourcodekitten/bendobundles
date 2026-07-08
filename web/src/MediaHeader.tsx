import { useState, useEffect, useRef } from 'react';
import type Hls from 'hls.js';
import type { SteamAppDetail } from './api';
import { prefersReducedMotion } from './motion';
import { titleColorClass } from './titleColor';

// ── Media header: trailer + screenshots carousel (issue #61) ──────────────────
// Owns ALL video/HLS state — the modal renders <MediaHeader> and knows nothing
// about slides. Thin fallback holds: no trailer + no screenshots renders the
// plain header image / title-hash block with zero carousel chrome; one media
// item renders alone (chrome appears only at ≥ 2 items). Delight never gates.

type MediaHeaderProps = {
  title: string;
  artworkUrl: string | null;
  detail: SteamAppDetail | null;
};

export function MediaHeader({ title, artworkUrl, detail }: MediaHeaderProps) {
  const [mediaIndex, setMediaIndex] = useState(0);
  const [videoPlaying, setVideoPlaying] = useState(false);
  const [hlsFailed, setHlsFailed] = useState(false);
  // Read once at mount (lazy initializer) — the preference can't usefully change
  // between renders and neither approach reacts live (no change listener).
  const [reducedMotion] = useState(prefersReducedMotion);
  const videoRef = useRef<HTMLVideoElement>(null);
  const hlsRef = useRef<Hls | null>(null);

  // ── HLS cleanup on unmount ──────────────────────────────────────────────────
  useEffect(() => {
    return () => {
      if (hlsRef.current) {
        hlsRef.current.destroy();
        hlsRef.current = null;
      }
    };
  }, []);

  // When hlsFailed, the trailer slide must not render AT ALL (not render-empty):
  // slideCount and the slide DOM both drop by one, keeping counter/index math honest.
  // `||` not `??`: an empty-string url (loosely-typed wire) must not render a phantom slide.
  const hlsUrl = !hlsFailed ? detail?.video_hls_url || null : null;
  const screenshots = detail?.screenshots ?? [];
  const trailerSlides = hlsUrl !== null ? 1 : 0;
  const slideCount = trailerSlides + screenshots.length;
  const artwork = detail?.header_image ?? artworkUrl;

  // ── Thin fallback: no media at all → today's header, no carousel chrome ─────
  if (slideCount === 0) {
    return artwork !== null ? (
      <img src={artwork} alt={title} className="aspect-video w-full object-cover" />
    ) : (
      <div
        className={`aspect-video w-full ${titleColorClass(title)}`}
        aria-hidden="true"
      />
    );
  }

  // items can shrink under the index (fatal HLS drops the trailer slide) —
  // clamp at render, leave state alone.
  const index = Math.min(mediaIndex, slideCount - 1);

  const goTo = (next: number) => {
    // Single slide = nothing to navigate; without this, an arrow key on a
    // trailer-only header would "navigate" in place and pause the video.
    if (slideCount < 2) return;
    const wrapped = (next + slideCount) % slideCount;
    // Leaving the trailer slide pauses playback; the ▶ overlay returns so
    // coming back is an explicit resume, never an auto-play.
    if (trailerSlides === 1 && index === 0) {
      videoRef.current?.pause();
      setVideoPlaying(false);
    }
    setMediaIndex(wrapped);
  };

  const handlePlay = async (url: string) => {
    if (!videoRef.current || videoPlaying) return;
    const video = videoRef.current;

    if (video.canPlayType('application/vnd.apple.mpegurl')) {
      // Native HLS — Safari. Only (re)assign on first play; resume keeps position.
      if (video.src === '') video.src = url;
    } else if (hlsRef.current === null) {
      // hls.js path — attach once; a resume after pause reuses the instance.
      const { default: HlsClass } = await import('hls.js');
      const hls = new HlsClass();
      hlsRef.current = hls;
      hls.loadSource(url);
      hls.attachMedia(video);
      hls.on(HlsClass.Events.ERROR, (_event, data) => {
        if (data.fatal) {
          setHlsFailed(true);
          hls.destroy();
          hlsRef.current = null;
        }
      });
    }

    setVideoPlaying(true);
    try {
      await video.play();
    } catch {
      // play() rejection (browser policy, no source in test env) — ignore
    }
  };

  const onKeyDown = (e: React.KeyboardEvent) => {
    // Native video controls own their arrow keys.
    if (e.target instanceof HTMLElement && e.target.closest('video')) return;
    if (e.key === 'ArrowLeft') {
      e.preventDefault();
      goTo(index - 1);
    } else if (e.key === 'ArrowRight') {
      e.preventDefault();
      goTo(index + 1);
    }
  };

  return (
    <div
      role="region"
      aria-roledescription="carousel"
      aria-label="media"
      onKeyDown={onKeyDown}
      className="relative overflow-hidden ring-1 ring-pixel"
    >
      {/* Slide strip — transform per index; reduced motion = instant swap */}
      <div
        className={`flex ${reducedMotion ? '' : 'transition-transform duration-300'}`}
        style={{ transform: `translateX(-${index * 100}%)` }}
      >
        {hlsUrl !== null && (
          <div
            className="relative w-full shrink-0"
            aria-hidden={index !== 0 || undefined}
            inert={index !== 0 || undefined}
          >
            <video
              ref={videoRef}
              poster={
                detail?.video_thumbnail ?? detail?.header_image ?? artworkUrl ?? undefined
              }
              className="aspect-video w-full object-cover"
              playsInline
              onError={() => {
                // Safari-native path has no hls.js error events; a 404/decode failure
                // must still drop the trailer slide. hls.js raises recoverable element
                // errors during normal MSE operation, so only act on the native path.
                if (hlsRef.current === null) setHlsFailed(true);
              }}
            />
            {!videoPlaying && (
              <button
                type="button"
                aria-label="play trailer"
                onClick={() => void handlePlay(hlsUrl)}
                className="absolute inset-0 flex items-center justify-center bg-black/40 hover:bg-black/50"
              >
                <span className="text-5xl text-white">▶</span>
              </button>
            )}
          </div>
        )}
        {screenshots.map((shot, i) => {
          const slideIdx = trailerSlides + i;
          return (
            // Key includes the position: Steam occasionally repeats an asset URL,
            // and duplicate keys would mis-reconcile the inert/aria-hidden flags.
            <div
              key={`${i}-${shot.full}`}
              className="w-full shrink-0"
              aria-hidden={index !== slideIdx || undefined}
              inert={index !== slideIdx || undefined}
            >
              {/* Mount the image only near the active slide: translated-out slides
                  sit within the browser's lazy-load distance band, so loading="lazy"
                  alone still fetches the next few full-res files on open. */}
              {Math.abs(slideIdx - index) <= 1 && (
                <img
                  src={shot.full}
                  alt={`${title} screenshot ${i + 1}`}
                  className="aspect-video w-full object-cover"
                />
              )}
            </div>
          );
        })}
      </div>

      {/* Chrome only at ≥ 2 items. Neutral control green — burgundy is giving-only. */}
      {slideCount > 1 && (
        <>
          <button
            type="button"
            aria-label="previous"
            onClick={() => goTo(index - 1)}
            className="absolute left-2 top-1/2 -translate-y-1/2 rounded bg-control px-2 py-1 text-sm hover:bg-control-bright"
          >
            ‹
          </button>
          <button
            type="button"
            aria-label="next"
            onClick={() => goTo(index + 1)}
            className="absolute right-2 top-1/2 -translate-y-1/2 rounded bg-control px-2 py-1 text-sm hover:bg-control-bright"
          >
            ›
          </button>
          <span
            aria-live="polite"
            className="absolute bottom-2 right-2 rounded bg-shelf px-2 py-0.5 text-xs text-ink-soft"
          >
            {index + 1} / {slideCount}
          </span>
        </>
      )}
    </div>
  );
}
