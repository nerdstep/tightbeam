// Durations typed in commands, like `/dnd 1h30m` or `/vanish 30s …`, and shown back.

const UNIT_SECONDS = { d: 86_400, h: 3600, m: 60, s: 1 } as const

/**
 * Seconds in a duration like `30s`, `5m`, `2h` or `1h30m`. Units are required, so a
 * bare number (`/vanish 5 apples`) stays text. `undefined` if it isn't one.
 */
export function parseDuration(text: string): number | undefined {
  const match = /^(?:(\d+)d)?(?:(\d+)h)?(?:(\d+)m)?(?:(\d+)s)?$/i.exec(text.trim())
  if (!match || !text.trim()) return undefined
  const [, d, h, m, s] = match
  const seconds =
    Number(d ?? 0) * UNIT_SECONDS.d +
    Number(h ?? 0) * UNIT_SECONDS.h +
    Number(m ?? 0) * UNIT_SECONDS.m +
    Number(s ?? 0)
  return seconds > 0 ? seconds : undefined
}

/** `90` → `1m 30s`, `7200` → `2h`. The two largest units. */
export function formatDuration(seconds: number) {
  const parts: string[] = []
  let rest = Math.round(seconds)
  for (const [unit, size] of Object.entries(UNIT_SECONDS)) {
    if (rest >= size || (unit === 's' && parts.length === 0)) {
      parts.push(`${Math.floor(rest / size)}${unit}`)
      rest %= size
    }
  }
  return parts.slice(0, 2).join(' ')
}

/** A time later today as `3:30 PM`, or on another day as `Thu 3:30 PM`. */
export function formatUntil(ms: number) {
  const when = new Date(ms)
  const time = when.toLocaleTimeString([], { hour: 'numeric', minute: '2-digit' })
  if (when.toDateString() === new Date().toDateString()) return time
  return `${when.toLocaleDateString([], { weekday: 'short' })} ${time}`
}
