import { useSelector } from '@tanstack/react-store'
import { cn } from 'cn'
import { TriangleAlertIcon, XIcon } from 'lucide-react'

import { api } from '@/api'
import { Button } from '@/components/ui/button'
import { dismissError, run, store } from '@/store'

function dismiss() {
  if (store.state.error) dismissError()
  else void run(api.dismissNotice())
}

/**
 * A dismissible banner at the top of the window: a failed command, or a notice from the
 * engine. Problems with what's being written show above the message box instead.
 */
export function Notice() {
  const text = useSelector(store, (s) => s.error ?? s.status?.notice ?? null)
  if (!text) return null

  return (
    <NoteCard
      className="fixed top-3 left-1/2 z-50 w-[min(28rem,calc(100%-2rem))] -translate-x-1/2"
      onDismiss={dismiss}
    >
      {text}
    </NoteCard>
  )
}

/** Something to fix or know, with a button to put it away. */
export function NoteCard({
  className,
  onDismiss,
  children,
}: {
  className?: string
  onDismiss: () => void
  children: React.ReactNode
}) {
  return (
    <div
      role="alert"
      className={cn(
        'flex items-start gap-2.5 rounded-lg border border-l-4 border-warning/40 border-l-warning bg-popover py-2 pr-1.5 pl-3 text-sm text-popover-foreground shadow-lg',
        className,
      )}
    >
      <TriangleAlertIcon className="mt-1.5 size-4 shrink-0 text-warning" aria-hidden />
      <p className="flex-1 py-1 text-pretty">{children}</p>
      <Button
        type="button"
        variant="ghost"
        size="icon-sm"
        aria-label="Dismiss"
        title="Dismiss"
        onClick={onDismiss}
      >
        <XIcon />
      </Button>
    </div>
  )
}
