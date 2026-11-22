import { Fragment } from 'preact'
import { useLayoutEffect, useRef, useState } from 'preact/hooks'

import { api, type Message } from '../api'
import { addMessages, messages, run, status } from '../store'

/** Paired: the conversation. */
export function Chat() {
  const s = status.value!
  const peer = s.peer!

  return (
    <section class="flex h-full flex-col">
      <header class="flex items-center justify-between gap-3 border-b border-line bg-panel px-4 py-3">
        <div>
          <h1 class="text-[17px] font-semibold">{peer.name || 'Your peer'}</h1>
          <span class="flex items-center gap-1.5 text-xs text-muted">
            <span class={`size-1.75 rounded-full ${s.connected ? 'bg-ok' : 'bg-muted'}`} />
            {s.connected ? 'Connected' : 'Not connected · messages send when you’re both online'}
          </span>
        </div>
        <Unpair name={peer.name} />
      </header>
      <MessageList myId={s.me.id} peerHas={s.peer_has} />
      <Composer />
    </section>
  )
}

const dayOf = (m: Message) => new Date(m.ts).toDateString()

function MessageList({ myId, peerHas }: { myId: string; peerHas: number }) {
  const list = messages.value
  const listRef = useRef<HTMLElement>(null)
  // Follow new messages unless the user has scrolled up to read.
  const stickRef = useRef(true)

  useLayoutEffect(() => {
    const el = listRef.current
    const last = list.at(-1)
    if (el && (stickRef.current || last?.author === myId)) el.scrollTop = el.scrollHeight
  }, [list, myId])

  if (list.length === 0) {
    return <main class="grid flex-1 place-items-center text-muted">No messages yet. Say hi.</main>
  }

  return (
    <main
      ref={listRef}
      aria-live="polite"
      class="flex flex-1 flex-col gap-1 overflow-y-auto p-4"
      onScroll={(e) => {
        const el = e.currentTarget
        stickRef.current = el.scrollHeight - el.scrollTop - el.clientHeight < 80
      }}
    >
      {list.map((m, i) => {
        const when = new Date(m.ts)
        const showDay = i === 0 || dayOf(list[i - 1]) !== dayOf(m)
        return (
          <Fragment key={`${m.author}:${m.seq}`}>
            {showDay && (
              <div class="mt-2.5 mb-1 self-center text-xs text-muted">
                {when.toLocaleDateString([], { weekday: 'long', month: 'short', day: 'numeric' })}
              </div>
            )}
            <Bubble m={m} when={when} mine={m.author === myId} peerHas={peerHas} />
          </Fragment>
        )
      })}
    </main>
  )
}

function Bubble({
  m,
  when,
  mine,
  peerHas,
}: {
  m: Message
  when: Date
  mine: boolean
  peerHas: number
}) {
  const time = when.toLocaleTimeString([], { hour: 'numeric', minute: '2-digit' })
  return (
    <div
      class={`max-w-[78%] rounded-2xl border px-2.75 py-1.75 wrap-break-word whitespace-pre-wrap ${
        mine
          ? 'self-end border-transparent bg-mine text-on-mine'
          : 'self-start border-line bg-theirs'
      }`}
    >
      {m.text}
      <span class="mt-0.5 block text-right text-[11px] opacity-70">
        {time}
        {mine && (m.seq <= peerHas ? ' · delivered' : ' · waiting')}
      </span>
    </div>
  )
}

function Composer() {
  const [text, setText] = useState('')
  const textRef = useRef<HTMLTextAreaElement>(null)

  // Grow with the content, up to the CSS max height. scrollHeight excludes the border,
  // so add it back or the box ends up 2px short and shows a scrollbar.
  useLayoutEffect(() => {
    const el = textRef.current
    if (!el) return
    const border = el.offsetHeight - el.clientHeight
    el.style.height = 'auto'
    el.style.height = `${el.scrollHeight + border}px`
  }, [text])

  const send = async () => {
    const value = text.trim()
    if (!value) return
    setText('')
    const sent = await run(api.sendMessage(value))
    if (sent) addMessages([sent])
    else setText(value) // keep the draft if it failed
  }

  return (
    <form
      class="flex gap-2 border-t border-line bg-panel px-4 py-3"
      onSubmit={(e) => {
        e.preventDefault()
        void send()
      }}
    >
      <textarea
        ref={textRef}
        rows={1}
        maxLength={4000}
        placeholder="Message"
        class="field max-h-35 resize-none"
        value={text}
        onInput={(e) => setText(e.currentTarget.value)}
        onKeyDown={(e) => {
          if (e.key === 'Enter' && !e.shiftKey && !e.isComposing) {
            e.preventDefault()
            void send()
          }
        }}
      />
      <button class="btn-primary">Send</button>
    </form>
  )
}

/** "Unpair", then an inline "Unpair from X? Yes / No". */
function Unpair({ name }: { name: string }) {
  const [confirming, setConfirming] = useState(false)

  if (!confirming) {
    return (
      <button class="btn-link text-[13px]" onClick={() => setConfirming(true)}>
        Unpair
      </button>
    )
  }
  return (
    <span class="flex items-center gap-2 text-[13px]">
      <span class="text-muted">Unpair from {name}?</span>
      <button class="btn-link" onClick={() => run(api.unpair())}>
        Yes
      </button>
      <button class="btn-link" onClick={() => setConfirming(false)}>
        No
      </button>
    </span>
  )
}
