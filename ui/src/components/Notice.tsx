import { useSelector } from '@tanstack/react-store'

import { api } from '@/api'
import { Alert, AlertAction, AlertDescription } from '@/components/ui/alert'
import { Button } from '@/components/ui/button'
import { dismissError, run, store } from '@/store'

function dismiss() {
  if (store.state.error) dismissError()
  else void run(api.dismissNotice())
}

/** A dismissible banner: a failed command, or a notice from the engine. */
export function Notice() {
  const text = useSelector(store, (s) => s.error ?? s.status?.notice ?? null)
  if (!text) return null

  return (
    <Alert className="fixed top-3 left-1/2 z-50 w-[min(28rem,calc(100%-2rem))] -translate-x-1/2 border-warning pr-24 shadow-lg">
      <AlertDescription className="text-foreground">{text}</AlertDescription>
      <AlertAction>
        <Button variant="ghost" size="sm" onClick={dismiss}>
          Dismiss
        </Button>
      </AlertAction>
    </Alert>
  )
}
