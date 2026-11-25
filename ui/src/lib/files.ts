// Getting files ready to send: their details, and for a picture its size and a tiny
// preview the other side shows until the file arrives.
//
// Files from disk (dropped or picked) are staged in Rust and go by path, so a big one is
// never read into the webview. Pasted files have no path, so they go as bytes.

import { api, type NewFile, type StagedFile } from '@/api'

/** The largest file that can be sent: 1 GB, as in Rust. */
export const MAX_FILE_BYTES = 1024 ** 3

/** Pasted files pass through the webview's memory, so they're kept smaller. */
export const MAX_PASTED_BYTES = 100 * 1000 * 1000

/** Pictures from disk get a preview when they're at most this big (as in Rust). */
const MAX_PREVIEW_SOURCE = 50 * 1024 * 1024

/** The preview's longest side, in pixels. It's shown blurred, so tiny is fine. */
const PREVIEW_PX = 24

/** A file chosen to send, waiting in the message box. */
export interface Staged {
  /** Tells staged files apart. */
  id: number
  name: string
  size: number
  /** Where its bytes come from: pasted, or a file on disk Rust staged under that id. */
  source: { pasted: File } | { disk: number }
  details: NewFile
  /** For the thumbnail in the message box. `null` if it isn't a picture this webview shows. */
  url: string | null
}

let staged = 0

/** Pictures the webview can try to show. SVG can carry scripts: always a plain file. */
const showable = (mime: string) => mime.startsWith('image/') && mime !== 'image/svg+xml'

/** A pasted file, such as a screenshot. */
export async function stagePasted(file: File): Promise<Staged> {
  const name = file.name || 'pasted file'
  const details = plain(name, file.type || 'application/octet-stream')
  const url = showable(details.mime) ? await describePicture(file, details) : null
  return { id: ++staged, name, size: file.size, source: { pasted: file }, details, url }
}

/** A file from disk that Rust has staged. */
export async function stageFromDisk(file: StagedFile): Promise<Staged> {
  const details = plain(file.name, file.mime)
  let url: string | null = null
  if (showable(file.mime) && file.size <= MAX_PREVIEW_SOURCE) {
    const picture = await api
      .stagedPicture(file.id)
      .then((bytes) => new Blob([bytes], { type: file.mime }))
      .catch(() => null)
    if (picture) url = await describePicture(picture, details)
  }
  const { name, size } = file
  return { id: ++staged, name, size, source: { disk: file.id }, details, url }
}

/** Frees a staged file's thumbnail, and its place in Rust. */
export function unstage(s: Staged) {
  if (s.url) URL.revokeObjectURL(s.url)
  if ('disk' in s.source) void api.unstage(s.source.disk).catch(() => {})
}

const plain = (name: string, mime: string): NewFile => ({
  name,
  mime,
  width: null,
  height: null,
  preview: null,
})

/**
 * Reads a picture's size and makes its preview, into `details`, and returns a thumbnail
 * URL. One this webview can't open (HEIC on Windows, say) is still sent, without them;
 * the other side shows it as a file.
 */
async function describePicture(picture: Blob, details: NewFile): Promise<string | null> {
  try {
    const bitmap = await createImageBitmap(picture)
    details.width = bitmap.width
    details.height = bitmap.height
    details.preview = preview(bitmap)
    bitmap.close()
    return URL.createObjectURL(picture)
  } catch {
    return null // not something this webview can show
  }
}

function preview(bitmap: ImageBitmap): string | null {
  const scale = PREVIEW_PX / Math.max(bitmap.width, bitmap.height)
  const canvas = document.createElement('canvas')
  canvas.width = Math.max(1, Math.round(bitmap.width * scale))
  canvas.height = Math.max(1, Math.round(bitmap.height * scale))
  const context = canvas.getContext('2d')
  if (!context) return null
  context.drawImage(bitmap, 0, 0, canvas.width, canvas.height)
  return canvas.toDataURL('image/jpeg', 0.7)
}

/** "340 KB", "1.2 MB". */
export function formatBytes(bytes: number) {
  if (bytes < 1000) return `${bytes} bytes`
  const units = ['KB', 'MB', 'GB']
  let value = bytes / 1000
  let unit = 0
  while (value >= 1000 && unit < units.length - 1) {
    value /= 1000
    unit++
  }
  return `${value.toFixed(value < 10 ? 1 : 0)} ${units[unit]}`
}

/** The size to show a picture at: at most `maxWidth` by `maxHeight`, never enlarged. */
export function fitWithin(width: number, height: number, maxWidth: number, maxHeight: number) {
  const scale = Math.min(1, maxWidth / width, maxHeight / height)
  return { width: Math.round(width * scale), height: Math.round(height * scale) }
}
