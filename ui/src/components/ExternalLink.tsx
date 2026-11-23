import { api } from '@/api'
import { run } from '@/store'

/** A link that opens in the default browser or mail app, never in the app's own window. */
export function ExternalLink({ href, children }: { href: string; children: React.ReactNode }) {
  return (
    <a
      href={href}
      className="underline underline-offset-2 wrap-anywhere hover:opacity-80"
      onClick={(e) => {
        e.preventDefault()
        void run(api.openLink(href))
      }}
    >
      {children}
    </a>
  )
}
