// Slash commands: typing `/name` at the start of the message box runs a command instead
// of sending. `//` sends a message that starts with a slash.

import { api } from './api'
import { run, shake } from './store'

/** What a command can ask of the message box. */
export interface CommandContext {
  /** Opens the "Clear this conversation?" dialog. */
  confirmClear: () => void
}

export interface Command {
  name: string
  description: string
  action: (ctx: CommandContext) => void | Promise<void>
}

export const commands: Command[] = [
  {
    name: 'clear',
    description: 'Erase this conversation on this computer',
    action: (ctx) => ctx.confirmClear(),
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
  | { kind: 'command'; name: string; command: Command | undefined }

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
  return { kind: 'command', name, command: commands.find((c) => c.name === name) }
}

/** The commands to suggest while a command's name is being typed: `/`, `/nu`, … */
export function suggest(text: string): Command[] {
  const match = /^\/([a-z]*)$/i.exec(text)
  if (!match) return []
  const typed = match[1].toLowerCase()
  return commands.filter((c) => c.name.startsWith(typed))
}
