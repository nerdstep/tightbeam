import { shallow, useSelector } from '@tanstack/react-store'
import { useVirtualizer } from '@tanstack/react-virtual'
import { memo, useEffect, useRef, useState } from 'react'

import { api, type Message as ChatMessage } from '@/api'
import { Linkified } from '@/components/Linkified'
import { SettingsMenu } from '@/components/SettingsMenu'
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
import { Bubble, BubbleContent } from '@/components/ui/bubble'
import { Button } from '@/components/ui/button'
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
import { addMessage, loadOlder, messageKey, run, store } from '@/store'

/** Paired: the conversation. */
export function Chat() {
  return (
    <MessageScrollerProvider defaultScrollPosition="end">
      <section className="flex h-full flex-col">
        <Header />
        <Transcript />
        <Composer />
      </section>
    </MessageScrollerProvider>
  )
}

function Header() {
  const { name, connected } = useSelector(
    store,
    (s) => ({ name: s.status?.peer?.name ?? '', connected: !!s.status?.connected }),
    { compare: shallow },
  )
  const [unpairing, setUnpairing] = useState(false)

  return (
    <header className="flex items-center justify-between gap-3 border-b bg-card px-4 py-3">
      <div>
        <h1 className="font-semibold">{name || 'Your peer'}</h1>
        <span className="flex items-center gap-1.5 text-xs text-muted-foreground">
          <span
            className={`size-1.75 rounded-full ${connected ? 'bg-success' : 'bg-muted-foreground'}`}
          />
          {connected ? 'Connected' : 'Not connected · messages send when you’re both online'}
        </span>
      </div>
      <SettingsMenu onUnpair={() => setUnpairing(true)} />
      <UnpairDialog name={name} open={unpairing} onOpenChange={setUnpairing} />
    </header>
  )
}

const dayOf = (m: ChatMessage) => new Date(m.ts).toDateString()

function Transcript() {
  const { messages, hasOlder, loadingOlder, myId, peerHas } = useSelector(
    store,
    (s) => ({
      messages: s.messages,
      hasOlder: s.hasOlder,
      loadingOlder: s.loadingOlder,
      myId: s.status?.me.id ?? '',
      peerHas: s.status?.peer_has ?? 0,
    }),
    { compare: shallow },
  )

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
      peerHas={peerHas}
    />
  )
}

/** Rows within this many of the oldest loaded one trigger loading the previous page. */
const LOAD_OLDER_WITHIN = 15

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
  peerHas,
}: {
  messages: ChatMessage[]
  hasOlder: boolean
  loadingOlder: boolean
  myId: string
  peerHas: number
}) {
  const viewportRef = useRef<HTMLDivElement>(null)
  // Ignore the first renders: before the initial jump to the end, the top rows are
  // "visible" and would trigger a pointless page load.
  const scrolledRef = useRef(false)

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
                    mine={m.author === myId}
                    delivered={m.author === myId && m.seq <= peerHas}
                    newDay={!prev || dayOf(prev) !== dayOf(m)}
                    groupStart={!prev || !sameGroup(prev, m)}
                    groupEnd={!next || !sameGroup(m, next)}
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
 * group ending because a new message arrived, or its delivery state).
 */
const Row = memo(function Row({
  message,
  mine,
  delivered,
  newDay,
  groupStart,
  groupEnd,
}: {
  message: ChatMessage
  mine: boolean
  delivered: boolean
  newDay: boolean
  /** First in its group: space above it. */
  groupStart: boolean
  /** Last in its group: show the time and delivery state under it. */
  groupEnd: boolean
}) {
  const when = new Date(message.ts)
  const align = mine ? 'end' : 'start'
  return (
    <div className={groupStart ? 'pt-3' : 'pt-0.5'}>
      {newDay && (
        <Marker variant="separator" className="mt-1 mb-3 justify-center text-xs">
          {when.toLocaleDateString([], { weekday: 'long', month: 'short', day: 'numeric' })}
        </Marker>
      )}
      <Message align={align}>
        <MessageContent className="gap-1">
          <Bubble variant={mine ? 'default' : 'secondary'} align={align}>
            <BubbleContent className="whitespace-pre-wrap" title={when.toLocaleString()}>
              <Linkified text={message.text} />
            </BubbleContent>
          </Bubble>
          {groupEnd && (
            <MessageFooter className="text-[11px] font-normal">
              {when.toLocaleTimeString([], { hour: 'numeric', minute: '2-digit' })}
              {mine && (delivered ? ' · delivered' : ' · waiting')}
            </MessageFooter>
          )}
        </MessageContent>
      </Message>
    </div>
  )
})

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
  const textRef = useRef<HTMLTextAreaElement>(null)
  const { scrollToEnd } = useMessageScroller()

  const send = async () => {
    const value = text.trim()
    if (!value) return
    setText('')
    if (!autoSizes && textRef.current) textRef.current.style.height = ''
    const sent = await run(api.sendMessage(value))
    if (sent) {
      addMessage(sent)
      scrollToEnd({ behavior: 'smooth' })
    } else {
      setText(value) // keep the draft if it failed
    }
  }

  return (
    <form
      className="flex items-end gap-2 border-t bg-card px-4 py-3"
      onSubmit={(e) => {
        e.preventDefault()
        void send()
      }}
    >
      <Textarea
        ref={textRef}
        rows={1}
        maxLength={4000}
        placeholder="Message"
        aria-label="Message"
        className="max-h-36 min-h-9 resize-none py-1.5 text-sm"
        value={text}
        onChange={(e) => {
          setText(e.target.value)
          fit(e.target)
        }}
        onKeyDown={(e) => {
          if (e.key === 'Enter' && !e.shiftKey && !e.nativeEvent.isComposing) {
            e.preventDefault()
            void send()
          }
        }}
      />
      <Button type="submit" size="lg">
        Send
      </Button>
    </form>
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
