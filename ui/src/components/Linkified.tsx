import { ExternalLink } from '@/components/ExternalLink'

/**
 * Web and email links: http(s):// or mailto:, up to whitespace, not ending in
 * punctuation that's more likely part of the sentence ("see https://x.com.").
 */
const LINK = /\b(?:https?:\/\/|mailto:)[^\s<>"]*[^\s<>".,;:!?'")\]}]/gi

/** Message text with links made clickable. */
export function Linkified({ text }: { text: string }) {
  const parts: React.ReactNode[] = []
  let last = 0
  for (const match of text.matchAll(LINK)) {
    const url = match[0]
    const start = match.index
    if (start > last) parts.push(text.slice(last, start))
    parts.push(
      <ExternalLink key={start} href={url}>
        {url}
      </ExternalLink>,
    )
    last = start + url.length
  }
  if (last < text.length) parts.push(text.slice(last))
  return parts
}
