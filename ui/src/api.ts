// Typed wrappers around the Rust commands in src-tauri/src/lib.rs.
// The interfaces mirror the Serialize structs in src-tauri/src/chat.rs and db.rs.

import { getVersion } from '@tauri-apps/api/app'
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
  /** Highest of my sequence numbers the peer has seen. Always 0 with peers before 0.3. */
  peer_seen: number
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
  /** Automatic update checks. `null` when updates aren't available (dev and test builds). */
  updates: boolean | null
}

/** An update that has downloaded and is waiting for a restart. */
export interface UpdateReady {
  version: string
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
  /** Quits the app. Closing the window only hides it to the tray. */
  quit: () => invoke<void>('quit'),
  /** Opens an http(s) or mailto link outside the app. */
  openLink: (url: string) => invoke<void>('open_link', { url }),
  /** The app version, from src-tauri/Cargo.toml. */
  version: () => getVersion(),
  setAutoUpdates: (on: boolean) => invoke<void>('set_auto_updates', { on }),
  /** Checks now and downloads what it finds. `null` means up to date. */
  checkForUpdates: () => invoke<UpdateReady | null>('check_for_updates'),
  getUpdate: () => invoke<UpdateReady | null>('get_update'),
  /** Installs the downloaded update and restarts the app. */
  installUpdate: () => invoke<void>('install_update'),
  onUpdateReady: (handler: (update: UpdateReady) => void) =>
    listen<UpdateReady>('update-ready', (e) => handler(e.payload)),
  onEvent: (handler: (event: CoreEvent) => void) =>
    listen<CoreEvent>('tightbeam', (e) => handler(e.payload)),
}
