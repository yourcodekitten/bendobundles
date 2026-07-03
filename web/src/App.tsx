import { BrowserRouter, Route, Routes } from 'react-router-dom';
import { Landing } from './friend/Landing';
import { LinkPage } from './friend/LinkPage';

function App() {
  return (
    <BrowserRouter>
      <Routes>
        <Route path="/" element={<Landing />} />
        <Route path="/l/:token" element={<LinkPage />} />
        <Route path="*" element={<Landing />} />
      </Routes>
    </BrowserRouter>
  );
}

export default App;
