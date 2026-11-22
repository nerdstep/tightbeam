import { useState } from 'preact/hooks'

import { api } from '../api'
import { run, status } from '../store'

/** Not paired yet: show who's nearby. */
export function Pair() {
  const s = status.value!

  return (
    <section class="flex h-full flex-col">
      <header class="flex items-center justify-between gap-3 border-b border-line bg-panel px-4 py-3">
        <h1 class="text-[17px] font-semibold">Pair</h1>
        <Rename name={s.me.name} />
      </header>
      <div class="flex flex-col gap-3.5 p-4">
        <p class="text-muted">
          Ask the other person to open Tightbeam on the same network. They’ll appear here.
        </p>
        <ul class="flex flex-col gap-2">
          {s.nearby.map((p) => (
            <li
              key={p.id}
              class="flex items-center justify-between rounded-xl border border-line bg-panel px-3 py-2.5"
            >
              <span>{p.name || 'Unnamed'}</span>
              <button class="btn" disabled={!!s.pairing} onClick={() => run(api.pairWith(p.id))}>
                Pair
              </button>
            </li>
          ))}
        </ul>
        {s.nearby.length === 0 && (
          <p class="flex items-center gap-2 text-muted">
            <span class="size-2 animate-pulse rounded-full bg-accent" />
            Looking for people nearby…
          </p>
        )}
      </div>
    </section>
  )
}

/** "You're Alice · change", turning into an inline field. */
function Rename({ name }: { name: string }) {
  const [editing, setEditing] = useState(false)
  const [draft, setDraft] = useState(name)

  if (!editing) {
    return (
      <span class="text-[13px] text-muted">
        You’re <b class="text-ink">{name}</b> ·{' '}
        <button
          class="btn-link"
          onClick={() => {
            setDraft(name)
            setEditing(true)
          }}
        >
          change
        </button>
      </span>
    )
  }

  return (
    <form
      class="flex items-center gap-2"
      onSubmit={async (e) => {
        e.preventDefault()
        await run(api.setName(draft))
        setEditing(false)
      }}
    >
      <input
        class="field w-36 py-1 text-[13px]"
        maxLength={40}
        autoFocus
        required
        value={draft}
        onInput={(e) => setDraft(e.currentTarget.value)}
        onKeyDown={(e) => e.key === 'Escape' && setEditing(false)}
      />
      <button class="btn-link text-[13px]">Save</button>
    </form>
  )
}
