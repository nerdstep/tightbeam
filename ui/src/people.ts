// Whose name a message shows: mine, the peer's, or someone this computer was paired with
// before, by the name they last had.

import type { Person } from './api'

/**
 * The people in `Status`, as one string: it stays equal across status updates that don't
 * change any names, so selecting it doesn't re-render the transcript.
 */
export const peopleKey = (people: Person[] | undefined) =>
  JSON.stringify((people ?? []).map((p) => [p.id, p.name]))

/** Names authors: "You" for me, otherwise their last known name, or else `fallback`. */
export function namer(key: string, myId: string, fallback: string): (author: string) => string {
  const names = new Map<string, string>(JSON.parse(key))
  return (author) => (author === myId ? 'You' : names.get(author) || fallback)
}
