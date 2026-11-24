// App state in one TanStack Store. Rust pushes status and message events; components
// subscribe to just the slices they render (useSelector), so a status update that only
// changes, say, the nearby list doesn't re-render the transcript.

import { createStore } from '@tanstack/react-store'

import { api, type Message, type Reaction, type Ref, type Status, type UpdateReady } from './api'
import { chime } from './lib/chime'

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
  /** A downloaded update waiting for a restart. */
  update: UpdateReady | null
  /** Counts nudges, sent and received; each one shakes the window. */
  nudges: number
  /** The message the composer is replying to. */
  replyingTo: Message | null
  /** The peer is typing, as of the last few seconds. */
  peerTyping: boolean
}

export const store = createStore<State>({
  status: null,
  messages: [],
  hasOlder: false,
  loadingOlder: false,
  error: null,
  update: null,
  nudges: 0,
  replyingTo: null,
  peerTyping: false,
})

/** How long "typing…" lasts without another signal. The sender repeats it more often. */
const TYPING_LAPSE_MS = 4000

export const messageKey = (m: Ref) => `${m.author}:${m.seq}`
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

/** New reactions on a message. Messages that aren't loaded get theirs when paged in. */
function setReactions(to: Ref, reactions: Reaction[]) {
  const key = messageKey(to)
  if (!loaded.has(key)) return
  store.setState((s) => ({
    ...s,
    messages: s.messages.map((m) => (messageKey(m) === key ? { ...m, reactions } : m)),
  }))
}

let typingLapse: ReturnType<typeof setTimeout> | undefined

function setPeerTyping(on: boolean) {
  clearTimeout(typingLapse)
  if (on) typingLapse = setTimeout(() => setPeerTyping(false), TYPING_LAPSE_MS)
  if (store.state.peerTyping !== on) store.setState((s) => ({ ...s, peerTyping: on }))
}

export function setReplyingTo(replyingTo: Message | null) {
  store.setState((s) => ({ ...s, replyingTo }))
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

/** Shakes the window; with `sound`, chimes too (a nudge from the peer). */
export function shake(sound: boolean) {
  store.setState((s) => ({ ...s, nudges: s.nudges + 1 }))
  // A hidden window gets a notification from Rust instead.
  if (sound && document.visibilityState === 'visible') chime()
}

/** Erases the conversation on this computer, then shows what's left. */
export async function clearHistory() {
  if (!(await run(api.clearHistory().then(() => true)))) return
  const page = await run(api.getMessages(undefined, PAGE_SIZE))
  if (!page) return
  loaded.clear()
  for (const m of page.messages) loaded.add(messageKey(m))
  store.setState((s) => ({
    ...s,
    messages: page.messages,
    hasOlder: page.has_more,
    replyingTo: null,
  }))
}

export function setUpdate(update: UpdateReady | null) {
  store.setState((s) => ({ ...s, update }))
}

export async function start() {
  // Listen first so nothing that happens while loading is missed.
  await api.onEvent((event) => {
    switch (event.kind) {
      case 'status':
        store.setState((s) => ({ ...s, status: event.data }))
        if (!event.data.connected) setPeerTyping(false)
        break
      case 'message':
        // Their message arriving means they've stopped typing it.
        if (event.data.author !== store.state.status?.me.id) setPeerTyping(false)
        addMessage(event.data)
        break
      case 'reactions':
        setReactions(event.data.to, event.data.reactions)
        break
      case 'typing':
        setPeerTyping(true)
        break
      case 'nudge':
        shake(true)
        break
    }
  })
  await api.onUpdateReady(setUpdate)
  // An update may have downloaded before the window loaded.
  setUpdate(await api.getUpdate())
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
