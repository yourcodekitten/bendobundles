import { StrictMode } from 'react'
import { createRoot } from 'react-dom/client'
// self-hosted fonts — the pixel hud 2 type system (see DESIGN.md Typography)
import '@fontsource-variable/chivo/index.css'
// a REAL italic face: the postcard's note (and the page's) was browser-faked italic (review 2)
import '@fontsource-variable/chivo/wght-italic.css'
import '@fontsource-variable/pixelify-sans/index.css'
import '@fontsource/silkscreen/index.css'
import './index.css'
import App from './App.tsx'

createRoot(document.getElementById('root')!).render(
  <StrictMode>
    <App />
  </StrictMode>,
)
