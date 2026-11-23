import { shallow, useSelector } from '@tanstack/react-store'
import { useEffect, useRef } from 'react'

import { Chat } from './components/Chat'
import { Notice } from './components/Notice'
import { Pair } from './components/Pair'
import { PairDialog } from './components/PairDialog'
import { Setup } from './components/Setup'
import { UpdateBar } from './components/UpdateBar'
import { store } from './store'

export function App() {
  const { ready, named, paired, pairing } = useSelector(
    store,
    (s) => ({
      ready: s.status !== null,
      named: !!s.status?.me.name,
      paired: !!s.status?.peer,
      pairing: s.status?.pairing ?? null,
    }),
    { compare: shallow },
  )

  const ref = useRef<HTMLDivElement>(null)
  useShakeOnNudge(ref)

  return (
    <div ref={ref} className="flex h-full flex-col">
      <UpdateBar />
      <Notice />
      <main className="min-h-0 flex-1">
        {ready && (!named ? <Setup /> : paired ? <Chat /> : <Pair />)}
      </main>
      {pairing && <PairDialog prompt={pairing} />}
    </div>
  )
}

/** A nudge, sent or received, shakes the whole window for a moment. */
function useShakeOnNudge(ref: React.RefObject<HTMLDivElement | null>) {
  const nudges = useSelector(store, (s) => s.nudges)
  useEffect(() => {
    if (nudges === 0 || matchMedia('(prefers-reduced-motion: reduce)').matches) return
    const offsets = [0, -8, 8, -6, 6, -3, 3, 0]
    ref.current?.animate(
      offsets.map((x) => ({ transform: `translateX(${x}px)` })),
      { duration: 450, easing: 'ease-in-out' },
    )
  }, [nudges, ref])
}
