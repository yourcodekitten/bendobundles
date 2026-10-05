import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, it, expect, vi, beforeEach } from "vitest";

vi.mock("../postcardCanvas", () => ({
  loadPostcardAssets: vi.fn(),
  paintPostcard: vi.fn(() => true),
  renderPostcardBlob: vi.fn(),
}));
import {
  loadPostcardAssets,
  paintPostcard,
  renderPostcardBlob,
} from "../postcardCanvas";
import { PostcardPanel } from "./PostcardPanel";
import type { PostcardInput } from "../postcard";

const base: Omit<PostcardInput, "note"> = {
  title: "Stardew Valley",
  artworkUrl: null,
  acquiredAt: "2014-03-02T17:00:00Z",
  unwrappedAt: "2026-10-05T16:00:00Z",
};

beforeEach(() => {
  vi.clearAllMocks();
  vi.mocked(loadPostcardAssets).mockResolvedValue({ art: null });
  vi.mocked(paintPostcard).mockReturnValue(true);
  vi.mocked(renderPostcardBlob).mockImplementation(async (input) => ({
    blob: new Blob([JSON.stringify(input)], { type: "image/png" }),
    artUsed: false,
  }));
});

describe("PostcardPanel", () => {
  it("loads assets ONCE and renders a canvas preview, not an <img> (D7/D10)", async () => {
    render(<PostcardPanel base={base} note="for you ♡" />);
    await waitFor(() =>
      expect(
        screen.getByRole("button", { name: /send a postcard/ }),
      ).toBeEnabled(),
    );
    expect(loadPostcardAssets).toHaveBeenCalledTimes(1);
    expect(screen.getByLabelText("postcard preview").tagName).toBe("CANVAS");
    await userEvent.click(screen.getByLabelText(/include ben's note/));
    await waitFor(() => expect(renderPostcardBlob).toHaveBeenCalledTimes(2));
    expect(loadPostcardAssets).toHaveBeenCalledTimes(1);
  });

  it("D8: the toggle is OFF by default and absent without a note", async () => {
    const { unmount } = render(<PostcardPanel base={base} note="for you ♡" />);
    expect(screen.getByLabelText(/include ben's note/)).not.toBeChecked();
    await waitFor(() => expect(renderPostcardBlob).toHaveBeenCalled());
    expect(vi.mocked(renderPostcardBlob).mock.calls[0]![0].note).toBeNull();
    expect(vi.mocked(paintPostcard).mock.calls[0]![1].note).toBeNull(); // preview == artifact input
    unmount();
    render(<PostcardPanel base={base} note={null} />);
    expect(screen.queryByLabelText(/include ben's note/)).toBeNull();
  });

  it("D11: save is disabled while the blob is for an older input, and a stale completion is discarded", async () => {
    let releaseFirst!: (r: { blob: Blob | null; artUsed: boolean }) => void;
    vi.mocked(renderPostcardBlob)
      .mockImplementationOnce(
        () =>
          new Promise((r) => {
            releaseFirst = r;
          }),
      )
      .mockImplementation(async (input) => ({
        blob: new Blob([String(input.note)], { type: "image/png" }),
        artUsed: false,
      }));
    render(<PostcardPanel base={base} note="for you ♡" />);
    await waitFor(() => expect(renderPostcardBlob).toHaveBeenCalledTimes(1));
    await userEvent.click(screen.getByLabelText(/include ben's note/)); // note ON while note-OFF render is pending
    await waitFor(() => expect(renderPostcardBlob).toHaveBeenCalledTimes(2));
    releaseFirst({
      blob: new Blob(["STALE"], { type: "image/png" }),
      artUsed: false,
    }); // the OFF render finishes LAST
    const share = vi.fn().mockResolvedValue(undefined);
    Object.assign(navigator, { canShare: () => true, share });
    const btn = await screen.findByRole("button", { name: /send a postcard/ });
    await waitFor(() => expect(btn).toBeEnabled());
    await userEvent.click(btn);
    const file = share.mock.calls[0]![0].files[0] as File;
    expect(await file.text()).toBe("for you ♡"); // the CURRENT input's blob, never "STALE"
  });

  it("D10: AbortError is a cancel — no download fallback", async () => {
    const share = vi
      .fn()
      .mockRejectedValue(Object.assign(new Error("x"), { name: "AbortError" }));
    Object.assign(navigator, { canShare: () => true, share });
    const create = vi.spyOn(URL, "createObjectURL");
    render(<PostcardPanel base={base} note={null} />);
    const btn = await screen.findByRole("button", { name: /send a postcard/ });
    await waitFor(() => expect(btn).toBeEnabled());
    await userEvent.click(btn);
    await waitFor(() => expect(share).toHaveBeenCalled());
    await new Promise((r) => setTimeout(r, 0)); // let the .catch microtask run BEFORE asserting absence
    expect(create).not.toHaveBeenCalled();
  });

  it("no canShare ⇒ download with the slug filename", async () => {
    Object.assign(navigator, { canShare: undefined, share: undefined });
    const create = vi.spyOn(URL, "createObjectURL").mockReturnValue("blob:x");
    const click = vi
      .spyOn(HTMLAnchorElement.prototype, "click")
      .mockImplementation(() => {});
    render(<PostcardPanel base={base} note={null} />);
    const btn = await screen.findByRole("button", { name: /send a postcard/ });
    await waitFor(() => expect(btn).toBeEnabled());
    await userEvent.click(btn);
    expect(create).toHaveBeenCalled();
    expect((click.mock.contexts[0] as HTMLAnchorElement).download).toBe(
      "postcard-stardew-valley.png",
    );
  });

  it("Lilith: a taint ⇒ the preview is repainted WITHOUT art before save enables", async () => {
    const art = { naturalWidth: 10, naturalHeight: 10 } as HTMLImageElement;
    vi.mocked(loadPostcardAssets).mockResolvedValue({ art });
    vi.mocked(renderPostcardBlob).mockResolvedValue({
      blob: new Blob(["artless"], { type: "image/png" }),
      artUsed: false,
    });
    render(
      <PostcardPanel
        base={{ ...base, artworkUrl: "https://hb.imgix.net/x.png" }}
        note={null}
      />,
    );
    await waitFor(() =>
      expect(
        screen.getByRole("button", { name: /send a postcard/ }),
      ).toBeEnabled(),
    );
    const calls = vi.mocked(paintPostcard).mock.calls;
    expect(calls[0]![2].art).toBe(art); // first paint showed the art
    expect(calls[calls.length - 1]![2].art).toBeNull(); // the preview now matches the artless blob
  });

  it("art kept ⇒ no art-less repaint", async () => {
    const art = { naturalWidth: 10, naturalHeight: 10 } as HTMLImageElement;
    vi.mocked(loadPostcardAssets).mockResolvedValue({ art });
    vi.mocked(renderPostcardBlob).mockResolvedValue({
      blob: new Blob(["withart"], { type: "image/png" }),
      artUsed: true,
    });
    render(
      <PostcardPanel
        base={{ ...base, artworkUrl: "https://hb.imgix.net/x.png" }}
        note={null}
      />,
    );
    await waitFor(() =>
      expect(
        screen.getByRole("button", { name: /send a postcard/ }),
      ).toBeEnabled(),
    );
    expect(
      vi.mocked(paintPostcard).mock.calls.every((c) => c[2].art === art),
    ).toBe(true);
  });

  it("D10 (OMBB): share is called SYNCHRONOUSLY inside the click — no await before it", async () => {
    const share = vi.fn().mockResolvedValue(undefined);
    Object.assign(navigator, { canShare: () => true, share });
    render(<PostcardPanel base={base} note={null} />);
    const btn = await screen.findByRole("button", { name: /send a postcard/ });
    await waitFor(() => expect(btn).toBeEnabled());
    fireEvent.click(btn); // sync dispatch: nothing between this line and the assertion may yield
    expect(share).toHaveBeenCalledTimes(1);
  });

  it("review-1 #1a: a THROWING preview paint never takes the page down — soft failure copy instead", async () => {
    vi.mocked(paintPostcard).mockImplementation(() => {
      throw new DOMException("x", "InvalidStateError");
    });
    render(<PostcardPanel base={base} note={null} />);
    expect(
      await screen.findByText(/couldn't make the postcard this time/),
    ).toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: /send a postcard/ }),
    ).toBeDisabled();
  });

  it("review-1 #1b: a REJECTED render shows the failure copy instead of a forever-disabled save", async () => {
    vi.mocked(renderPostcardBlob).mockRejectedValue(
      new DOMException("broken", "InvalidStateError"),
    );
    render(<PostcardPanel base={base} note={null} />);
    expect(
      await screen.findByText(/couldn't make the postcard this time/),
    ).toBeInTheDocument();
  });

  it("review-1 #2: a second click while a share is still open neither re-shares nor downloads", async () => {
    let n = 0;
    const share = vi.fn(() =>
      ++n === 1
        ? new Promise<void>(() => {})
        : Promise.reject(
            Object.assign(new Error("earlier share pending"), {
              name: "InvalidStateError",
            }),
          ),
    );
    Object.assign(navigator, { canShare: () => true, share });
    const create = vi.spyOn(URL, "createObjectURL").mockReturnValue("blob:x");
    render(<PostcardPanel base={base} note={null} />);
    const btn = await screen.findByRole("button", { name: /send a postcard/ });
    await waitFor(() => expect(btn).toBeEnabled());
    fireEvent.click(btn);
    fireEvent.click(btn);
    await new Promise((r) => setTimeout(r, 0));
    expect(share).toHaveBeenCalledTimes(1);
    expect(create).not.toHaveBeenCalled();
  });

  it("review-1 #2b: InvalidStateError from share is NOT a reason to download", async () => {
    const share = vi
      .fn()
      .mockRejectedValue(
        Object.assign(new Error("x"), { name: "InvalidStateError" }),
      );
    Object.assign(navigator, { canShare: () => true, share });
    const create = vi.spyOn(URL, "createObjectURL").mockReturnValue("blob:x");
    render(<PostcardPanel base={base} note={null} />);
    const btn = await screen.findByRole("button", { name: /send a postcard/ });
    await waitFor(() => expect(btn).toBeEnabled());
    fireEvent.click(btn);
    await new Promise((r) => setTimeout(r, 0));
    expect(create).not.toHaveBeenCalled();
  });

  it("review-1 #4: the preview is an image to assistive tech, and loading/failure are announced", async () => {
    render(<PostcardPanel base={base} note={null} />);
    expect(screen.getByRole("img", { name: /postcard preview/ }).tagName).toBe(
      "CANVAS",
    );
    expect(screen.getByRole("status")).toHaveTextContent(
      /getting your postcard ready/,
    );
  });

  it("render failure ⇒ soft message, save stays disabled", async () => {
    vi.mocked(renderPostcardBlob).mockResolvedValue({
      blob: null,
      artUsed: false,
    });
    render(<PostcardPanel base={base} note={null} />);
    expect(
      await screen.findByText(/couldn't make the postcard this time/),
    ).toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: /send a postcard/ }),
    ).toBeDisabled();
  });
});
