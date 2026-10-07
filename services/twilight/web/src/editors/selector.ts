import type { Completion, CompletionContext, CompletionResult } from '@codemirror/autocomplete'
import { autocompletion } from '@codemirror/autocomplete'
import type { StringStream } from '@codemirror/language'
import { StreamLanguage } from '@codemirror/language'
import type { Diagnostic } from '@codemirror/lint'
import type { SelectorError } from '../api/types'
import {
  codePointOffsetToIndex,
  commonFactKeys,
  selectorFields,
  selectorKeywords,
} from '../selector/syntax'

const fieldNames = new Set(selectorFields.map((field) => field.name))
const keywords = new Set<string>(selectorKeywords)

function token(stream: StringStream): string | null {
  if (stream.eatSpace()) {
    return null
  }
  if (stream.match(/^"(?:[^"\\]|\\.)*"?/)) {
    return 'string'
  }
  if (stream.match(/^-?\d+(?:\.\d+)?(?:[eE][+-]?\d+)?/)) {
    return 'number'
  }
  if (stream.match(/^(?:==|!=|<=|>=|<|>)/)) {
    return 'operator'
  }
  if (stream.match(/^[()[\],]/)) {
    return 'bracket'
  }
  const word = stream.match(/^[A-Za-z_][A-Za-z0-9_]*/)
  if (typeof word === 'object' && word !== null) {
    const text = word[0]
    if (text === 'true' || text === 'false') {
      return 'bool'
    }
    if (text === 'facts') {
      return 'propertyName'
    }
    if (keywords.has(text)) {
      return 'keyword'
    }
    return fieldNames.has(text) ? 'variableName' : 'invalid'
  }
  stream.next()
  return 'invalid'
}

export const selectorLanguage = StreamLanguage.define<null>({
  name: 'selector',
  startState: () => null,
  token,
})

const fieldCompletions: Completion[] = selectorFields.map((field) => ({
  label: field.name,
  type: 'variable',
  detail: field.versioned === true ? 'semver' : 'field',
  info: field.description,
}))

const keywordCompletions: Completion[] = [
  { label: 'and', type: 'keyword' },
  { label: 'or', type: 'keyword' },
  { label: 'not', type: 'keyword' },
  { label: 'in', type: 'keyword', info: 'Membership in a list: os_name in ["debian", "ubuntu"]' },
  {
    label: 'not in',
    type: 'keyword',
    info: 'Absence from a list; false when the value is missing',
  },
  {
    label: 'has',
    type: 'function',
    apply: 'has(facts[""])',
    info: 'True when the node reports the fact or the field is set',
  },
  {
    label: 'semver',
    type: 'function',
    apply: 'semver()',
    info: 'Compare a field or fact as a semantic version',
  },
  {
    label: 'facts',
    type: 'property',
    apply: 'facts[""]',
    info: 'Any fact a node reports, for example facts["dusk.device.memory_bytes"]',
  },
]

function selectorCompletions(context: CompletionContext): CompletionResult | null {
  const factKey = context.matchBefore(/facts\[\s*"[^"\]]*/)
  if (factKey !== null) {
    const quote = factKey.text.indexOf('"')
    return {
      from: factKey.from + quote + 1,
      options: commonFactKeys.map((key) => ({ label: key, type: 'property' })),
      validFor: /^[^"\]]*$/,
    }
  }
  const word = context.matchBefore(/[A-Za-z_][A-Za-z0-9_]*/)
  if (word === null && !context.explicit) {
    return null
  }
  const before = context.state.sliceDoc(0, word?.from ?? context.pos)
  if ((before.match(/"/g)?.length ?? 0) % 2 === 1) {
    return null
  }
  return {
    from: word?.from ?? context.pos,
    options: [...fieldCompletions, ...keywordCompletions],
    validFor: /^[A-Za-z_][A-Za-z0-9_]*$/,
  }
}

export const selectorExtension = [
  selectorLanguage,
  autocompletion({ override: [selectorCompletions], icons: false }),
]

export function selectorDiagnostics(source: string, error: SelectorError | null): Diagnostic[] {
  if (error === null) {
    return []
  }
  const from = codePointOffsetToIndex(source, error.position)
  const end = error.end === undefined ? error.position + 1 : Math.max(error.end, error.position + 1)
  let to = codePointOffsetToIndex(source, end)
  let start = from
  if (start >= source.length && source.length > 0) {
    start = source.length - 1
    to = source.length
  }
  return [{ from: start, to: Math.max(to, start), severity: 'error', message: error.message }]
}
