// App state as signals. Rust pushes status and message events; components just read.

import { signal } from '@preact/signals'

import { api, type Message, type Status } from './api'

export const status = signal<Status | null>(null)
export const messages = signal<Message[]>([])
/** A UI-side error (a failed command). Shown in place of the Rust notice. */
export const error = signal<string | null>(null)

const seen = new Set<string>()

export function addMessages(incoming: Message[]) {
  const fresh = incoming.filter((m) => {
    const key = `${m.author}:${m.seq}`
    if (seen.has(key)) return false
    seen.add(key)
    return true
  })
  if (fresh.length) messages.value = [...messages.value, ...fresh]
}

/** Run a command; show its error instead of throwing. Resolves to undefined on failure. */
export async function run<T>(promise: Promise<T>): Promise<T | undefined> {
  try {
    return await promise
  } catch (e) {
    error.value = String(e)
    return undefined
  }
}

export async function start() {
  // Listen first so nothing that happens while loading is missed.
  await api.onEvent((event) => {
    if (event.kind === 'status') status.value = event.data
    else addMessages([event.data])
  })
  status.value = await api.getStatus()
  addMessages(await api.getMessages())
}
