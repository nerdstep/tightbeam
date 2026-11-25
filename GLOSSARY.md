# Tightbeam

Private chat between two paired peers on the same network. Each person's history is a log that syncs to the other peer.

## Language

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
