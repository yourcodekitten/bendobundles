import { describe, it, expect, vi, afterEach } from "vitest";
import { drawPostcard, loadPostcardAssets, renderPostcardBlob, type DrawCtx } from "./postcardCanvas";
import { postcardText, type PostcardInput } from "./postcard";

const input: PostcardInput = {
  title: "Stardew Valley",
  artworkUrl: "https://hb.imgix.net/stardew.png",
  note: "for you ♡",
  acquiredAt: "2014-03-02T17:00:00Z",
  unwrappedAt: "2026-10-05T16:00:00Z",
};

function recordingCtx() {
  const texts: string[] = [];
  const images: unknown[] = [];
  const ctx = {
    fillStyle: "", strokeStyle: "", lineWidth: 1, font: "", textAlign: "left", textBaseline: "alphabetic",
    fillRect: vi.fn(), strokeRect: vi.fn(), save: vi.fn(), restore: vi.fn(),
    fillText: vi.fn((s: string) => { texts.push(s); }),
    measureText: vi.fn((s: string) => ({ width: s.length * 20 }) as TextMetrics),
    drawImage: vi.fn((img: unknown) => { images.push(img); }),
  } as unknown as DrawCtx;
  return { ctx, texts, images };
}

afterEach(() => vi.restoreAllMocks());

describe("drawPostcard", () => {
  it("draws the title, from-line, note and postmark — and NO url text (D1)", () => {
    const { ctx, texts } = recordingCtx();
    drawPostcard(ctx, input, { art: null });
    const all = texts.join("\n");
    const t = postcardText(input);
    expect(all).toContain("Stardew Valley");
    expect(all).toContain(t.from);
    expect(all).toContain("for you ♡");
    expect(texts).toContain("waited 12 years for you"); // drawn as ONE line — the headline never wraps mid-phrase
    expect(all).toContain("bendobundles");
    expect(all).not.toMatch(/https?:|www\.|\.com\//);
  });

  it("note null ⇒ no quote drawn (D8 off)", () => {
    const { ctx, texts } = recordingCtx();
    drawPostcard(ctx, { ...input, note: null }, { art: null });
    expect(texts.join("\n")).not.toContain("for you ♡");
  });

  it("art present ⇒ drawImage once; art absent ⇒ placeholder, no drawImage (D2)", () => {
    const a = recordingCtx();
    const img = { naturalWidth: 400, naturalHeight: 400 } as HTMLImageElement;
    drawPostcard(a.ctx, input, { art: img });
    expect(a.images).toEqual([img]);
    const b = recordingCtx();
    drawPostcard(b.ctx, input, { art: null });
    expect(b.images).toEqual([]);
  });
});

describe("loadPostcardAssets", () => {
  it("null url ⇒ { art: null } without creating an Image", async () => {
    const spy = vi.spyOn(window, "Image");
    await expect(loadPostcardAssets(null, 50)).resolves.toEqual({ art: null });
    expect(spy).not.toHaveBeenCalled();
  });

  // ⚠️ B2 (plan review): vitest 4 cannot `new` an ARROW mock implementation — it throws, and an
  // error-path test then passes for the wrong reason. Every Image mock is a `function`.
  it("sets crossOrigin=anonymous and resolves art on load (D2)", async () => {
    let made: HTMLImageElement | null = null;
    vi.spyOn(window, "Image").mockImplementation(function () {
      const el = document.createElement("img");
      made = el;
      queueMicrotask(() => el.onload?.(new Event("load")));
      return el;
    });
    const a = await loadPostcardAssets("https://hb.imgix.net/x.png", 1000);
    expect(made!.crossOrigin).toBe("anonymous");
    expect(a.art).toBe(made);
  });

  it("error OR timeout ⇒ { art: null }, never rejects (D2) — and the Image WAS constructed", async () => {
    const errSpy = vi.spyOn(window, "Image").mockImplementation(function () {
      const el = document.createElement("img");
      queueMicrotask(() => el.onerror?.(new Event("error")));
      return el;
    });
    await expect(loadPostcardAssets("https://x/y.png", 1000)).resolves.toEqual({ art: null });
    expect(errSpy).toHaveBeenCalledTimes(1); // a throwing constructor must not satisfy this arm
    errSpy.mockRestore();
    const hangSpy = vi.spyOn(window, "Image").mockImplementation(function () {
      return document.createElement("img"); // never settles ⇒ the 20ms cap must fire
    });
    await expect(loadPostcardAssets("https://x/y.png", 20)).resolves.toEqual({ art: null });
    expect(hangSpy).toHaveBeenCalledTimes(1);
  });
});

describe("renderPostcardBlob (M1: taint ⇒ a FRESH canvas, never the same one)", () => {
  function fakeCanvas(toBlobImpl: (cb: (b: Blob | null) => void) => void) {
    const { ctx } = recordingCtx();
    return {
      width: 0, height: 0,
      getContext: () => ctx,
      toBlob: vi.fn((cb: (b: Blob | null) => void) => toBlobImpl(cb)),
    } as unknown as HTMLCanvasElement;
  }
  const img = { naturalWidth: 10, naturalHeight: 10 } as HTMLImageElement;

  it("clean canvas ⇒ one canvas, one blob", async () => {
    const made: HTMLCanvasElement[] = [];
    const b = await renderPostcardBlob(input, { art: img }, () => {
      const c = fakeCanvas((cb) => cb(new Blob(["ok"], { type: "image/png" })));
      made.push(c); return c;
    });
    expect(await b.blob!.text()).toBe("ok");
    expect(b.artUsed).toBe(true);
    expect(made).toHaveLength(1);
  });

  it("tainted ⇒ retries WITHOUT art on a SECOND canvas and still saves (D2)", async () => {
    const made: HTMLCanvasElement[] = [];
    const b = await renderPostcardBlob(input, { art: img }, () => {
      const first = made.length === 0;
      const c = fakeCanvas((cb) => {
        if (first) throw Object.assign(new Error("tainted"), { name: "SecurityError" });
        cb(new Blob(["artless"], { type: "image/png" }));
      });
      made.push(c); return c;
    });
    expect(made).toHaveLength(2);
    expect(await b.blob!.text()).toBe("artless");
    expect(b.artUsed).toBe(false); // the panel must repaint its preview without art
  });

  it("no art and encode fails ⇒ null, no pointless retry", async () => {
    const made: HTMLCanvasElement[] = [];
    const b = await renderPostcardBlob(input, { art: null }, () => {
      const c = fakeCanvas((cb) => cb(null)); made.push(c); return c;
    });
    expect(b).toEqual({ blob: null, artUsed: false });
    expect(made).toHaveLength(1);
  });
});
