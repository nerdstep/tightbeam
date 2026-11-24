// `/status`: the connection and both versions, refreshed every second while open.

import { useSelector } from '@tanstack/react-store'
import { useEffect, useState } from 'react'

import { api, type ConnectionInfo } from '@/api'
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogContent,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from '@/components/ui/alert-dialog'
import { formatDuration } from '@/lib/duration'
import { store } from '@/store'

export function StatusDialog({
  open,
  onOpenChange,
}: {
  open: boolean
  onOpenChange: (open: boolean) => void
}) {
  const name = useSelector(store, (s) => s.status?.peer?.name || 'Them')
  const [info, setInfo] = useState<ConnectionInfo | null>(null)
  const [now, setNow] = useState(Date.now)

  useEffect(() => {
    if (!open) return undefined
    const load = () =>
      void api.connectionInfo().then(
        (loaded) => {
          setInfo(loaded)
          setNow(Date.now())
        },
        () => {},
      )
    load()
    const timer = setInterval(load, 1000)
    return () => clearInterval(timer)
  }, [open])

  const rows: [string, string][] = info
    ? [
        [
          'Connection',
          info.since === null
            ? 'Not connected'
            : `Connected for ${formatDuration((now - info.since) / 1000)}`,
        ],
        ...(info.since === null
          ? []
          : ([
              [`${name}’s address`, info.address ?? 'Unknown'],
              [
                'Round trip',
                info.rtt_ms === null
                  ? 'Unknown'
                  : `${info.rtt_ms.toFixed(info.rtt_ms < 10 ? 1 : 0)} ms`,
              ],
            ] satisfies [string, string][])),
        ['Your version', info.my_version],
        [`${name}’s version`, info.peer_version ?? '0.5 or earlier, or not connected yet'],
      ]
    : []

  return (
    <AlertDialog open={open} onOpenChange={onOpenChange}>
      <AlertDialogContent size="sm">
        <AlertDialogHeader>
          <AlertDialogTitle>Status</AlertDialogTitle>
        </AlertDialogHeader>
        <dl className="grid grid-cols-[auto_1fr] gap-x-4 gap-y-1.5 text-sm">
          {rows.map(([label, value]) => (
            <div key={label} className="contents">
              <dt className="text-muted-foreground">{label}</dt>
              <dd className="min-w-0 truncate text-right font-medium tabular-nums">{value}</dd>
            </div>
          ))}
        </dl>
        {/* One button: span the small dialog's two-column footer. */}
        <AlertDialogFooter className="group-data-[size=sm]/alert-dialog-content:grid-cols-1">
          {/* AlertDialogAction is a plain button: it doesn't close the dialog itself. */}
          <AlertDialogAction onClick={() => onOpenChange(false)}>Done</AlertDialogAction>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  )
}
