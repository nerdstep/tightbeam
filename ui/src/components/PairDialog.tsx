import { api, type PairPrompt } from '@/api'
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from '@/components/ui/alert-dialog'
import { run } from '@/store'

/** Pairing in progress, either direction. Both sides show the same code. Esc declines. */
export function PairDialog({ prompt }: { prompt: PairPrompt }) {
  const who = prompt.name || 'Someone'

  return (
    <AlertDialog
      open
      onOpenChange={(open) => {
        // Cancel, Decline and Esc all close the dialog.
        if (!open) void run(api.answerPair(false))
      }}
    >
      <AlertDialogContent size="sm">
        <AlertDialogHeader>
          <AlertDialogTitle>
            {prompt.incoming ? `${who} wants to pair` : `Waiting for ${who}`}
          </AlertDialogTitle>
          <AlertDialogDescription>
            {prompt.incoming
              ? 'Only accept if their screen shows the same code.'
              : 'Check that their screen shows this code, then they’ll accept.'}
          </AlertDialogDescription>
        </AlertDialogHeader>
        <div className="py-2 text-center font-heading text-4xl font-semibold tracking-[0.15em] tabular-nums">
          {prompt.code}
        </div>
        <AlertDialogFooter>
          <AlertDialogCancel>{prompt.incoming ? 'Decline' : 'Cancel'}</AlertDialogCancel>
          {prompt.incoming && (
            <AlertDialogAction autoFocus onClick={() => void run(api.answerPair(true))}>
              Pair
            </AlertDialogAction>
          )}
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  )
}
