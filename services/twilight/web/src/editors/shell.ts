import type { Completion, CompletionContext, CompletionResult } from '@codemirror/autocomplete'
import { autocompletion } from '@codemirror/autocomplete'
import type { StringStream } from '@codemirror/language'
import { StreamLanguage } from '@codemirror/language'

export const shellPrograms: readonly { name: string; description: string }[] = [
  { name: 'cp', description: 'copy a file between the client and the node' },
  { name: 'date', description: 'show or set the current time' },
  { name: 'echo', description: 'echo text back' },
  { name: 'false', description: 'do nothing, unsuccessfully' },
  { name: 'hostname', description: 'show the hostname' },
  { name: 'kill', description: 'send a signal to a process' },
  { name: 'kvs', description: 'key-value store' },
  { name: 'logs', description: 'access logs' },
  { name: 'programs', description: 'list available programs' },
  { name: 'ps', description: 'list processes' },
  { name: 'sh', description: 'run Dusk shell commands' },
  { name: 'sleep', description: 'sleep for a duration' },
  { name: 'true', description: 'do nothing successfully' },
]

const programNames = new Set(shellPrograms.map((program) => program.name))

interface ShellState {
  commandStart: boolean
}

function word(stream: StringStream): string {
  let text = ''
  while (!stream.eol()) {
    const next = stream.peek()
    if (next === undefined || /[\s;&|(){}]/.test(next)) {
      break
    }
    text += stream.next() ?? ''
  }
  return text
}

function token(stream: StringStream, state: ShellState): string | null {
  if (stream.sol()) {
    state.commandStart = true
  }
  if (stream.eatSpace()) {
    return null
  }
  if (stream.peek() === '#') {
    stream.skipToEnd()
    return 'comment'
  }
  if (stream.match('&&') || stream.match('||')) {
    state.commandStart = true
    return 'operator'
  }
  const character = stream.peek()
  if (character === ';' || character === '{' || character === '}') {
    stream.next()
    state.commandStart = true
    return character === ';' ? 'operator' : 'bracket'
  }
  if (character === '(' || character === ')') {
    stream.next()
    return 'bracket'
  }
  if (character === '"' || character === "'") {
    stream.next()
    while (!stream.eol()) {
      if (stream.next() === character) {
        break
      }
    }
    state.commandStart = false
    return 'string'
  }
  const commandPosition = state.commandStart
  const text = word(stream)
  if (text.length === 0) {
    stream.next()
    return null
  }
  state.commandStart = false
  if (commandPosition) {
    return programNames.has(text) ? 'variableName.function' : 'variableName'
  }
  if (text.startsWith('-')) {
    return /^-?\d+(\.\d+)?$/.test(text) ? 'number' : 'attributeName'
  }
  if (text.startsWith(':')) {
    return 'string.special'
  }
  if (/^\d+(\.\d+)?$/.test(text)) {
    return 'number'
  }
  return null
}

export const shellLanguage = StreamLanguage.define<ShellState>({
  name: 'dusk-shell',
  startState: () => ({ commandStart: true }),
  copyState: (state) => ({ ...state }),
  token,
  languageData: { commentTokens: { line: '#' } },
})

const programCompletions: Completion[] = shellPrograms.map((program) => ({
  label: program.name,
  type: 'function',
  detail: program.description,
}))

function shellCompletions(context: CompletionContext): CompletionResult | null {
  const typed = context.matchBefore(/[A-Za-z_]*/)
  if (typed === null) {
    return null
  }
  const before = context.state.sliceDoc(context.state.doc.lineAt(context.pos).from, typed.from)
  const atCommand = /(^|[;&|{]\s*)\s*$/.test(before)
  if (!atCommand || (typed.from === typed.to && !context.explicit)) {
    return null
  }
  return { from: typed.from, options: programCompletions, validFor: /^[A-Za-z_]*$/ }
}

export const shellExtension = [
  shellLanguage,
  autocompletion({ override: [shellCompletions], icons: false }),
]
