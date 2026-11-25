// App state in one TanStack Store. Rust pushes status and message events; components
// subscribe to just the slices they render (useSelector), so a status update that only
// changes, say, the nearby list doesn't re-render the transcript.

import { createStore } from '@tanstack/react-store'

import {
  api,
  type Attachment,
  type Cursor,
  type Hit,
  type Message,
  type Ref,
  type Status,
  type UpdateReady,
} from './api'
import { chime } from './lib/chime'
import { formatBytes, isPicture, MAX_FILE_BYTES, type Staged, stage, unstage } from './lib/files'
import * as transcript from './transcript'
import { messageKey, type Transcript } from './transcript'

const PAGE_SIZE = 200
/** Search once typing pauses this long. */
const SEARCH_DELAY_MS = 150

export interface Search {
  query: string
  /** For `query`, newest first. Empty until the first results arrive. */
  hits: Hit[]
  /** Which hit Enter opens. */
  active: number
  /** Results for the current query haven't arrived yet. */
  pending: boolean
}

interface State {
  status: Status | null
  /** The messages shown, as far back as they're loaded. */
  transcript: Transcript
  loadingOlder: boolean
  /** A UI-side error (a failed command), shown in place of the engine's notice. */
  error: string | null
  /** A downloaded update waiting for a restart. */
  update: UpdateReady | null
  /** Counts nudges, sent and received; each one shakes the window. */
  nudges: number
  /** The message the composer is replying to. */
  replyingTo: Message | null
  /** The message of mine the composer is editing. */
  editing: Message | null
  /** The message of mine waiting for "Delete?" to be confirmed. */
  deleting: Message | null
  /** The peer is typing, as of the last few seconds. */
  peerTyping: boolean
  /** The search bar, while it's open. */
  search: Search | null
  /** A message for the transcript to scroll to and highlight. `n` tells repeats apart. */
  jump: { at: Ref; n: number } | null
  /** Pictures waiting in the message box to be sent. */
  staged: Staged[]
  /** Files on their way, to or from here, by hash: how far along, 0 to 1. */
  transfers: Record<string, number>
  /** The picture shown full size. */
  viewing: Attachment | null
}

export const store = createStore<State>({
  status: null,
  transcript: transcript.empty,
  loadingOlder: false,
  error: null,
  update: null,
  nudges: 0,
  replyingTo: null,
  editing: null,
  deleting: null,
  peerTyping: false,
  search: null,
  jump: null,
  staged: [],
  transfers: {},
  viewing: null,
})

/** How long "typing…" lasts without another signal. The sender repeats it more often. */
const TYPING_LAPSE_MS = 4000

/** Shows a live message (see `transcript.add`). */
export function addMessage(m: Message) {
  const next = transcript.add(store.state.transcript, m)
  if (next !== store.state.transcript) store.setState((s) => ({ ...s, transcript: next }))
}

/** A message changed (reactions, an edit, a delete). */
function updateMessage(updated: Message) {
  const key = messageKey(updated)
  store.setState((s) => ({
    ...s,
    transcript: transcript.update(s.transcript, updated),
    // Don't keep replying to, or editing, a message that's gone.
    replyingTo:
      updated.deleted && s.replyingTo && messageKey(s.replyingTo) === key ? null : s.replyingTo,
    editing: updated.deleted && s.editing && messageKey(s.editing) === key ? null : s.editing,
    // Nor keep showing a picture that was deleted.
    viewing:
      updated.deleted &&
      s.viewing &&
      transcript.find(s.transcript, updated)?.file?.hash === s.viewing.hash
        ? null
        : s.viewing,
  }))
}

/**
 * Adds pictures to the message box, to go with the next message. Anything else, or
 * anything over 1 GB, is turned away with a note.
 */
export async function stageFiles(files: File[]) {
  const pictures = files.filter(isPicture)
  const fitting = pictures.filter((f) => f.size > 0 && f.size <= MAX_FILE_BYTES)
  if (fitting.length < files.length) {
    const tooBig = pictures.find((f) => f.size > MAX_FILE_BYTES)
    const note = tooBig
      ? `${tooBig.name} is ${formatBytes(tooBig.size)}. Files can be up to 1 GB.`
      : 'Only pictures can be sent for now.'
    store.setState((s) => ({ ...s, error: note }))
  }
  const ready = await Promise.all(fitting.map(stage))
  if (ready.length) store.setState((s) => ({ ...s, staged: [...s.staged, ...ready] }))
}

/** Takes a picture out of the message box. */
export function unstageFile(id: number) {
  const gone = store.state.staged.find((s) => s.id === id)
  if (gone) unstage(gone)
  store.setState((s) => ({ ...s, staged: s.staged.filter((x) => x.id !== id) }))
}

/** Shows a picture full size, or closes it with `null`. */
export function setViewing(viewing: Attachment | null) {
  store.setState((s) => ({ ...s, viewing }))
}

function setTransfer(hash: string, have: number, size: number) {
  store.setState((s) => {
    const { [hash]: _, ...others } = s.transfers
    return { ...s, transfers: have >= size ? others : { ...others, [hash]: have / size } }
  })
}

let typingLapse: ReturnType<typeof setTimeout> | undefined

function setPeerTyping(on: boolean) {
  clearTimeout(typingLapse)
  if (on) typingLapse = setTimeout(() => setPeerTyping(false), TYPING_LAPSE_MS)
  if (store.state.peerTyping !== on) store.setState((s) => ({ ...s, peerTyping: on }))
}

/** Replying and editing take turns: starting one ends the other. */
export function setReplyingTo(replyingTo: Message | null) {
  store.setState((s) => ({ ...s, replyingTo, editing: replyingTo ? null : s.editing }))
}

export function setEditing(editing: Message | null) {
  store.setState((s) => ({ ...s, editing, replyingTo: editing ? null : s.replyingTo }))
}

export function setDeleting(deleting: Message | null) {
  store.setState((s) => ({ ...s, deleting }))
}

let searchTimer: ReturnType<typeof setTimeout> | undefined
let searchRun = 0
/** What was searched for last, offered again when search reopens. */
let lastQuery = ''

/** Opens search, with `query` or else the last one. */
export function openSearch(query?: string) {
  setSearchQuery(query ?? store.state.search?.query ?? lastQuery)
}

export function closeSearch() {
  clearTimeout(searchTimer)
  searchRun++
  store.setState((s) => ({ ...s, search: null }))
}

/** Updates the query; results follow once typing pauses. */
export function setSearchQuery(query: string) {
  lastQuery = query
  clearTimeout(searchTimer)
  const ticket = ++searchRun
  const wanted = query.trim() !== ''
  store.setState((s) => ({
    ...s,
    search: { query, hits: wanted ? (s.search?.hits ?? []) : [], active: 0, pending: wanted },
  }))
  if (!wanted) return
  searchTimer = setTimeout(async () => {
    const hits = await api.search(query).catch(() => [])
    if (ticket !== searchRun) return // the query changed, or search closed, meanwhile
    store.setState((s) => ({ ...s, search: s.search && { ...s.search, hits, pending: false } }))
  }, SEARCH_DELAY_MS)
}

export function setSearchActive(active: number) {
  store.setState((s) => ({ ...s, search: s.search && { ...s.search, active } }))
}

let jumps = 0

/**
 * Scrolls the transcript to a message, from a search hit or a reply, loading the history
 * back to it first if needed.
 */
export async function reveal(target: Cursor) {
  const gap = transcript.gapTo(store.state.transcript, target)
  if (gap) {
    const missing = await run(api.getMessagesBetween(gap.from, gap.before))
    // Against the latest transcript, in case a page of older messages arrived meanwhile.
    store.setState((s) => ({ ...s, transcript: transcript.fill(s.transcript, missing ?? []) }))
  }
  const at = { author: target.author, seq: target.seq }
  store.setState((s) => ({ ...s, jump: { at, n: ++jumps } }))
}

/** Loads the page before the oldest loaded message. Safe to call repeatedly. */
export async function loadOlder() {
  const { transcript: shown, loadingOlder } = store.state
  const oldest = shown.messages[0]
  if (!shown.hasOlder || !oldest || loadingOlder) return
  store.setState((s) => ({ ...s, loadingOlder: true }))
  const page = await run(api.getMessages(transcript.cursorOf(oldest), PAGE_SIZE))
  store.setState((s) => ({
    ...s,
    transcript: page ? transcript.prepend(s.transcript, page) : s.transcript,
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
  closeSearch() // its matches are gone
  const page = await run(api.getMessages(undefined, PAGE_SIZE))
  if (!page) return
  store.setState((s) => ({
    ...s,
    transcript: transcript.fromPage(page),
    replyingTo: null,
    editing: null,
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
      case 'updated':
        updateMessage(event.data)
        break
      case 'typing':
        setPeerTyping(true)
        break
      case 'transfer':
        setTransfer(event.data.hash, event.data.have, event.data.size)
        break
      case 'nudge':
        // Do not disturb: nudges are refused at the source, but an older build may send one.
        if ((store.state.status?.dnd_until ?? 0) <= Date.now()) shake(true)
        break
    }
  })
  await api.onUpdateReady(setUpdate)
  // An update may have downloaded before the window loaded.
  setUpdate(await api.getUpdate())
  const status = await api.getStatus()
  const page = await api.getMessages(undefined, PAGE_SIZE)
  // Anything that arrived live while loading is newer, and stays after the page.
  store.setState((s) => ({ ...s, status, transcript: transcript.prepend(s.transcript, page) }))
}
