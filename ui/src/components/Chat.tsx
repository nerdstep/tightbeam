import { shallow, useSelector } from '@tanstack/react-store'
import { useVirtualizer } from '@tanstack/react-virtual'
import {
  EllipsisIcon,
  ImagePlusIcon,
  PencilIcon,
  ReplyIcon,
  SearchIcon,
  SmilePlusIcon,
  TimerIcon,
  Trash2Icon,
  XIcon,
} from 'lucide-react'
import { memo, useEffect, useMemo, useRef, useState } from 'react'

import { api, type Message as ChatMessage, type Quote, type Reaction } from '@/api'
import { type Command, parse, suggest } from '@/commands'
import { describeFile, FileView, StagedFiles, Viewer } from '@/components/Files'
import { Linkified } from '@/components/Linkified'
import { SearchBar, SearchResults } from '@/components/Search'
import { SettingsMenu } from '@/components/SettingsMenu'
import { StatusDialog } from '@/components/StatusDialog'
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from '@/components/ui/alert-dialog'
import { Bubble, BubbleContent, BubbleReactions } from '@/components/ui/bubble'
import { Button } from '@/components/ui/button'
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu'
import { Marker } from '@/components/ui/marker'
import { Message, MessageContent, MessageFooter } from '@/components/ui/message'
import {
  MessageScroller,
  MessageScrollerButton,
  MessageScrollerContent,
  MessageScrollerProvider,
  MessageScrollerViewport,
  useMessageScroller,
} from '@/components/ui/message-scroller'
import { Spinner } from '@/components/ui/spinner'
import { Textarea } from '@/components/ui/textarea'
import { formatDuration, formatUntil } from '@/lib/duration'
import { namer, peopleKey } from '@/people'
import {
  addMessage,
  clearHistory,
  loadOlder,
  openSearch,
  reveal,
  run,
  setDeleting,
  setEditing,
  setReplyingTo,
  stageFiles,
  store,
  unstageFile,
} from '@/store'
import { indexOf, messageKey, targetOf } from '@/transcript'

/** The reactions on offer. */
const EMOJI = ['👍', '❤️', '😂', '😮', '😢', '🎉']

/** While typing, tell the peer this often. Their "typing…" lapses a little after. */
const TYPING_EVERY_MS = 2500

/** Whether something dragged over the window is files. */
const carriesFiles = (e: React.DragEvent) => e.dataTransfer.types.includes('Files')

/** Paired: the conversation. */
export function Chat() {
  // Ctrl+F (Cmd+F on macOS) opens search.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if ((e.ctrlKey || e.metaKey) && !e.altKey && e.key.toLowerCase() === 'f') {
        e.preventDefault()
        openSearch()
      }
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [])

  // Pictures dragged onto the window go in the message box.
  const [dragging, setDragging] = useState(false)

  return (
    <MessageScrollerProvider defaultScrollPosition="end">
      <section
        className="relative flex h-full flex-col"
        onDragOver={(e) => {
          if (!carriesFiles(e)) return
          e.preventDefault()
          e.dataTransfer.dropEffect = 'copy'
          setDragging(true)
        }}
        onDragLeave={(e) => {
          // Leaving the window, not just moving between the elements inside it.
          const to = e.relatedTarget
          if (!(to instanceof Node && e.currentTarget.contains(to))) setDragging(false)
        }}
        onDrop={(e) => {
          if (!carriesFiles(e)) return
          e.preventDefault()
          setDragging(false)
          void stageFiles([...e.dataTransfer.files])
        }}
      >
        <Header />
        <SearchBar />
        <div className="relative flex min-h-0 flex-1 flex-col">
          <Transcript />
          <SearchResults />
        </div>
        <Composer />
        <DeleteDialog />
        <Viewer />
        {dragging && (
          <div className="pointer-events-none absolute inset-2 z-40 grid place-items-center rounded-xl border-2 border-dashed border-primary bg-background/85 text-sm font-medium">
            Drop pictures to send
          </div>
        )}
      </section>
    </MessageScrollerProvider>
  )
}

function Header() {
  const { name, connected, typing, away, dndUntil } = useSelector(
    store,
    (s) => ({
      name: s.status?.peer?.name ?? '',
      connected: !!s.status?.connected,
      typing: s.peerTyping,
      // A string so the header only re-renders when it changes: '' is away without a note.
      away: s.status?.peer_away ? (s.status.peer_away.note ?? '') : null,
      dndUntil: s.status?.peer_dnd_until ?? null,
    }),
    { compare: shallow },
  )
  const [unpairing, setUnpairing] = useState(false)
  const dnd = useUntil(dndUntil)
  const dot = !connected
    ? 'bg-muted-foreground'
    : dnd
      ? 'bg-destructive'
      : away === null
        ? 'bg-success'
        : 'bg-warning'

  return (
    <header className="flex items-center justify-between gap-3 border-b bg-card px-4 py-3">
      <div className="min-w-0">
        <h1 className="font-semibold">{name || 'Your peer'}</h1>
        <span className="flex items-center gap-1.5 text-xs text-muted-foreground">
          <span
            className={`size-1.75 shrink-0 rounded-full ${dot} ${typing ? 'motion-safe:animate-pulse' : ''}`}
          />
          <span className="truncate">
            {typing
              ? 'Typing…'
              : connected
                ? dnd
                  ? 'Do not disturb'
                  : away === null
                    ? 'Connected'
                    : `Away${away && ` · ${away}`}`
                : 'Not connected · messages send when you’re both online'}
          </span>
        </span>
      </div>
      <div className="flex items-center gap-1">
        <Button variant="ghost" size="icon-sm" aria-label="Search" onClick={() => openSearch()}>
          <SearchIcon />
        </Button>
        <SettingsMenu onUnpair={() => setUnpairing(true)} />
      </div>
      <UnpairDialog name={name} open={unpairing} onOpenChange={setUnpairing} />
    </header>
  )
}

const dayOf = (m: ChatMessage) => new Date(m.ts).toDateString()

function Transcript() {
  const { messages, hasOlder, loadingOlder, myId, myName, peerName, people, peerHas, peerSeen } =
    useSelector(
      store,
      (s) => ({
        messages: s.transcript.messages,
        hasOlder: s.transcript.hasOlder,
        loadingOlder: s.loadingOlder,
        myId: s.status?.me.id ?? '',
        myName: s.status?.me.name || 'You',
        peerName: s.status?.peer?.name || 'Them',
        people: peopleKey(s.status?.people),
        peerHas: s.status?.peer_has ?? 0,
        peerSeen: s.status?.peer_seen ?? 0,
      }),
      { compare: shallow },
    )
  // Stable while nobody's name changes, so the memoized rows stay put.
  const nameOf = useMemo(() => namer(people, myId, peerName), [people, myId, peerName])

  if (messages.length === 0) {
    return (
      <div className="grid flex-1 place-items-center text-sm text-muted-foreground">
        No messages yet. Say hi.
      </div>
    )
  }
  return (
    <VirtualTranscript
      messages={messages}
      hasOlder={hasOlder}
      loadingOlder={loadingOlder}
      myId={myId}
      myName={myName}
      peerName={peerName}
      nameOf={nameOf}
      peerHas={peerHas}
      peerSeen={peerSeen}
    />
  )
}

/** Where one of my messages has got to. */
type Receipt = 'waiting' | 'delivered' | 'seen'

function receiptFor(seq: number, peerHas: number, peerSeen: number): Receipt {
  if (seq <= peerSeen) return 'seen'
  return seq <= peerHas ? 'delivered' : 'waiting'
}

/** Rows within this many of the oldest loaded one trigger loading the previous page. */
const LOAD_OLDER_WITHIN = 15
/** How long a message stays highlighted after jumping to it from a reply or search. */
const HIGHLIGHT_MS = 1500

/**
 * The transcript, virtualized: only the rows on screen (plus `overscan` either side)
 * are in the DOM, so a long scroll-back costs the same as a short one.
 *
 * TanStack Virtual owns the rows: `anchorTo: 'end'` keeps the row you're reading in
 * place when older pages are added above, and `followOnAppend` follows new messages
 * while you're at the bottom. The Message Scroller provides the frame: the initial
 * jump to the end and the jump-to-latest button.
 */
function VirtualTranscript({
  messages,
  hasOlder,
  loadingOlder,
  myId,
  myName,
  peerName,
  nameOf,
  peerHas,
  peerSeen,
}: {
  messages: ChatMessage[]
  hasOlder: boolean
  loadingOlder: boolean
  myId: string
  myName: string
  peerName: string
  nameOf: (author: string) => string
  peerHas: number
  peerSeen: number
}) {
  const viewportRef = useRef<HTMLDivElement>(null)
  // Ignore the first renders: before the initial jump to the end, the top rows are
  // "visible" and would trigger a pointless page load.
  const scrolledRef = useRef(false)
  const [highlighted, setHighlighted] = useState<string | null>(null)

  // useVirtualizer returns a mutable instance by design. That only matters to the React
  // Compiler (not used here); the memoized rows get plain values, never the instance.
  // oxlint-disable-next-line react/incompatible-library
  const virtualizer = useVirtualizer({
    count: messages.length,
    getScrollElement: () => viewportRef.current,
    estimateSize: () => 48,
    getItemKey: (index) => messageKey(messages[index]),
    overscan: 12,
    anchorTo: 'end',
    followOnAppend: true,
  })
  const items = virtualizer.getVirtualItems()
  const firstIndex = items[0]?.index ?? 0

  useEffect(() => {
    if (scrolledRef.current && hasOlder && !loadingOlder && firstIndex < LOAD_OLDER_WITHIN) {
      void loadOlder()
    }
  }, [firstIndex, hasOlder, loadingOlder])

  useEffect(() => {
    const timer = highlighted ? setTimeout(() => setHighlighted(null), HIGHLIGHT_MS) : undefined
    return () => clearTimeout(timer)
  }, [highlighted])

  const jump = useSelector(store, (s) => s.jump)
  useEffect(() => {
    if (!jump) return
    const index = indexOf(store.state.transcript, jump.at)
    if (index < 0) return
    virtualizer.scrollToIndex(index, { align: 'center' })
    setHighlighted(messageKey(jump.at))
  }, [jump, virtualizer])

  return (
    <MessageScroller className="flex-1">
      <MessageScrollerViewport
        ref={viewportRef}
        preserveScrollOnPrepend={false}
        onScroll={() => (scrolledRef.current = true)}
      >
        <MessageScrollerContent className="block min-h-full px-4 pt-2 pb-4">
          <div className="relative w-full" style={{ height: virtualizer.getTotalSize() }}>
            {items.map((item) => {
              const m = messages[item.index]
              const prev = messages[item.index - 1]
              const next = messages[item.index + 1]
              return (
                <div
                  key={item.key}
                  ref={virtualizer.measureElement}
                  data-index={item.index}
                  className="absolute inset-s-0 top-0 w-full"
                  style={{ transform: `translateY(${item.start}px)` }}
                >
                  <Row
                    message={m}
                    receipt={m.author === myId ? receiptFor(m.seq, peerHas, peerSeen) : null}
                    myId={myId}
                    myName={myName}
                    peerName={peerName}
                    nameOf={nameOf}
                    newDay={!prev || dayOf(prev) !== dayOf(m)}
                    groupStart={!prev || !sameGroup(prev, m)}
                    groupEnd={!next || !sameGroup(m, next)}
                    highlighted={highlighted === item.key}
                    onJump={jumpToQuote}
                  />
                </div>
              )
            })}
          </div>
        </MessageScrollerContent>
      </MessageScrollerViewport>
      {loadingOlder && (
        <Spinner className="absolute top-3 left-1/2 -translate-x-1/2 text-muted-foreground" />
      )}
      <MessageScrollerButton />
    </MessageScroller>
  )
}

/** Consecutive messages from one person, close together in time, form a group. */
const GROUP_GAP_MS = 5 * 60_000
const sameGroup = (a: ChatMessage, b: ChatMessage) =>
  a.author === b.author && b.ts - a.ts < GROUP_GAP_MS && dayOf(a) === dayOf(b)

/**
 * One message. Memoized: re-renders only when its own props change (for example its
 * group ending because a new message arrived, its delivery state, or a reaction).
 */
const Row = memo(function Row({
  message,
  receipt,
  myId,
  myName,
  peerName,
  nameOf,
  newDay,
  groupStart,
  groupEnd,
  highlighted,
  onJump,
}: {
  message: ChatMessage
  /** Only for my own messages. */
  receipt: Receipt | null
  myId: string
  myName: string
  /** Whoever I'm paired with now. */
  peerName: string
  /** An author's name: "You" for me, and the name they last had for anyone else. */
  nameOf: (author: string) => string
  newDay: boolean
  /** First in its group: space above it. */
  groupStart: boolean
  /** Last in its group: show the time and delivery state under it. */
  groupEnd: boolean
  /** Just jumped to from a reply. */
  highlighted: boolean
  onJump: (quote: Quote) => void
}) {
  const when = new Date(message.ts)
  const mine = receipt !== null
  const align = mine ? 'end' : 'start'
  const myReaction = message.reactions.find((r) => r.author === myId)?.emoji
  // Picking my current reaction again takes it back.
  const react = (emoji: string) =>
    void run(
      api.react({ author: message.author, seq: message.seq }, emoji === myReaction ? '' : emoji),
    )
  const footer = groupEnd && (
    <MessageFooter className="text-[11px] font-normal">
      {when.toLocaleTimeString([], { hour: 'numeric', minute: '2-digit' })}
      {receipt && ` · ${receipt}`}
    </MessageFooter>
  )

  if (message.deleted) {
    return (
      <div className={groupStart ? 'pt-3' : 'pt-0.5'}>
        {newDay && <DayMarker when={when} />}
        <Message align={align}>
          <MessageContent className="gap-1">
            <Bubble variant="outline" align={align}>
              <BubbleContent className="text-muted-foreground italic" title={when.toLocaleString()}>
                {message.vanished
                  ? 'Message vanished'
                  : mine
                    ? 'You deleted this message'
                    : 'Message deleted'}
              </BubbleContent>
            </Bubble>
            {footer}
          </MessageContent>
        </Message>
      </div>
    )
  }

  // `/me waves`: an action line, "Alex waves", instead of a bubble.
  const action = message.text.startsWith('/me ') && !message.file ? message.text.slice(4) : null
  const { file } = message
  const words = (
    <>
      {action === null ? (
        <Linkified text={message.text} />
      ) : (
        <span className="italic">
          <span className="font-semibold not-italic">{mine ? myName : nameOf(message.author)}</span>{' '}
          <Linkified text={action} />
        </span>
      )}
      {message.edited && <span className="ms-1.5 text-[11px] opacity-70">edited</span>}
      {message.vanish !== null && (
        <VanishTimer seconds={message.vanish} expires={message.expires} />
      )}
    </>
  )

  return (
    <div className={groupStart ? 'pt-3' : 'pt-0.5'}>
      {newDay && <DayMarker when={when} />}
      <Message align={align}>
        <MessageContent className="gap-1">
          <Bubble
            variant={action !== null ? 'ghost' : mine ? 'default' : 'secondary'}
            align={align}
            // Room for the reaction chip, which hangs three quarters below the bubble.
            className={message.reactions.length > 0 ? 'mb-6' : ''}
          >
            <BubbleContent
              // A file sits close to the bubble's edge, with the caption under it.
              className={`whitespace-pre-wrap transition-shadow ${file ? 'p-1' : ''} ${highlighted ? 'ring-3 ring-ring' : ''}`}
              title={when.toLocaleString()}
            >
              {message.reply_to && (
                <QuoteButton
                  quote={message.reply_to}
                  name={nameOf(message.reply_to.author)}
                  onJump={onJump}
                />
              )}
              {file ? (
                <>
                  {/* Keyed by state: a picture that failed to load gets another go once it comes back. */}
                  <FileView
                    key={file.state}
                    file={file}
                    peer={mine ? peerName : nameOf(message.author)}
                  />
                  {(message.text || message.edited) && (
                    <div className="px-2 pt-1 pb-0.5">{words}</div>
                  )}
                </>
              ) : (
                words
              )}
            </BubbleContent>
            {message.reactions.length > 0 && (
              <Reactions
                reactions={message.reactions}
                align={mine ? 'start' : 'end'}
                nameOf={nameOf}
                mine={myReaction}
                onReact={react}
              />
            )}
            <Actions
              side={mine ? 'start' : 'end'}
              mine={myReaction}
              onReact={react}
              onReply={() => setReplyingTo(message)}
              onEdit={mine ? () => setEditing(message) : undefined}
              onDelete={mine ? () => setDeleting(message) : undefined}
            />
          </Bubble>
          {footer}
        </MessageContent>
      </Message>
    </div>
  )
})

/**
 * On a vanishing message: how long it lasts once seen, then a countdown. Ticks only
 * while counting down.
 */
function VanishTimer({ seconds, expires }: { seconds: number; expires: number | null }) {
  const [now, setNow] = useState(Date.now)
  useEffect(() => {
    if (expires === null) return undefined
    const timer = setInterval(() => setNow(Date.now()), 1000)
    return () => clearInterval(timer)
  }, [expires])
  const label =
    expires === null
      ? `${formatDuration(seconds)} once seen`
      : formatDuration(Math.max(0, (expires - now) / 1000))
  return (
    <span
      className="ms-1.5 inline-flex items-center gap-0.5 align-[-1px] text-[11px] opacity-70"
      title="Vanishes from both computers once the time is up"
    >
      <TimerIcon className="size-3" />
      {label}
    </span>
  )
}

/** Whether `until` (ms) is still ahead. Re-renders when it passes. */
function useUntil(until: number | null) {
  const [now, setNow] = useState(Date.now)
  useEffect(() => {
    if (until === null) return undefined
    // Also runs right away for a time already past. setTimeout waits at most about
    // 24.8 days; do not disturb tops out at 7.
    const timer = setTimeout(() => setNow(Date.now()), Math.max(0, until - Date.now()) + 50)
    return () => clearTimeout(timer)
  }, [until])
  return until !== null && until > now
}

function DayMarker({ when }: { when: Date }) {
  return (
    <Marker variant="separator" className="mt-1 mb-3 justify-center text-xs">
      {when.toLocaleDateString([], { weekday: 'long', month: 'short', day: 'numeric' })}
    </Marker>
  )
}

/** Jumps from a reply to the message it quotes, if it's on this computer. */
function jumpToQuote(quote: Quote) {
  const target = targetOf(quote)
  if (target) void reveal(target)
}

/** The message a reply answers, at the top of its bubble. Click to jump to it. */
function QuoteButton({
  quote,
  name,
  onJump,
}: {
  quote: Quote
  name: string
  onJump: (quote: Quote) => void
}) {
  return (
    <button
      type="button"
      className="mb-1.5 block w-full rounded-md border-l-3 border-current/70 bg-current/10 py-1 pr-2 pl-2 text-left text-xs hover:bg-current/15"
      onClick={() => onJump(quote)}
    >
      <span className="font-medium">{name}</span>
      <span className="line-clamp-2 opacity-80">
        {quote.vanished
          ? 'Message vanished'
          : quote.deleted
            ? 'Deleted message'
            : quote.file && !quote.text
              ? describeFile(quote.file)
              : quote.text?.startsWith('/me ')
                ? `* ${quote.text.slice(4)}`
                : (quote.text ?? 'A message that isn’t on this computer')}
      </span>
    </button>
  )
}

/** The reactions under a bubble. Click one to react the same, or to take yours back. */
function Reactions({
  reactions,
  align,
  nameOf,
  mine,
  onReact,
}: {
  reactions: Reaction[]
  align: 'start' | 'end'
  nameOf: (author: string) => string
  mine: string | undefined
  onReact: (emoji: string) => void
}) {
  // Same emoji from both people: one chip with a count.
  const byEmoji = new Map<string, string[]>()
  for (const r of reactions) byEmoji.set(r.emoji, [...(byEmoji.get(r.emoji) ?? []), r.author])
  return (
    <BubbleReactions align={align}>
      {[...byEmoji].map(([emoji, authors]) => (
        <button
          key={emoji}
          type="button"
          className={`flex items-center gap-0.5 rounded-full px-1.5 py-0.5 ${emoji === mine ? 'bg-accent' : ''}`}
          title={`${authors.map(nameOf).join(' and ')} reacted ${emoji}`}
          aria-label={
            emoji === mine
              ? `Take back your ${emoji}`
              : `React ${emoji}, like ${authors.map(nameOf).join(' and ')}`
          }
          onClick={() => onReact(emoji)}
        >
          {emoji}
          {authors.length > 1 && <span className="text-xs">{authors.length}</span>}
        </button>
      ))}
    </BubbleReactions>
  )
}

/** React, Reply, and (on my own messages) Edit and Delete, beside the bubble on hover. */
function Actions({
  side,
  mine,
  onReact,
  onReply,
  onEdit,
  onDelete,
}: {
  /** Which side of the bubble. */
  side: 'start' | 'end'
  mine: string | undefined
  onReact: (emoji: string) => void
  onReply: () => void
  onEdit?: () => void
  onDelete?: () => void
}) {
  return (
    <div
      className={`absolute top-1/2 flex -translate-y-1/2 items-center opacity-0 transition-opacity group-hover/bubble:opacity-100 focus-within:opacity-100 has-data-popup-open:opacity-100 ${side === 'start' ? 'right-full flex-row-reverse pr-1' : 'left-full pl-1'}`}
    >
      <DropdownMenu>
        <DropdownMenuTrigger render={<Button variant="ghost" size="icon-sm" aria-label="React" />}>
          <SmilePlusIcon />
        </DropdownMenuTrigger>
        <DropdownMenuContent side="top" align="center" className="flex w-auto min-w-0 gap-0.5">
          {EMOJI.map((emoji) => (
            <DropdownMenuItem
              key={emoji}
              className={`px-1.5 text-lg ${emoji === mine ? 'bg-accent' : ''}`}
              aria-label={emoji === mine ? `Take back ${emoji}` : `React ${emoji}`}
              onClick={() => onReact(emoji)}
            >
              {emoji}
            </DropdownMenuItem>
          ))}
        </DropdownMenuContent>
      </DropdownMenu>
      <Button variant="ghost" size="icon-sm" aria-label="Reply" onClick={onReply}>
        <ReplyIcon />
      </Button>
      {onEdit && onDelete && (
        <DropdownMenu>
          <DropdownMenuTrigger render={<Button variant="ghost" size="icon-sm" aria-label="More" />}>
            <EllipsisIcon />
          </DropdownMenuTrigger>
          <DropdownMenuContent side="top" align="center" className="w-32">
            <DropdownMenuItem onClick={onEdit}>
              <PencilIcon />
              Edit
            </DropdownMenuItem>
            <DropdownMenuItem variant="destructive" onClick={onDelete}>
              <Trash2Icon />
              Delete…
            </DropdownMenuItem>
          </DropdownMenuContent>
        </DropdownMenu>
      )}
    </div>
  )
}

/** My newest message that's still there to edit. (`findLast` is too new for the target.) */
function lastOfMine(messages: ChatMessage[], myId: string) {
  for (let i = messages.length - 1; i >= 0; i--) {
    if (messages[i].author === myId && !messages[i].deleted) return messages[i]
  }
  return undefined
}

/** Browsers without CSS `field-sizing` (older WebKit) need help growing the textarea. */
const autoSizes = typeof CSS !== 'undefined' && CSS.supports('field-sizing', 'content')

/** Fit the textarea to its text, up to its CSS max height. */
function fit(el: HTMLTextAreaElement) {
  if (autoSizes) return
  // scrollHeight excludes the border; add it back or a scrollbar appears.
  const border = el.offsetHeight - el.clientHeight
  el.style.height = 'auto'
  el.style.height = `${el.scrollHeight + border}px`
}

function Composer() {
  const [text, setText] = useState('')
  const [hint, setHint] = useState<string | null>(null)
  // Which suggestion is highlighted, and whether Escape closed the list.
  const [active, setActive] = useState(0)
  const [dismissed, setDismissed] = useState(false)
  const [clearing, setClearing] = useState(false)
  const [sending, setSending] = useState(false)
  const textRef = useRef<HTMLTextAreaElement>(null)
  const pickerRef = useRef<HTMLInputElement>(null)
  const typingSentRef = useRef(0)
  const { scrollToEnd } = useMessageScroller()
  const { replyingTo, editing, peerName, myId, away, dndUntil, staged } = useSelector(
    store,
    (s) => ({
      replyingTo: s.replyingTo,
      editing: s.editing,
      peerName: s.status?.peer?.name || 'them',
      myId: s.status?.me.id ?? '',
      away: s.status?.away ? (s.status.away.note ?? '') : null,
      dndUntil: s.status?.dnd_until ?? null,
      staged: s.staged.length,
    }),
    { compare: shallow },
  )
  const dnd = useUntil(dndUntil)
  const [showingStatus, setShowingStatus] = useState(false)
  // What was in the box before editing started; it comes back when editing ends.
  const draftRef = useRef<string | null>(null)

  // Choosing Reply on a message: start typing straight away.
  useEffect(() => {
    if (replyingTo) textRef.current?.focus()
  }, [replyingTo])

  // Editing puts the message's text in the box; finishing or cancelling puts the draft back.
  useEffect(() => {
    if (editing) {
      setText((current) => {
        draftRef.current ??= current
        return editing.text
      })
      textRef.current?.focus()
    } else if (draftRef.current !== null) {
      setText(draftRef.current)
      draftRef.current = null
    }
    setHint(null)
    setDismissed(true) // an edit is text, not a command
    const el = textRef.current
    if (el) requestAnimationFrame(() => fit(el))
  }, [editing])

  const suggestions = dismissed ? [] : suggest(text)
  const highlighted = suggestions[Math.min(active, suggestions.length - 1)]

  const change = (value: string) => {
    setText(value)
    setHint(null)
    setActive(0)
    setDismissed(!!editing)
    const now = Date.now()
    const typing = value.trim() && !value.startsWith('/') && !editing
    if (typing && now - typingSentRef.current > TYPING_EVERY_MS) {
      typingSentRef.current = now
      void api.typing().catch(() => {})
    }
  }

  const reset = () => {
    setText('')
    setHint(null)
    setActive(0)
    setDismissed(false)
    if (!autoSizes && textRef.current) textRef.current.style.height = ''
  }

  const perform = (command: Command, args = '') => {
    const typed = text
    reset()
    void command.action(
      {
        confirmClear: () => setClearing(true),
        send: (value, vanish) => sendText(value, vanish, typed),
        hint: (message) => {
          setText(typed) // keep what was typed, to fix it
          setDismissed(true)
          setHint(message)
        },
        showStatus: () => setShowingStatus(true),
      },
      args,
    )
  }

  const saveEdit = async (of: ChatMessage) => {
    const value = text.trim()
    if (!value && !of.file) {
      setHint('An edit can’t be empty. To remove the message, choose Delete instead.')
      return
    }
    const saved = await run(
      api.editMessage({ author: of.author, seq: of.seq }, value).then(() => true),
    )
    if (saved) setEditing(null)
  }

  const send = async () => {
    if (editing) return saveEdit(editing)
    const parsed = parse(text)
    if (parsed.kind === 'command') {
      if (parsed.command) perform(parsed.command, parsed.args)
      else setHint(`There's no /${parsed.name} command. To send it as a message, start with //.`)
      return
    }
    if (store.state.staged.length > 0) await sendFiles(parsed.text)
    else if (parsed.text) await sendText(parsed.text, undefined, parsed.text)
  }

  /**
   * Sends the pictures in the message box, one message each. The first carries the
   * caption and the reply. One that fails stays in the box, with those after it.
   */
  const sendFiles = async (caption: string) => {
    if (sending) return
    const reply = replyingTo
    reset()
    setReplyingTo(null)
    setSending(true)
    for (const [i, s] of store.state.staged.entries()) {
      const first = i === 0
      const to = first && reply ? { author: reply.author, seq: reply.seq } : undefined
      const sent = await run(
        s.file
          .arrayBuffer()
          .then((bytes) => api.sendFile(bytes, s.details, first ? caption : '', to)),
      )
      if (!sent) {
        if (first) {
          setText(caption)
          setReplyingTo(reply)
        }
        break
      }
      unstageFile(s.id)
      addMessage(sent)
      scrollToEnd({ behavior: 'smooth' })
    }
    setSending(false)
  }

  /** Sends `value`, replying if a reply was started. `draft` comes back if it fails. */
  const sendText = async (value: string, vanish: number | undefined, draft: string) => {
    const reply = replyingTo
    reset()
    setReplyingTo(null)
    typingSentRef.current = 0 // the next message signals typing right away
    const to = reply ? { author: reply.author, seq: reply.seq } : undefined
    const sent = await run(api.sendMessage(value, to, vanish))
    if (sent) {
      addMessage(sent)
      scrollToEnd({ behavior: 'smooth' })
    } else {
      // Keep the draft, and what it was replying to, if it failed.
      setText(draft)
      setReplyingTo(reply)
    }
  }

  const onKeyDown = (e: React.KeyboardEvent<HTMLTextAreaElement>) => {
    if (e.nativeEvent.isComposing) return
    if (highlighted) {
      const step = { ArrowDown: 1, ArrowUp: -1 }[e.key]
      if (step) {
        e.preventDefault()
        const at = suggestions.indexOf(highlighted)
        setActive((at + step + suggestions.length) % suggestions.length)
        return
      }
      if (e.key === 'Tab') {
        e.preventDefault()
        setText(`/${highlighted.name}${highlighted.args ? ' ' : ''}`)
        return
      }
      if (e.key === 'Escape') {
        e.preventDefault()
        setDismissed(true)
        return
      }
      if (e.key === 'Enter' && !e.shiftKey) {
        e.preventDefault()
        perform(highlighted)
        return
      }
    }
    if (e.key === 'Escape' && (replyingTo || editing)) {
      e.preventDefault()
      if (editing) setEditing(null)
      else setReplyingTo(null)
      return
    }
    // Up in an empty box edits my last message.
    if (e.key === 'ArrowUp' && !text && !editing) {
      const last = lastOfMine(store.state.transcript.messages, myId)
      if (last) {
        e.preventDefault()
        setEditing(last)
      }
      return
    }
    if (e.key === 'Enter' && !e.shiftKey) {
      e.preventDefault()
      void send()
    }
  }

  return (
    <form
      className="relative border-t bg-card px-4 py-3"
      onSubmit={(e) => {
        e.preventDefault()
        void send()
      }}
    >
      {highlighted && (
        <CommandList
          commands={suggestions}
          highlighted={highlighted}
          onHighlight={(c) => setActive(suggestions.indexOf(c))}
          onPick={perform}
        />
      )}
      {hint && (
        <p className="absolute right-4 bottom-full left-4 mb-2 rounded-lg border bg-popover px-3 py-2 text-sm text-popover-foreground shadow-md">
          {hint}
        </p>
      )}
      {dnd && dndUntil !== null && (
        <div className="mb-2 flex items-center gap-2 rounded-md border-l-2 border-destructive bg-muted py-1 pr-1 pl-2 text-xs">
          <div className="min-w-0 flex-1">
            <div className="truncate font-medium">Do not disturb until {formatUntil(dndUntil)}</div>
            <div className="truncate text-muted-foreground">
              No notifications, and nudges are paused
            </div>
          </div>
          <Button
            type="button"
            variant="ghost"
            size="xs"
            onClick={() => void run(api.setDnd(null))}
          >
            Turn off
          </Button>
        </div>
      )}
      {away !== null && (
        <div className="mb-2 flex items-center gap-2 rounded-md border-l-2 border-warning bg-muted py-1 pr-1 pl-2 text-xs">
          <div className="min-w-0 flex-1">
            <div className="truncate font-medium">You’re away{away && `: ${away}`}</div>
            <div className="truncate text-muted-foreground">
              Until you send a message or type /back
            </div>
          </div>
          <Button type="button" variant="ghost" size="xs" onClick={() => void run(api.back())}>
            I’m back
          </Button>
        </div>
      )}
      {replyingTo && (
        <div className="mb-2 flex items-center gap-2 rounded-md border-l-2 border-primary bg-muted py-1 pr-1 pl-2 text-xs">
          <div className="min-w-0 flex-1">
            <div className="font-medium">
              Replying to {replyingTo.author === myId ? 'yourself' : peerName}
            </div>
            <div className="truncate text-muted-foreground">
              {replyingTo.text || (replyingTo.file && describeFile(replyingTo.file))}
            </div>
          </div>
          <Button
            type="button"
            variant="ghost"
            size="icon-xs"
            aria-label="Cancel reply"
            onClick={() => setReplyingTo(null)}
          >
            <XIcon />
          </Button>
        </div>
      )}
      {editing && (
        <div className="mb-2 flex items-center gap-2 rounded-md border-l-2 border-primary bg-muted py-1 pr-1 pl-2 text-xs">
          <div className="min-w-0 flex-1">
            <div className="font-medium">Editing your message</div>
            <div className="truncate text-muted-foreground">Enter to save · Esc to cancel</div>
          </div>
          <Button
            type="button"
            variant="ghost"
            size="icon-xs"
            aria-label="Cancel edit"
            onClick={() => setEditing(null)}
          >
            <XIcon />
          </Button>
        </div>
      )}
      <StagedFiles sending={sending} />
      <div className="flex items-end gap-2">
        {!editing && (
          <>
            <Button
              type="button"
              variant="ghost"
              size="icon-lg"
              aria-label="Add pictures"
              title="Add pictures (or paste, or drop them here)"
              onClick={() => pickerRef.current?.click()}
            >
              <ImagePlusIcon />
            </Button>
            <input
              ref={pickerRef}
              type="file"
              accept="image/*,.heic,.heif"
              multiple
              hidden
              onChange={(e) => {
                void stageFiles([...(e.target.files ?? [])])
                e.target.value = '' // so the same picture can be chosen again
              }}
            />
          </>
        )}
        <Textarea
          ref={textRef}
          rows={1}
          maxLength={4000}
          placeholder={staged ? 'Add a caption' : 'Message, or / for commands'}
          aria-label="Message"
          className="max-h-36 min-h-9 resize-none py-1.5 text-sm"
          value={text}
          onChange={(e) => {
            change(e.target.value)
            fit(e.target)
          }}
          onKeyDown={onKeyDown}
          onPaste={(e) => {
            // A pasted picture, such as a screenshot, goes with the next message.
            const files = [...e.clipboardData.files]
            if (files.length > 0 && !editing) {
              e.preventDefault()
              void stageFiles(files)
            }
          }}
        />
        <Button type="submit" size="lg" disabled={sending}>
          {sending ? <Spinner /> : editing ? 'Save' : 'Send'}
        </Button>
      </div>
      <ClearDialog open={clearing} onOpenChange={setClearing} />
      <StatusDialog open={showingStatus} onOpenChange={setShowingStatus} />
    </form>
  )
}

/** The commands matching what's typed so far, above the message box. */
function CommandList({
  commands,
  highlighted,
  onHighlight,
  onPick,
}: {
  commands: Command[]
  highlighted: Command
  onHighlight: (command: Command) => void
  onPick: (command: Command) => void
}) {
  return (
    <ul className="absolute bottom-full left-4 mb-2 w-md max-w-[calc(100%-2rem)] rounded-lg border bg-popover p-1 text-popover-foreground shadow-md">
      {commands.map((c) => (
        <li key={c.name}>
          <button
            type="button"
            className={`flex w-full items-baseline gap-3 rounded-md px-2 py-1.5 text-left text-sm ${c === highlighted ? 'bg-accent text-accent-foreground' : ''}`}
            // Keep the focus in the message box.
            onMouseDown={(e) => e.preventDefault()}
            onMouseEnter={() => onHighlight(c)}
            onClick={() => onPick(c)}
          >
            <span className="shrink-0 font-mono whitespace-nowrap">
              /{c.name}
              {c.args && <span className="text-muted-foreground"> {c.args}</span>}
            </span>
            <span className="text-xs text-muted-foreground">{c.description}</span>
          </button>
        </li>
      ))}
      <li className="px-2 pt-1 pb-0.5 text-[11px] text-muted-foreground">
        ↑↓ to choose · Enter to run · Tab to complete · Esc to close
      </li>
    </ul>
  )
}

function ClearDialog({
  open,
  onOpenChange,
}: {
  open: boolean
  onOpenChange: (open: boolean) => void
}) {
  const name = useSelector(store, (s) => s.status?.peer?.name || 'The other person')
  return (
    <AlertDialog open={open} onOpenChange={onOpenChange}>
      <AlertDialogContent size="sm">
        <AlertDialogHeader>
          <AlertDialogTitle>Clear this conversation?</AlertDialogTitle>
          <AlertDialogDescription>
            This erases the history on this computer, and it can’t be undone. {name} keeps their
            copy. Messages that haven’t reached them yet stay, so they still get delivered.
          </AlertDialogDescription>
        </AlertDialogHeader>
        <AlertDialogFooter>
          <AlertDialogCancel>Cancel</AlertDialogCancel>
          <AlertDialogAction
            variant="destructive"
            onClick={() => {
              // AlertDialogAction is a plain button: it doesn't close the dialog itself.
              onOpenChange(false)
              void clearHistory()
            }}
          >
            Clear
          </AlertDialogAction>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  )
}

/** "Delete this message?" for the message chosen in its ⋯ menu. */
function DeleteDialog() {
  const { deleting, name } = useSelector(
    store,
    (s) => ({ deleting: s.deleting, name: s.status?.peer?.name || 'the other person' }),
    { compare: shallow },
  )
  return (
    <AlertDialog open={!!deleting} onOpenChange={(open) => !open && setDeleting(null)}>
      <AlertDialogContent size="sm">
        <AlertDialogHeader>
          <AlertDialogTitle>Delete this message?</AlertDialogTitle>
          <AlertDialogDescription>
            It’s erased for you and {name}. If they’re offline, it’s erased when they’re back.
          </AlertDialogDescription>
        </AlertDialogHeader>
        <AlertDialogFooter>
          <AlertDialogCancel>Cancel</AlertDialogCancel>
          <AlertDialogAction
            variant="destructive"
            onClick={() => {
              // AlertDialogAction is a plain button: it doesn't close the dialog itself.
              setDeleting(null)
              if (deleting)
                void run(api.deleteMessage({ author: deleting.author, seq: deleting.seq }))
            }}
          >
            Delete
          </AlertDialogAction>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  )
}

function UnpairDialog({
  name,
  open,
  onOpenChange,
}: {
  name: string
  open: boolean
  onOpenChange: (open: boolean) => void
}) {
  return (
    <AlertDialog open={open} onOpenChange={onOpenChange}>
      <AlertDialogContent size="sm">
        <AlertDialogHeader>
          <AlertDialogTitle>Unpair from {name}?</AlertDialogTitle>
          <AlertDialogDescription>
            Your history stays on this computer. To chat again you’ll need to pair again.
          </AlertDialogDescription>
        </AlertDialogHeader>
        <AlertDialogFooter>
          <AlertDialogCancel>Cancel</AlertDialogCancel>
          <AlertDialogAction variant="destructive" onClick={() => void run(api.unpair())}>
            Unpair
          </AlertDialogAction>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  )
}
