import { useEffect, useRef } from 'preact/hooks'

import { api, type PairPrompt } from '../api'
import { run } from '../store'

/** Pairing in progress, either direction. Both sides show the same code. Esc declines. */
export function PairDialog({ prompt }: { prompt: PairPrompt }) {
  const dialogRef = useRef<HTMLDialogElement>(null)
  const who = prompt.name || 'Someone'

  useEffect(() => {
    const dialog = dialogRef.current
    dialog?.showModal()
    return () => dialog?.close()
  }, [])

  return (
    <dialog
      ref={dialogRef}
      aria-labelledby="pair-title"
      class="m-auto w-[calc(100%-2rem)] max-w-85 rounded-2xl border border-line bg-panel p-5 text-ink backdrop:bg-black/45"
      onCancel={(e) => {
        e.preventDefault()
        void run(api.answerPair(false))
      }}
    >
      <div class="flex flex-col gap-2.5">
        <h2 id="pair-title" class="text-lg font-semibold">
          {prompt.incoming ? `${who} wants to pair` : `Waiting for ${who}`}
        </h2>
        <p class="text-muted">
          {prompt.incoming
            ? 'Only accept if their screen shows the same code.'
            : 'Check that their screen shows this code, then they’ll accept.'}
        </p>
        <div class="py-2.5 text-center font-mono text-3xl font-semibold tracking-[3px]">
          {prompt.code}
        </div>
        <div class="flex justify-end gap-2">
          <button class="btn" onClick={() => void run(api.answerPair(false))}>
            {prompt.incoming ? 'Decline' : 'Cancel'}
          </button>
          {prompt.incoming && (
            <button class="btn-primary" autoFocus onClick={() => void run(api.answerPair(true))}>
              Pair
            </button>
          )}
        </div>
      </div>
    </dialog>
  )
}
