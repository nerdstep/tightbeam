// Slash commands: typing `/name` at the start of the message box runs a command instead
// of sending. `//` sends a message that starts with a slash.

import { api } from './api'
import { openSearch, run, shake } from './store'

/** What a command can ask of the message box. */
export interface CommandContext {
  /** Opens the "Clear this conversation?" dialog. */
  confirmClear: () => void
}

export interface Command {
  name: string
  /** Shown after the name in the list, like `[note]`. */
  args?: string
  description: string
  /** `args` is whatever follows the name, trimmed. */
  action: (ctx: CommandContext, args: string) => void | Promise<void>
}

export const commands: Command[] = [
  {
    name: 'clear',
    description: 'Erase this conversation on this computer',
    action: (ctx) => ctx.confirmClear(),
  },
  {
    name: 'search',
    args: '[words]',
    description: 'Search this conversation',
    action: (_, words) => openSearch(words),
  },
  {
    name: 'away',
    args: '[note]',
    description: 'Show as away until /back or your next message',
    action: (_, note) => void run(api.setAway(note)),
  },
  {
    name: 'back',
    description: 'Stop showing as away',
    action: () => void run(api.back()),
  },
  {
    name: 'nudge',
    description: 'Shake their window and play a sound',
    action: async () => {
      if (await run(api.nudge().then(() => true))) shake(false)
    },
  },
]

export type Parsed =
  | { kind: 'text'; text: string }
  /** `command` is undefined when no command has that name. */
  | { kind: 'command'; name: string; args: string; command: Command | undefined }

/**
 * Reads the message box. A command is a slash and a word at the very start, like
 * `/nudge`. Anything else is a message, so `/usr/bin` or `/ hi` send as typed.
 */
export function parse(text: string): Parsed {
  const value = text.trim()
  if (value.startsWith('//')) return { kind: 'text', text: value.slice(1) }
  const match = /^\/([a-z]+)(?:\s|$)/i.exec(value)
  if (!match) return { kind: 'text', text: value }
  const name = match[1].toLowerCase()
  const args = value.slice(match[0].length).trim()
  return { kind: 'command', name, args, command: commands.find((c) => c.name === name) }
}

/** The commands to suggest while a command's name is being typed: `/`, `/nu`, … */
export function suggest(text: string): Command[] {
  const match = /^\/([a-z]*)$/i.exec(text)
  if (!match) return []
  const typed = match[1].toLowerCase()
  return commands.filter((c) => c.name.startsWith(typed))
}
