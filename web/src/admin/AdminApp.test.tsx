import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { vi, describe, it, expect } from 'vitest';
import { MemoryRouter, Route, Routes, useNavigate } from 'react-router-dom';
import { AdminApp } from './AdminApp';
import { withAuth } from './withAuth';
import { Unauthorized } from '../api';

// Stub child that exercises withAuth directly — validates the guard mechanism
// without needing a real API call (Unauthorized is thrown synchronously in the mock).
function GuardedChild({ onAction }: { onAction: () => Promise<void> }) {
  const navigate = useNavigate();
  return (
    <button
      onClick={() => {
        void withAuth(onAction, navigate);
      }}
    >
      trigger
    </button>
  );
}

function renderAdminWithChild(child: React.ReactElement) {
  return render(
    <MemoryRouter initialEntries={['/admin/catalog']}>
      <Routes>
        <Route path="/admin" element={<AdminApp />}>
          <Route path="catalog" element={child} />
        </Route>
        <Route path="/admin/login" element={<div>login page</div>} />
      </Routes>
    </MemoryRouter>,
  );
}

describe('AdminApp layout', () => {
  it('renders nav links for catalog, links, and ops', () => {
    renderAdminWithChild(<div>catalog content</div>);
    expect(screen.getByRole('link', { name: /catalog/i })).toBeInTheDocument();
    expect(screen.getByRole('link', { name: /links/i })).toBeInTheDocument();
    expect(screen.getByRole('link', { name: /ops/i })).toBeInTheDocument();
  });

  it('renders child route content via Outlet', () => {
    renderAdminWithChild(<div>catalog content</div>);
    expect(screen.getByText('catalog content')).toBeInTheDocument();
  });
});

describe('withAuth guard', () => {
  it('redirects to /admin/login when api call throws Unauthorized', async () => {
    const user = userEvent.setup();
    const throwUnauthorized = vi.fn().mockRejectedValue(new Unauthorized());

    renderAdminWithChild(<GuardedChild onAction={throwUnauthorized} />);

    await user.click(screen.getByRole('button', { name: /trigger/i }));

    await waitFor(() => {
      expect(screen.getByText('login page')).toBeInTheDocument();
    });
  });

  it('does not redirect when the call succeeds', async () => {
    const user = userEvent.setup();
    const success = vi.fn().mockResolvedValue(undefined);

    renderAdminWithChild(<GuardedChild onAction={success} />);

    await user.click(screen.getByRole('button', { name: /trigger/i }));

    await waitFor(() => {
      expect(success).toHaveBeenCalledOnce();
    });
    expect(screen.queryByText('login page')).not.toBeInTheDocument();
  });
});
