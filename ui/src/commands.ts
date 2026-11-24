// Slash commands: typing `/name` at the start of the message box runs a command instead
// of sending. `//` sends a message that starts with a slash.

import { api } from './api'
import { parseDuration } from './lib/duration'
import { openSearch, run, shake } from './store'

/** What a command can ask of the message box. */
export interface CommandContext {
  /** Opens the "Clear this conversation?" dialog. */
  confirmClear: () => void
  /** Sends a message, as if typed (replying, if a reply was started). */
  send: (text: string, vanish?: number) => Promise<void>
  /** Explains, above the message box, what the command needs. */
  hint: (text: string) => void
  /** Opens the connection details. */
  showStatus: () => void
}

/** The shortest and longest a vanishing message lasts, and do not disturb. */
const VANISH = { min: 5, max: 7 * 86_400 }
const DND_MAX = 7 * 86_400

/** Sends a text face after whatever was typed with it: `/shrug fine` → `fine ¯\_(ツ)_/¯`. */
const face =
  (art: string) =>
  (ctx: CommandContext, text: string): Promise<void> =>
    ctx.send(text ? `${text} ${art}` : art)

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
    name: 'me',
    args: '<action>',
    description: 'Say what you’re doing, like /me waves',
    action: (ctx, action) =>
      action ? ctx.send(`/me ${action}`) : ctx.hint('Add what you’re doing, like /me waves.'),
  },
  { name: 'shrug', args: '[message]', description: '¯\\_(ツ)_/¯', action: face('¯\\_(ツ)_/¯') },
  {
    name: 'tableflip',
    args: '[message]',
    description: '(╯°□°)╯︵ ┻━┻',
    action: face('(╯°□°)╯︵ ┻━┻'),
  },
  {
    name: 'unflip',
    args: '[message]',
    description: '┬─┬ノ( º _ºノ)',
    action: face('┬─┬ノ( º _ºノ)'),
  },
  {
    name: 'vanish',
    args: '<time> <message>',
    description: 'A message that erases itself once seen, like /vanish 1m …',
    action: (ctx, args) => {
      const match = /^(\S+)\s+([\s\S]+)$/.exec(args)
      const seconds = match ? parseDuration(match[1]) : undefined
      if (!match || seconds === undefined) {
        return ctx.hint(
          'Say how long it lasts once seen, then the message: /vanish 1m the code is 4321',
        )
      }
      if (seconds < VANISH.min || seconds > VANISH.max) {
        return ctx.hint('A vanishing message lasts 5 seconds to 7 days.')
      }
      return ctx.send(match[2], seconds)
    },
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
    name: 'dnd',
    args: '[time | off]',
    description: 'Do not disturb: no notifications, and nudges paused (1h)',
    action: (ctx, args) => {
      if (args.toLowerCase() === 'off') return void run(api.setDnd(null))
      const seconds = args ? parseDuration(args) : 3600
      if (seconds === undefined || seconds > DND_MAX) {
        return ctx.hint('Try /dnd 30m, /dnd 2h, or /dnd off. It lasts up to 7 days.')
      }
      void run(api.setDnd(Date.now() + seconds * 1000))
    },
  },
  {
    name: 'status',
    description: 'Connection details and versions',
    action: (ctx) => ctx.showStatus(),
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
