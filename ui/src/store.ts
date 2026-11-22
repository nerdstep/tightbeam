// App state in one TanStack Store. Rust pushes status and message events; components
// subscribe to just the slices they render (useSelector), so a status update that only
// changes, say, the nearby list doesn't re-render the transcript.

import { createStore } from '@tanstack/react-store'

import { api, type Message, type Status } from './api'

const PAGE_SIZE = 200

interface State {
  status: Status | null
  /** The loaded window of history, oldest first, in display order. */
  messages: Message[]
  /** Whether older messages exist that aren't loaded yet. */
  hasOlder: boolean
  loadingOlder: boolean
  /** A UI-side error (a failed command), shown in place of the engine's notice. */
  error: string | null
}

export const store = createStore<State>({
  status: null,
  messages: [],
  hasOlder: false,
  loadingOlder: false,
  error: null,
})

export const messageKey = (m: Message) => `${m.author}:${m.seq}`
const loaded = new Set<string>()

/** Display order, matching the SQL `ORDER BY ts, author, seq`. */
function compare(a: Message, b: Message) {
  if (a.ts !== b.ts) return a.ts - b.ts
  if (a.author !== b.author) return a.author < b.author ? -1 : 1
  return a.seq - b.seq
}

/** Adds a live message in display order. Usually it's the newest, so search from the end. */
export function addMessage(m: Message) {
  const key = messageKey(m)
  if (loaded.has(key)) return
  const { messages, hasOlder } = store.state
  // Older than everything loaded while older pages remain: it will show when paged in.
  if (hasOlder && messages.length > 0 && compare(m, messages[0]) < 0) return
  loaded.add(key)
  let at = messages.length
  while (at > 0 && compare(messages[at - 1], m) > 0) at--
  store.setState((s) => ({ ...s, messages: [...messages.slice(0, at), m, ...messages.slice(at)] }))
}

/** Loads the page before the oldest loaded message. Safe to call repeatedly. */
export async function loadOlder() {
  const { messages, hasOlder, loadingOlder } = store.state
  if (!hasOlder || loadingOlder) return
  store.setState((s) => ({ ...s, loadingOlder: true }))
  const oldest = messages[0]
  const page = await run(
    api.getMessages({ ts: oldest.ts, author: oldest.author, seq: oldest.seq }, PAGE_SIZE),
  )
  const older = (page?.messages ?? []).filter((m) => !loaded.has(messageKey(m)))
  for (const m of older) loaded.add(messageKey(m))
  store.setState((s) => ({
    ...s,
    messages: [...older, ...s.messages],
    hasOlder: page ? page.has_more : s.hasOlder,
    loadingOlder: false,
  }))
}

/** Runs a command; shows its error instead of throwing. Resolves to undefined on failure. */
export async function run<T>(promise: Promise<T>): Promise<T | undefined> {
  try {
    return await promise
  } catch (e) {
    store.setState((s) => ({ ...s, error: String(e) }))
    return undefined
  }
}

export function dismissError() {
  store.setState((s) => ({ ...s, error: null }))
}

export async function start() {
  // Listen first so nothing that happens while loading is missed.
  await api.onEvent((event) => {
    if (event.kind === 'status') store.setState((s) => ({ ...s, status: event.data }))
    else addMessage(event.data)
  })
  const status = await api.getStatus()
  const page = await api.getMessages(undefined, PAGE_SIZE)
  const fresh = page.messages.filter((m) => !loaded.has(messageKey(m)))
  for (const m of fresh) loaded.add(messageKey(m))
  store.setState((s) => ({
    ...s,
    status,
    // Anything that arrived live while loading is newer; keep it after the page.
    messages: [...fresh, ...s.messages],
    hasOlder: page.has_more,
  }))
}
