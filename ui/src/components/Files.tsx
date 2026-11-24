// Files in the conversation: pictures in bubbles, a card for anything that can't be shown,
// the full-size viewer, and the pictures waiting in the message box.

import { Dialog } from '@base-ui/react/dialog'
import { useSelector } from '@tanstack/react-store'
import { DownloadIcon, ExternalLinkIcon, FileIcon, FolderOpenIcon, XIcon } from 'lucide-react'
import { useState } from 'react'

import { api, type Attachment, type FileInfo } from '@/api'
import { Button } from '@/components/ui/button'
import { fitWithin, formatBytes } from '@/lib/files'
import { run, setViewing, store, unstageFile } from '@/store'

/** Largest a picture shows in the transcript. */
const PICTURE_MAX = { width: 260, height: 300 }

/** Kinds of file that open in their default app (the same list as Rust's). */
const OPENABLE = /\.(jpe?g|png|gif|webp|avif|bmp|heic|heif|tiff?)$/i

const onMac = navigator.userAgent.includes('Mac')
const SHOW_IN_FOLDER = onMac ? 'Show in Finder' : 'Show in folder'
const SAVE_A_COPY = 'Save a copy…'

/** A picture, or a file's name: for quotes, the reply bar, and notifications. */
export const describeFile = (file: FileInfo) => (file.width ? 'Picture' : file.name)

/** A message's file: the picture itself, or a card for any other file. */
export function FileView({ file, peer }: { file: Attachment; peer: string }) {
  // The other side can't always show what the sender could, like HEIC on Windows.
  const [broken, setBroken] = useState(false)
  if (file.width && file.height && !broken) {
    return (
      <Picture
        file={file}
        width={file.width}
        height={file.height}
        peer={peer}
        onBroken={() => setBroken(true)}
      />
    )
  }
  return <FileCard file={file} peer={peer} />
}

function Picture({
  file,
  width,
  height,
  peer,
  onBroken,
}: {
  file: Attachment
  width: number
  height: number
  peer: string
  onBroken: () => void
}) {
  const box = fitWithin(width, height, PICTURE_MAX.width, PICTURE_MAX.height)
  const done = file.state === 'done'
  return (
    <button
      type="button"
      className="relative block max-w-full overflow-hidden rounded-lg bg-current/10"
      // The size is known before it loads, so the transcript doesn't jump.
      style={{ width: box.width, aspectRatio: `${width} / ${height}` }}
      disabled={!done}
      aria-label={done ? `Open ${file.name}` : file.name}
      onClick={() => setViewing(file)}
    >
      {done ? (
        <img
          src={api.fileUrl(file.hash)}
          alt={file.name}
          className="size-full object-cover"
          decoding="async"
          draggable={false}
          onError={onBroken}
        />
      ) : (
        file.preview && (
          <img src={file.preview} alt="" className="size-full scale-110 object-cover blur-md" />
        )
      )}
      <span className="absolute inset-x-0 bottom-1.5 flex justify-center">
        <TransferNote file={file} peer={peer} pill />
      </span>
    </button>
  )
}

/** A file that isn't shown as a picture: name, size, and what can be done with it. */
function FileCard({ file, peer }: { file: Attachment; peer: string }) {
  const done = file.state === 'done'
  return (
    <div className="flex max-w-full min-w-52 items-center gap-2 rounded-lg bg-current/10 py-1.5 pr-1 pl-2">
      <FileIcon className="size-7 shrink-0 opacity-80" />
      <div className="min-w-0 flex-1">
        <div className="truncate font-medium">{file.name}</div>
        <div className="text-xs opacity-80">
          {formatBytes(file.size)}
          <TransferNote file={file} peer={peer} />
        </div>
      </div>
      {done && OPENABLE.test(file.name) && (
        <Button
          variant="ghost"
          size="icon-sm"
          aria-label={`Open ${file.name}`}
          title="Open"
          onClick={() => void run(api.openFile(file.hash))}
        >
          <ExternalLinkIcon />
        </Button>
      )}
      {done && (
        <Button
          variant="ghost"
          size="icon-sm"
          aria-label={SAVE_A_COPY}
          title={SAVE_A_COPY}
          onClick={() => void run(api.saveFileCopy(file.hash))}
        >
          <DownloadIcon />
        </Button>
      )}
      {done && (
        <Button
          variant="ghost"
          size="icon-sm"
          aria-label={SHOW_IN_FOLDER}
          title={SHOW_IN_FOLDER}
          onClick={() => void run(api.showFile(file.hash))}
        >
          <FolderOpenIcon />
        </Button>
      )}
    </div>
  )
}

/**
 * How a file's transfer is going: a percentage while it moves, or why it hasn't come.
 * Nothing once it's here and not being sent. Reads progress from the store itself, so
 * progress never re-renders the message rows.
 */
function TransferNote({ file, peer, pill }: { file: Attachment; peer: string; pill?: boolean }) {
  const progress = useSelector(store, (s) => s.transfers[file.hash])
  let note: string | null = null
  if (progress !== undefined) {
    note = `${file.state === 'done' ? 'Sending' : 'Receiving'} ${Math.floor(progress * 100)}%`
  } else if (file.state === 'pending') {
    note = `Waiting for ${peer}`
  } else if (file.state === 'gone') {
    note = 'No longer available'
  }
  if (note === null) return null
  return pill ? (
    <span className="rounded-full bg-black/60 px-2 py-0.5 text-[11px] text-white tabular-nums">
      {note}
    </span>
  ) : (
    <span className="tabular-nums"> · {note}</span>
  )
}

/** The picture chosen in the transcript, as big as the window allows. */
export function Viewer() {
  const viewing = useSelector(store, (s) => s.viewing)
  return (
    <Dialog.Root open={!!viewing} onOpenChange={(open) => !open && setViewing(null)}>
      <Dialog.Portal>
        <Dialog.Backdrop className="fixed inset-0 z-50 bg-black/85 duration-100 data-open:animate-in data-open:fade-in-0 data-closed:animate-out data-closed:fade-out-0" />
        <Dialog.Popup
          className="fixed inset-0 z-50 flex flex-col items-center justify-center gap-3 p-4 outline-none"
          // Clicking around the picture closes it.
          onClick={(e) => e.target === e.currentTarget && setViewing(null)}
        >
          {viewing && (
            <>
              <Dialog.Title className="sr-only">{viewing.name}</Dialog.Title>
              <img
                src={api.fileUrl(viewing.hash)}
                alt={viewing.name}
                className="min-h-0 max-w-full rounded-md object-contain"
              />
              <div className="flex max-w-full flex-wrap items-center justify-center gap-2 text-sm text-white">
                <span className="w-full truncate text-center">
                  {viewing.name} · {formatBytes(viewing.size)}
                </span>
                {OPENABLE.test(viewing.name) && (
                  <Button
                    variant="secondary"
                    size="sm"
                    onClick={() => void run(api.openFile(viewing.hash))}
                  >
                    <ExternalLinkIcon />
                    Open
                  </Button>
                )}
                <Button
                  variant="secondary"
                  size="sm"
                  onClick={() => void run(api.saveFileCopy(viewing.hash))}
                >
                  <DownloadIcon />
                  {SAVE_A_COPY}
                </Button>
                <Button
                  variant="secondary"
                  size="sm"
                  onClick={() => void run(api.showFile(viewing.hash))}
                >
                  <FolderOpenIcon />
                  {SHOW_IN_FOLDER}
                </Button>
                <Dialog.Close
                  render={<Button variant="secondary" size="icon-sm" aria-label="Close" />}
                >
                  <XIcon />
                </Dialog.Close>
              </div>
            </>
          )}
        </Dialog.Popup>
      </Dialog.Portal>
    </Dialog.Root>
  )
}

/** The pictures waiting in the message box, each with a button to take it out. */
export function StagedFiles({ sending }: { sending: boolean }) {
  const staged = useSelector(store, (s) => s.staged)
  if (staged.length === 0) return null
  return (
    <ul className="mb-2 flex gap-2 overflow-x-auto pb-1" aria-label="Pictures to send">
      {staged.map((s) => (
        <li
          key={s.id}
          className="relative shrink-0"
          title={`${s.file.name} · ${formatBytes(s.file.size)}`}
        >
          {s.url ? (
            <img src={s.url} alt={s.file.name} className="size-16 rounded-md object-cover" />
          ) : (
            <div className="flex size-16 flex-col items-center justify-center gap-1 rounded-md bg-muted p-1 text-[10px] text-muted-foreground">
              <FileIcon className="size-5" />
              <span className="w-full truncate text-center">{s.file.name}</span>
            </div>
          )}
          {!sending && (
            <Button
              type="button"
              variant="secondary"
              size="icon-xs"
              // Inside the corner: the scrolling list would clip it outside.
              className="absolute top-1 right-1 rounded-full shadow"
              aria-label={`Remove ${s.file.name}`}
              onClick={() => unstageFile(s.id)}
            >
              <XIcon />
            </Button>
          )}
        </li>
      ))}
    </ul>
  )
}
