import { api } from '../api'
import { error, run, status } from '../store'

function dismiss() {
  if (error.value) error.value = null
  else void run(api.dismissNotice())
}

/** A dismissible banner: a failed command, or a notice from the engine. */
export function Notice() {
  const text = error.value ?? status.value?.notice
  if (!text) return null

  return (
    <output class="fixed top-2.5 left-1/2 z-20 flex max-w-[calc(100%-2rem)] -translate-x-1/2 items-center gap-3 rounded-lg border border-warn bg-panel px-3 py-2 text-[13px]">
      <span>{text}</span>
      <button class="btn-link" onClick={dismiss}>
        Dismiss
      </button>
    </output>
  )
}
