import { describe, expect, test } from 'bun:test'

import type { Message, Quote } from './api'
import {
  add,
  compare,
  empty,
  fill,
  find,
  fromPage,
  gapTo,
  indexOf,
  messageKey,
  prepend,
  update,
} from './transcript'

/** A message by `author`, numbered `seq`, at display position `hlc`. */
function msg(author: string, seq: number, hlc = seq, more: Partial<Message> = {}): Message {
  return {
    author,
    seq,
    ts: hlc,
    hlc,
    text: `${author}${seq}`,
    edited: false,
    deleted: false,
    file: null,
    vanish: null,
    expires: null,
    vanished: false,
    reply_to: null,
    reactions: [],
    ...more,
  }
}

function quote(of: Message): Quote {
  return {
    author: of.author,
    seq: of.seq,
    text: of.text,
    deleted: false,
    vanished: false,
    file: null,
  }
}

const keys = (t: { messages: Message[] }) => t.messages.map(messageKey)

describe('display order', () => {
  test('by clock, then author, then number', () => {
    const order = [msg('a', 2, 1), msg('b', 1, 1), msg('a', 1, 2), msg('a', 3, 2)]
    const shuffled = [order[3], order[1], order[0], order[2]]
    // oxlint-disable-next-line unicorn/no-array-sort -- a fresh array
    expect(shuffled.sort(compare)).toEqual(order)
  })
})

describe('live messages', () => {
  test('go in display order, usually last', () => {
    let t = fromPage({ messages: [msg('a', 1, 10), msg('b', 1, 30)], has_more: false })
    t = add(t, msg('a', 2, 40))
    t = add(t, msg('b', 2, 20)) // written before they saw a:2
    expect(keys(t)).toEqual(['a:1', 'b:2', 'b:1', 'a:2'])
  })

  test('arrive once, however often they are told', () => {
    const m = msg('a', 1)
    const once = add(empty, m)
    expect(add(once, m)).toBe(once)
  })

  test('older than what is loaded wait for their page', () => {
    const t = fromPage({ messages: [msg('a', 5, 50)], has_more: true })
    expect(add(t, msg('b', 1, 10))).toBe(t)
    const all = fromPage({ messages: [msg('a', 5, 50)], has_more: false })
    expect(keys(add(all, msg('b', 1, 10)))).toEqual(['b:1', 'a:5'])
  })
})

describe('pages', () => {
  test('older pages go before, without repeats', () => {
    let t = fromPage({ messages: [msg('a', 3), msg('a', 4)], has_more: true })
    t = prepend(t, { messages: [msg('a', 1), msg('a', 2), msg('a', 3)], has_more: false })
    expect(keys(t)).toEqual(['a:1', 'a:2', 'a:3', 'a:4'])
    expect(t.hasOlder).toBe(false)
  })

  test('messages that arrived while the first page loaded stay after it', () => {
    const live = add(empty, msg('b', 9, 90))
    const t = prepend(live, { messages: [msg('a', 1), msg('b', 9, 90)], has_more: true })
    expect(keys(t)).toEqual(['a:1', 'b:9'])
    expect(t.hasOlder).toBe(true)
  })

  test('starting again forgets what was loaded', () => {
    const before = fromPage({ messages: [msg('a', 1), msg('a', 2)], has_more: false })
    const after = fromPage({ messages: [], has_more: false })
    expect(find(after, { author: 'a', seq: 1 })).toBeUndefined()
    expect(keys(add(after, before.messages[0]))).toEqual(['a:1'])
  })
})

describe('reaching a message that is not loaded', () => {
  const t = fromPage({ messages: [msg('a', 5, 50), msg('a', 6, 60)], has_more: true })

  test('loads from it to the oldest loaded', () => {
    const target = msg('b', 1, 10)
    expect(gapTo(t, target)).toEqual({
      from: target,
      before: { hlc: 50, author: 'a', seq: 5 },
    })
    const gap = [msg('b', 1, 10), msg('a', 4, 40)]
    const filled = fill(t, gap)
    expect(keys(filled)).toEqual(['b:1', 'a:4', 'a:5', 'a:6'])
    expect(indexOf(filled, target)).toBe(0)
  })

  test('needs nothing when it is here, or nothing is', () => {
    expect(gapTo(t, msg('a', 6, 60))).toBeNull()
    expect(gapTo(empty, msg('a', 6, 60))).toBeNull()
  })

  test('adds only what is missing, in order', () => {
    const filled = fill(t, [msg('a', 6, 60), msg('a', 4, 40)])
    expect(keys(filled)).toEqual(['a:4', 'a:5', 'a:6'])
    expect(fill(filled, [msg('a', 4, 40)])).toBe(filled)
  })
})

describe('updates', () => {
  const original = msg('a', 1)
  const reply = msg('b', 1, 2, { reply_to: quote(original) })
  const t = fromPage({ messages: [original, reply], has_more: false })

  test('swap in the new version, and the quotes of it', () => {
    const edited = { ...original, text: 'fixed', edited: true }
    const after = update(t, edited)
    expect(find(after, original)).toEqual(edited)
    expect(find(after, reply)?.reply_to?.text).toBe('fixed')
  })

  test('a delete blanks the quotes of it', () => {
    const deleted = { ...original, text: '', deleted: true }
    const shown = find(update(t, deleted), reply)?.reply_to
    expect(shown).toMatchObject({ text: null, deleted: true })
  })

  test('a message that vanishes says so in the quotes of it', () => {
    const vanished = { ...original, text: '', deleted: true, vanished: true }
    const shown = find(update(t, vanished), reply)?.reply_to
    expect(shown).toMatchObject({ text: null, deleted: true, vanished: true, file: null })
  })

  test('of messages not loaded change nothing', () => {
    expect(keys(update(t, msg('c', 1)))).toEqual(['a:1', 'b:1'])
  })
})
