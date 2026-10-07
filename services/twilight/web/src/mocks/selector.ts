import type { JsonValue } from '../api/types'
import { selectorFields } from '../selector/syntax'

type TokenKind =
  | 'end'
  | 'identifier'
  | 'string'
  | 'number'
  | 'left_parenthesis'
  | 'right_parenthesis'
  | 'left_bracket'
  | 'right_bracket'
  | 'comma'
  | 'equal'
  | 'not_equal'
  | 'less'
  | 'less_or_equal'
  | 'greater'
  | 'greater_or_equal'

interface Token {
  kind: TokenKind
  text: string
  value: string
  position: number
  end: number
}

export interface SelectorError {
  position: number
  end: number
  message: string
}

class SelectorFailure extends Error {
  readonly detail: SelectorError

  constructor(position: number, end: number, message: string) {
    super(message)
    this.detail = { position, end, message }
  }
}

type Operator = '==' | '!=' | '<' | '<=' | '>' | '>=' | 'in' | 'not in'
type LiteralKind = 'string' | 'number' | 'bool'

interface Literal {
  kind: LiteralKind
  text: string
  bool: boolean
  position: number
  end: number
}

interface Field {
  kind: 'text' | 'version' | 'fact'
  column: string
  factKey: string
  position: number
  end: number
}

export type Expression =
  | { kind: 'all' }
  | { kind: 'and'; left: Expression; right: Expression }
  | { kind: 'or'; left: Expression; right: Expression }
  | { kind: 'not'; operand: Expression }
  | { kind: 'has'; field: Field }
  | { kind: 'compare'; field: Field; semver: boolean; operator: Operator; values: Literal[] }

const maximumLength = 16384
const maximumDepth = 64
const maximumListLength = 1024
const maximumComparisons = 1024

const columns = new Map(
  selectorFields.map(
    (field) => [field.name, field.versioned === true ? 'version' : 'text'] as const,
  ),
)

const keywords = new Set(['and', 'or', 'not', 'in', 'has', 'semver', 'facts', 'true', 'false'])

const singleCharacterTokens: Record<string, TokenKind> = {
  '(': 'left_parenthesis',
  ')': 'right_parenthesis',
  '[': 'left_bracket',
  ']': 'right_bracket',
  ',': 'comma',
}

const operatorTokens: readonly (readonly [string, TokenKind])[] = [
  ['==', 'equal'],
  ['!=', 'not_equal'],
  ['<=', 'less_or_equal'],
  ['>=', 'greater_or_equal'],
  ['<', 'less'],
  ['>', 'greater'],
]

const comparisonOperators: Partial<Record<TokenKind, Operator>> = {
  equal: '==',
  not_equal: '!=',
  less: '<',
  less_or_equal: '<=',
  greater: '>',
  greater_or_equal: '>=',
}

const literalNames: Record<LiteralKind, string> = {
  string: 'strings',
  number: 'numbers',
  bool: 'booleans',
}

function quote(text: string): string {
  return JSON.stringify(text)
}

function describe(kind: TokenKind): string {
  switch (kind) {
    case 'end':
      return 'the end of the selector'
    case 'identifier':
      return 'a name'
    case 'string':
      return 'a string'
    case 'number':
      return 'a number'
    case 'left_parenthesis':
      return '"("'
    case 'right_parenthesis':
      return '")"'
    case 'left_bracket':
      return '"["'
    case 'right_bracket':
      return '"]"'
    case 'comma':
      return '","'
    default:
      return quote(operatorTokens.find(([, candidate]) => candidate === kind)?.[0] ?? '')
  }
}

function isIdentifierStart(character: string): boolean {
  return /^[A-Za-z_]$/.test(character)
}

function isIdentifierPart(character: string): boolean {
  return /^[A-Za-z0-9_]$/.test(character)
}

function isDigit(character: string | undefined): boolean {
  return character !== undefined && character >= '0' && character <= '9'
}

function tokenize(source: string): Token[] {
  const characters = Array.from(source)
  const tokens: Token[] = []
  let offset = 0
  const slice = (start: number, end: number) => characters.slice(start, end).join('')
  for (;;) {
    while (offset < characters.length && /^[ \t\n\r]$/.test(characters[offset] ?? '')) {
      offset += 1
    }
    const start = offset
    if (offset >= characters.length) {
      tokens.push({ kind: 'end', text: '', value: '', position: start, end: start })
      return tokens
    }
    const character = characters[offset] ?? ''
    const single = singleCharacterTokens[character]
    if (single !== undefined) {
      offset += 1
      tokens.push({ kind: single, text: character, value: '', position: start, end: start + 1 })
      continue
    }
    const operator = operatorTokens.find(([text]) => slice(offset, offset + text.length) === text)
    if (operator !== undefined) {
      offset += operator[0].length
      tokens.push({
        kind: operator[1],
        text: operator[0],
        value: '',
        position: start,
        end: start + operator[0].length,
      })
      continue
    }
    if (character === '"') {
      let length = 1
      for (;;) {
        if (offset + length >= characters.length) {
          throw new SelectorFailure(start, characters.length, 'this string is not closed')
        }
        const next = characters[offset + length]
        if (next === '\\') {
          length += 2
          continue
        }
        length += 1
        if (next === '"') {
          break
        }
      }
      const text = slice(offset, offset + length)
      let value: string
      try {
        value = JSON.parse(text) as string
      } catch {
        throw new SelectorFailure(
          start,
          start + length,
          'this string has an invalid escape or control character',
        )
      }
      if (value.includes('\u0000')) {
        throw new SelectorFailure(start, start + length, 'strings cannot contain U+0000')
      }
      offset += length
      tokens.push({ kind: 'string', text, value, position: start, end: start + length })
      continue
    }
    if (character === '-' || isDigit(character)) {
      let length = 0
      const at = (position: number) => characters[offset + position]
      const digits = () => {
        let count = 0
        while (isDigit(at(length))) {
          length += 1
          count += 1
        }
        return count
      }
      const invalid = (): never => {
        while (offset + length < characters.length && /^[A-Za-z0-9_.+-]$/.test(at(length) ?? '')) {
          length += 1
        }
        throw new SelectorFailure(
          start,
          start + length,
          `${quote(slice(offset, offset + length))} is not a number`,
        )
      }
      if (at(length) === '-') {
        length += 1
      }
      const integerStart = length
      if (digits() === 0) {
        invalid()
      }
      if (at(integerStart) === '0' && length - integerStart > 1) {
        invalid()
      }
      if (at(length) === '.') {
        length += 1
        if (digits() === 0) {
          invalid()
        }
      }
      if (at(length) === 'e' || at(length) === 'E') {
        length += 1
        if (at(length) === '+' || at(length) === '-') {
          length += 1
        }
        if (digits() === 0) {
          invalid()
        }
      }
      const following = at(length)
      if (following !== undefined && (isIdentifierPart(following) || following === '.')) {
        invalid()
      }
      const text = slice(offset, offset + length)
      offset += length
      tokens.push({ kind: 'number', text, value: text, position: start, end: start + length })
      continue
    }
    if (isIdentifierStart(character)) {
      let length = 1
      while (isIdentifierPart(characters[offset + length] ?? '')) {
        length += 1
      }
      const text = slice(offset, offset + length)
      offset += length
      tokens.push({ kind: 'identifier', text, value: text, position: start, end: start + length })
      continue
    }
    if (character === '=') {
      throw new SelectorFailure(start, start + 1, 'use "==" to compare')
    }
    if (character === '!') {
      throw new SelectorFailure(start, start + 1, 'use "not" to negate, or "!=" to compare')
    }
    if (character === "'") {
      throw new SelectorFailure(start, start + 1, 'strings use double quotes')
    }
    throw new SelectorFailure(start, start + 1, `unexpected character ${quote(character)}`)
  }
}

class Parser {
  private readonly tokens: Token[]
  private index = 0
  private depth = 0
  private comparisons = 0

  constructor(tokens: Token[]) {
    this.tokens = tokens
  }

  private peek(): Token {
    const token = this.tokens[this.index]
    if (token === undefined) {
      throw new Error('the token list has no end token')
    }
    return token
  }

  private take(): Token {
    const current = this.peek()
    if (current.kind !== 'end') {
      this.index += 1
    }
    return current
  }

  private keyword(word: string): boolean {
    const current = this.peek()
    return current.kind === 'identifier' && current.text === word
  }

  private unexpected(expected: string): SelectorFailure {
    const current = this.peek()
    const found =
      current.kind === 'identifier' || current.kind === 'number' || current.kind === 'string'
        ? quote(current.text)
        : describe(current.kind)
    return new SelectorFailure(
      current.position,
      Math.max(current.end, current.position + 1),
      `expected ${expected}, found ${found}`,
    )
  }

  private expect(kind: TokenKind): Token {
    if (this.peek().kind !== kind) {
      throw this.unexpected(describe(kind))
    }
    return this.take()
  }

  parse(): Expression {
    if (this.tokens.length === 1) {
      return { kind: 'all' }
    }
    const root = this.or()
    if (this.peek().kind !== 'end') {
      throw this.unexpected('"and", "or" or the end of the selector')
    }
    return root
  }

  private or(): Expression {
    let left = this.and()
    while (this.keyword('or')) {
      this.take()
      left = { kind: 'or', left, right: this.and() }
    }
    return left
  }

  private and(): Expression {
    let left = this.unary()
    while (this.keyword('and')) {
      this.take()
      left = { kind: 'and', left, right: this.unary() }
    }
    return left
  }

  private unary(): Expression {
    this.depth += 1
    try {
      if (this.depth > maximumDepth) {
        const current = this.peek()
        throw new SelectorFailure(
          current.position,
          current.end,
          `the selector nests deeper than ${maximumDepth} levels`,
        )
      }
      if (this.keyword('not')) {
        this.take()
        return { kind: 'not', operand: this.unary() }
      }
      return this.primary()
    } finally {
      this.depth -= 1
    }
  }

  private primary(): Expression {
    if (this.peek().kind === 'left_parenthesis') {
      this.take()
      const inner = this.or()
      this.expect('right_parenthesis')
      return inner
    }
    if (this.keyword('has')) {
      this.take()
      this.expect('left_parenthesis')
      const field = this.field()
      this.expect('right_parenthesis')
      return { kind: 'has', field }
    }
    return this.comparison()
  }

  private field(): Field {
    const current = this.peek()
    if (current.kind !== 'identifier') {
      throw this.unexpected('a field name or facts["key"]')
    }
    if (current.text === 'facts') {
      this.take()
      this.expect('left_bracket')
      const key = this.peek()
      if (key.kind !== 'string') {
        throw this.unexpected('the fact name as a string')
      }
      this.take()
      if (key.value === '') {
        throw new SelectorFailure(key.position, key.end, 'a fact name cannot be empty')
      }
      const closing = this.expect('right_bracket')
      return {
        kind: 'fact',
        column: '',
        factKey: key.value,
        position: current.position,
        end: closing.end,
      }
    }
    const kind = columns.get(current.text)
    if (kind === undefined) {
      throw new SelectorFailure(
        current.position,
        current.end,
        keywords.has(current.text)
          ? `expected a field name, found ${quote(current.text)}`
          : `unknown field ${quote(current.text)}; use facts[${quote(current.text)}] for a fact`,
      )
    }
    this.take()
    return {
      kind,
      column: current.text,
      factKey: '',
      position: current.position,
      end: current.end,
    }
  }

  private comparison(): Expression {
    const start = this.peek().position
    let semver = false
    let field: Field
    if (this.keyword('semver')) {
      this.take()
      semver = true
      this.expect('left_parenthesis')
      field = this.field()
      this.expect('right_parenthesis')
    } else {
      field = this.field()
    }
    const operatorToken = this.peek()
    let operator: Operator
    const known = comparisonOperators[operatorToken.kind]
    if (known !== undefined) {
      operator = known
      this.take()
    } else if (this.keyword('in')) {
      operator = 'in'
      this.take()
    } else if (
      this.keyword('not') &&
      this.tokens[this.index + 1]?.kind === 'identifier' &&
      this.tokens[this.index + 1]?.text === 'in'
    ) {
      operator = 'not in'
      this.take()
      this.take()
    } else {
      throw this.unexpected('a comparison (==, !=, <, <=, >, >=, in, not in)')
    }
    this.comparisons += 1
    if (this.comparisons > maximumComparisons) {
      throw new SelectorFailure(
        start,
        operatorToken.end,
        `the selector has more than ${maximumComparisons} comparisons`,
      )
    }
    const values: Literal[] = []
    if (operator === 'in' || operator === 'not in') {
      const opening = this.expect('left_bracket')
      for (;;) {
        const value = this.literal()
        values.push(value)
        if (values.length > maximumListLength) {
          throw new SelectorFailure(
            opening.position,
            value.end,
            `a list holds at most ${maximumListLength} values`,
          )
        }
        if (this.peek().kind === 'comma') {
          this.take()
          continue
        }
        this.expect('right_bracket')
        break
      }
    } else {
      values.push(this.literal())
    }
    const comparison = { kind: 'compare' as const, field, semver, operator, values }
    check(comparison, start)
    return comparison
  }

  private literal(): Literal {
    const current = this.peek()
    if (current.kind === 'string' || current.kind === 'number') {
      this.take()
      return {
        kind: current.kind,
        text: current.value,
        bool: false,
        position: current.position,
        end: current.end,
      }
    }
    if (this.keyword('true') || this.keyword('false')) {
      this.take()
      return {
        kind: 'bool',
        text: current.text,
        bool: current.text === 'true',
        position: current.position,
        end: current.end,
      }
    }
    throw this.unexpected('a string, number, true or false')
  }
}

const orderingOperators = new Set<Operator>(['<', '<=', '>', '>='])

function fieldName(field: Field): string {
  return field.kind === 'fact' ? `facts[${quote(field.factKey)}]` : field.column
}

function check(comparison: Extract<Expression, { kind: 'compare' }>, start: number) {
  const { field, values } = comparison
  const first = values[0]
  if (first === undefined) {
    return
  }
  for (const value of values.slice(1)) {
    if (value.kind !== first.kind) {
      throw new SelectorFailure(
        value.position,
        value.end,
        `a list holds values of one type; this list starts with ${literalNames[first.kind]}`,
      )
    }
  }
  const ordering = orderingOperators.has(comparison.operator)
  if (comparison.semver || (field.kind === 'version' && ordering)) {
    const operand = comparison.semver ? `semver(${fieldName(field)})` : fieldName(field)
    for (const value of values) {
      if (value.kind !== 'string') {
        throw new SelectorFailure(
          value.position,
          value.end,
          `${operand} compares with version strings like "1.2.3"`,
        )
      }
      if (semverKey(value.text) === null) {
        throw new SelectorFailure(
          value.position,
          value.end,
          `${quote(value.text)} is not a semantic version like "1.2.3"`,
        )
      }
    }
    return
  }
  if (field.kind !== 'fact') {
    if (first.kind !== 'string') {
      throw new SelectorFailure(
        first.position,
        first.end,
        `${field.column} compares with strings, not ${literalNames[first.kind]}`,
      )
    }
    return
  }
  if (first.kind === 'bool' && ordering) {
    const last = values[values.length - 1] ?? first
    throw new SelectorFailure(
      start,
      last.end,
      'booleans can only be compared with ==, !=, in and not in',
    )
  }
}

export type ParsedSelector = { ok: true; expression: Expression } | ({ ok: false } & SelectorError)

export function parseSelector(source: string): ParsedSelector {
  if (Array.from(source).length > maximumLength) {
    return {
      ok: false,
      position: maximumLength,
      end: maximumLength,
      message: `the selector is longer than ${maximumLength} characters`,
    }
  }
  try {
    return { ok: true, expression: new Parser(tokenize(source)).parse() }
  } catch (failure) {
    if (failure instanceof SelectorFailure) {
      return { ok: false, ...failure.detail }
    }
    throw failure
  }
}

const semverPattern =
  /^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)(-(0|[1-9][0-9]*|[0-9]*[A-Za-z-][0-9A-Za-z-]*)(\.(0|[1-9][0-9]*|[0-9]*[A-Za-z-][0-9A-Za-z-]*))*)?(\+[0-9A-Za-z-]+(\.[0-9A-Za-z-]+)*)?$/

export function semverKey(version: string): number[] | null {
  const match = semverPattern.exec(version)
  if (match === null) {
    return null
  }
  return [Number(match[1]), Number(match[2]), Number(match[3]), match[4] === undefined ? 1 : 0]
}

function compareKeys(left: number[], right: number[]): number {
  for (let position = 0; position < left.length; position += 1) {
    const difference = (left[position] ?? 0) - (right[position] ?? 0)
    if (difference !== 0) {
      return difference
    }
  }
  return 0
}

function compareText(left: string, right: string): number {
  return left < right ? -1 : left > right ? 1 : 0
}

function holds(operator: Operator, order: number): boolean {
  switch (operator) {
    case '==':
      return order === 0
    case '!=':
      return order !== 0
    case '<':
      return order < 0
    case '<=':
      return order <= 0
    case '>':
      return order > 0
    case '>=':
      return order >= 0
    default:
      return false
  }
}

function decide(operator: Operator, values: Literal[], order: (value: Literal) => number): boolean {
  if (operator === 'in' || operator === 'not in') {
    const member = values.some((value) => order(value) === 0)
    return operator === 'in' ? member : !member
  }
  const first = values[0]
  return first !== undefined && holds(operator, order(first))
}

export interface SelectableNode {
  fields: Record<string, string | null>
  facts: Record<string, JsonValue>
}

function compareFact(
  comparison: Extract<Expression, { kind: 'compare' }>,
  value: JsonValue,
): boolean {
  const kind = comparison.values[0]?.kind
  if (kind === 'string') {
    return (
      typeof value === 'string' &&
      decide(comparison.operator, comparison.values, (literal) => compareText(value, literal.text))
    )
  }
  if (kind === 'number') {
    return (
      typeof value === 'number' &&
      decide(comparison.operator, comparison.values, (literal) => value - Number(literal.text))
    )
  }
  return (
    typeof value === 'boolean' &&
    decide(comparison.operator, comparison.values, (literal) => (value === literal.bool ? 0 : 1))
  )
}

export function evaluate(expression: Expression, node: SelectableNode): boolean {
  switch (expression.kind) {
    case 'all':
      return true
    case 'and':
      return evaluate(expression.left, node) && evaluate(expression.right, node)
    case 'or':
      return evaluate(expression.left, node) || evaluate(expression.right, node)
    case 'not':
      return !evaluate(expression.operand, node)
    case 'has':
      return expression.field.kind === 'fact'
        ? Object.hasOwn(node.facts, expression.field.factKey)
        : (node.fields[expression.field.column] ?? null) !== null
    case 'compare': {
      const { field } = expression
      let text: string | null
      if (field.kind === 'fact') {
        if (!Object.hasOwn(node.facts, field.factKey)) {
          return false
        }
        const value = node.facts[field.factKey] ?? null
        if (!expression.semver) {
          return compareFact(expression, value)
        }
        text = typeof value === 'string' ? value : null
      } else {
        text = node.fields[field.column] ?? null
      }
      if (text === null) {
        return false
      }
      const actual = text
      if (
        expression.semver ||
        (field.kind === 'version' && orderingOperators.has(expression.operator))
      ) {
        const key = semverKey(actual)
        if (key === null) {
          return false
        }
        return decide(expression.operator, expression.values, (literal) =>
          compareKeys(key, semverKey(literal.text) ?? []),
        )
      }
      return decide(expression.operator, expression.values, (literal) =>
        compareText(actual, literal.text),
      )
    }
  }
}
