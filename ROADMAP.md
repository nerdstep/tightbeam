# Roadmap

Tightbeam has no server: messages only move while both apps are running and can reach each other. That shapes the priorities below. Features that keep the app running and noticeable come before new chat features.

Items in each section are in suggested order.

## 0. Groundwork

- [x] **Ignore unknown frame types.** `read_loop` in `src-tauri/src/chat.rs` currently ends the session on any frame it doesn't recognize. Two peers will often run different builds, so any new frame type would break the connection to an older peer. Skip unknown frames instead (keep `Hello` / `PairRequest` strict). Do this before anything in sections 2 and 3 adds a frame type.

## 0.5. Long histories and UI foundation

Previously the whole history loaded at startup and every bubble re-rendered on each status event (acks, and mDNS discovery updates every second or two). That was fine for a few thousand messages but would get slow at tens of thousands.

- [x] **Move the UI to React + shadcn/ui (Base UI).** shadcn is React-only, so this replaces Preact. It brings the [Message Scroller](https://ui.shadcn.com/docs/components/base/message-scroller), which pins to the bottom, keeps your place when older messages are added above, and has a jump-to-latest button. Related components: Bubble, Message, and Marker (for day separators). State moves from Preact signals to TanStack Store, which other TanStack libraries (Virtual, Form) would share. The palette maps onto shadcn's tokens.
- [x] **Stop status updates from re-rendering messages.** Bubbles read only their own delivery state and are memoized, so acks and discovery events don't touch the transcript.
- [x] **Paged loading.** `get_messages(before, limit)` in Rust, with an index on `(ts, author, seq)`. Open with the latest ~200 messages and load older pages when the reader nears the top. The Message Scroller handles keeping the scroll position. We write the trigger (an `IntersectionObserver`).
- [x] **Load-test.** Use a generated 50,000-message database to check startup time and typing responsiveness. Result: the newest page loads in ~0.25 ms, and the app opens at the bottom. A window uses ~250 MB in total, mostly WebView2's baseline.
- [x] **Virtualize the transcript** with TanStack Virtual. Only on-screen rows (plus overscan) are in the DOM, so long scroll-backs stay smooth and flat in memory. `anchorTo: 'end'` keeps your place when older pages load, and `followOnAppend` follows new messages. This replaced the earlier `content-visibility` approach (blank rows on fast scrolls) and the plan to cap the loaded window.

## 1. Daily use

- [ ] **Keep running in the background.** Closing the window hides it to the tray (Windows) or menu bar (macOS), with Open and Quit items. Add an optional "start at login" (`tauri-plugin-autostart`).
- [ ] **Notifications.** Show a system notification for a new message when the window isn't focused (`tauri-plugin-notification`), with a setting to turn it off.
- [ ] **Unread count.** Show a badge on the Dock / taskbar icon, cleared when the chat is focused.
- [ ] **Remember window size and position** (`tauri-plugin-window-state`).
- [ ] **Clickable links**, opening in the default browser (`tauri-plugin-opener`).

## 2. Chat features

New frame types or message kinds. The per-author append-only log stays as it is: changes are new entries that refer to earlier ones, never rewrites.

- [ ] **Seen receipts.** `Seen { up_to }` is sent when the chat is focused, the same shape as `Ack`. Show "seen" in place of "delivered".
- [ ] **Typing indicator.** A throwaway frame, never stored. It expires after a few seconds without a refresh.
- [ ] **Reactions.** A message kind that targets `(author, seq)`.
- [ ] **Replies.** A message carrying an optional `reply_to: (author, seq)`, shown as a quoted snippet.
- [ ] **Edit and delete.** New messages that say "replace or hide `(author, seq)`". The UI shows the latest version and an "edited" mark.
- [ ] **Search** with SQLite FTS5 over the message text.
- [ ] **Stable ordering.** Messages are sorted by each author's wall clock, so clock skew can misorder them. Use a hybrid logical clock (or Lamport timestamps) for display order.

## 3. Bigger pieces

- [ ] **Images and files**, including pasted screenshots. Use `iroh-blobs` for transfer. Messages carry a content hash, and the bytes sync separately.
- [ ] **Automatic builds and updates.** GitHub Actions builds the Windows and macOS apps (macOS runners are free for public repos) and publishes a release, and `tauri-plugin-updater` installs it. This replaces building over SSH. It needs an updater signing key, stored as a repository secret.
- [ ] **Off-network chat, opt-in.** Enable iroh's relay servers so peers can reach each other off the LAN. Messages stay end-to-end encrypted, but the relay operator can see that the two endpoints talk. Off by default, with a clear setting.

## Later

- [ ] **More than one device per person** (for example a laptop plus a desktop, and eventually phones via Tauri mobile). This is the biggest architectural change: each person's log has to sync across their own devices, and pairing becomes per person rather than per device.
- [ ] Voice notes.
- [ ] A "nudge" with a sound.
- [ ] An away status after a period of inactivity.
- [ ] Per-person colors or themes.
