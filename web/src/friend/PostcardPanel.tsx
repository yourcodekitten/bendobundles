import { useEffect, useMemo, useRef, useState } from "react";
import { postcardFilename, postcardKey, type PostcardInput } from "../postcard";
import { loadPostcardAssets, paintPostcard, renderPostcardBlob, type PostcardAssets } from "../postcardCanvas";

// 🖼️ the postcard panel (docs/spec-postcard.md D7/D8/D10/D11). Assets load once on
// mount; every input change re-renders synchronously-then-encodes; the stored artifact
// is a {key, blob} PAIR and save is live only while its key is the current input's.

type Artifact = { key: string; blob: Blob | null };

// OMBB minor 4: some browsers ignore a click on a DETACHED anchor, and revoking at 0ms
// can race the download start — attach for the click, revoke after 30s.
function download(blob: Blob, filename: string) {
  const url = URL.createObjectURL(blob);
  const a = document.createElement("a");
  a.href = url;
  a.download = filename;
  a.style.display = "none";
  document.body.appendChild(a);
  a.click();
  a.remove();
  setTimeout(() => URL.revokeObjectURL(url), 30_000);
}

export function PostcardPanel({
  base,
  note,
}: {
  base: Omit<PostcardInput, "note">;
  note: string | null;
}) {
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const [assets, setAssets] = useState<PostcardAssets | null>(null);
  const [includeNote, setIncludeNote] = useState(false); // D8: default OFF
  const [artifact, setArtifact] = useState<Artifact | null>(null);

  const current: PostcardInput = useMemo(
    () => ({ ...base, note: includeNote && note !== null ? note : null }),
    [base, includeNote, note],
  );
  const curKey = postcardKey(current);
  const latestKey = useRef(curKey);
  latestKey.current = curKey;

  // D10: load ONCE. base.artworkUrl is fixed for the panel's life.
  const artworkUrl = base.artworkUrl;
  useEffect(() => {
    let live = true;
    void loadPostcardAssets(artworkUrl).then((a) => { if (live) setAssets(a); });
    return () => { live = false; };
  }, [artworkUrl]);

  useEffect(() => {
    if (assets === null || canvasRef.current === null) return;
    const key = curKey;
    const canvas = canvasRef.current;
    paintPostcard(canvas, current, assets); // the preview — same draw, same input
    void renderPostcardBlob(current, assets).then(({ blob, artUsed }) => {
      if (key !== latestKey.current) return; // D11: stale ⇒ discarded
      // Lilith: a taint saved WITHOUT art — repaint the preview to match before save enables
      if (!artUsed && assets.art !== null) paintPostcard(canvas, current, { art: null });
      setArtifact({ key, blob });
    });
    // `current` is fully determined by curKey
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [assets, curKey]);

  const ready = artifact !== null && artifact.blob !== null && artifact.key === curKey;
  const failed = artifact !== null && artifact.key === curKey && artifact.blob === null;

  function onSave() {
    // synchronous until share (D10): no await before navigator.share
    if (artifact === null || artifact.blob === null || artifact.key !== latestKey.current) return;
    const filename = postcardFilename(base.title);
    const blob = artifact.blob;
    const file = new File([blob], filename, { type: "image/png" });
    if (typeof navigator.canShare === "function" && navigator.canShare({ files: [file] })) {
      navigator.share({ files: [file] }).catch((e: unknown) => {
        if ((e as { name?: string } | null)?.name !== "AbortError") download(blob, filename);
      });
      return;
    }
    download(blob, filename);
  }

  return (
    <div className="mt-3 flex flex-col items-center gap-3">
      <canvas
        ref={canvasRef}
        aria-label="postcard preview"
        className="h-auto w-full max-w-[270px] rounded shadow"
      />
      {assets === null && <p className="text-xs text-dust">getting your postcard ready…</p>}
      {failed && <p className="text-xs text-dust">couldn&apos;t make the postcard this time</p>}
      {note !== null && (
        <label className="flex items-center gap-2 text-sm text-ink-soft">
          <input
            type="checkbox"
            checked={includeNote}
            onChange={(e) => setIncludeNote(e.target.checked)}
          />
          include ben&apos;s note
        </label>
      )}
      <button
        type="button"
        disabled={!ready}
        onClick={onSave}
        className="rounded bg-give px-4 py-2 text-sm text-give-ink transition-colors hover:bg-give-bright disabled:opacity-50 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-pixel focus-visible:ring-offset-2 focus-visible:ring-offset-floor"
      >
        send a postcard ♡
      </button>
    </div>
  );
}
