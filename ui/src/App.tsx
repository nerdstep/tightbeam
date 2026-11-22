import { shallow, useSelector } from '@tanstack/react-store'

import { Chat } from './components/Chat'
import { Notice } from './components/Notice'
import { Pair } from './components/Pair'
import { PairDialog } from './components/PairDialog'
import { Setup } from './components/Setup'
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
    <>
      <Notice />
      {ready && (!named ? <Setup /> : paired ? <Chat /> : <Pair />)}
      {pairing && <PairDialog prompt={pairing} />}
    </>
  )
}
