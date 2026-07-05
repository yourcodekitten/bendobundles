import { useState, useEffect, useCallback } from 'react';
import { NavLink, Outlet, useNavigate } from 'react-router-dom';
import { adminStatus, type StatusView } from '../api';
import { withAuth } from './withAuth';

// Status + its refresh callback are threaded to child routes via Outlet context.
// Outlet context is used (over a separate React context) because the only
// consumer is the Ops child route — a direct child of this layout — so there
// is no deep prop-threading and no need for an extra context provider.
// status lives HERE only; children must not keep their own copy of it.
export type AdminOutletContext = {
  status: StatusView | null;
  refreshStatus: () => void;
};

// One place for the nav active/inactive style — three NavLinks share it.
const navLinkClass = ({ isActive }: { isActive: boolean }) =>
  isActive ? 'text-zinc-100 font-medium' : 'text-zinc-400 hover:text-zinc-200';

export function AdminApp() {
  const navigate = useNavigate();
  const [status, setStatus] = useState<StatusView | null>(null);

  const fetchStatus = useCallback(() => {
    withAuth(() => adminStatus(), navigate)
      .then(setStatus)
      .catch(() => {});
  }, [navigate]);

  // Fetch once on mount. Status only changes on sync, and that path already
  // calls refreshStatus() — refetching per route change would run
  // handle_status's full-table Scan on every tab switch for nothing.
  useEffect(() => {
    fetchStatus();
  }, [fetchStatus]);

  // While a sync run is live, poll — fulfillment writes SyncState only at the END of a run,
  // so without polling the card would show the previous run's result until a manual reload.
  // Polling stops on its own: run completion deletes the marker, so `running` flips false.
  const syncRunning = status?.sync_run?.running === true;
  useEffect(() => {
    if (!syncRunning) return;
    const id = setInterval(fetchStatus, 5000);
    return () => clearInterval(id);
  }, [syncRunning, fetchStatus]);

  // Show the banner only when cookie_ok is explicitly false — null (no sync yet)
  // does not trigger the banner.
  const cookieAttentionNeeded = status?.sync?.cookie_ok === false;

  return (
    <div className="min-h-screen bg-zinc-950 text-zinc-100">
      <nav className="border-b border-zinc-800 px-6 py-3 flex gap-6" aria-label="admin navigation">
        <NavLink to="/admin/catalog" className={navLinkClass}>
          catalog
        </NavLink>
        <NavLink to="/admin/links" className={navLinkClass}>
          links
        </NavLink>
        <NavLink to="/admin/ops" className={navLinkClass}>
          ops
        </NavLink>
      </nav>

      {cookieAttentionNeeded && (
        <div
          role="alert"
          aria-label="humble session needs attention"
          className="bg-red-900 px-6 py-3 text-sm text-red-100"
        >
          humble session needs attention — self-login retries on the next sync; if it keeps
          failing, update the humble-cookie SSM param directly (AWS console/CLI)
        </div>
      )}

      <main className="p-6">
        <Outlet context={{ status, refreshStatus: fetchStatus } satisfies AdminOutletContext} />
      </main>
    </div>
  );
}
