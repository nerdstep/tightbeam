// Typed wrappers around the Rust commands in src-tauri/src/lib.rs.
// The interfaces mirror the Serialize structs in src-tauri/src/chat.rs and db.rs.

import { invoke } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'

export interface Person {
  id: string
  name: string
}

export interface PairPrompt {
  id: string
  name: string
  /** Six digits, shown on both screens. */
  code: string
  /** True when the other person asked us. */
  incoming: boolean
}

export interface Status {
  me: Person
  peer: Person | null
  connected: boolean
  /** Highest of my sequence numbers the peer has confirmed. */
  peer_has: number
  nearby: Person[]
  pairing: PairPrompt | null
  notice: string | null
}

export interface Message {
  author: string
  seq: number
  /** Milliseconds since the epoch, from the author's clock. */
  ts: number
  text: string
}

/** A position in display order; a page loads the messages before it. */
export type Cursor = Pick<Message, 'ts' | 'author' | 'seq'>

export interface Page {
  /** Oldest first. */
  messages: Message[]
  /** Whether there are messages older than the first one. */
  has_more: boolean
}

export interface Prefs {
  notifications: boolean
  /** `null` when start-at-login isn't available (test mode). */
  start_at_login: boolean | null
}

export type CoreEvent = { kind: 'status'; data: Status } | { kind: 'message'; data: Message }

export const api = {
  getStatus: () => invoke<Status>('get_status'),
  getMessages: (before?: Cursor, limit?: number) =>
    invoke<Page>('get_messages', { before: before ?? null, limit: limit ?? null }),
  setName: (name: string) => invoke<void>('set_name', { name }),
  sendMessage: (text: string) => invoke<Message>('send_message', { text }),
  pairWith: (id: string) => invoke<void>('pair_with', { id }),
  answerPair: (accept: boolean) => invoke<void>('answer_pair', { accept }),
  unpair: () => invoke<void>('unpair'),
  dismissNotice: () => invoke<void>('dismiss_notice'),
  getPrefs: () => invoke<Prefs>('get_prefs'),
  setNotifications: (on: boolean) => invoke<void>('set_notifications', { on }),
  setStartAtLogin: (on: boolean) => invoke<void>('set_start_at_login', { on }),
  /** Opens an http(s) or mailto link outside the app. */
  openLink: (url: string) => invoke<void>('open_link', { url }),
  onEvent: (handler: (event: CoreEvent) => void) =>
    listen<CoreEvent>('tightbeam', (e) => handler(e.payload)),
}
