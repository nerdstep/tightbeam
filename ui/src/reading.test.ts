import { describe, expect, test } from 'bun:test'

import type { Attachment, Hit, Message, Quote } from './api'
import { actionInSnippet, actionOf, goneText, readingOf, summaryOf } from './reading'

function msg(more: Partial<Message> = {}): Message {
  return {
    author: 'a',
    seq: 1,
    ts: 1,
    hlc: 1,
    text: 'hello',
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

function quote(more: Partial<Quote> = {}): Quote {
  return {
    author: 'a',
    seq: 1,
    hlc: 1,
    text: 'hello',
    deleted: false,
    vanished: false,
    file: null,
    ...more,
  }
}

const pdf: Attachment = {
  hash: 'h',
  name: 'Report.pdf',
  mime: 'application/pdf',
  size: 10,
  state: 'done',
  openable: true,
}
const picture: Attachment = { ...pdf, name: 'beach.jpg', mime: 'image/jpeg', width: 4, height: 3 }

const deleted = { text: '', deleted: true }
const vanished = { text: '', deleted: true, vanished: true }

describe('what a message reads as', () => {
  const cases: [string, Message | Quote, ReturnType<typeof readingOf>][] = [
    ['text', msg(), { kind: 'said', text: 'hello', file: null }],
    ['an action', msg({ text: '/me waves' }), { kind: 'action', action: 'waves' }],
    ['"/me" alone, not an action', msg({ text: '/me' }), { kind: 'said', text: '/me', file: null }],
    [
      'a caption that starts with /me, not an action',
      msg({ text: '/me waves', file: picture }),
      { kind: 'said', text: '/me waves', file: picture },
    ],
    ['a file alone', msg({ text: '', file: pdf }), { kind: 'said', text: '', file: pdf }],
    ['deleted', msg(deleted), { kind: 'gone', why: 'deleted' }],
    ['vanished', msg(vanished), { kind: 'gone', why: 'vanished' }],
    [
      'a quote of a deleted one',
      quote({ text: null, deleted: true }),
      { kind: 'gone', why: 'deleted' },
    ],
    [
      'a quote of a vanished one',
      quote({ text: null, deleted: true, vanished: true }),
      { kind: 'gone', why: 'vanished' },
    ],
    [
      'a quote of one not on this computer',
      quote({ text: null }),
      { kind: 'gone', why: 'missing' },
    ],
  ]
  for (const [name, m, reading] of cases) {
    test(name, () => expect(readingOf(m)).toEqual(reading))
  }
})

describe('placeholders', () => {
  test('say whose delete it was, and nothing else', () => {
    expect(goneText('deleted', true)).toBe('You deleted this message')
    expect(goneText('deleted', false)).toBe('Message deleted')
    expect(goneText('vanished', true)).toBe('Message vanished')
    expect(goneText('missing', false)).toBe('A message that isn’t on this computer')
  })
})

describe('a message in one line', () => {
  const cases: [string, Message | Quote, string][] = [
    ['text', quote(), 'hello'],
    ['an action, as the transcript says it', quote({ text: '/me waves' }), 'Alex waves'],
    ['an action in the reply bar', msg({ text: '/me waves' }), 'Alex waves'],
    ['a caption', quote({ text: 'look', file: picture }), 'look'],
    ['a caption that starts with /me', msg({ text: '/me waves', file: picture }), '/me waves'],
    ['a picture alone', quote({ text: '', file: picture }), 'Picture'],
    ['a file alone', quote({ text: '', file: pdf }), 'Report.pdf'],
    ['deleted', quote({ text: null, deleted: true }), 'Message deleted'],
    ['deleted, in the reply bar', msg(deleted), 'Message deleted'],
    ['vanished', quote({ text: null, deleted: true, vanished: true }), 'Message vanished'],
    ['not on this computer', quote({ text: null }), 'A message that isn’t on this computer'],
  ]
  for (const [name, m, line] of cases) {
    test(name, () => expect(summaryOf(m, 'Alex')).toBe(line))
  }
})

/** A match on `/me waves at everyone`, unless `more` says otherwise. */
const hit = (snippet: string, more: Partial<Message> = {}): Hit => ({
  message: msg({ text: '/me waves at everyone', ...more }),
  snippet,
})

describe('a search match', () => {
  test('on an action loses the /me, keeping its highlights', () => {
    expect(actionInSnippet(hit('/me \u0002waves\u0003 at everyone'))).toBe(
      '\u0002waves\u0003 at everyone',
    )
  })
  test('that starts mid-message stays as it is', () => {
    expect(actionInSnippet(hit('…\u0002waves\u0003 at everyone'))).toBeNull()
  })
  test('on anything else stays as it is', () => {
    expect(actionInSnippet(hit('/me \u0002waves\u0003', { file: picture }))).toBeNull()
    expect(actionInSnippet(hit('\u0002hello\u0003', { text: 'hello' }))).toBeNull()
  })
})

describe('actions', () => {
  test('are text that starts with "/me "', () => {
    expect(actionOf('/me waves')).toBe('waves')
    expect(actionOf('/me')).toBeNull()
    expect(actionOf('/men are here')).toBeNull()
    expect(actionOf('hi /me waves')).toBeNull()
  })
})
