import { NavLink, Outlet } from 'react-router-dom';

export function AdminApp() {
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
      <main className="p-6">
        <Outlet />
      </main>
    </div>
  );
}
