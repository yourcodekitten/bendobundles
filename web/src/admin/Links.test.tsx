import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { vi, describe, it, expect, beforeEach } from 'vitest';
import { MemoryRouter, Route, Routes } from 'react-router-dom';
import { Links } from './Links';
import type { AdminLink, AdminClaimView } from '../api';

vi.mock('../api');
import { adminLinks, adminCreateLink, adminRevoke, adminLinkClaims } from '../api';

function renderLinks() {
  return render(
    <MemoryRouter initialEntries={['/admin/links']}>
      <Routes>
        <Route path="/admin/links" element={<Links />} />
        <Route path="/admin/login" element={<div>login page</div>} />
      </Routes>
    </MemoryRouter>,
  );
}

const link1: AdminLink = {
  token: 'tok-abc123',
  label: 'Alice',
  claims_allowed: 3,
  claims_used: 1,
  revoked: false,
  expires_at: null,
  created_at: '2026-07-01T00:00:00Z',
};

const link2: AdminLink = {
  token: 'tok-def456',
  label: 'Bob',
  claims_allowed: 1,
  claims_used: 1,
  revoked: true,
  expires_at: '2026-08-01T00:00:00Z',
  created_at: '2026-06-15T00:00:00Z',
};

describe('Links', () => {
  // Spy on navigator.clipboard.writeText before each test.
  // happy-dom v20 provides a native Clipboard implementation, so vi.spyOn
  // is more reliable than Object.defineProperty (which may not override a
  // prototype getter). vi.restoreAllMocks() in afterEach cleans up the spy.
  beforeEach(() => {
    vi.clearAllMocks();
    // Ensure clipboard object exists (happy-dom should provide it, but guard anyway)
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
    it('renders link list with label, usage, dates, and revoked chip', async () => {
      vi.mocked(adminLinks).mockResolvedValue([link1, link2]);
      renderLinks();

      await waitFor(() => screen.getByText('Alice'));

      expect(screen.getByText('1/3 used')).toBeInTheDocument();
      expect(screen.getByText('Bob')).toBeInTheDocument();
      // link2 is revoked
      expect(screen.getByText('revoked')).toBeInTheDocument();
      // link1 has no expiry
      expect(screen.getByText(/expires never/i)).toBeInTheDocument();
    });

    it('shows error state when adminLinks fails', async () => {
      vi.mocked(adminLinks).mockRejectedValue(new Error('network timeout'));
      renderLinks();

      await waitFor(() =>
        expect(screen.getByText(/couldn't load links/i)).toBeInTheDocument(),
      );
      expect(screen.getByRole('button', { name: /retry/i })).toBeInTheDocument();
    });

    it('retry button re-calls adminLinks and shows loaded state', async () => {
      const user = userEvent.setup();
      vi.mocked(adminLinks)
        .mockRejectedValueOnce(new Error('network timeout'))
        .mockResolvedValue([link1]);

      renderLinks();
      await waitFor(() => screen.getByRole('button', { name: /retry/i }));

      await user.click(screen.getByRole('button', { name: /retry/i }));

      await waitFor(() => expect(screen.getByText('Alice')).toBeInTheDocument());
      expect(adminLinks).toHaveBeenCalledTimes(2);
    });

    it('non-Unauthorized error shows error state, not login redirect', async () => {
      vi.mocked(adminLinks).mockRejectedValue(new Error('ECONNREFUSED'));
      renderLinks();

      await waitFor(() =>
        expect(screen.getByText(/couldn't load links/i)).toBeInTheDocument(),
      );
      expect(screen.queryByText('login page')).not.toBeInTheDocument();
    });
  });

  describe('create form', () => {
    it('submits form and shows full invite URL with accessible copy button', async () => {
      const user = userEvent.setup();
      vi.mocked(adminLinks).mockResolvedValue([]);
      vi.mocked(adminCreateLink).mockResolvedValue({
        token: 'tok-new',
        url_path: '/l/tok-new',
      });

      renderLinks();
      // Wait for loaded state (form present)
      await waitFor(() => screen.getByRole('button', { name: /create invite link/i }));

      await user.type(screen.getByRole('textbox', { name: 'label' }), 'Charlie');
      await user.click(screen.getByRole('button', { name: /create invite link/i }));

      // Wait for both: api called AND full URL in DOM (after reload settles)
      const expectedUrl = `${window.location.origin}/l/tok-new`;
      await waitFor(() => {
        expect(adminCreateLink).toHaveBeenCalledWith('Charlie', 1, undefined);
        expect(screen.getByText(expectedUrl)).toBeInTheDocument();
      });

      // Copy button accessible-named with the link's label
      expect(
        screen.getByRole('button', { name: 'copy invite for Charlie' }),
      ).toBeInTheDocument();
    });

    it('create copy button writes the full URL to navigator.clipboard', async () => {
      const user = userEvent.setup();
      vi.mocked(adminLinks).mockResolvedValue([]);
      vi.mocked(adminCreateLink).mockResolvedValue({
        token: 'tok-new',
        url_path: '/l/tok-new',
      });

      renderLinks();
      await waitFor(() => screen.getByRole('button', { name: /create invite link/i }));

      await user.type(screen.getByRole('textbox', { name: 'label' }), 'Charlie');
      await user.click(screen.getByRole('button', { name: /create invite link/i }));

      await waitFor(() =>
        screen.getByRole('button', { name: 'copy invite for Charlie' }),
      );

      await user.click(screen.getByRole('button', { name: 'copy invite for Charlie' }));

      expect(navigator.clipboard.writeText).toHaveBeenCalledWith(
        `${window.location.origin}/l/tok-new`,
      );
    });
  });

  describe('copy invite URL — per-row', () => {
    it('copy invite for <label> writes the invite URL to clipboard', async () => {
      const user = userEvent.setup();
      vi.mocked(adminLinks).mockResolvedValue([link1]);

      renderLinks();
      await waitFor(() => screen.getByText('Alice'));

      await user.click(screen.getByRole('button', { name: 'copy invite for Alice' }));

      expect(navigator.clipboard.writeText).toHaveBeenCalledWith(
        `${window.location.origin}/l/tok-abc123`,
      );
    });
  });

  describe('revoke — two-step', () => {
    it('first revoke click arms the button without calling adminRevoke', async () => {
      const user = userEvent.setup();
      vi.mocked(adminLinks).mockResolvedValue([link1]);

      renderLinks();
      await waitFor(() => screen.getByText('Alice'));

      await user.click(screen.getByRole('button', { name: 'revoke Alice' }));

      // Button should now be in armed (confirm) state
      expect(
        screen.getByRole('button', { name: 'confirm revoke Alice' }),
      ).toBeInTheDocument();
      expect(adminRevoke).not.toHaveBeenCalled();
    });

    it('second revoke click calls adminRevoke and reloads list', async () => {
      const user = userEvent.setup();
      vi.mocked(adminLinks)
        .mockResolvedValueOnce([link1])
        .mockResolvedValue([{ ...link1, revoked: true }]);
      vi.mocked(adminRevoke).mockResolvedValue(undefined);

      renderLinks();
      await waitFor(() => screen.getByText('Alice'));

      // Arm
      await user.click(screen.getByRole('button', { name: 'revoke Alice' }));
      // Confirm
      await user.click(screen.getByRole('button', { name: 'confirm revoke Alice' }));

      await waitFor(() => {
        expect(adminRevoke).toHaveBeenCalledWith('tok-abc123');
      });
      // After revoke, load() fires → adminLinks called a second time
      await waitFor(() => {
        expect(adminLinks).toHaveBeenCalledTimes(2);
      });
    });

    it('revoke failure shows a loud error and keeps the button armed for retry', async () => {
      const user = userEvent.setup();
      vi.mocked(adminLinks).mockResolvedValue([link1]);
      vi.mocked(adminRevoke).mockRejectedValue(
        new Error('revoke failed — the link may still be live'),
      );

      renderLinks();
      await waitFor(() => screen.getByText('Alice'));

      await user.click(screen.getByRole('button', { name: 'revoke Alice' }));
      await user.click(screen.getByRole('button', { name: 'confirm revoke Alice' }));

      // Failure surfaces as an alert — never silent success
      await waitFor(() => {
        expect(screen.getByRole('alert')).toHaveTextContent(/revoke failed.*still be live/i);
      });
      // Button stays armed so the next click retries immediately
      expect(screen.getByRole('button', { name: 'confirm revoke Alice' })).toBeInTheDocument();
      // No reload happened (list fetch only once, from mount)
      expect(adminLinks).toHaveBeenCalledTimes(1);
    });

    it('revoked links do not show a revoke button', async () => {
      vi.mocked(adminLinks).mockResolvedValue([link2]); // revoked: true

      renderLinks();
      await waitFor(() => screen.getByText('Bob'));

      expect(
        screen.queryByRole('button', { name: /revoke Bob/i }),
      ).not.toBeInTheDocument();
      expect(
        screen.queryByRole('button', { name: /confirm revoke Bob/i }),
      ).not.toBeInTheDocument();
    });
  });

  describe('audit expand', () => {
    // The gift URL itself is redacted server-side (AdminClaimView) — the
    // admin client only ever sees issued:bool.
    const claimFulfilled: AdminClaimView = {
      game_id: 'game-hollow-knight',
      state: 'fulfilled',
      issued: true,
    };

    const claimPending: AdminClaimView = {
      game_id: 'game-celeste',
      state: 'pending',
      issued: false,
    };

    it('expand audit button loads claims and renders game_id + state chips', async () => {
      const user = userEvent.setup();
      vi.mocked(adminLinks).mockResolvedValue([link1]);
      vi.mocked(adminLinkClaims).mockResolvedValue([claimFulfilled, claimPending]);

      renderLinks();
      await waitFor(() => screen.getByText('Alice'));

      await user.click(screen.getByRole('button', { name: 'expand audit for Alice' }));

      await waitFor(() => {
        expect(adminLinkClaims).toHaveBeenCalledWith('tok-abc123');
        expect(screen.getByText('game-hollow-knight')).toBeInTheDocument();
        expect(screen.getByText('game-celeste')).toBeInTheDocument();
        expect(screen.getByText('fulfilled')).toBeInTheDocument();
        expect(screen.getByText('pending')).toBeInTheDocument();
      });
    });

    it('renders "issued ✓" when issued is true', async () => {
      const user = userEvent.setup();
      vi.mocked(adminLinks).mockResolvedValue([link1]);
      vi.mocked(adminLinkClaims).mockResolvedValue([claimFulfilled, claimPending]);

      renderLinks();
      await waitFor(() => screen.getByText('Alice'));

      await user.click(screen.getByRole('button', { name: 'expand audit for Alice' }));

      await waitFor(() => expect(screen.getByText('issued ✓')).toBeInTheDocument());
      // fulfilled is issued → ✓; pending is not → no indicator
    });

    it('CRITICAL: the AdminClaimView type has no gift_url — the secret cannot reach the DOM', async () => {
      const user = userEvent.setup();
      vi.mocked(adminLinks).mockResolvedValue([link1]);
      vi.mocked(adminLinkClaims).mockResolvedValue([claimFulfilled, claimPending]);

      renderLinks();
      await waitFor(() => screen.getByText('Alice'));

      await user.click(screen.getByRole('button', { name: 'expand audit for Alice' }));
      await waitFor(() => screen.getByText('issued ✓'));

      // Defense-in-depth used to live here (assert the URL string absent from
      // innerHTML); the redaction moved server-side, so the client type can't
      // even carry the secret. Keep a canary: no href-bearing anchor may render
      // inside the audit panel.
      expect(document.body.innerHTML).not.toContain('humble.gift');
    });

    it('collapse button hides the audit panel', async () => {
      const user = userEvent.setup();
      vi.mocked(adminLinks).mockResolvedValue([link1]);
      vi.mocked(adminLinkClaims).mockResolvedValue([claimFulfilled]);

      renderLinks();
      await waitFor(() => screen.getByText('Alice'));

      // Expand
      await user.click(screen.getByRole('button', { name: 'expand audit for Alice' }));
      await waitFor(() => screen.getByText('game-hollow-knight'));

      // Collapse
      await user.click(screen.getByRole('button', { name: 'collapse audit for Alice' }));

      expect(screen.queryByText('game-hollow-knight')).not.toBeInTheDocument();
    });
  });
});
