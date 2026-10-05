import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter, Route, Routes } from "react-router-dom";
import { vi, describe, it, expect, beforeEach } from "vitest";
import { ShelfPage } from "./ShelfPage";
import type { ShelfView } from "../api";

// Partial mock: fetchShelf mocked, error classes REAL so instanceof checks in
// ShelfPage exercise the production classes — mirrors LinkPage.test.tsx.
vi.mock("../api", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../api")>();
  return {
    ...actual,
    fetchShelf: vi.fn(),
  };
});

import { fetchShelf, NotFound, FetchFailed } from "../api";

vi.mock("../postcardCanvas", async (orig) => ({
  ...(await orig<typeof import("../postcardCanvas")>()),
  canMakePostcards: vi.fn(() => true),
  loadPostcardAssets: vi.fn().mockResolvedValue({ art: null }),
  paintPostcard: vi.fn(() => true),
  renderPostcardBlob: vi.fn().mockResolvedValue({ blob: new Blob(["x"], { type: "image/png" }), artUsed: false }),
}));


function renderShelfPage(token = "abc123") {
  return render(
    <MemoryRouter initialEntries={[`/s/${token}`]}>
      <Routes>
        <Route path="/s/:token" element={<ShelfPage />} />
      </Routes>
    </MemoryRouter>,
  );
}

const baseShelf: ShelfView = {
  name: "sarah",
  gifts: [],
};

describe("ShelfPage", () => {
  const gift = (id: string, title: string) => ({
    game_id: id, title, artwork_url: null, unwrapped_at: "2026-10-05T16:00:00Z",
    gift_note: `note for ${title}`, thank_note: null, acquired_at: "2014-03-02T17:00:00Z",
  });

  it("D13: ticking the note on a shelf postcard carries that gift's gift_note", async () => {
    vi.mocked(fetchShelf).mockResolvedValue({ name: "sarah", gifts: [gift("g1", "Celeste")] });
    renderShelfPage();
    await userEvent.click(await screen.findByRole("button", { name: /send a postcard/ }));
    await userEvent.click(await screen.findByLabelText(/include ben's note/));
    const { renderPostcardBlob } = await import("../postcardCanvas");
    await waitFor(() =>
      expect(vi.mocked(renderPostcardBlob).mock.calls.some((c) => c[0].note === "note for Celeste")).toBe(true),
    );
  });

  it("one postcard open at a time: opening B closes A, and B's entry toggles it shut", async () => {
    vi.mocked(fetchShelf).mockResolvedValue({ name: "sarah", gifts: [gift("a", "Celeste"), gift("b", "Hades")] });
    renderShelfPage();
    const [a, b] = await screen.findAllByRole("button", { name: /send a postcard/ });
    await userEvent.click(a!);
    expect(screen.getAllByRole("img", { name: /postcard of/ })).toHaveLength(1);
    await userEvent.click(b!);
    const open = screen.getAllByRole("img", { name: /postcard of/ });
    expect(open).toHaveLength(1);
    expect(open[0]).toHaveAccessibleName(/Hades/);
    await userEvent.click(b!);
    expect(screen.queryAllByRole("img", { name: /postcard of/ })).toHaveLength(0);
  });

  it("D6: no canvas toBlob ⇒ the shelf offers no postcard at all", async () => {
    const { canMakePostcards } = await import("../postcardCanvas");
    vi.mocked(canMakePostcards).mockReturnValue(false);
    try {
      vi.mocked(fetchShelf).mockResolvedValue({ name: "sarah", gifts: [gift("a", "Celeste")] });
      renderShelfPage();
      await waitFor(() => screen.getByText("Celeste"));
      expect(screen.queryByRole("button", { name: /send a postcard/ })).toBeNull();
    } finally {
      vi.mocked(canMakePostcards).mockReturnValue(true);
    }
  });

  it("each gift offers a postcard carrying its acquired_at and unwrap instant", async () => {
    vi.mocked(fetchShelf).mockResolvedValue({
      name: "sarah",
      gifts: [{
        game_id: "g1", title: "Stardew Valley", artwork_url: null,
        unwrapped_at: "2026-10-05T16:00:00Z", gift_note: "for you ♡", thank_note: null,
        acquired_at: "2014-03-02T17:00:00Z",
      }],
    });
    renderShelfPage();
    await userEvent.click(await screen.findByRole("button", { name: /send a postcard/ }));
    const { renderPostcardBlob } = await import("../postcardCanvas");
    await waitFor(() => expect(renderPostcardBlob).toHaveBeenCalled());
    expect(vi.mocked(renderPostcardBlob).mock.calls[0]![0]).toEqual({
      title: "Stardew Valley", artworkUrl: null, note: null,
      acquiredAt: "2014-03-02T17:00:00Z", unwrappedAt: "2026-10-05T16:00:00Z",
    });
  });

  beforeEach(() => {
    vi.clearAllMocks();
  });

  it("shows loading state initially", () => {
    // never resolves
    vi.mocked(fetchShelf).mockImplementation(() => new Promise(() => {}));
    renderShelfPage();
    expect(screen.getByText(/loading/i)).toBeInTheDocument();
  });

  it("renders the warm greeting with the friend's name and the ♡", async () => {
    vi.mocked(fetchShelf).mockResolvedValue({ ...baseShelf, name: "sarah" });
    renderShelfPage();
    await waitFor(() => {
      expect(
        screen.getByRole("heading", { name: /ben's shelf for sarah/i }),
      ).toBeInTheDocument();
    });
    expect(screen.getByText(/♡/)).toBeInTheDocument();
  });

  it("renders gifts in exactly the server's array order — fed non-chronological on purpose, so a client-side sort would flip it", async () => {
    // Deliberately NOT oldest-first: if the component sorted by unwrapped_at
    // (either direction), the render order would differ from the array order
    // and this assert would catch it. An already-sorted feed cannot.
    vi.mocked(fetchShelf).mockResolvedValue({
      name: "sarah",
      gifts: [
        {
          game_id: "g2",
          title: "Portal",
          artwork_url: "https://example.com/portal.jpg",
          unwrapped_at: "2024-06-01T12:00:00Z",
          gift_note: null,
          thank_note: null,
        },
        {
          game_id: "g1",
          title: "Celeste",
          artwork_url: null,
          unwrapped_at: "2022-03-01T12:00:00Z",
          gift_note: null,
          thank_note: null,
        },
      ],
    });
    renderShelfPage();
    await waitFor(() => screen.getByText("Celeste"));

    // order: the page must render exactly the server's array order, never resort it
    const titles = screen
      .getAllByRole("heading", { level: 2 })
      .map((h) => h.textContent);
    expect(titles).toEqual(["Portal", "Celeste"]);

    // year is derived from unwrapped_at
    expect(screen.getByText(/unwrapped 2022/)).toBeInTheDocument();
    expect(screen.getByText(/unwrapped 2024/)).toBeInTheDocument();

    // artwork_url present renders an <img>; null falls back gracefully (no broken img)
    expect(screen.getByRole("img", { name: "Portal" })).toHaveAttribute(
      "src",
      "https://example.com/portal.jpg",
    );
    expect(screen.queryByRole("img", { name: "Celeste" })).not.toBeInTheDocument();
  });

  it("renders ben's gift_note and the friend's thank_note when present", async () => {
    vi.mocked(fetchShelf).mockResolvedValue({
      name: "sarah",
      gifts: [
        {
          game_id: "g1",
          title: "Celeste",
          artwork_url: null,
          unwrapped_at: "2022-03-01T12:00:00Z",
          gift_note: "thought of you the second I saw this",
          thank_note: "omg I love it!!",
        },
      ],
    });
    renderShelfPage();
    await waitFor(() => {
      expect(
        screen.getByText(/thought of you the second I saw this/),
      ).toBeInTheDocument();
    });
    // ben's voice is attributed to him
    expect(screen.getByText(/— ben/)).toBeInTheDocument();
    // the friend's reply is present and attributed as their own words back
    expect(screen.getByText(/omg I love it!!/)).toBeInTheDocument();
    expect(screen.getByText(/delivered to ben/)).toBeInTheDocument();
  });

  it("renders no note paragraphs when gift_note and thank_note are both absent", async () => {
    vi.mocked(fetchShelf).mockResolvedValue({
      name: "sarah",
      gifts: [
        {
          game_id: "g1",
          title: "Celeste",
          artwork_url: null,
          unwrapped_at: "2022-03-01T12:00:00Z",
          gift_note: null,
          thank_note: null,
        },
      ],
    });
    renderShelfPage();
    await waitFor(() => screen.getByText("Celeste"));
    expect(screen.queryByText(/— ben/)).not.toBeInTheDocument();
    expect(screen.queryByText(/delivered to ben/)).not.toBeInTheDocument();
  });

  it("shows the in-voice empty-shelf copy when there are no gifts", async () => {
    vi.mocked(fetchShelf).mockResolvedValue({ ...baseShelf, gifts: [] });
    renderShelfPage();
    await waitFor(() => {
      expect(
        screen.getByText(/this shelf is waiting for its first story/),
      ).toBeInTheDocument();
    });
  });

  it("shows a soft not-found state on NotFound (404) — no shop/claim language", async () => {
    vi.mocked(fetchShelf).mockRejectedValue(new NotFound());
    renderShelfPage();
    await waitFor(() => {
      expect(
        screen.getByRole("heading", { name: /shelf not found/i }),
      ).toBeInTheDocument();
    });
    expect(screen.queryByRole("button")).not.toBeInTheDocument();
  });

  it('shows a soft retry state (NOT "shelf not found") on a transient fetch error', async () => {
    vi.mocked(fetchShelf).mockRejectedValue(new FetchFailed());
    renderShelfPage();
    await waitFor(() => {
      expect(
        screen.getByRole("heading", { name: /couldn't load this shelf/i }),
      ).toBeInTheDocument();
    });
    expect(screen.queryByText(/shelf not found/i)).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: /retry/i })).toBeInTheDocument();
  });

  it("retry after a transient failure loads the shelf", async () => {
    const user = userEvent.setup();
    vi.mocked(fetchShelf)
      .mockRejectedValueOnce(new FetchFailed())
      .mockResolvedValueOnce({ ...baseShelf, name: "sarah", gifts: [] });
    renderShelfPage();
    await waitFor(() => {
      expect(screen.getByRole("button", { name: /retry/i })).toBeInTheDocument();
    });

    await user.click(screen.getByRole("button", { name: /retry/i }));
    await waitFor(() => {
      expect(
        screen.getByRole("heading", { name: /ben's shelf for sarah/i }),
      ).toBeInTheDocument();
    });
  });

  it("derives the unwrapped year in America/New_York, not UTC — a late-Dec-EST unwrap stays in the old year", async () => {
    vi.mocked(fetchShelf).mockResolvedValue({
      name: "sarah",
      gifts: [
        {
          game_id: "g1",
          // 2026-01-01T04:30:00Z is 2025-12-31 23:30 EST — UTC rolls to
          // 2026, but the friend (and ben) are in America/New_York and this
          // gift was unwrapped in 2025.
          title: "Celeste",
          artwork_url: null,
          unwrapped_at: "2026-01-01T04:30:00Z",
          gift_note: null,
          thank_note: null,
        },
      ],
    });
    renderShelfPage();
    await waitFor(() => screen.getByText("Celeste"));
    expect(screen.getByText(/unwrapped 2025/)).toBeInTheDocument();
    expect(screen.queryByText(/unwrapped 2026/)).not.toBeInTheDocument();
  });

  it('renders no "unwrapped" label at all for an unparseable unwrapped_at — never a dangling "unwrapped "', async () => {
    vi.mocked(fetchShelf).mockResolvedValue({
      name: "sarah",
      gifts: [
        {
          game_id: "g1",
          title: "Celeste",
          artwork_url: null,
          unwrapped_at: "not-a-date",
          gift_note: null,
          thank_note: null,
        },
      ],
    });
    renderShelfPage();
    await waitFor(() => screen.getByText("Celeste"));
    expect(screen.queryByText(/unwrapped/)).not.toBeInTheDocument();
  });

  it("never renders a claim affordance — the postcard entry is its only button", async () => {
    vi.mocked(fetchShelf).mockResolvedValue({
      name: "sarah",
      gifts: [
        {
          game_id: "g1",
          title: "Celeste",
          artwork_url: null,
          unwrapped_at: "2022-03-01T12:00:00Z",
          gift_note: null,
          thank_note: null,
        },
      ],
    });
    renderShelfPage();
    await waitFor(() => screen.getByText("Celeste"));
    const buttons = screen.queryAllByRole("button");
    expect(buttons).toHaveLength(1); // OMBB: an empty list would pass .every() vacuously
    expect(buttons.every((b) => /send a postcard/.test(b.textContent ?? ""))).toBe(true); // only the postcard entry
    expect(screen.queryByRole("link")).not.toBeInTheDocument();
  });
});
