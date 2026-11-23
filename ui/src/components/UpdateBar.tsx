import { useSelector } from '@tanstack/react-store'
import { useState } from 'react'

import { api } from '@/api'
import { ExternalLink } from '@/components/ExternalLink'
import { Button } from '@/components/ui/button'
import { Spinner } from '@/components/ui/spinner'
import { run, store } from '@/store'

const RELEASES = 'https://github.com/nerdstep/tightbeam/releases/tag/v'

/** A downloaded update, above every screen until the person restarts into it. */
export function UpdateBar() {
  const update = useSelector(store, (s) => s.update)
  const [restarting, setRestarting] = useState(false)
  if (!update) return null

  const restart = async () => {
    setRestarting(true)
    // On success the app quits and comes back as the new version.
    const ok = await run(api.installUpdate().then(() => true))
    if (!ok) setRestarting(false)
  }

  return (
    <output className="flex items-center gap-3 bg-primary px-4 py-2 text-sm text-primary-foreground">
      <span className="flex-1">
        Tightbeam {update.version} is ready ·{' '}
        <ExternalLink href={RELEASES + update.version}>What’s new</ExternalLink>
      </span>
      <Button size="sm" variant="secondary" disabled={restarting} onClick={() => void restart()}>
        {restarting && <Spinner />}
        {restarting ? 'Restarting…' : 'Restart'}
      </Button>
    </output>
  )
}
