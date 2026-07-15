import { render, screen, waitFor, act, fireEvent } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter, Route, Routes } from "react-router-dom";
import { vi, describe, it, expect, beforeEach, afterEach } from "vitest";
import { LinkPage } from "./LinkPage";
import type { LinkView } from "../api";

// Partial mock: fetch functions mocked, error classes REAL so instanceof
// checks in LinkPage exercise the production classes.
vi.mock("../api", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../api")>();
  return {
    ...actual,
    fetchLink: vi.fn(),
    claimGame: vi.fn(),
    steamOwnedForLink: vi.fn(),
    fetchGameDetail: vi.fn(),
    sendThanks: vi.fn(),
  };
});

vi.mock("../steamIdentity");

import {
  fetchLink,
  claimGame,
  NotFound,
  FetchFailed,
  steamOwnedForLink,
  fetchGameDetail,
  sendThanks,
} from "../api";
import { clearGameDetailCache } from "../gameDetailCache";
import {
  consumeReturnFragment,
  loadIdentity,
  beginConnect,
} from "../steamIdentity";

function renderLinkPage(token = "abc123") {
  return render(
    <MemoryRouter initialEntries={[`/l/${token}`]}>
      <Routes>
        <Route path="/l/:token" element={<LinkPage />} />
      </Routes>
    </MemoryRouter>,
  );
}

const baseLink: LinkView = {
  label: "Test Bundle",
  claims_allowed: 3,
  claims_used: 1,
  state: "active",
  games: [],
  claims: [],
};

describe("LinkPage", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    clearGameDetailCache();
    // Default steam state: no fragment, no stored identity
    vi.mocked(consumeReturnFragment).mockReturnValue(null);
    vi.mocked(loadIdentity).mockReturnValue(null);
    vi.mocked(beginConnect).mockImplementation(() => {});
  });

  it("renders ben's gift note with attribution when present", async () => {
    vi.mocked(fetchLink).mockResolvedValue({
      ...baseLink,
      gift_note: "picked these with you in mind",
    });
    renderLinkPage();
    await waitFor(() => {
      expect(
        screen.getByText(/picked these with you in mind/),
      ).toBeInTheDocument();
    });
    expect(screen.getByText(/— ben/)).toBeInTheDocument();
  });

  it("renders no note paragraph or attribution when gift_note is absent", async () => {
    vi.mocked(fetchLink).mockResolvedValue({ ...baseLink });
    renderLinkPage();
    await waitFor(() => {
      expect(screen.getByText("Test Bundle")).toBeInTheDocument();
    });
    expect(screen.queryByText(/— ben/)).not.toBeInTheDocument();
  });

  describe("say-thanks card", () => {
    const claimedLink: LinkView = {
      ...baseLink,
      claims: [
        {
          game_id: "gk1:mn",
          title: "Dome Keeper",
          state: "fulfilled",
          gift_url: "https://humble.example/g",
        },
      ],
    };

    it("shows the compose card when claims exist and no note was sent", async () => {
      vi.mocked(fetchLink).mockResolvedValue({ ...claimedLink });
      renderLinkPage();
      await waitFor(() => {
        expect(screen.getByText(/say thanks to ben/)).toBeInTheDocument();
      });
      expect(
        screen.getByRole("textbox", { name: /your thank-you note/i }),
      ).toBeInTheDocument();
    });

    it("hides the card entirely when there are no claims", async () => {
      vi.mocked(fetchLink).mockResolvedValue({ ...baseLink });
      renderLinkPage();
      await waitFor(() => {
        expect(screen.getByText("Test Bundle")).toBeInTheDocument();
      });
      expect(screen.queryByText(/say thanks to ben/)).not.toBeInTheDocument();
    });

    it("hides the card on a dead link even with claims", async () => {
      vi.mocked(fetchLink).mockResolvedValue({
        ...claimedLink,
        state: "revoked",
      });
      renderLinkPage();
      await waitFor(() => {
        expect(
          screen.getByText(/this invite isn't active anymore/),
        ).toBeInTheDocument();
      });
      expect(screen.queryByText(/say thanks to ben/)).not.toBeInTheDocument();
    });

    it("renders the sent note instead of the compose when thank_note is present", async () => {
      vi.mocked(fetchLink).mockResolvedValue({
        ...claimedLink,
        thank_note: "omg thank you!!",
      });
      renderLinkPage();
      await waitFor(() => {
        expect(screen.getByText(/omg thank you!!/)).toBeInTheDocument();
      });
      expect(screen.getByText(/— you, delivered to ben/)).toBeInTheDocument();
      expect(screen.queryByRole("textbox", { name: /your thank-you note/i })).not.toBeInTheDocument();
    });

    it("sends the trimmed note once and flips to the sent state", async () => {
      const user = userEvent.setup();
      vi.mocked(fetchLink).mockResolvedValue({ ...claimedLink });
      vi.mocked(sendThanks).mockResolvedValue({
        kind: "sent",
        thank_note: "ben you legend",
      });
      renderLinkPage("tok123");
      await waitFor(() => {
        expect(screen.getByText(/say thanks to ben/)).toBeInTheDocument();
      });

      const box = screen.getByRole("textbox", { name: /your thank-you note/i });
      await user.type(box, "  ben you legend  ");
      await user.click(screen.getByRole("button", { name: /send it/i }));

      await waitFor(() => {
        expect(screen.getByText(/ben you legend/)).toBeInTheDocument();
      });
      expect(sendThanks).toHaveBeenCalledWith("tok123", "ben you legend");
      expect(screen.getByText(/— you, delivered to ben/)).toBeInTheDocument();
      expect(
        screen.queryByRole("textbox", { name: /your thank-you note/i }),
      ).not.toBeInTheDocument();
    });

    it("keeps the compose and shows the message when the server refuses", async () => {
      const user = userEvent.setup();
      vi.mocked(fetchLink).mockResolvedValue({ ...claimedLink });
      vi.mocked(sendThanks).mockResolvedValue({
        kind: "refused",
        message: "thanks already sent",
      });
      renderLinkPage();
      await waitFor(() => {
        expect(screen.getByText(/say thanks to ben/)).toBeInTheDocument();
      });

      await user.type(
        screen.getByRole("textbox", { name: /your thank-you note/i }),
        "hello",
      );
      await user.click(screen.getByRole("button", { name: /send it/i }));

      await waitFor(() => {
        expect(screen.getByRole("alert")).toHaveTextContent(
          "thanks already sent",
        );
      });
      expect(
        screen.getByRole("textbox", { name: /your thank-you note/i }),
      ).toBeInTheDocument();
    });

    it("disables send while the note is empty", async () => {
      vi.mocked(fetchLink).mockResolvedValue({ ...claimedLink });
      renderLinkPage();
      await waitFor(() => {
        expect(screen.getByText(/say thanks to ben/)).toBeInTheDocument();
      });
      expect(screen.getByRole("button", { name: /send it/i })).toBeDisabled();
    });
  });

  it("shows loading state initially", () => {
    // never resolves
    vi.mocked(fetchLink).mockImplementation(() => new Promise(() => {}));
    renderLinkPage();
    expect(screen.getByText(/loading/i)).toBeInTheDocument();
  });

  it("shows not-found view on NotFound (genuine 404)", async () => {
    vi.mocked(fetchLink).mockRejectedValue(new NotFound());
    renderLinkPage();
    await waitFor(() => {
      expect(
        screen.getByRole("heading", { name: /link not found/i }),
      ).toBeInTheDocument();
    });
  });

  it('shows retryable error view (NOT "link not found") on transient failure', async () => {
    vi.mocked(fetchLink).mockRejectedValue(new FetchFailed());
    renderLinkPage();
    await waitFor(() => {
      expect(
        screen.getByRole("heading", { name: /couldn't load this page/i }),
      ).toBeInTheDocument();
    });
    expect(screen.queryByText(/link not found/i)).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: /retry/i })).toBeInTheDocument();
  });

  it("retry after a transient failure loads the link", async () => {
    const user = userEvent.setup();
    vi.mocked(fetchLink)
      .mockRejectedValueOnce(new FetchFailed())
      .mockResolvedValueOnce({ ...baseLink });
    renderLinkPage();
    await waitFor(() => {
      expect(
        screen.getByRole("button", { name: /retry/i }),
      ).toBeInTheDocument();
    });

    await user.click(screen.getByRole("button", { name: /retry/i }));
    await waitFor(() => {
      expect(screen.getByText("Test Bundle")).toBeInTheDocument();
    });
  });

  it("refresh after a claim keeps the page visible (no full-page loading flash)", async () => {
    const user = userEvent.setup();
    const withGame: LinkView = {
      ...baseLink,
      games: [
        {
          id: "1",
          title: "Portal",
          bundle: "B",
          key_type: "steam",
          artwork_url: null,
          steam_app_id: null,
        },
      ],
    };
    // First load resolves; the refreshTick refetch hangs forever — the old view must stay.
    vi.mocked(fetchLink)
      .mockResolvedValueOnce(withGame)
      .mockImplementation(() => new Promise(() => {}));
    vi.mocked(fetchGameDetail).mockResolvedValue({
      game: withGame.games[0]!,
      steam: null,
    });
    vi.mocked(claimGame).mockResolvedValue({
      kind: "refused",
      message: "already claimed",
    });

    renderLinkPage();
    await waitFor(() => {
      expect(screen.getByText("Portal")).toBeInTheDocument();
    });

    // Full claim round-trip: details → chest game (mash to burst) → dialog
    // confirm → refused → close (refresh)
    await user.click(screen.getByRole("button", { name: /details/i }));
    await waitFor(() => {
      expect(
        screen.getByRole("button", { name: /^claim$/i }),
      ).toBeInTheDocument();
    });
    await user.click(screen.getByRole("button", { name: /^claim$/i }));
    // Seed 30 + 18/mash → 4 mashes crest 100; the dialog opens after the
    // burst beat (CLAIM_BURST_MS).
    const masher = screen.getByRole("button", { name: /mash to claim/i });
    for (let i = 0; i < 4; i++) await user.click(masher);
    await waitFor(
      () =>
        expect(
          screen.getByRole("button", { name: /confirm/i }),
        ).toBeInTheDocument(),
      { timeout: 2000 },
    );
    await user.click(screen.getByRole("button", { name: /confirm/i }));
    await waitFor(() => {
      expect(screen.getByText("already claimed")).toBeInTheDocument();
    });
    await user.click(screen.getByRole("button", { name: /close/i }));

    // Soft refresh: header and grid still there, no full-page spinner.
    // (waitFor: the dialog-box title types in, so the full label lands async.)
    await waitFor(() => {
      expect(screen.getByText("Test Bundle")).toBeInTheDocument();
    });
    expect(screen.getByText("Portal")).toBeInTheDocument();
    expect(screen.queryByText(/^loading\.\.\.$/)).not.toBeInTheDocument();
  });

  it("shows loaded state with label and claim counts", async () => {
    vi.mocked(fetchLink).mockResolvedValue({ ...baseLink });
    renderLinkPage();
    await waitFor(() => {
      expect(screen.getByText("Test Bundle")).toBeInTheDocument();
      // counter is now the "N gifts waiting" beacon; aria-label preserves the count
      expect(screen.getByLabelText("1 of 3 claims used")).toBeInTheDocument();
    });
  });

  it("shows exhausted banner; grid browsable but the modal claim is disabled", async () => {
    const user = userEvent.setup();
    const game = {
      id: "1",
      title: "Portal",
      bundle: "B",
      key_type: "steam",
      artwork_url: null,
      steam_app_id: null,
    };
    vi.mocked(fetchLink).mockResolvedValue({
      ...baseLink,
      state: "exhausted",
      games: [game],
    });
    vi.mocked(fetchGameDetail).mockResolvedValue({ game, steam: null });
    renderLinkPage();
    await waitFor(() => {
      expect(screen.getByRole("alert")).toHaveTextContent(
        "you've used all your claims",
      );
    });
    // the grid never claims directly — details still browsable, modal claim disabled
    expect(
      screen.queryByRole("button", { name: /^claim$/i }),
    ).not.toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: /details/i }));
    await waitFor(() => {
      expect(screen.getByRole("button", { name: /^claim$/i })).toBeDisabled();
    });
  });

  it('shows revoked banner and no grid on state:"revoked"', async () => {
    vi.mocked(fetchLink).mockResolvedValue({
      ...baseLink,
      state: "revoked",
      games: [],
    });
    renderLinkPage();
    await waitFor(() => {
      expect(screen.getByRole("alert")).toHaveTextContent(
        "this invite isn't active anymore — bug ben",
      );
    });
    // no grid rendered at all — neither details nor claim affordances
    expect(
      screen.queryByRole("button", { name: /details/i }),
    ).not.toBeInTheDocument();
    expect(
      screen.queryByRole("button", { name: /claim/i }),
    ).not.toBeInTheDocument();
  });

  it('shows the same dead banner on state:"expired"', async () => {
    vi.mocked(fetchLink).mockResolvedValue({
      ...baseLink,
      state: "expired",
      games: [],
    });
    renderLinkPage();
    await waitFor(() => {
      expect(screen.getByRole("alert")).toHaveTextContent(
        "this invite isn't active anymore",
      );
    });
  });

  it("banner follows state, not games.length: revoked + games present is still revoked", async () => {
    // The exact ambiguity the state field exists to kill: a revoked link that
    // (for any backend reason) still carries a games array must NOT render the
    // amber exhausted banner.
    vi.mocked(fetchLink).mockResolvedValue({
      ...baseLink,
      state: "revoked",
      games: [
        {
          id: "1",
          title: "Celeste",
          bundle: "B",
          key_type: "steam",
          artwork_url: null,
          steam_app_id: null,
        },
      ],
    });
    renderLinkPage();
    await waitFor(() => {
      expect(screen.getByRole("alert")).toHaveTextContent(
        "this invite isn't active anymore",
      );
    });
    expect(screen.queryByText(/used all your claims/i)).not.toBeInTheDocument();
    // dead link → grid hidden regardless of games payload
    expect(screen.queryByText("Celeste")).not.toBeInTheDocument();
  });

  // ── steam identity ──────────────────────────────────────────────────────────

  describe("steam identity", () => {
    it("shows connect button when no steam identity", async () => {
      vi.mocked(fetchLink).mockResolvedValue(baseLink);
      renderLinkPage();
      await waitFor(() =>
        expect(screen.getByText("Test Bundle")).toBeInTheDocument(),
      );
      expect(
        screen.getByRole("button", { name: /connect to steam/i }),
      ).toBeInTheDocument();
    });

    it("shows persona chip and disconnect button when identity is stored", async () => {
      vi.mocked(fetchLink).mockResolvedValue(baseLink);
      vi.mocked(loadIdentity).mockReturnValue({
        steamid: "76561198000000001",
        persona: "Alice",
        owned: [],
        fetched_at: 0,
      });
      renderLinkPage();
      await waitFor(() =>
        expect(screen.getByText("Alice")).toBeInTheDocument(),
      );
      expect(
        screen.getByRole("button", { name: /disconnect/i }),
      ).toBeInTheDocument();
    });

    it('shows "you own this" pill on a card whose steam_app_id is in the owned set', async () => {
      vi.mocked(fetchLink).mockResolvedValue({
        ...baseLink,
        games: [
          {
            id: "1",
            title: "Portal",
            bundle: "B",
            key_type: "steam",
            artwork_url: null,
            steam_app_id: 420,
          },
        ],
      });
      vi.mocked(loadIdentity).mockReturnValue({
        steamid: "123",
        persona: "Alice",
        owned: [420],
        fetched_at: 0,
      });
      renderLinkPage();
      await waitFor(() =>
        expect(screen.getByText("Portal")).toBeInTheDocument(),
      );
      expect(screen.getByText(/you own this/i)).toBeInTheDocument();
    });

    it('does NOT show "you own this" pill when steam_app_id is not in owned set', async () => {
      vi.mocked(fetchLink).mockResolvedValue({
        ...baseLink,
        games: [
          {
            id: "1",
            title: "Portal",
            bundle: "B",
            key_type: "steam",
            artwork_url: null,
            steam_app_id: 420,
          },
        ],
      });
      vi.mocked(loadIdentity).mockReturnValue({
        steamid: "123",
        persona: "Alice",
        owned: [730],
        fetched_at: 0,
      });
      renderLinkPage();
      await waitFor(() =>
        expect(screen.getByText("Portal")).toBeInTheDocument(),
      );
      expect(screen.queryByText(/you own this/i)).not.toBeInTheDocument();
    });

    it("fetches owned on steam fragment, saves identity, shows persona chip", async () => {
      vi.mocked(fetchLink).mockResolvedValue(baseLink);
      vi.mocked(consumeReturnFragment).mockReturnValue({
        steamid: "76561198000000001",
        persona: "Alice",
      });
      vi.mocked(steamOwnedForLink).mockResolvedValue([420, 730]);
      renderLinkPage();
      await waitFor(() =>
        expect(screen.getByText("Alice")).toBeInTheDocument(),
      );
    });

    it('shows privacy message when steamOwnedForLink returns "private"', async () => {
      vi.mocked(fetchLink).mockResolvedValue(baseLink);
      vi.mocked(consumeReturnFragment).mockReturnValue({
        steamid: "76561198000000001",
        persona: "Alice",
      });
      vi.mocked(steamOwnedForLink).mockResolvedValue("private");
      renderLinkPage();
      // The <em> tag splits the text node — check the em element directly
      await waitFor(() =>
        expect(screen.getByText("game details")).toBeInTheDocument(),
      );
      // And the surrounding paragraph contains the privacy copy
      expect(
        screen.getByText(/couldn't read your library/i),
      ).toBeInTheDocument();
    });

    it("shows error message on verify_failed fragment", async () => {
      vi.mocked(fetchLink).mockResolvedValue(baseLink);
      vi.mocked(consumeReturnFragment).mockReturnValue({
        error: "verify_failed",
      });
      renderLinkPage();
      await waitFor(() =>
        expect(screen.getByText(/couldn't verify/i)).toBeInTheDocument(),
      );
    });

    it("shows error message on steam_unreachable fragment", async () => {
      vi.mocked(fetchLink).mockResolvedValue(baseLink);
      vi.mocked(consumeReturnFragment).mockReturnValue({
        error: "steam_unreachable",
      });
      renderLinkPage();
      await waitFor(() =>
        expect(
          screen.getByText(/steam.*unavailable|unavailable.*steam/i),
        ).toBeInTheDocument(),
      );
    });
  });

// ── typewriter, animations ON ────────────────────────────────────────────────
// test-setup.ts forces prefers-reduced-motion for the whole suite, so every
// test above runs the instant-snap path. These tests override matchMedia to
// motion-on + fake timers to exercise the animated entrance itself: the
// tap-to-skip affordance, the error→retry entrance (regression: an invisible
// pre-run behind the error view used to mark the entrance played and suppress
// it), and code-point slicing around emoji.
describe("typewriter (animations on)", () => {
  const realMatchMedia = window.matchMedia;

  beforeEach(() => {
    vi.useFakeTimers();
    window.matchMedia = ((query: string) =>
      ({
        matches: false, // motion allowed
        media: query,
        onchange: null,
        addListener: () => {},
        removeListener: () => {},
        addEventListener: () => {},
        removeEventListener: () => {},
        dispatchEvent: () => false,
      }) as MediaQueryList) as typeof window.matchMedia;
  });

  afterEach(() => {
    window.matchMedia = realMatchMedia;
    vi.useRealTimers();
  });

  const tick = async (ms: number) => {
    await act(async () => {
      await vi.advanceTimersByTimeAsync(ms);
    });
  };

  // the dialog box is the replay button's direct parent
  const dialogBox = () =>
    screen.getByRole("button", { name: "replay the text" })
      .parentElement as HTMLElement;

  it("clicking the dialog box mid-typing completes the text (tap-to-skip)", async () => {
    vi.mocked(fetchLink).mockResolvedValue({
      ...baseLink,
      gift_note: "a note from ben",
    });
    renderLinkPage();
    await tick(2900); // boot
    await tick(1100); // 1s thinking beat + a few ticks — typing in progress
    expect(screen.queryByText(/— ben/)).not.toBeInTheDocument();

    fireEvent.click(dialogBox());
    expect(screen.getByText(/a note from ben/)).toBeInTheDocument();
    expect(screen.getByText(/— ben/)).toBeInTheDocument();
  });

  it("pressing Enter mid-typing completes the text (keyboard skip)", async () => {
    vi.mocked(fetchLink).mockResolvedValue({
      ...baseLink,
      gift_note: "a note from ben",
    });
    renderLinkPage();
    await tick(2900);
    await tick(1100);
    expect(screen.queryByText(/— ben/)).not.toBeInTheDocument();

    fireEvent.keyDown(window, { key: "Enter" });
    expect(screen.getByText(/— ben/)).toBeInTheDocument();
  });

  it("error → retry still plays the entrance (no invisible pre-run suppression)", async () => {
    vi.mocked(fetchLink)
      .mockRejectedValueOnce(new FetchFailed())
      .mockResolvedValueOnce({ ...baseLink, gift_note: "hello friend" });
    renderLinkPage();
    await tick(2900); // boot done; error view up
    // dwell far past the old invisible-run window (~2.4s) — the regression
    // stamped the entrance as played during this dwell
    await tick(5000);

    fireEvent.click(screen.getByRole("button", { name: /retry/i }));
    await tick(0); // flush the refetch microtask

    // the entrance must ANIMATE after retry, not appear pre-typed
    await tick(1100); // 1s beat + a few ticks
    expect(screen.queryByText(/— ben/)).not.toBeInTheDocument();

    await tick(14 * 200); // let it finish
    expect(screen.getByText(/hello friend/)).toBeInTheDocument();
    expect(screen.getByText(/— ben/)).toBeInTheDocument();
  });

  it("never renders a split surrogate or U+FFFD while typing an emoji note", async () => {
    vi.mocked(fetchLink).mockResolvedValue({
      ...baseLink,
      gift_note: "\u{1F381}\u{1F381}\u{1F381}",
    });
    renderLinkPage();
    await tick(2900);
    await tick(1000); // thinking beat
    // walk the entire animation one 14ms tick at a time and inspect each frame
    for (let i = 0; i < 140; i++) {
      await tick(14);
      const text = document.body.textContent ?? "";
      expect(text).not.toMatch(/\uFFFD/);
      // a high surrogate not followed by a low surrogate = a split emoji
      expect(text).not.toMatch(/[\uD800-\uDBFF](?![\uDC00-\uDFFF])/);
    }
    expect(screen.getByText(/— ben/)).toBeInTheDocument();
  });

  it("types ZWJ emoji families atomically (grapheme clusters, not code points)", async () => {
    const family = "\u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467}"; // 👨‍👩‍👧, 5 code points
    vi.mocked(fetchLink).mockResolvedValue({
      ...baseLink,
      gift_note: `for the ${family} and you`,
    });
    renderLinkPage();
    await tick(2900);
    await tick(1000);
    // walk the animation; the family must only ever appear WHOLE — a lone
    // member (or partial ZWJ join) means the slicer cut inside the cluster
    for (let i = 0; i < 160; i++) {
      await tick(14);
      const text = document.body.textContent ?? "";
      if (text.includes("\u{1F468}")) {
        expect(text).toContain(family);
      }
    }
    expect(screen.getByText(/— ben/)).toBeInTheDocument();
  });
});

});
