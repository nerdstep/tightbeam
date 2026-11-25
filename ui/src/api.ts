// Typed wrappers around the Rust commands in src-tauri/src/lib.rs.
// The interfaces mirror the Serialize structs in src-tauri/src/chat.rs and db.rs.

import { getVersion } from '@tauri-apps/api/app'
import { convertFileSrc, invoke } from '@tauri-apps/api/core'
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
  /** Everyone I've been paired with, by their last known name, to label their messages. */
  people: Person[]
  pairing: PairPrompt | null
  notice: string | null
  /** Set when I went away with `/away`. Away from being idle isn't shown to me. */
  away: Away | null
  /** The peer is away. Only while connected. */
  peer_away: Away | null
  /** My do not disturb is on until then (ms). `null` once it's past. */
  dnd_until: number | null
  /** The peer's do not disturb, by their clock. Only while connected. */
  peer_dnd_until: number | null
}

/** For `/status`. */
export interface ConnectionInfo {
  my_version: string
  /** `null` for peers before 0.6, or when we haven't been connected since. */
  peer_version: string | null
  /** When this session started (ms). The rest are also only while connected. */
  since: number | null
  address: string | null
  rtt_ms: number | null
}

export interface Away {
  /** What they said with `/away`, if anything. */
  note: string | null
}

/** A message, by author and sequence number. */
export interface Ref {
  author: string
  seq: number
}

/** The message a reply answers. */
export interface Quote extends Ref {
  /** Its place in display order, to load the history back to it. `null` when it isn't on this computer. */
  hlc: number | null
  /** Its latest text. `null` when it isn't on this computer (cleared) or was deleted. */
  text: string | null
  /** Deleted, or vanished. */
  deleted: boolean
  vanished: boolean
  /** Its file, unless deleted. */
  file: FileInfo | null
}

/** A file attached to a message, as its sender describes it. */
export interface FileInfo {
  /** BLAKE3 of the contents. Also how the file is fetched and shown (`fileUrl`). */
  hash: string
  name: string
  size: number
  mime: string
  /** Set for an image the sender's app could display. */
  width?: number
  height?: number
  /** A tiny copy of the image as a `data:` URL, shown until the file arrives. */
  preview?: string
}

/** A message's file on this computer: still to come, here, or gone from the sender. */
export interface Attachment extends FileInfo {
  state: 'pending' | 'done' | 'gone'
  /** It may open in its default app; if not, it can only be shown in its folder. */
  openable: boolean
}

/** A file from disk, staged in Rust to be sent by path. */
export interface StagedFile {
  id: number
  name: string
  size: number
  mime: string
}

/** What came of staging files from disk: the ones staged, and why the others weren't. */
export interface StagedFiles {
  staged: StagedFile[]
  refused: string[]
}

/** What the UI says about a file it sends, besides the bytes. */
export interface NewFile {
  name: string
  mime: string
  width: number | null
  height: number | null
  preview: string | null
}

export interface Reaction {
  author: string
  emoji: string
}

export interface Message {
  author: string
  seq: number
  /** Milliseconds since the epoch, from the author's clock. */
  /** When it was written, by its author's clock. */
  ts: number
  /** Display order: like `ts`, but never before a message its author had already seen. */
  hlc: number
  /** The latest edit's text, if edited. Empty once deleted. With a file, the caption. */
  text: string
  edited: boolean
  /** Deleted by its author, or vanished: a placeholder, without reply, reactions or file. */
  deleted: boolean
  file: Attachment | null
  /** A vanishing message: seconds it lasts once seen. */
  vanish: number | null
  /** When it vanishes (ms), once its countdown has started. */
  expires: number | null
  /** Its time ran out. `deleted` is true too. */
  vanished: boolean
  reply_to: Quote | null
  /** At most one per person. */
  reactions: Reaction[]
}

/** A message that matches a search. */
export interface Hit {
  message: Message
  /** The matching part of its text, with each match between \u0002 and \u0003. */
  snippet: string
}

/** A position in display order; a page loads the messages before it. */
export type Cursor = Pick<Message, 'hlc' | 'author' | 'seq'>

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

export type CoreEvent =
  | { kind: 'status'; data: Status }
  | { kind: 'message'; data: Message }
  /** The peer nudged us. */
  | { kind: 'nudge' }
  /** A message changed: its reactions, an edit, or a delete. */
  | { kind: 'updated'; data: Message }
  /** The peer is typing. Repeats every few seconds while they type. */
  | { kind: 'typing' }
  /** A file is on its way, to or from here. The last one has `have === size`. */
  | { kind: 'transfer'; data: { hash: string; have: number; size: number } }

/** Escapes everything outside ASCII, so JSON fits in an HTTP header. */
const asciiJson = (value: unknown) =>
  JSON.stringify(value).replaceAll(
    /[\u007f-￿]/g,
    (c) => `\\u${c.charCodeAt(0).toString(16).padStart(4, '0')}`,
  )

export const api = {
  getStatus: () => invoke<Status>('get_status'),
  getMessages: (before?: Cursor, limit?: number) =>
    invoke<Page>('get_messages', { before: before ?? null, limit: limit ?? null }),
  /** Messages from `from` up to `before`: fills the gap back to a search hit. */
  getMessagesBetween: (from: Cursor, before: Cursor) =>
    invoke<Message[]>('get_messages_between', { from, before }),
  /** Messages matching `query`, newest first, at most 100. */
  search: (query: string) => invoke<Hit[]>('search', { query }),
  setName: (name: string) => invoke<void>('set_name', { name }),
  /** `vanish`: seconds it lasts once they've seen it. */
  sendMessage: (text: string, replyTo?: Ref, vanish?: number) =>
    invoke<Message>('send_message', { text, replyTo: replyTo ?? null, vanish: vanish ?? null }),
  /** Sends a file, with `text` as its caption. Needs the peer on 0.7 or later. */
  sendFile: async (bytes: ArrayBuffer, file: NewFile, text: string, replyTo?: Ref) =>
    invoke<Message>('send_file', bytes, {
      headers: { 'x-tightbeam-file': asciiJson({ ...file, text, reply_to: replyTo ?? null }) },
    }),
  /** Stages files dropped on the window. */
  stagePaths: (paths: string[]) => invoke<StagedFiles>('stage_paths', { paths }),
  /** Asks which files to send, then stages them. */
  pickFiles: () => invoke<StagedFiles>('pick_files'),
  /** Sends a staged file, by path. */
  sendStaged: (id: number, file: NewFile, text: string, replyTo?: Ref) =>
    invoke<Message>('send_staged', { id, details: { ...file, text, reply_to: replyTo ?? null } }),
  unstage: (id: number) => invoke<void>('unstage', { id }),
  /** A staged picture's bytes, to make its preview. */
  stagedPicture: (id: number) => invoke<ArrayBuffer>('staged_picture', { id }),
  /** Where the webview loads a file that's here. */
  fileUrl: (hash: string) => convertFileSrc(hash, 'tbfile'),
  /** Opens a picture in its default app. */
  openFile: (hash: string) => invoke<void>('open_file', { hash }),
  /** Shows a file in Explorer or Finder. */
  showFile: (hash: string) => invoke<void>('show_file', { hash }),
  /** Asks where to save a copy, then saves it. `false` if cancelled. */
  saveFileCopy: (hash: string) => invoke<boolean>('save_file_copy', { hash }),
  /** `/dnd`: until then (ms), or `null` to turn it off. */
  setDnd: (until: number | null) => invoke<void>('set_dnd', { until }),
  /** `/status`. */
  connectionInfo: () => invoke<ConnectionInfo>('connection_info'),
  /** Sets my reaction to a message, replacing any earlier one. `''` takes it back. */
  react: (to: Ref, emoji: string) => invoke<void>('react', { to, emoji }),
  /** Replaces the text of one of my messages. */
  editMessage: (of: Ref, text: string) => invoke<void>('edit_message', { of, text }),
  /** Deletes one of my messages, for both people. */
  deleteMessage: (of: Ref) => invoke<void>('delete_message', { of }),
  /** `/away`: away until `/back` or my next message. `note` may be empty. */
  setAway: (note: string) => invoke<void>('set_away', { note }),
  /** `/back`. */
  back: () => invoke<void>('back'),
  /** Tells the peer I'm typing. Call every few seconds while typing. */
  typing: () => invoke<void>('typing'),
  /** Erases the conversation on this computer. Undelivered messages of mine stay. */
  clearHistory: () => invoke<void>('clear_history'),
  /** Fails when not connected, or right after another nudge. */
  nudge: () => invoke<void>('nudge'),
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
