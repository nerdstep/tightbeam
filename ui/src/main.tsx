import { StrictMode } from 'react'
import { createRoot } from 'react-dom/client'

import { App } from './App'
import { start, store } from './store'

import './styles.css'

createRoot(document.getElementById('app')!).render(
  <StrictMode>
    <App />
  </StrictMode>,
)
start().catch((e) => store.setState((s) => ({ ...s, error: String(e) })))
