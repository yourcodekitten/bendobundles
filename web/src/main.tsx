import { StrictMode } from 'react'
import { createRoot } from 'react-dom/client'
// self-hosted fonts — the pixel hud 2 type system (see DESIGN.md Typography)
import '@fontsource-variable/chivo/index.css'
import '@fontsource-variable/pixelify-sans/index.css'
import '@fontsource/silkscreen/index.css'
import './index.css'
import App from './App.tsx'

createRoot(document.getElementById('root')!).render(
  <StrictMode>
    <App />
  </StrictMode>,
)
