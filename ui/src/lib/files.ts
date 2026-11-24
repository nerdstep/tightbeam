// Getting pictures ready to send: their size, and a tiny preview the other side shows
// until the file arrives.

import type { NewFile } from '@/api'

/** The largest file that can be sent: 1 GB, as in Rust. */
export const MAX_FILE_BYTES = 1024 ** 3

/** The preview's longest side, in pixels. It's shown blurred, so tiny is fine. */
const PREVIEW_PX = 24

/** Picture extensions, for files whose type the system doesn't report (HEIC on Windows). */
const PICTURE_EXTENSIONS = /\.(jpe?g|png|gif|webp|avif|bmp|heic|heif|tiff?)$/i

/** A picture chosen to send, waiting in the message box. */
export interface Staged {
  /** Tells staged files apart. */
  id: number
  file: File
  details: NewFile
  /** For the thumbnail in the message box. `null` if this webview can't show it. */
  url: string | null
}

let staged = 0

/** Whether a file is a picture, by its type or its name. */
export const isPicture = (file: File) =>
  file.type.startsWith('image/') || PICTURE_EXTENSIONS.test(file.name)

/**
 * Reads a picture's size and makes its preview. One this webview can't open (HEIC on
 * Windows, say) is still sent, without them; the other side shows it as a file.
 */
export async function stage(file: File): Promise<Staged> {
  const details: NewFile = {
    name: file.name || 'picture',
    mime: file.type || 'application/octet-stream',
    width: null,
    height: null,
    preview: null,
  }
  // SVG can carry scripts: always a plain file.
  if (file.type !== 'image/svg+xml') {
    try {
      const bitmap = await createImageBitmap(file)
      details.width = bitmap.width
      details.height = bitmap.height
      details.preview = preview(bitmap)
      bitmap.close()
    } catch {
      // Not something this webview can show.
    }
  }
  const url = details.width === null ? null : URL.createObjectURL(file)
  return { id: ++staged, file, details, url }
}

/** Frees a staged file's thumbnail. */
export function unstage(s: Staged) {
  if (s.url) URL.revokeObjectURL(s.url)
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
