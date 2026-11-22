import { Chat } from './components/Chat'
import { Notice } from './components/Notice'
import { Pair } from './components/Pair'
import { PairDialog } from './components/PairDialog'
import { Setup } from './components/Setup'
import { status } from './store'

export function App() {
  const s = status.value
  return (
    <>
      <Notice />
      {s && (!s.me.name ? <Setup /> : s.peer ? <Chat /> : <Pair />)}
      {s?.pairing && <PairDialog prompt={s.pairing} />}
    </>
  )
}
