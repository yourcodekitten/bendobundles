// 🖼️ the postcard's canvas half (docs/spec-postcard.md D2/D3/D10). Assets load ONCE
// per panel; drawPostcard is synchronous so a D8 toggle re-render never awaits.
import {
  POSTCARD_H,
  POSTCARD_PALETTE as P,
  POSTCARD_W,
  postcardText,
  wrapLines,
  type PostcardInput,
} from "./postcard";

export type PostcardAssets = { art: HTMLImageElement | null };

export type DrawCtx = Pick<
  CanvasRenderingContext2D,
  | "fillStyle"
  | "strokeStyle"
  | "lineWidth"
  | "font"
  | "textAlign"
  | "textBaseline"
  | "fillRect"
  | "strokeRect"
  | "fillText"
  | "measureText"
  | "drawImage"
  | "save"
  | "restore"
>;

export const POSTCARD_FONTS = [
  '64px "Pixelify Sans Variable"',
  '36px "Chivo Variable"',
  'italic 36px "Chivo Variable"',
  '28px "Silkscreen"',
] as const;

const SERIF_FALLBACK = "ui-sans-serif, system-ui, sans-serif";
const F = {
  title: `64px "Pixelify Sans Variable", ${SERIF_FALLBACK}`,
  from: `36px "Chivo Variable", ${SERIF_FALLBACK}`,
  note: `italic 36px "Chivo Variable", ${SERIF_FALLBACK}`,
  post: `26px "Chivo Variable", ${SERIF_FALLBACK}`,
  waited: `44px "Pixelify Sans Variable", ${SERIF_FALLBACK}`,
  mark: `28px "Silkscreen", ui-monospace, monospace`,
};

export function canMakePostcards(): boolean {
  return (
    typeof HTMLCanvasElement !== "undefined" &&
    typeof HTMLCanvasElement.prototype.toBlob === "function"
  );
}

function withTimeout<T>(p: Promise<T>, ms: number, fallback: T): Promise<T> {
  return new Promise((resolve) => {
    const t = setTimeout(() => resolve(fallback), ms);
    p.then(
      (v) => {
        clearTimeout(t);
        resolve(v);
      },
      () => {
        clearTimeout(t);
        resolve(fallback);
      },
    );
  });
}

/** D2 + D3. Never rejects. Art via <img crossOrigin>, NEVER fetch (connect-src). */
export async function loadPostcardAssets(
  artworkUrl: string | null,
  timeoutMs = 4000,
): Promise<PostcardAssets> {
  const fonts =
    typeof document !== "undefined" && document.fonts
      ? Promise.all(POSTCARD_FONTS.map((f) => document.fonts.load(f))).then(
          () => undefined,
        )
      : Promise.resolve(undefined);
  const art: Promise<HTMLImageElement | null> =
    artworkUrl === null
      ? Promise.resolve(null)
      : new Promise((resolve) => {
          const img = new Image();
          img.crossOrigin = "anonymous";
          img.onload = () => resolve(img);
          img.onerror = () => resolve(null);
          img.src = artworkUrl;
        });
  const [, a] = await Promise.all([
    withTimeout(fonts, timeoutMs, undefined),
    withTimeout(art, timeoutMs, null),
  ]);
  return { art: a };
}

const M = 72; // outer margin
const ART = { x: M + 24, y: M + 24, w: POSTCARD_W - 2 * (M + 24), h: 580 };

function drawPlaceholder(ctx: DrawCtx) {
  ctx.fillStyle = P.mat;
  ctx.fillRect(ART.x, ART.y, ART.w, ART.h);
  // the house pixel-gift glyph, drawn in blocks (no asset, cannot fail)
  const s = 24,
    cx = ART.x + ART.w / 2,
    cy = ART.y + ART.h / 2;
  ctx.fillStyle = P.give;
  ctx.fillRect(cx - 4 * s, cy - 2 * s, 8 * s, 6 * s); // box
  ctx.fillStyle = P.paper;
  ctx.fillRect(cx - s / 2, cy - 2 * s, s, 6 * s); // ribbon v
  ctx.fillRect(cx - 4 * s, cy, 8 * s, s); // ribbon h
  ctx.fillStyle = P.give;
  ctx.fillRect(cx - 3 * s, cy - 4 * s, 2 * s, 2 * s); // bow l
  ctx.fillRect(cx + s, cy - 4 * s, 2 * s, 2 * s); // bow r
}

function drawArt(ctx: DrawCtx, img: HTMLImageElement) {
  ctx.fillStyle = P.mat;
  ctx.fillRect(ART.x, ART.y, ART.w, ART.h);
  const iw = img.naturalWidth || ART.w,
    ih = img.naturalHeight || ART.h;
  const k = Math.min(ART.w / iw, ART.h / ih); // contain, never crop (spec: the card)
  const w = iw * k,
    h = ih * k;
  ctx.drawImage(img, ART.x + (ART.w - w) / 2, ART.y + (ART.h - h) / 2, w, h);
}

export function drawPostcard(
  ctx: DrawCtx,
  input: PostcardInput,
  assets: PostcardAssets,
): void {
  const t = postcardText(input);
  ctx.save();
  ctx.fillStyle = P.mat;
  ctx.fillRect(0, 0, POSTCARD_W, POSTCARD_H);
  ctx.fillStyle = P.paper;
  ctx.fillRect(M, M, POSTCARD_W - 2 * M, POSTCARD_H - 2 * M);
  ctx.strokeStyle = P.frame;
  ctx.lineWidth = 6;
  ctx.strokeRect(M, M, POSTCARD_W - 2 * M, POSTCARD_H - 2 * M);
  if (assets.art !== null) drawArt(ctx, assets.art);
  else drawPlaceholder(ctx);

  const x = ART.x,
    maxW = ART.w;
  let y = ART.y + ART.h + 80;
  ctx.textAlign = "left";
  ctx.textBaseline = "alphabetic";
  ctx.font = F.title;
  ctx.fillStyle = P.ink;
  for (const line of wrapLines(
    t.title,
    maxW,
    2,
    (s) => ctx.measureText(s).width,
  )) {
    ctx.fillText(line, x, y);
    y += 72;
  }
  y += 8;
  ctx.font = F.from;
  ctx.fillStyle = P.give;
  ctx.fillText(t.from, x, y);
  y += 52;
  if (t.note !== null) {
    ctx.font = F.note;
    for (const line of wrapLines(
      `“${t.note}”`,
      maxW,
      3,
      (s) => ctx.measureText(s).width,
    )) {
      ctx.fillText(line, x, y);
      y += 46;
    }
  }
  ctx.font = F.post;
  ctx.fillStyle = P.dust;
  const postY = POSTCARD_H - M - 70;
  const postLines = wrapLines(
    t.postmark,
    maxW,
    2,
    (s) => ctx.measureText(s).width,
  );
  for (const [i, line] of postLines.entries()) {
    ctx.fillText(line, x, postY - (postLines.length - 1 - i) * 34);
  }
  if (t.waited !== null) {
    // the headline: one line, give-pink, sitting on top of the small print.
    // Layout bound (art h 580): worst-case note baseline 1052; this line's top at
    // 2-line postmark ≈ 1208-34-56-44 = 1074 — keep that inequality if sizes change.
    ctx.font = F.waited;
    ctx.fillStyle = P.give;
    ctx.fillText(t.waited, x, postY - (postLines.length - 1) * 34 - 56);
  }
  ctx.font = F.mark;
  ctx.textAlign = "right";
  ctx.fillStyle = P.line;
  ctx.fillText("bendobundles", POSTCARD_W - M - 24, POSTCARD_H - M - 24);
  ctx.restore();
}

function toBlob(canvas: HTMLCanvasElement): Promise<Blob | null> {
  return new Promise((resolve) => {
    try {
      canvas.toBlob((b) => resolve(b), "image/png");
    } catch {
      resolve(null);
    }
  });
}

/** Size + draw a canvas. Used for the PREVIEW (display may be tainted — fine) and,
 *  on fresh elements, for encoding. */
export function paintPostcard(
  canvas: HTMLCanvasElement,
  input: PostcardInput,
  assets: PostcardAssets,
): boolean {
  canvas.width = POSTCARD_W;
  canvas.height = POSTCARD_H;
  const ctx = canvas.getContext("2d");
  if (ctx === null) return false;
  drawPostcard(ctx, input, assets);
  return true;
}

const freshCanvas = () => document.createElement("canvas");

/** Encode on a FRESH canvas. A taint (SecurityError / null with art) redraws WITHOUT
 *  art on ANOTHER fresh canvas — origin-clean is never restored on an element, so the
 *  same canvas would fail forever (plan review M1). Null only when the art-less encode
 *  also fails. */
export type PostcardRender = { blob: Blob | null; artUsed: boolean };

/** One encode attempt on its own canvas. Never throws (a paint can — drawImage on an
 *  undrawable image, a lost context: review-1 #1), and always releases the bitmap after:
 *  iOS Safari caps TOTAL canvas memory and frees it lazily, so a 1080×1350 canvas left
 *  sized after every toggle would eventually make getContext return null (review-1 #6). */
async function encodeOnce(
  c: HTMLCanvasElement,
  input: PostcardInput,
  assets: PostcardAssets,
): Promise<Blob | null> {
  try {
    if (!paintPostcard(c, input, assets)) return null;
    return await toBlob(c);
  } catch {
    return null;
  } finally {
    c.width = 0;
    c.height = 0;
  }
}

export async function renderPostcardBlob(
  input: PostcardInput,
  assets: PostcardAssets,
  makeCanvas: () => HTMLCanvasElement = freshCanvas,
): Promise<PostcardRender> {
  const b = await encodeOnce(makeCanvas(), input, assets);
  if (b !== null) return { blob: b, artUsed: assets.art !== null };
  if (assets.art === null) return { blob: null, artUsed: false };
  return {
    blob: await encodeOnce(makeCanvas(), input, { art: null }),
    artUsed: false,
  };
}
