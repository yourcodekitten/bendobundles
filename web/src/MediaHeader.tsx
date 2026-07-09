import { useState, useEffect, useRef } from "react";
import type Hls from "hls.js";
import type { SteamAppDetail } from "./api";
import { titleColorClass } from "./titleColor";

// ── Media header: trailer + screenshots contact sheet (issue #61) ─────────────
// Owns ALL video/HLS state — the modal renders <MediaHeader> and knows nothing
// about media items. The stage shows one item; below it, every item is one tap
// away as a thumbnail (contact-sheet grammar — no sequential arrows, chosen in
// the impeccable live session 2026-07-08: thumbnails preview content, show
// quantity, and give direct access). Thin fallback holds: no trailer + no
// screenshots renders the plain header image / title-hash block with zero
// chrome; one media item renders alone (thumbnails appear only at ≥ 2 items).
// Delight never gates.

type MediaHeaderProps = {
  title: string;
  artworkUrl: string | null;
  detail: SteamAppDetail | null;
};

type MediaItem =
  | { kind: "video"; url: string; poster: string | null; thumb: string | null }
  | { kind: "shot"; full: string; thumb: string; alt: string };

export function MediaHeader({ title, artworkUrl, detail }: MediaHeaderProps) {
  const [mediaIndex, setMediaIndex] = useState(0);
  const [videoPlaying, setVideoPlaying] = useState(false);
  const [hlsFailed, setHlsFailed] = useState(false);
  // Minimal trailer HUD (visible only while playing): pause / seek / sound /
  // fullscreen. Fullscreen targets the WRAPPER (video + HUD together) so the
  // controls stay available; Esc and the button both exit.
  const [videoMuted, setVideoMuted] = useState(false);
  const [videoPct, setVideoPct] = useState(0);
  const [videoFullscreen, setVideoFullscreen] = useState(false);
  const videoRef = useRef<HTMLVideoElement>(null);
  const videoWrapRef = useRef<HTMLDivElement>(null);
  const hlsRef = useRef<Hls | null>(null);

  useEffect(() => {
    const onFsChange = () =>
      setVideoFullscreen(
        document.fullscreenElement !== null &&
          document.fullscreenElement === videoWrapRef.current,
      );
    document.addEventListener("fullscreenchange", onFsChange);
    return () => document.removeEventListener("fullscreenchange", onFsChange);
  }, []);

  const pauseVideo = () => {
    videoRef.current?.pause();
    setVideoPlaying(false);
  };

  const toggleVideoFullscreen = () => {
    if (videoFullscreen) {
      void document.exitFullscreen?.().catch(() => {});
    } else {
      void videoWrapRef.current?.requestFullscreen?.().catch(() => {});
    }
  };

  const seekVideo = (pct: number) => {
    const v = videoRef.current;
    if (v === null || !Number.isFinite(v.duration) || v.duration === 0) return;
    v.currentTime = (pct / 100) * v.duration;
    setVideoPct(pct);
  };

  const toggleVideoSound = () => {
    const v = videoRef.current;
    if (v === null) return;
    v.muted = !v.muted;
    setVideoMuted(v.muted);
  };

  // ── HLS cleanup on unmount ──────────────────────────────────────────────────
  useEffect(() => {
    return () => {
      if (hlsRef.current) {
        hlsRef.current.destroy();
        hlsRef.current = null;
      }
    };
  }, []);

  // When hlsFailed, the trailer must not render AT ALL (not render-empty):
  // the item list and thumbnail grid both drop by one, keeping index math honest.
  // `||` not `??`: an empty-string url (loosely-typed wire) must not render a phantom item.
  const hlsUrl = !hlsFailed ? detail?.video_hls_url || null : null;
  const screenshots = detail?.screenshots ?? [];
  const items: MediaItem[] = [
    ...(hlsUrl !== null
      ? [
          {
            kind: "video" as const,
            url: hlsUrl,
            poster:
              detail?.video_thumbnail ?? detail?.header_image ?? artworkUrl,
            thumb:
              detail?.video_thumbnail ?? detail?.header_image ?? artworkUrl,
          },
        ]
      : []),
    ...screenshots.map((shot, i) => ({
      kind: "shot" as const,
      full: shot.full,
      thumb: shot.thumbnail,
      alt: `${title} screenshot ${i + 1}`,
    })),
  ];
  const count = items.length;
  const artwork = detail?.header_image ?? artworkUrl;

  // ── Thin fallback: no media at all → today's header, no chrome ──────────────
  if (count === 0) {
    return artwork !== null ? (
      <img
        src={artwork}
        alt={title}
        className="aspect-video w-full object-cover"
      />
    ) : (
      <div
        className={`aspect-video w-full ${titleColorClass(title)}`}
        aria-hidden="true"
      />
    );
  }

  // items can shrink under the index (fatal HLS drops the trailer) —
  // clamp at render, leave state alone.
  const index = Math.min(mediaIndex, count - 1);

  const goTo = (next: number) => {
    // Single item = nothing to navigate; without this, an arrow key on a
    // trailer-only header would "navigate" in place and pause the video.
    if (count < 2) return;
    const wrapped = (next + count) % count;
    // Leaving the trailer pauses playback; the ▶ overlay returns so
    // coming back is an explicit resume, never an auto-play.
    if (items[index]?.kind === "video") {
      videoRef.current?.pause();
      setVideoPlaying(false);
    }
    setMediaIndex(wrapped);
  };

  const handlePlay = async (url: string) => {
    if (!videoRef.current || videoPlaying) return;
    const video = videoRef.current;

    if (video.canPlayType("application/vnd.apple.mpegurl")) {
      // Native HLS — Safari. Only (re)assign on first play; resume keeps position.
      if (video.src === "") video.src = url;
    } else if (hlsRef.current === null) {
      // hls.js path — attach once; a resume after pause reuses the instance.
      const { default: HlsClass } = await import("hls.js");
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
    if (e.target instanceof HTMLElement && e.target.closest("video")) return;
    if (e.key === "ArrowLeft") {
      e.preventDefault();
      goTo(index - 1);
    } else if (e.key === "ArrowRight") {
      e.preventDefault();
      goTo(index + 1);
    }
  };

  const video = items.find(
    (m): m is Extract<MediaItem, { kind: "video" }> => m.kind === "video",
  );

  return (
    <div
      role="region"
      aria-roledescription="carousel"
      aria-label="media"
      onKeyDown={onKeyDown}
      className="flex flex-col gap-3"
    >
      {/* Stage — the video stays mounted while hidden so the HLS instance and
          playback position survive stepping away and back. Screenshot neighbors
          (±1) stay mounted-hidden so sequential arrow steps never load-flash;
          grid jumps mount fresh and settle in with .media-fade. */}
      <div className="relative overflow-hidden ring-1 ring-pixel">
        {video !== undefined && (
          <div
            ref={videoWrapRef}
            className={
              items[index]?.kind === "video"
                ? "media-video-wrap relative"
                : "hidden"
            }
            aria-hidden={items[index]?.kind !== "video" || undefined}
            inert={items[index]?.kind !== "video" || undefined}
          >
            <video
              ref={videoRef}
              poster={video.poster ?? undefined}
              className="aspect-video w-full object-cover"
              playsInline
              onClick={() => {
                // The playing video itself is the biggest pause target.
                if (videoPlaying) pauseVideo();
              }}
              onTimeUpdate={(e) => {
                const v = e.currentTarget;
                if (Number.isFinite(v.duration) && v.duration > 0)
                  setVideoPct((v.currentTime / v.duration) * 100);
              }}
              onEnded={() => setVideoPlaying(false)}
              onError={() => {
                // Safari-native path has no hls.js error events; a 404/decode failure
                // must still drop the trailer. hls.js raises recoverable element
                // errors during normal MSE operation, so only act on the native path.
                if (hlsRef.current === null) setHlsFailed(true);
              }}
            />
            {!videoPlaying && (
              <button
                type="button"
                aria-label="play trailer"
                onClick={() => void handlePlay(video.url)}
                className="absolute inset-0 flex items-center justify-center bg-black/40 hover:bg-black/50"
              >
                <span className="text-5xl text-white">▶</span>
              </button>
            )}
            {videoPlaying && (
              // The trailer HUD — super minimal, playing-state only. Pausing
              // (strip, or tapping the video) brings the big ▶ overlay back,
              // preserving the explicit-resume contract.
              <div className="absolute inset-x-0 bottom-0 flex items-center gap-2 bg-black/45 px-2 py-1">
                <button
                  type="button"
                  aria-label="pause trailer"
                  onClick={pauseVideo}
                  className="px-1 text-xs leading-none text-white/90 hover:text-white focus-visible:outline-2 focus-visible:outline-offset-1 focus-visible:outline-white/90"
                >
                  ❚❚
                </button>
                <input
                  type="range"
                  min={0}
                  max={100}
                  step={0.1}
                  value={videoPct}
                  aria-label="trailer progress"
                  onChange={(e) => seekVideo(Number(e.target.value))}
                  className="h-1 min-w-0 flex-1 cursor-pointer accent-pixel"
                />
                <button
                  type="button"
                  onClick={toggleVideoSound}
                  className="min-w-[7ch] px-1 text-left text-xs leading-none text-white/90 hover:text-white focus-visible:outline-2 focus-visible:outline-offset-1 focus-visible:outline-white/90"
                >
                  {videoMuted ? "sound off" : "sound on"}
                </button>
                <button
                  type="button"
                  aria-label={
                    videoFullscreen ? "exit fullscreen" : "fullscreen"
                  }
                  onClick={toggleVideoFullscreen}
                  className="px-1 text-sm leading-none text-white/90 hover:text-white focus-visible:outline-2 focus-visible:outline-offset-1 focus-visible:outline-white/90"
                >
                  ⛶
                </button>
              </div>
            )}
          </div>
        )}
        {items.map((item, i) =>
          item.kind === "shot" && Math.abs(i - index) <= 1 ? (
            <img
              key={`${i}-${item.full}`}
              src={item.full}
              alt={item.alt}
              aria-hidden={i !== index || undefined}
              className={
                i === index
                  ? "media-fade aspect-video w-full object-cover"
                  : "hidden"
              }
            />
          ) : null,
        )}
        {count > 1 && (
          <span aria-live="polite" className="sr-only">
            item {index + 1} of {count}
          </span>
        )}
      </div>

      {/* Contact sheet — chrome only at ≥ 2 items. Active tile wears the pixel
          ring (The Bezel Rule marks the media frame; the tile echoes it). */}
      {count > 1 && (
        <div className="grid grid-cols-4 gap-1.5 px-6 sm:grid-cols-6">
          {items.map((item, i) => (
            <button
              // Key includes the position: Steam occasionally repeats an asset URL,
              // and duplicate keys would mis-reconcile the active flags.
              key={`${i}-${item.kind === "video" ? item.url : item.full}`}
              type="button"
              aria-label={item.kind === "video" ? "trailer" : item.alt}
              aria-current={i === index ? "true" : undefined}
              onClick={() => goTo(i)}
              className={`relative aspect-video overflow-hidden rounded transition-opacity motion-reduce:transition-none focus-visible:outline-2 focus-visible:outline-pixel focus-visible:outline-offset-2 ${
                i === index
                  ? "opacity-100 ring-2 ring-pixel"
                  : "opacity-65 saturate-[0.8] hover:opacity-90"
              }`}
            >
              {item.thumb !== null && item.thumb !== "" ? (
                <img
                  src={item.thumb}
                  alt=""
                  loading="lazy"
                  className="h-full w-full object-cover"
                />
              ) : (
                <span className="flex h-full w-full items-center justify-center bg-shelf text-ink-soft">
                  ▶
                </span>
              )}
              {item.kind === "video" && (
                <span
                  aria-hidden="true"
                  className="absolute bottom-0.5 right-0.5 rounded-sm bg-black/55 px-1 py-0.5 text-[10px] leading-none text-white"
                >
                  ▶
                </span>
              )}
            </button>
          ))}
        </div>
      )}
    </div>
  );
}
