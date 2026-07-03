import { BrowserRouter, Navigate, Route, Routes } from 'react-router-dom';
import { Landing } from './friend/Landing';
import { LinkPage } from './friend/LinkPage';
import { AdminApp } from './admin/AdminApp';
import { Login } from './admin/Login';
import { Catalog } from './admin/Catalog';
import { Links } from './admin/Links';
import { Ops } from './admin/Ops';

function App() {
  return (
    <BrowserRouter>
      <Routes>
        {/* Friend surface */}
        <Route path="/" element={<Landing />} />
        <Route path="/l/:token" element={<LinkPage />} />

        {/* Admin surface */}
        <Route path="/admin/login" element={<Login />} />
        <Route path="/admin" element={<AdminApp />}>
          <Route index element={<Navigate to="catalog" replace />} />
          <Route path="catalog" element={<Catalog />} />
          <Route path="links" element={<Links />} />
          <Route path="ops" element={<Ops />} />
        </Route>

        <Route path="*" element={<Landing />} />
      </Routes>
    </BrowserRouter>
  );
}

export default App;
