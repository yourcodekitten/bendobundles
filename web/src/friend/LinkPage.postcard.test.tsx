// review 2 (M1): LinkPage must hand the link's gift_note to the ClaimDialog — the
// postcard's D13 fallback note. Deleting that prop left all 573 tests green.
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter, Route, Routes } from "react-router-dom";
import { vi, it, expect, beforeEach } from "vitest";
import type { LinkView } from "../api";

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
const captured: { linkNote?: string }[] = [];
vi.mock("./ClaimDialog", () => ({
  ClaimDialog: (p: { linkNote?: string }) => {
    captured.push(p);
    return <div>claim dialog stub</div>;
  },
}));

import { fetchLink, fetchGameDetail, steamWishlistForLink } from "../api";
import {
  consumeReturnFragment,
  loadIdentity,
  beginConnect,
} from "../steamIdentity";
import { clearGameDetailCache } from "../gameDetailCache";
import { LinkPage } from "./LinkPage";

beforeEach(() => {
  vi.clearAllMocks();
  captured.length = 0;
  clearGameDetailCache();
  vi.mocked(consumeReturnFragment).mockReturnValue(null);
  vi.mocked(loadIdentity).mockReturnValue(null);
  vi.mocked(beginConnect).mockImplementation(() => {});
  vi.mocked(steamWishlistForLink).mockResolvedValue([]);
});

const LINK: LinkView = {
  label: "Test Bundle",
  gift_note: "picked these with you in mind",
  claims_allowed: 3,
  claims_used: 0,
  state: "active",
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
  claims: [],
};

async function openChest(user: ReturnType<typeof userEvent.setup>) {
  vi.mocked(fetchLink).mockResolvedValue(LINK);
  vi.mocked(fetchGameDetail).mockResolvedValue({
    game: LINK.games[0]!,
    steam: null,
  });
  render(
    <MemoryRouter initialEntries={["/l/abc123"]}>
      <Routes>
        <Route path="/l/:token" element={<LinkPage />} />
      </Routes>
    </MemoryRouter>,
  );
  await waitFor(() => expect(screen.getByText("Portal")).toBeInTheDocument());
  await user.click(screen.getByRole("button", { name: /details/i }));
  await waitFor(() =>
    expect(
      screen.getByRole("button", { name: /^claim$/i }),
    ).toBeInTheDocument(),
  );
  await user.click(screen.getByRole("button", { name: /^claim$/i }));
}

// ROOT CAUSE of OMBB's blocker 2: the chest's charge is "drained in the parent, filled by
// mashing" (ClaimChest.tsx; GameDetailModal.tsx CLAIM_DRAIN_PER_SEC=15, seed 30, +18/mash).
// "4 mashes crest 100" holds only if the clicks outrun the drain. So: mash until it opens,
// bounded, and report how many clicks it took. The sibling LinkPage.test.tsx round-trip
// carries the same latent 4-click assumption.
async function mashUntilOpen(
  user: ReturnType<typeof userEvent.setup>,
  cadenceMs: number,
) {
  let clicks = 0;
  for (
    let i = 0;
    i < 60 && screen.queryByText("claim dialog stub") === null;
    i++
  ) {
    const masher = screen.queryByRole("button", { name: /mash to claim/i });
    if (masher === null) break; // burst in progress — the wait below catches the dialog
    await user.click(masher);
    clicks++;
    if (cadenceMs > 0) await new Promise((r) => setTimeout(r, cadenceMs));
  }
  await waitFor(
    () => expect(screen.getByText("claim dialog stub")).toBeInTheDocument(),
    { timeout: 10_000 },
  );
  return clicks;
}

it("passes the link's gift_note to the claim dialog as linkNote (D13 fallback)", async () => {
  const user = userEvent.setup();
  await openChest(user);
  await mashUntilOpen(user, 0);
  expect(captured[captured.length - 1]!.linkNote).toBe(
    "picked these with you in mind",
  );
}, 20_000);

it("a click cadence SLOWER than 4-to-crest still opens the dialog — the race, made deterministic", async () => {
  // Lilith: reproduce the race IN the test, never by loading the shared box. At 400ms between
  // mashes the drain takes 6 per gap ⇒ net +12/click: 4 clicks reach only ~78 (the old shape
  // fails), ~6 are needed. Asserting > 4 proves this cadence actually exercises the race.
  const user = userEvent.setup();
  await openChest(user);
  const clicks = await mashUntilOpen(user, 400);
  expect(clicks).toBeGreaterThan(4);
  expect(captured[captured.length - 1]!.linkNote).toBe(
    "picked these with you in mind",
  );
}, 20_000);
