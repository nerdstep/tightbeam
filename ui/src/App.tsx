import { shallow, useSelector } from '@tanstack/react-store'

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

  return (
    <div className="flex h-full flex-col">
      <UpdateBar />
      <Notice />
      <main className="min-h-0 flex-1">
        {ready && (!named ? <Setup /> : paired ? <Chat /> : <Pair />)}
      </main>
      {pairing && <PairDialog prompt={pairing} />}
    </div>
  )
}
