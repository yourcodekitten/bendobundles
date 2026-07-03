import { useState, useEffect, useCallback } from 'react';
import { NavLink, Outlet, useLocation, useNavigate } from 'react-router-dom';
import { adminStatus, type StatusView } from '../api';
import { withAuth } from './withAuth';

// The refresh callback is threaded to child routes via Outlet context.
// Outlet context is used (over a separate React context) because the only
// consumer is the Ops child route — a direct child of this layout — so there
// is no deep prop-threading and no need for an extra context provider.
export type AdminOutletContext = { refreshStatus: () => void };

export function AdminApp() {
  const navigate = useNavigate();
  const { pathname } = useLocation();
  const [status, setStatus] = useState<StatusView | null>(null);

  const fetchStatus = useCallback(() => {
    withAuth(() => adminStatus(), navigate)
      .then(setStatus)
      .catch(() => {});
  }, [navigate]);

  // Re-fetch on mount and on every route change so the banner stays current.
  // pathname in deps is intentional: it triggers the effect on navigation even
  // though it is not read inside the effect body.
  useEffect(() => {
    fetchStatus();
  }, [fetchStatus, pathname]);

  // Show the banner only when cookie_ok is explicitly false — null (no sync yet)
  // does not trigger the banner.
  const cookieAttentionNeeded = status?.sync?.cookie_ok === false;

  return (
    <div className="min-h-screen bg-zinc-950 text-zinc-100">
      <nav className="border-b border-zinc-800 px-6 py-3 flex gap-6" aria-label="admin navigation">
        <NavLink
          to="/admin/catalog"
          className={({ isActive }) =>
            isActive ? 'text-zinc-100 font-medium' : 'text-zinc-400 hover:text-zinc-200'
          }
        >
          catalog
        </NavLink>
        <NavLink
          to="/admin/links"
          className={({ isActive }) =>
            isActive ? 'text-zinc-100 font-medium' : 'text-zinc-400 hover:text-zinc-200'
          }
        >
          links
        </NavLink>
        <NavLink
          to="/admin/ops"
          className={({ isActive }) =>
            isActive ? 'text-zinc-100 font-medium' : 'text-zinc-400 hover:text-zinc-200'
          }
        >
          ops
        </NavLink>
      </nav>

      {cookieAttentionNeeded && (
        <div
          role="alert"
          aria-label="humble session needs attention"
          className="bg-red-900 px-6 py-3 text-sm text-red-100"
        >
          humble session needs attention — paste a fresh cookie in ops
        </div>
      )}

      <main className="p-6">
        <Outlet context={{ refreshStatus: fetchStatus } satisfies AdminOutletContext} />
      </main>
    </div>
  );
}
