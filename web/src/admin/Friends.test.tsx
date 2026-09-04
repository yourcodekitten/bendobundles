import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { vi, describe, it, expect, beforeEach, afterEach } from 'vitest';
import { MemoryRouter, Route, Routes } from 'react-router-dom';
import { Friends } from './Friends';
import type { AdminFriend } from '../api';

// Partial mock: the fetch wrappers are stubbed, but CreateLinkValidationError
// stays the real class — Friends.tsx branches on `instanceof`, and an
// automocked class constructor wouldn't carry `.message` through (mirrors
// Links.test.tsx's mock shape).
vi.mock('../api', async (importOriginal) => {
  const actual = await importOriginal<typeof import('../api')>();
  return {
    ...actual,
    adminFriends: vi.fn(),
    adminCreateFriend: vi.fn(),
    adminReissueFriendToken: vi.fn(),
    adminRevokeFriendToken: vi.fn(),
  };
});
import {
  adminFriends,
  adminCreateFriend,
  adminReissueFriendToken,
  adminRevokeFriendToken,
  CreateLinkValidationError,
} from '../api';

function renderFriends() {
  return render(
    <MemoryRouter initialEntries={['/admin/friends']}>
      <Routes>
        <Route path="/admin/friends" element={<Friends />} />
        <Route path="/admin/login" element={<div>login page</div>} />
      </Routes>
    </MemoryRouter>,
  );
}

const TOKEN_A = 'a'.repeat(64);
const TOKEN_B = 'b'.repeat(64);

const maya: AdminFriend = {
  id: 'f1',
  name: 'Maya',
  shelf_token: TOKEN_A,
  created_at: '2026-07-01T00:00:00Z',
};

const rae: AdminFriend = {
  id: 'f2',
  name: 'Rae',
  shelf_token: '',
  created_at: '2026-06-01T00:00:00Z',
};

describe('Friends', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    if (!navigator.clipboard) {
      Object.defineProperty(navigator, 'clipboard', {
        value: { writeText: vi.fn<() => Promise<void>>().mockResolvedValue(undefined) },
        configurable: true,
      });
    }
    vi.spyOn(navigator.clipboard, 'writeText').mockResolvedValue(undefined);
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  describe('loading + rendering', () => {
    it('renders the friend list with names and their shelf URLs', async () => {
      vi.mocked(adminFriends).mockResolvedValue([maya, rae]);
      renderFriends();

      await waitFor(() => screen.getByText('Maya'));

      expect(screen.getByText('Rae')).toBeInTheDocument();
      // Maya has a live token — shown via shelfUrl(), never a bare literal
      expect(screen.getByText(new RegExp(`/s/${TOKEN_A}$`))).toBeInTheDocument();
      // Rae is revoked (empty token) — no shelf link
      expect(screen.getByText(/no shelf link/i)).toBeInTheDocument();
    });

    it('shows error state when adminFriends fails', async () => {
      vi.mocked(adminFriends).mockRejectedValue(new Error('network timeout'));
      renderFriends();

      await waitFor(() =>
        expect(screen.getByText(/couldn't load friends/i)).toBeInTheDocument(),
      );
      expect(screen.getByRole('button', { name: /retry/i })).toBeInTheDocument();
    });
  });

  describe('create flow', () => {
    it('posts the name and shows the copyable shelf URL via shelfUrl (never a bare literal)', async () => {
      const user = userEvent.setup();
      vi.mocked(adminFriends).mockResolvedValue([]);
      vi.mocked(adminCreateFriend).mockResolvedValue({
        id: 'f9',
        name: 'Priya',
        shelf_token: TOKEN_A,
        shelf_url_path: `/s/${TOKEN_A}`,
      });
      renderFriends();

      await waitFor(() => screen.getByLabelText(/name/i));
      await user.type(screen.getByLabelText(/name/i), 'Priya');
      await user.click(screen.getByRole('button', { name: /add friend/i }));

      await waitFor(() =>
        expect(adminCreateFriend).toHaveBeenCalledWith('Priya'),
      );
      // window.location.origin + shelfUrlPath(token) — the shelfUrl() contract
      expect(
        screen.getByText(`${window.location.origin}/s/${TOKEN_A}`),
      ).toBeInTheDocument();
    });

    it('shows the server 422 message on validation failure', async () => {
      const user = userEvent.setup();
      vi.mocked(adminFriends).mockResolvedValue([]);
      vi.mocked(adminCreateFriend).mockRejectedValue(
        new CreateLinkValidationError('name must be 1-64 characters'),
      );
      renderFriends();

      await waitFor(() => screen.getByLabelText(/name/i));
      await user.type(screen.getByLabelText(/name/i), 'x'.repeat(65));
      await user.click(screen.getByRole('button', { name: /add friend/i }));

      await waitFor(() =>
        expect(screen.getByText('name must be 1-64 characters')).toBeInTheDocument(),
      );
    });
  });

  describe('reissue', () => {
    it('confirms in-voice and updates the shown token', async () => {
      const user = userEvent.setup();
      vi.mocked(adminFriends)
        .mockResolvedValueOnce([maya])
        .mockResolvedValueOnce([{ ...maya, shelf_token: TOKEN_B }]);
      vi.mocked(adminReissueFriendToken).mockResolvedValue({
        shelf_token: TOKEN_B,
        shelf_url_path: `/s/${TOKEN_B}`,
      });
      renderFriends();

      await waitFor(() => screen.getByText('Maya'));
      await user.click(screen.getByRole('button', { name: /reissue shelf link for maya/i }));

      // The in-voice confirm copy — must render before the request fires
      expect(
        screen.getByText(/the old link stops working — hand them the new one/i),
      ).toBeInTheDocument();
      expect(adminReissueFriendToken).not.toHaveBeenCalled();

      await user.click(screen.getByRole('button', { name: /confirm reissue for maya/i }));

      await waitFor(() => expect(adminReissueFriendToken).toHaveBeenCalledWith('f1'));
      await waitFor(() =>
        expect(screen.getByText(`${window.location.origin}/s/${TOKEN_B}`)).toBeInTheDocument(),
      );
      // old token no longer shown
      expect(screen.queryByText(new RegExp(TOKEN_A))).not.toBeInTheDocument();
    });

    it('disables confirm and cancel while the request is in flight — a second click fires no second POST', async () => {
      const user = userEvent.setup();
      vi.mocked(adminFriends).mockResolvedValue([maya]);
      let resolveReissue!: (v: { shelf_token: string; shelf_url_path: string }) => void;
      vi.mocked(adminReissueFriendToken).mockImplementation(
        () =>
          new Promise((resolve) => {
            resolveReissue = resolve;
          }),
      );
      renderFriends();

      await waitFor(() => screen.getByText('Maya'));
      await user.click(screen.getByRole('button', { name: /reissue shelf link for maya/i }));
      const confirm = screen.getByRole('button', { name: /confirm reissue for maya/i });
      await user.click(confirm);

      // request pending → both panel buttons disabled
      expect(adminReissueFriendToken).toHaveBeenCalledTimes(1);
      expect(confirm).toBeDisabled();
      expect(
        screen.getByRole('button', { name: /cancel reissue for maya/i }),
      ).toBeDisabled();

      // a second click must be a no-op — two POSTs is two token rotations
      await user.click(confirm);
      expect(adminReissueFriendToken).toHaveBeenCalledTimes(1);

      resolveReissue({ shelf_token: TOKEN_B, shelf_url_path: `/s/${TOKEN_B}` });
      await waitFor(() =>
        expect(
          screen.queryByRole('button', { name: /confirm reissue for maya/i }),
        ).not.toBeInTheDocument(),
      );
    });

    it("hedges the failure copy — the client can't know whether the old link survived", async () => {
      const user = userEvent.setup();
      vi.mocked(adminFriends).mockResolvedValue([maya]);
      vi.mocked(adminReissueFriendToken).mockRejectedValue(new Error('boom'));
      renderFriends();

      await waitFor(() => screen.getByText('Maya'));
      await user.click(screen.getByRole('button', { name: /reissue shelf link for maya/i }));
      await user.click(screen.getByRole('button', { name: /confirm reissue for maya/i }));

      await waitFor(() =>
        expect(screen.getByRole('alert')).toHaveTextContent(
          "couldn't reissue — the old link may still be live. try again.",
        ),
      );
    });
  });

  describe('reissue on a revoked friend', () => {
    it('shows an "issue a new link" affordance, confirms, fires the same endpoint, and refreshes', async () => {
      const user = userEvent.setup();
      vi.mocked(adminFriends)
        .mockResolvedValueOnce([rae])
        .mockResolvedValueOnce([{ ...rae, shelf_token: TOKEN_B }]);
      vi.mocked(adminReissueFriendToken).mockResolvedValue({
        shelf_token: TOKEN_B,
        shelf_url_path: `/s/${TOKEN_B}`,
      });
      renderFriends();

      await waitFor(() => screen.getByText('Rae'));
      await user.click(screen.getByRole('button', { name: /issue a new link for rae/i }));

      // arming copy fits the revoked state — no old link dies here
      expect(screen.getByText(/this hands them a brand-new link/i)).toBeInTheDocument();
      expect(adminReissueFriendToken).not.toHaveBeenCalled();

      await user.click(screen.getByRole('button', { name: /confirm reissue for rae/i }));

      await waitFor(() => expect(adminReissueFriendToken).toHaveBeenCalledWith('f2'));
      // list refreshed — the new shelf URL is on the row now
      await waitFor(() =>
        expect(
          screen.getByText(`${window.location.origin}/s/${TOKEN_B}`),
        ).toBeInTheDocument(),
      );
    });
  });

  describe('revoke', () => {
    it('shows "no shelf link" after a confirmed revoke', async () => {
      const user = userEvent.setup();
      vi.mocked(adminFriends)
        .mockResolvedValueOnce([maya])
        .mockResolvedValueOnce([{ ...maya, shelf_token: '' }]);
      vi.mocked(adminRevokeFriendToken).mockResolvedValue(undefined);
      renderFriends();

      await waitFor(() => screen.getByText('Maya'));
      await user.click(screen.getByRole('button', { name: /^revoke maya$/i }));
      // two-step: first click arms it, no request yet
      expect(adminRevokeFriendToken).not.toHaveBeenCalled();
      await user.click(screen.getByRole('button', { name: /confirm revoke maya/i }));

      await waitFor(() => expect(adminRevokeFriendToken).toHaveBeenCalledWith('f1'));
      await waitFor(() => expect(screen.getByText(/no shelf link/i)).toBeInTheDocument());
    });

    it('disables the armed button while the request is in flight — a second click fires no second POST', async () => {
      const user = userEvent.setup();
      vi.mocked(adminFriends).mockResolvedValue([maya]);
      let resolveRevoke!: () => void;
      vi.mocked(adminRevokeFriendToken).mockImplementation(
        () =>
          new Promise((resolve) => {
            resolveRevoke = () => resolve(undefined);
          }),
      );
      renderFriends();

      await waitFor(() => screen.getByText('Maya'));
      await user.click(screen.getByRole('button', { name: /^revoke maya$/i }));
      const confirm = screen.getByRole('button', { name: /confirm revoke maya/i });
      await user.click(confirm);

      expect(adminRevokeFriendToken).toHaveBeenCalledTimes(1);
      expect(confirm).toBeDisabled();

      await user.click(confirm);
      expect(adminRevokeFriendToken).toHaveBeenCalledTimes(1);

      resolveRevoke();
      await waitFor(() => expect(confirm).not.toBeDisabled());
    });
  });

  describe('copy', () => {
    it('the row copy button writes the shelf URL to the clipboard', async () => {
      const user = userEvent.setup();
      vi.mocked(adminFriends).mockResolvedValue([maya]);
      renderFriends();

      await waitFor(() => screen.getByText('Maya'));
      await user.click(screen.getByRole('button', { name: /copy shelf link for maya/i }));

      expect(navigator.clipboard.writeText).toHaveBeenCalledWith(
        `${window.location.origin}/s/${TOKEN_A}`,
      );
    });
  });
});
