// The transcript: the messages shown, in display order, as far back as they're loaded.
//
// A plain value and functions that return a new one, so the store can hold it and tests
// can check it without the app around it. Fetching pages is the store's job; this
// module decides what a page, a live message or an update does to what's shown.

import type { Cursor, Message, Page, Quote, Ref } from './api'

export interface Transcript {
  /** Oldest first, in display order. */
  readonly messages: Message[]
  /** `messageKey` of every message here. */
  readonly keys: ReadonlySet<string>
  /** Whether older messages exist that aren't loaded yet. */
  readonly hasOlder: boolean
}

export const empty: Transcript = { messages: [], keys: new Set(), hasOlder: false }

export const messageKey = (m: Ref) => `${m.author}:${m.seq}`

export const cursorOf = (m: Message): Cursor => ({ hlc: m.hlc, author: m.author, seq: m.seq })

/** Display order. The UI's one copy of the log's SQL `ORDER BY hlc, author, seq`. */
export function compare(a: Cursor, b: Cursor) {
  if (a.hlc !== b.hlc) return a.hlc - b.hlc
  if (a.author !== b.author) return a.author < b.author ? -1 : 1
  return a.seq - b.seq
}

/** Starts again from the newest page, as after clearing the history. */
export function fromPage(page: Page): Transcript {
  return prepend(empty, page)
}

/**
 * Adds a page of older messages before what's here. The first page goes in this way
 * too: messages that arrived live while it loaded are newer, so they stay after it.
 */
export function prepend(t: Transcript, page: Page): Transcript {
  const older = page.messages.filter((m) => !t.keys.has(messageKey(m)))
  return {
    messages: [...older, ...t.messages],
    keys: withKeys(t.keys, older),
    hasOlder: page.has_more,
  }
}

/**
 * Adds a live message in display order. One that's here already is left alone, and so
 * is one older than everything here while older pages remain: it shows when its page
 * loads.
 */
export function add(t: Transcript, m: Message): Transcript {
  const { messages } = t
  if (t.keys.has(messageKey(m))) return t
  if (t.hasOlder && messages.length > 0 && compare(m, messages[0]) < 0) return t
  // Usually it's the newest, so search from the end.
  let at = messages.length
  while (at > 0 && compare(messages[at - 1], m) > 0) at--
  return {
    ...t,
    messages: [...messages.slice(0, at), m, ...messages.slice(at)],
    keys: withKeys(t.keys, [m]),
  }
}

/** Adds the messages back to one that isn't loaded yet (see `gapTo`). */
export function fill(t: Transcript, gap: Message[]): Transcript {
  const missing = gap.filter((m) => !t.keys.has(messageKey(m)))
  if (!missing.length) return t
  // oxlint-disable-next-line unicorn/no-array-sort -- a fresh array; toSorted needs ES2023
  const messages = [...missing, ...t.messages].sort(compare)
  return { ...t, messages, keys: withKeys(t.keys, missing) }
}

/**
 * A message changed (reactions, an edit, a delete): swaps in the new version, and
 * updates the replies that quote it. Messages that aren't loaded are current when they
 * load.
 */
export function update(t: Transcript, updated: Message): Transcript {
  const key = messageKey(updated)
  const quote = quoteOf(updated)
  return {
    ...t,
    messages: t.messages.map((m) => {
      if (messageKey(m) === key) return updated
      if (m.reply_to && messageKey(m.reply_to) === key) {
        return { ...m, reply_to: { ...m.reply_to, ...quote } }
      }
      return m
    }),
  }
}

/**
 * What a reply shows of the message it quotes, as Rust's `Db::show` builds it: nothing of
 * a deleted or vanished one but the fact.
 */
export function quoteOf(m: Message): Pick<Quote, 'text' | 'deleted' | 'vanished' | 'file'> {
  const gone = m.deleted || m.vanished
  return {
    text: gone ? null : m.text,
    deleted: gone,
    vanished: m.vanished,
    file: gone ? null : m.file,
  }
}

/** Where a reply's quoted message sits, or `null` if it isn't on this computer. */
export function targetOf(q: Quote): Cursor | null {
  return q.hlc === null ? null : { hlc: q.hlc, author: q.author, seq: q.seq }
}

export function find(t: Transcript, at: Ref): Message | undefined {
  const key = messageKey(at)
  return t.keys.has(key) ? t.messages.find((m) => messageKey(m) === key) : undefined
}

export function indexOf(t: Transcript, at: Ref): number {
  const key = messageKey(at)
  return t.keys.has(key) ? t.messages.findIndex((m) => messageKey(m) === key) : -1
}

/**
 * What to load to reach `target`: the messages from it up to the oldest one here. `null`
 * when it's here already, or nothing is.
 */
export function gapTo(t: Transcript, target: Cursor): { from: Cursor; before: Cursor } | null {
  const oldest = t.messages[0]
  if (t.keys.has(messageKey(target)) || !oldest) return null
  return { from: target, before: cursorOf(oldest) }
}

function withKeys(keys: ReadonlySet<string>, added: Message[]): ReadonlySet<string> {
  if (!added.length) return keys
  const next = new Set(keys)
  for (const m of added) next.add(messageKey(m))
  return next
}
