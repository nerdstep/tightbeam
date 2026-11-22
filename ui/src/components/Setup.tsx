import { useState } from 'preact/hooks'

import { api } from '../api'
import { run } from '../store'

/** First launch: choose a display name. */
export function Setup() {
  const [name, setName] = useState('')

  return (
    <section class="flex h-full flex-col items-center justify-center gap-2.5 p-6 text-center">
      <img src="/icon.svg" alt="" class="size-18" />
      <h1 class="text-[17px] font-semibold">Tightbeam</h1>
      <p class="text-muted">A private line between two people on the same network.</p>
      <form
        class="mt-2 flex w-full max-w-75 flex-col gap-2.5 text-left"
        onSubmit={(e) => {
          e.preventDefault()
          void run(api.setName(name))
        }}
      >
        <label htmlFor="name">Your name</label>
        <input
          id="name"
          class="field"
          maxLength={40}
          autoFocus
          required
          value={name}
          onInput={(e) => setName(e.currentTarget.value)}
        />
        <button class="btn-primary">Continue</button>
      </form>
    </section>
  )
}
