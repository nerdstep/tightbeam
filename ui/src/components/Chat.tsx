import { shallow, useSelector } from '@tanstack/react-store'
import { memo, useEffect, useRef, useState } from 'react'

import { api, type Message as ChatMessage } from '@/api'
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
  AlertDialogTrigger,
} from '@/components/ui/alert-dialog'
import { Bubble, BubbleContent } from '@/components/ui/bubble'
import { Button } from '@/components/ui/button'
import { Marker } from '@/components/ui/marker'
import { Message, MessageContent, MessageFooter } from '@/components/ui/message'
import {
  MessageScroller,
  MessageScrollerButton,
  MessageScrollerContent,
  MessageScrollerItem,
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
    <MessageScrollerProvider autoScroll defaultScrollPosition="end">
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
      <Unpair name={name} />
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
  const viewportRef = useRef<HTMLDivElement>(null)
  const sentinelRef = useRef<HTMLDivElement>(null)

  // Load older pages as the top comes into view (600px early, so it's usually ready
  // before you get there).
  const topVisibleRef = useRef(false)
  useEffect(() => {
    const sentinel = sentinelRef.current
    if (!sentinel || !hasOlder) return undefined
    const observer = new IntersectionObserver(
      (entries) => {
        topVisibleRef.current = entries.some((e) => e.isIntersecting)
        if (topVisibleRef.current) void loadOlder()
      },
      { root: viewportRef.current, rootMargin: '600px 0px 0px 0px' },
    )
    observer.observe(sentinel)
    return () => observer.disconnect()
  }, [hasOlder])

  // The observer only reports changes, so if a page finished loading and the top is
  // still in view (a short page, or a tall window), load the next one.
  useEffect(() => {
    if (!loadingOlder && hasOlder && topVisibleRef.current) void loadOlder()
  }, [loadingOlder, hasOlder])

  if (messages.length === 0) {
    return (
      <div className="grid flex-1 place-items-center text-sm text-muted-foreground">
        No messages yet. Say hi.
      </div>
    )
  }

  return (
    <MessageScroller className="flex-1">
      <MessageScrollerViewport ref={viewportRef}>
        <MessageScrollerContent className="gap-1 px-4 py-4">
          {hasOlder && (
            <div ref={sentinelRef} className="flex justify-center py-2">
              {loadingOlder && <Spinner className="text-muted-foreground" />}
            </div>
          )}
          {messages.map((m, i) => (
            <MessageScrollerItem key={messageKey(m)} messageId={messageKey(m)}>
              <Row
                message={m}
                mine={m.author === myId}
                delivered={m.author === myId && m.seq <= peerHas}
                newDay={i === 0 || dayOf(messages[i - 1]) !== dayOf(m)}
              />
            </MessageScrollerItem>
          ))}
        </MessageScrollerContent>
      </MessageScrollerViewport>
      <MessageScrollerButton />
    </MessageScroller>
  )
}

/** One message. Memoized: re-renders only when its own props change. */
const Row = memo(function Row({
  message,
  mine,
  delivered,
  newDay,
}: {
  message: ChatMessage
  mine: boolean
  delivered: boolean
  newDay: boolean
}) {
  const when = new Date(message.ts)
  const align = mine ? 'end' : 'start'
  return (
    <>
      {newDay && (
        <Marker variant="separator" className="my-3 justify-center text-xs">
          {when.toLocaleDateString([], { weekday: 'long', month: 'short', day: 'numeric' })}
        </Marker>
      )}
      <Message align={align}>
        <MessageContent className="gap-0.5">
          <Bubble variant={mine ? 'default' : 'secondary'} align={align}>
            <BubbleContent className="whitespace-pre-wrap">{message.text}</BubbleContent>
          </Bubble>
          <MessageFooter className="text-[11px] font-normal">
            {when.toLocaleTimeString([], { hour: 'numeric', minute: '2-digit' })}
            {mine && (delivered ? ' · delivered' : ' · waiting')}
          </MessageFooter>
        </MessageContent>
      </Message>
    </>
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
        className="max-h-36 min-h-9 resize-none"
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
      <Button type="submit">Send</Button>
    </form>
  )
}

function Unpair({ name }: { name: string }) {
  return (
    <AlertDialog>
      <AlertDialogTrigger render={<Button variant="ghost" size="sm" />}>Unpair</AlertDialogTrigger>
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
