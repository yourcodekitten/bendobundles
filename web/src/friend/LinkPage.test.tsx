import { render, screen, waitFor, act, fireEvent } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter, Route, Routes } from "react-router-dom";
import { vi, describe, it, expect, beforeEach, afterEach } from "vitest";
import { LinkPage } from "./LinkPage";
import { ThanksCard } from "./ThanksCard";
import type { GameView, LinkView } from "../api";

// Partial mock: fetch functions mocked, error classes REAL so instanceof
// checks in LinkPage exercise the production classes.
vi.mock("../api", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../api")>();
  return {
    ...actual,
    fetchLink: vi.fn(),
    claimGame: vi.fn(),
    steamOwnedForLink: vi.fn(),
    steamWishlistForLink: vi.fn(),
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
  steamWishlistForLink,
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

// copied verbatim from GameGrid.test.tsx:7 — local const there, not exported
const makeGame = (overrides: Partial<GameView> & { id: string }): GameView => ({
  title: "Default Game",
  bundle: "Default Bundle",
  key_type: "steam",
  artwork_url: null,
  steam_app_id: null,
  ...overrides,
});

function mockLink(link: LinkView) {
  vi.mocked(fetchLink).mockResolvedValue(link);
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
    // ⭐ default: no wishlist overlap. MUST live here — the steam-identity tests restore an
    // identity, and a bare vi.fn() would hand the page `undefined` instead of a promise.
    vi.mocked(steamWishlistForLink).mockResolvedValue([]);
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

  it("curated link renders games in server order without shuffling", async () => {
    // 4 games: a preserved order under the open-shelf shuffle would be a 1/24
    // coincidence — this asserts the shuffle is BYPASSED, not merely lucky.
    mockLink({
      ...baseLink,
      curated: true,
      games: [
        makeGame({ id: "g-3", title: "Ccc" }),
        makeGame({ id: "g-1", title: "Aaa" }),
        makeGame({ id: "g-4", title: "Ddd" }),
        makeGame({ id: "g-2", title: "Bbb" }),
      ],
    });
    renderLinkPage();
    await waitFor(() => screen.getByText("Ccc"));
    const labels = screen
      .getAllByRole("button", { name: /details/i })
      .map((b) => b.getAttribute("aria-label"));
    expect(labels).toEqual([
      "Ccc — details", "Aaa — details", "Ddd — details", "Bbb — details",
    ]);
  });

  it("curated link swaps the dialog body copy", async () => {
    mockLink({ ...baseLink, curated: true, games: [makeGame({ id: "g-1" })] });
    renderLinkPage();
    await waitFor(() => screen.getByText(/picked these out just for you/));
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

    it("hides the card entirely when nothing has been claimed", async () => {
      vi.mocked(fetchLink).mockResolvedValue({ ...baseLink, claims_used: 0 });
      renderLinkPage();
      await waitFor(() => {
        expect(screen.getByText("Test Bundle")).toBeInTheDocument();
      });
      expect(screen.queryByText(/say thanks to ben/)).not.toBeInTheDocument();
    });

    it("still shows a SENT note when claims_used dropped to 0 after compensation", async () => {
      // claims_used is non-monotonic: a thanks can land during the
      // claim→park→compensate span, ending at claims_used 0 WITH a note. The
      // friend's delivered note must never vanish from their own page — only
      // the COMPOSE is gated on claims_used (converge pass).
      vi.mocked(fetchLink).mockResolvedValue({
        ...baseLink,
        claims_used: 0,
        thank_note: "sent before the gift fell through",
        claims: [
          {
            game_id: "gk1:mn",
            title: "Dome Keeper",
            state: "compensated",
            gift_url: null,
          },
        ],
      });
      renderLinkPage();
      await waitFor(() => {
        expect(
          screen.getByText(/sent before the gift fell through/),
        ).toBeInTheDocument();
      });
      expect(
        screen.queryByRole("textbox", { name: /your thank-you note/i }),
      ).not.toBeInTheDocument();
    });

    it("hides the card when the only claim was compensated (claims_used back to 0)", async () => {
      // the gate must use claims_used — the server's predicate — not
      // claims.length: the claims list includes compensated records, so a
      // friend whose one claim failed fulfillment would otherwise get a
      // compose box that can only ever 409 (review pass 2)
      vi.mocked(fetchLink).mockResolvedValue({
        ...baseLink,
        claims_used: 0,
        claims: [
          {
            game_id: "gk1:mn",
            title: "Dome Keeper",
            state: "compensated",
            gift_url: null,
          },
        ],
      });
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
      expect(sendThanks).toHaveBeenCalledTimes(1);
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
      // the compose must be USABLE after a refusal, not just present — a
      // sending-flag that never resets would freeze both controls forever
      // and this test would still pass on presence alone (review pass 1)
      expect(
        screen.getByRole("textbox", { name: /your thank-you note/i }),
      ).toBeEnabled();
      expect(screen.getByRole("button", { name: /send it/i })).toBeEnabled();
    });

    it("refused send triggers a refetch so a cross-tab 'already sent' converges", async () => {
      // the 409 body carries only the error string, so the refetch is the ONLY
      // path that can deliver the other tab's note text (review pass 2)
      const user = userEvent.setup();
      vi.mocked(fetchLink)
        .mockResolvedValueOnce({ ...claimedLink })
        .mockResolvedValue({
          ...claimedLink,
          thank_note: "sent from my phone",
        });
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
        "hello again",
      );
      await user.click(screen.getByRole("button", { name: /send it/i }));

      // the refused path bumps refreshTick → second fetchLink resolves with the
      // note → the derived prop flips the card to the sent view
      await waitFor(() => {
        expect(screen.getByText(/sent from my phone/)).toBeInTheDocument();
      });
      expect(fetchLink).toHaveBeenCalledTimes(2);
      expect(
        screen.queryByRole("textbox", { name: /your thank-you note/i }),
      ).not.toBeInTheDocument();
    });

    it("flips to the sent view when a later thankNote prop surfaces a note sent elsewhere", async () => {
      // cross-tab: the card is mounted composing; a refetch (claim in another
      // tab bumps refreshTick) delivers thank_note through the prop. The card
      // derives its view from the prop, so it must flip WITHOUT a remount —
      // mount-time-only seeding would show the compose forever (review pass 1).
      const { rerender } = render(<ThanksCard token="tok123" />);
      expect(
        screen.getByRole("textbox", { name: /your thank-you note/i }),
      ).toBeInTheDocument();

      rerender(<ThanksCard token="tok123" thankNote="sent from my phone" />);
      expect(screen.getByText(/sent from my phone/)).toBeInTheDocument();
      expect(
        screen.queryByRole("textbox", { name: /your thank-you note/i }),
      ).not.toBeInTheDocument();
    });

    it("disables the button while a send is in flight — a double-click can't fire twice", async () => {
      const user = userEvent.setup();
      vi.mocked(fetchLink).mockResolvedValue({ ...claimedLink });
      // never resolves: the send stays in flight for the whole test
      vi.mocked(sendThanks).mockImplementation(() => new Promise(() => {}));
      renderLinkPage();
      await waitFor(() => {
        expect(screen.getByText(/say thanks to ben/)).toBeInTheDocument();
      });

      await user.type(
        screen.getByRole("textbox", { name: /your thank-you note/i }),
        "hello",
      );
      const send = screen.getByRole("button", { name: /send/i });
      await user.click(send);
      expect(send).toBeDisabled();
      await user.click(send); // second click lands on a disabled button
      expect(sendThanks).toHaveBeenCalledTimes(1);
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


  describe("wishing well", () => {
    afterEach(() => {
      vi.useRealTimers();
      vi.restoreAllMocks();
    });
    const stored = { steamid: "76561198000000001", persona: "Alice", owned: [], fetched_at: 0 };
    const shelf = (n: number) => Array.from({ length: n }, (_, i) =>
      makeGame({ id: String(i), title: `G${i}`, steam_app_id: 1000 + i }));

    it("stars a wishlisted game and shows the count line (N ≥ 1)", async () => {
      vi.mocked(fetchLink).mockResolvedValue({ ...baseLink, games: shelf(5) });
      vi.mocked(loadIdentity).mockReturnValue(stored);
      vi.mocked(steamWishlistForLink).mockResolvedValue([
        { appid: 1003, added: 1678000000 }, { appid: 1001, added: 1678000000 }]);
      renderLinkPage();
      await waitFor(() => expect(screen.getByText("⭐ 2 of these are on your wishlist")).toBeInTheDocument());
      expect(screen.getAllByText("⭐ on your wishlist")).toHaveLength(2);
    });

    it("singular count copy", async () => {
      vi.mocked(fetchLink).mockResolvedValue({ ...baseLink, games: shelf(3) });
      vi.mocked(loadIdentity).mockReturnValue(stored);
      vi.mocked(steamWishlistForLink).mockResolvedValue([{ appid: 1002, added: 1678000000 }]);
      renderLinkPage();
      await waitFor(() => expect(screen.getByText("⭐ 1 of these is on your wishlist")).toBeInTheDocument());
    });

    it("no count line at 0 — and none on failure (silent, no error text)", async () => {
      vi.mocked(fetchLink).mockResolvedValue({ ...baseLink, games: shelf(3) });
      vi.mocked(loadIdentity).mockReturnValue(stored);
      vi.mocked(steamWishlistForLink).mockRejectedValue(new FetchFailed());
      renderLinkPage();
      await waitFor(() => expect(screen.getByText("G0")).toBeInTheDocument());
      expect(screen.queryByText(/of these (are|is) on your wishlist/)).not.toBeInTheDocument();
      expect(screen.queryByText(/wishlist/i)).not.toBeInTheDocument();
      expect(screen.queryByRole("alert")).not.toBeInTheDocument();
      // steamError renders as a plain <p> (no role) — assert its TEXT is absent too (pass 2 #4)
      expect(screen.queryByText(/Steam is currently unavailable|couldn.t verify/i)).not.toBeInTheDocument();
    });

    it("open shelf: starred cards render first", async () => {
      // Pin the shuffle: rand≈1 makes Fisher–Yates the identity, so G6 is first ONLY via the float.
      vi.spyOn(Math, "random").mockReturnValue(0.999);
      vi.mocked(fetchLink).mockResolvedValue({ ...baseLink, games: shelf(8) });
      vi.mocked(loadIdentity).mockReturnValue(stored);
      vi.mocked(steamWishlistForLink).mockResolvedValue([{ appid: 1006, added: 1678000000 }]);
      renderLinkPage();
      await waitFor(() => expect(screen.getByText("⭐ on your wishlist")).toBeInTheDocument());
      const titles = screen.getAllByRole("heading", { level: 3 }).map((h) => h.textContent);
      expect(titles[0]).toBe("G6");
    });

    it("curated: order is ben's, stars do not move, count line still shows", async () => {
      const games = shelf(4);
      vi.mocked(fetchLink).mockResolvedValue({ ...baseLink, curated: true, games });
      vi.mocked(loadIdentity).mockReturnValue(stored);
      vi.mocked(steamWishlistForLink).mockResolvedValue([{ appid: 1003, added: 1678000000 }]);
      renderLinkPage();
      await waitFor(() => expect(screen.getByText("⭐ 1 of these is on your wishlist")).toBeInTheDocument());
      const titles = screen.getAllByRole("heading", { level: 3 }).map((h) => h.textContent);
      expect(titles).toEqual(["G0", "G1", "G2", "G3"]);
    });

    it("the hold ENGAGES: a deferred wishlist keeps the loading view, then floats (OMBB M1)", async () => {
      vi.useFakeTimers({ shouldAdvanceTime: false }); // OMBB nit: no wall-clock drift past 400 on a loaded box
      vi.spyOn(Math, "random").mockReturnValue(0.999); // identity shuffle ⇒ G5 first only via the float
      let resolveWish!: (v: { appid: number; added: number }[]) => void;
      vi.mocked(fetchLink).mockResolvedValue({ ...baseLink, games: shelf(6) });
      vi.mocked(loadIdentity).mockReturnValue(stored);
      vi.mocked(steamWishlistForLink).mockReturnValue(new Promise((r) => { resolveWish = r; }));
      renderLinkPage();
      await act(async () => { await vi.advanceTimersByTimeAsync(0); });
      await act(async () => { await vi.advanceTimersByTimeAsync(200); });
      // the HOLD is the grid's, not the page's (review pass 2 #7): the page is up, the grid empty
      expect(screen.queryByText("loading...")).not.toBeInTheDocument();
      expect(screen.getByText("Test Bundle")).toBeInTheDocument();
      expect(screen.queryAllByRole("heading", { level: 3 })).toHaveLength(0);
      await act(async () => { resolveWish([{ appid: 1005, added: 1678000000 }]); });
      await act(async () => { await vi.advanceTimersByTimeAsync(0); });
      expect(screen.getAllByRole("heading", { level: 3 })[0]).toHaveTextContent("G5");
    });

    it("the hold is CAPPED at 400ms: held at 399, rendered (unfloated) at 401 (OMBB M1)", async () => {
      vi.useFakeTimers({ shouldAdvanceTime: false });
      vi.spyOn(Math, "random").mockReturnValue(0.999);
      vi.mocked(fetchLink).mockResolvedValue({ ...baseLink, games: shelf(4) });
      vi.mocked(loadIdentity).mockReturnValue(stored);
      vi.mocked(steamWishlistForLink).mockReturnValue(new Promise(() => {})); // never lands
      renderLinkPage();
      await act(async () => { await vi.advanceTimersByTimeAsync(0); });
      await act(async () => { await vi.advanceTimersByTimeAsync(399); });
      expect(screen.queryAllByRole("heading", { level: 3 })).toHaveLength(0);
      await act(async () => { await vi.advanceTimersByTimeAsync(2); });
      expect(screen.getAllByRole("heading", { level: 3 }).map((h) => h.textContent)).toEqual(["G0", "G1", "G2", "G3"]);
    });

    it("a wishlist that lands AFTER the hold cap adds stars without moving any card", async () => {
      vi.useFakeTimers({ shouldAdvanceTime: true });
      // Pinned (OMBB M2): identity order puts G7 LAST, so a wrong late re-rank (G7 → first)
      // cannot pass on shuffle luck. Unpinned, it passed 1 run in 8 for free.
      vi.spyOn(Math, "random").mockReturnValue(0.999);
      let resolveWish!: (v: { appid: number; added: number }[]) => void;
      vi.mocked(fetchLink).mockResolvedValue({ ...baseLink, games: shelf(8) });
      vi.mocked(loadIdentity).mockReturnValue(stored);
      vi.mocked(steamWishlistForLink).mockReturnValue(new Promise((r) => { resolveWish = r; }));
      renderLinkPage();
      await act(async () => { await vi.advanceTimersByTimeAsync(0); });   // let fetchLink resolve ⇒ hold timer scheduled
      await act(async () => { await vi.advanceTimersByTimeAsync(450); }); // past WISH_HOLD_MS
      await waitFor(() => expect(screen.getByText("G0")).toBeInTheDocument());
      const before = screen.getAllByRole("heading", { level: 3 }).map((h) => h.textContent);
      await act(async () => { resolveWish([{ appid: 1007, added: 1678000000 }]); });
      await waitFor(() => expect(screen.getByText("⭐ on your wishlist")).toBeInTheDocument());
      const after = screen.getAllByRole("heading", { level: 3 }).map((h) => h.textContent);
      expect(after).toEqual(before);
    });

    it("two copies of one wishlisted title count ONCE (the grid dedupes by title)", async () => {
      vi.mocked(fetchLink).mockResolvedValue({ ...baseLink, games: [
        makeGame({ id: "a", title: "Portal", steam_app_id: 420 }),
        makeGame({ id: "b", title: "Portal", steam_app_id: 420 }),
        makeGame({ id: "c", title: "Other", steam_app_id: 999 }),
      ] });
      vi.mocked(loadIdentity).mockReturnValue(stored);
      vi.mocked(steamWishlistForLink).mockResolvedValue([{ appid: 420, added: 1678000000 }]);
      renderLinkPage();
      await waitFor(() => expect(screen.getByText("⭐ 1 of these is on your wishlist")).toBeInTheDocument());
    });

    it("restore path fetches the wishlist with the stored steamid", async () => {
      vi.mocked(fetchLink).mockResolvedValue({ ...baseLink, games: shelf(2) });
      vi.mocked(loadIdentity).mockReturnValue(stored);
      renderLinkPage();
      await waitFor(() => expect(steamWishlistForLink).toHaveBeenCalledWith("abc123", stored.steamid));
    });

    it("disconnect clears the stars and the count line", async () => {
      vi.mocked(fetchLink).mockResolvedValue({ ...baseLink, games: shelf(3) });
      vi.mocked(loadIdentity).mockReturnValue(stored);
      vi.mocked(steamWishlistForLink).mockResolvedValue([{ appid: 1001, added: 1678000000 }]);
      renderLinkPage();
      await waitFor(() => expect(screen.getByText("⭐ 1 of these is on your wishlist")).toBeInTheDocument());
      await userEvent.click(screen.getByRole("button", { name: /disconnect/i }));
      await waitFor(() => expect(screen.queryByText(/on your wishlist/)).not.toBeInTheDocument());
    });

    // ── review pass 2 (#259) ──

    it("an EMPTY wishlist ([] — private or empty, same bytes) shows no count line and no error", async () => {
      vi.mocked(fetchLink).mockResolvedValue({ ...baseLink, games: shelf(3) });
      vi.mocked(loadIdentity).mockReturnValue(stored);
      vi.mocked(steamWishlistForLink).mockResolvedValue([]);
      renderLinkPage();
      await waitFor(() => expect(screen.getByText("G0")).toBeInTheDocument());
      await act(async () => { await new Promise((r) => setTimeout(r, 30)); });
      expect(screen.queryByText(/on your wishlist/)).not.toBeInTheDocument();
      expect(screen.queryByText(/Steam is currently unavailable|couldn.t verify/i)).not.toBeInTheDocument();
    });

    it("the count line's slot is RESERVED before the wishlist lands, so a late line moves no card (pass 2 #1)", async () => {
      vi.mocked(fetchLink).mockResolvedValue({ ...baseLink, curated: true, games: shelf(3) });
      vi.mocked(loadIdentity).mockReturnValue(stored);
      let res!: (v: { appid: number; added: number }[]) => void;
      vi.mocked(steamWishlistForLink).mockReturnValue(new Promise((r) => { res = r; }));
      renderLinkPage();
      await waitFor(() => expect(screen.getByText("G0")).toBeInTheDocument());
      const slot = screen.getByTestId("wish-count-line");
      expect(slot).toHaveTextContent("");
      await act(async () => { res([{ appid: 1001, added: 1678000000 }]); });
      await waitFor(() => expect(screen.getByTestId("wish-count-line")).toHaveTextContent("⭐ 1 of these is on your wishlist"));
      expect(screen.getByTestId("wish-count-line")).toBe(slot); // same node: it filled, it did not mount
    });

    it("open shelf: the count matches the grid's title-deduped cards, not raw appids (pass 2 #6)", async () => {
      vi.mocked(fetchLink).mockResolvedValue({ ...baseLink, games: [
        makeGame({ id: "a", title: "Edition A", steam_app_id: 420 }),
        makeGame({ id: "b", title: "Edition B", steam_app_id: 420 }),
      ] });
      vi.mocked(loadIdentity).mockReturnValue(stored);
      vi.mocked(steamWishlistForLink).mockResolvedValue([{ appid: 420, added: 1678000000 }]);
      renderLinkPage();
      await waitFor(() => expect(screen.getAllByText("⭐ on your wishlist")).toHaveLength(2));
      expect(screen.getByText("⭐ 2 of these are on your wishlist")).toBeInTheDocument();
    });

    // ── review pass 1 (#259): each of these reproduced a defect before its fix ──

    it("OpenID return: an owned+wishlisted game never floats (the hold waits for owned too)", async () => {
      vi.spyOn(Math, "random").mockReturnValue(0.999);
      vi.mocked(consumeReturnFragment).mockReturnValue({ steamid: "76561198000000001", persona: "A" });
      let resolveOwned!: (v: number[]) => void;
      vi.mocked(steamOwnedForLink).mockReturnValue(new Promise((r) => { resolveOwned = r; }));
      vi.mocked(steamWishlistForLink).mockResolvedValue([{ appid: 1005, added: 1678000000 }]);
      vi.mocked(fetchLink).mockResolvedValue({ ...baseLink, games: shelf(6) });
      renderLinkPage();
      // the grid must render (hold cap) BEFORE owned lands — that is the race
      await waitFor(() => expect(screen.getAllByRole("heading", { level: 3 })).toHaveLength(6));
      await act(async () => { resolveOwned([1005]); });
      await waitFor(() => expect(screen.getByText(/you own this/i)).toBeInTheDocument());
      const titles = screen.getAllByRole("heading", { level: 3 }).map((h) => h.textContent);
      expect(titles[0]).toBe("G0");
      expect(screen.queryByText("⭐ on your wishlist")).not.toBeInTheDocument();
    });

    it("a wrapped gift that unwraps in-session gets its stars (and its shuffle)", async () => {
      vi.spyOn(Math, "random").mockReturnValue(0); // rand=0 Fisher–Yates on 4 ⇒ G1 G2 G3 G0
      vi.mocked(loadIdentity).mockReturnValue(stored);
      vi.mocked(steamWishlistForLink)
        .mockRejectedValueOnce(new FetchFailed()) // the proxy 409s while sealed
        .mockResolvedValue([{ appid: 1002, added: 1678000000 }]);
      vi.mocked(fetchLink)
        .mockResolvedValueOnce({ ...baseLink, state: "sealed", unlocks_in_seconds: 1, unlocks_at: "2026-09-30T12:00:00Z" } as LinkView)
        .mockResolvedValue({ ...baseLink, games: shelf(4) });
      renderLinkPage();
      await waitFor(() => expect(screen.getByText("⭐ 1 of these is on your wishlist")).toBeInTheDocument(), { timeout: 5000 });
      const titles = screen.getAllByRole("heading", { level: 3 }).map((h) => h.textContent);
      expect(titles[0]).toBe("G2"); // floated; and the rest shuffled, not frozen over the sealed []
      expect(titles).not.toEqual(["G2", "G0", "G1", "G3"]);
    }, 10000);

    it("owned fails, wishlist works ⇒ no stars (no identity, no disconnect to clear them)", async () => {
      vi.mocked(consumeReturnFragment).mockReturnValue({ steamid: "76561198000000001", persona: "A" });
      vi.mocked(steamOwnedForLink).mockRejectedValue(new FetchFailed());
      vi.mocked(steamWishlistForLink).mockResolvedValue([{ appid: 1002, added: 1678000000 }]);
      vi.mocked(fetchLink).mockResolvedValue({ ...baseLink, games: shelf(4) });
      renderLinkPage();
      await waitFor(() => expect(screen.getByText(/Steam is currently unavailable/)).toBeInTheDocument());
      await act(async () => { await new Promise((r) => setTimeout(r, 50)); });
      expect(screen.queryByText(/on your wishlist/)).not.toBeInTheDocument();
    });

    it("disconnect while the wishlist is in flight ⇒ the late result never paints stars", async () => {
      vi.mocked(loadIdentity).mockReturnValue(stored);
      let res!: (v: { appid: number; added: number }[]) => void;
      vi.mocked(steamWishlistForLink).mockReturnValue(new Promise((r) => { res = r; }));
      vi.mocked(fetchLink).mockResolvedValue({ ...baseLink, games: shelf(3) });
      renderLinkPage();
      await waitFor(() => expect(screen.getAllByRole("heading", { level: 3 })).toHaveLength(3), { timeout: 3000 });
      await userEvent.click(screen.getByRole("button", { name: /disconnect/i }));
      await act(async () => { res([{ appid: 1001, added: 1678000000 }]); });
      await act(async () => { await new Promise((r) => setTimeout(r, 50)); });
      expect(screen.queryByText(/on your wishlist/)).not.toBeInTheDocument();
    });

    it("curated: two copies are two gifts, so the count line counts cards", async () => {
      vi.mocked(fetchLink).mockResolvedValue({ ...baseLink, curated: true, games: [
        makeGame({ id: "a", title: "Portal", steam_app_id: 420 }),
        makeGame({ id: "b", title: "Portal", steam_app_id: 420 }),
      ] });
      vi.mocked(loadIdentity).mockReturnValue(stored);
      vi.mocked(steamWishlistForLink).mockResolvedValue([{ appid: 420, added: 1678000000 }]);
      renderLinkPage();
      await waitFor(() => expect(screen.getByText("⭐ 2 of these are on your wishlist")).toBeInTheDocument());
      expect(screen.getAllByText("⭐ on your wishlist")).toHaveLength(2);
    });

    it("wishlist failure never blocks owned", async () => {
      vi.mocked(fetchLink).mockResolvedValue({ ...baseLink, games: shelf(2) });
      vi.mocked(consumeReturnFragment).mockReturnValue({ steamid: "76561198000000001", persona: "Alice" });
      vi.mocked(steamOwnedForLink).mockResolvedValue([1000]);
      vi.mocked(steamWishlistForLink).mockRejectedValue(new FetchFailed());
      renderLinkPage();
      await waitFor(() => expect(screen.getByText(/you own this/i)).toBeInTheDocument());
    });
  });
});
