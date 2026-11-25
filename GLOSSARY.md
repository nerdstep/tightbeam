# Tightbeam

Private chat between two paired peers on the same network. Each person's history is a log that syncs to the other peer.

## Language

### Peers

**Peer**:
The one other person's app this one is paired with. Before pairing, the apps nearby are just nearby.
_Avoid_: contact, friend, remote

**Pairing**:
Two apps agreeing to trust each other's key, after both screens show the same code. Each app pairs with one peer at a time, and a new peer gets only what's written after pairing with them.
_Avoid_: linking, connecting

**Session**:
One connected stretch between two paired peers: from their greetings until the connection drops. A newer session replaces an older one.
_Avoid_: connection (for the stretch of chat), call

**Catch-up**:
The entries a peer is missing, sent at the start of each session, before anything live.
_Avoid_: sync (for this step), backfill

### Logs

**Log**:
One author's history: an append-only, gap-free sequence of entries numbered from 1.
_Avoid_: history, timeline (for the stored sequence)

**Entry**:
One item in a log, identified by its author and sequence number. Its kind is a message, a reaction, an edit or a delete; everything but a message refers to an earlier entry.
_Avoid_: record, event, row

**Admission**:
The rules a peer's entry passes before it's stored: what's kept, clamped or set aside as not counting.
_Avoid_: validation, sanitising

**Inert entry**:
An entry that is kept in its author's log but never applied or shown: an edit or delete of someone else's message, or a kind this build doesn't know.
_Avoid_: ignored entry, unknown entry

**Message**:
What the transcript shows for a message entry: its current text with later edits, deletes and reactions folded in.
_Avoid_: entry (when meaning the shown result), post

**Transcript**:
The messages shown, in display order, as far back as they're loaded. Older ones load as the reader scrolls up, or when a search hit or a reply's quote reaches back to them.
_Avoid_: chat log, history (for what's shown), feed
