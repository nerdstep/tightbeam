// Search: a bar under the header, and the matches over the transcript. Picking a match
// closes search and scrolls to that message.

import { shallow, useSelector } from '@tanstack/react-store'
import { SearchIcon, XIcon } from 'lucide-react'
import { useEffect, useRef } from 'react'

import type { Hit } from '@/api'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Spinner } from '@/components/ui/spinner'
import { closeSearch, reveal, setSearchActive, setSearchQuery, store } from '@/store'
import { messageKey } from '@/transcript'

/** Matches past this many aren't shown (see SEARCH_LIMIT in chat.rs). */
const LIMIT = 100

function pick(hit: Hit) {
  closeSearch()
  void reveal(hit.message)
}

export function SearchBar() {
  const search = useSelector(store, (s) => s.search)
  const inputRef = useRef<HTMLInputElement>(null)
  const open = search !== null

  // On opening, focus the box with its text selected, so typing replaces the last query.
  useEffect(() => {
    if (!open) return
    inputRef.current?.focus()
    inputRef.current?.select()
  }, [open])

  if (!open) return null
  const { query, hits, active } = search

  const onKeyDown = (e: React.KeyboardEvent<HTMLInputElement>) => {
    const step = { ArrowDown: 1, ArrowUp: -1 }[e.key]
    if (step && hits.length > 0) {
      e.preventDefault()
      setSearchActive((active + step + hits.length) % hits.length)
    } else if (e.key === 'Enter' && hits[active]) {
      e.preventDefault()
      pick(hits[active])
    } else if (e.key === 'Escape') {
      e.preventDefault()
      closeSearch()
    }
  }

  return (
    <div className="flex items-center gap-2 border-b bg-card px-4 py-2">
      <SearchIcon className="size-4 shrink-0 text-muted-foreground" />
      <Input
        ref={inputRef}
        value={query}
        onChange={(e) => setSearchQuery(e.target.value)}
        onKeyDown={onKeyDown}
        placeholder="Search messages"
        aria-label="Search messages"
        className="h-8"
      />
      <Button variant="ghost" size="icon-sm" aria-label="Close search" onClick={closeSearch}>
        <XIcon />
      </Button>
    </div>
  )
}

export function SearchResults() {
  const { search, myId, peerName } = useSelector(
    store,
    (s) => ({
      search: s.search,
      myId: s.status?.me.id ?? '',
      peerName: s.status?.peer?.name || 'Them',
    }),
    { compare: shallow },
  )
  const listRef = useRef<HTMLUListElement>(null)
  const active = search?.active ?? 0

  // Keep the active match in view as ↑↓ move it.
  useEffect(() => {
    listRef.current?.children[active]?.scrollIntoView({ block: 'nearest' })
  }, [active])

  if (!search || !search.query.trim()) return null
  const { hits, pending } = search

  return (
    <div className="absolute inset-0 z-10 overflow-y-auto bg-background">
      {hits.length === 0 ? (
        <div className="grid h-full place-items-center text-sm text-muted-foreground">
          {pending ? <Spinner /> : 'No messages match.'}
        </div>
      ) : (
        <>
          <ul ref={listRef} className="p-2">
            {hits.map((hit, i) => (
              <li key={messageKey(hit.message)}>
                <button
                  type="button"
                  className={`block w-full rounded-md px-3 py-2 text-left ${i === active ? 'bg-accent text-accent-foreground' : ''}`}
                  onMouseEnter={() => setSearchActive(i)}
                  onClick={() => pick(hit)}
                >
                  <div className="flex items-baseline justify-between gap-3 text-xs text-muted-foreground">
                    <span className="font-medium text-foreground">
                      {hit.message.author === myId ? 'You' : peerName}
                    </span>
                    <span>{when(hit.message.ts)}</span>
                  </div>
                  <Snippet text={hit.snippet} />
                </button>
              </li>
            ))}
          </ul>
          <p className="px-5 pb-3 text-xs text-muted-foreground">
            {hits.length >= LIMIT
              ? `The newest ${LIMIT} matches. Add words to narrow it down.`
              : `${hits.length} ${hits.length === 1 ? 'match' : 'matches'}`}
            {' · ↑↓ to choose · Enter to go to it · Esc to close'}
          </p>
        </>
      )}
    </div>
  )
}

/** The date, and the time, of a match. */
function when(ts: number) {
  const d = new Date(ts)
  const sameYear = d.getFullYear() === new Date().getFullYear()
  const date = d.toLocaleDateString([], {
    month: 'short',
    day: 'numeric',
    year: sameYear ? undefined : 'numeric',
  })
  return `${date}, ${d.toLocaleTimeString([], { hour: 'numeric', minute: '2-digit' })}`
}

/** A snippet from Rust, with the matches (between \u0002 and \u0003) highlighted. */
function Snippet({ text }: { text: string }) {
  const [before, ...rest] = text.split('\u0002')
  return (
    <p className="mt-0.5 line-clamp-2 text-sm break-words">
      {before}
      {rest.map((part, i) => {
        const [match, after = ''] = part.split('\u0003')
        return (
          <span key={i}>
            <mark className="rounded-sm bg-warning/35 text-inherit">{match}</mark>
            {after}
          </span>
        )
      })}
    </p>
  )
}
