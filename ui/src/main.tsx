import { render } from 'preact'

import { App } from './App'
import { error, start } from './store'

import './styles.css'

render(<App />, document.getElementById('app')!)
start().catch((e) => (error.value = String(e)))
