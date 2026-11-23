import { api } from '@/api'
import { run } from '@/store'

/**
 * Web and email links: http(s):// or mailto:, up to whitespace, not ending in
 * punctuation that's more likely part of the sentence ("see https://x.com.").
 */
const LINK = /\b(?:https?:\/\/|mailto:)[^\s<>"]*[^\s<>".,;:!?'")\]}]/gi

/** Message text with links made clickable. They open in the default browser or mail app. */
export function Linkified({ text }: { text: string }) {
  const parts: React.ReactNode[] = []
  let last = 0
  for (const match of text.matchAll(LINK)) {
    const url = match[0]
    const start = match.index
    if (start > last) parts.push(text.slice(last, start))
    parts.push(
      <a
        key={start}
        href={url}
        className="underline underline-offset-2 wrap-anywhere hover:opacity-80"
        onClick={(e) => {
          // Never navigate the app's own window.
          e.preventDefault()
          void run(api.openLink(url))
        }}
      >
        {url}
      </a>,
    )
    last = start + url.length
  }
  if (last < text.length) parts.push(text.slice(last))
  return parts
}
