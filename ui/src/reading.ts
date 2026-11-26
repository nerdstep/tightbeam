// How a message reads: the one place that decides its wording, wherever it's shown. The
// transcript lays out a message's reading; quotes and the reply bar show its summary.
//
// Notifications are worded in Rust (`desktop::on_message`). Keep the two in step.

import type { FileInfo, Message, Quote } from './api'

/** What a message reads as. */
export type Reading =
  /** Deleted, vanished, or not on this computer: a placeholder. */
  | { kind: 'gone'; why: Gone }
  /** An action, like `/me waves`: shown as "Alex waves". */
  | { kind: 'action'; action: string }
  /** Anything else: its text, and its file if it has one (the text is then its caption). */
  | { kind: 'said'; text: string; file: FileInfo | null }

export type Gone = 'deleted' | 'vanished' | 'missing'

const ACTION = '/me '

/** The action in `/me waves` (`waves`), or `null` if the text isn't one. */
export function actionOf(text: string): string | null {
  return text.startsWith(ACTION) ? text.slice(ACTION.length) : null
}

/** What a message, or a reply's quote of one, reads as. */
export function readingOf(m: Message | Quote): Reading {
  if (m.vanished) return { kind: 'gone', why: 'vanished' }
  if (m.deleted) return { kind: 'gone', why: 'deleted' }
  if (m.text === null) return { kind: 'gone', why: 'missing' }
  // With a file, the text is a caption, never an action.
  const action = m.file ? null : actionOf(m.text)
  if (action !== null) return { kind: 'action', action }
  return { kind: 'said', text: m.text, file: m.file }
}

/** The placeholder for a message that's gone. `mine`: I wrote it. */
export function goneText(why: Gone, mine: boolean) {
  if (why === 'vanished') return 'Message vanished'
  if (why === 'missing') return 'A message that isn’t on this computer'
  return mine ? 'You deleted this message' : 'Message deleted'
}

/** A file, in a word: "Picture", or its name. */
export const describeFile = (file: FileInfo) => (file.width ? 'Picture' : file.name)

/** A message in one line, for a reply's quote. */
export function summaryOf(m: Message | Quote): string {
  const r = readingOf(m)
  if (r.kind === 'gone') return r.why === 'deleted' ? 'Deleted message' : goneText(r.why, false)
  if (r.kind === 'action') return `* ${r.action}`
  return r.text || (r.file ? describeFile(r.file) : '')
}
