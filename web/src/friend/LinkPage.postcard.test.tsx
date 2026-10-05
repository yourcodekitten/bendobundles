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

it("passes the link's gift_note to the claim dialog as linkNote (D13 fallback)", async () => {
  const user = userEvent.setup();
  const link: LinkView = {
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
  vi.mocked(fetchLink).mockResolvedValue(link);
  vi.mocked(fetchGameDetail).mockResolvedValue({
    game: link.games[0]!,
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
  // ROOT CAUSE of OMBB's blocker 2 (measured under load, not guessed): the chest's charge is
  // "drained in the parent, filled by mashing" (ClaimChest.tsx). "4 mashes crest 100" holds
  // only if the clicks outrun the drain — under CPU load they don't, the dialog NEVER opens,
  // and no timeout can fix that (10s waited, 2 of 3 loaded runs red). So: mash until it opens,
  // bounded. The sibling LinkPage.test.tsx round-trip carries the same latent assumption.
  for (
    let i = 0;
    i < 60 && screen.queryByText("claim dialog stub") === null;
    i++
  ) {
    const masher = screen.queryByRole("button", { name: /mash to claim/i });
    if (masher === null) break; // burst in progress — let the wait below catch the dialog
    await user.click(masher);
  }
  await waitFor(
    () => expect(screen.getByText("claim dialog stub")).toBeInTheDocument(),
    { timeout: 10_000 },
  );
  expect(captured[captured.length - 1]!.linkNote).toBe(
    "picked these with you in mind",
  );
}, 20_000);
