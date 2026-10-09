import { describe, expect, it } from 'vitest'
import type { SelectableNode } from './selector'
import { evaluate, parseSelector, semverKey } from './selector'

const node: SelectableNode = {
  fields: {
    country: 'US',
    os_name: 'debian',
    dusk_version: '0.1.1',
    reported_version: null,
    lifecycle: 'active',
    tenant: null,
  },
  facts: {
    'dusk.device.memory_bytes': 8589934592,
    'dusk.os.locale': 'en_US',
    'acme.beta': true,
  },
}

function matches(source: string): boolean {
  const parsed = parseSelector(source)
  if (!parsed.ok) {
    throw new Error(`${source}: ${parsed.message}`)
  }
  return evaluate(parsed.expression, node)
}

function failure(source: string) {
  const parsed = parseSelector(source)
  if (parsed.ok) {
    throw new Error(`${source} parsed`)
  }
  return { position: parsed.position, end: parsed.end, message: parsed.message }
}

describe('parseSelector', () => {
  it('treats an empty selector as matching every node', () => {
    expect(matches('')).toBe(true)
    expect(matches('   ')).toBe(true)
  })

  it('reports the position and end of an unknown field in code points', () => {
    expect(failure('country == "US" and contry == "DE"')).toEqual({
      position: 20,
      end: 26,
      message: 'unknown field "contry"; use facts["contry"] for a fact',
    })
  })

  it('counts positions in code points, not UTF-16 units', () => {
    expect(failure('hostname == "🛰" and x')).toMatchObject({ position: 20, end: 21 })
  })

  it('explains the single equals sign', () => {
    expect(failure('country = "US"')).toEqual({
      position: 8,
      end: 9,
      message: 'use "==" to compare',
    })
  })

  it('refuses single quotes and bare exclamation marks', () => {
    expect(failure("country == 'US'").message).toBe('strings use double quotes')
    expect(failure('!has(facts["k"])').message).toBe('use "not" to negate, or "!=" to compare')
  })

  it('underlines an unclosed string to the end of the selector', () => {
    expect(failure('country == "US')).toEqual({
      position: 11,
      end: 14,
      message: 'this string is not closed',
    })
  })

  it('requires strings for columns', () => {
    expect(failure('country == 5').message).toBe('country compares with strings, not numbers')
  })

  it('requires semantic versions when ordering a version column', () => {
    expect(failure('dusk_version < "latest"').message).toBe(
      '"latest" is not a semantic version like "1.2.3"',
    )
    expect(failure('dusk_version < 2').message).toBe(
      'dusk_version compares with version strings like "1.2.3"',
    )
  })

  it('keeps lists to one type', () => {
    expect(failure('facts["k"] in ["a", 1]')).toMatchObject({
      position: 20,
      message: 'a list holds values of one type; this list starts with strings',
    })
  })

  it('refuses ordering booleans', () => {
    expect(failure('facts["acme.beta"] > true').message).toBe(
      'booleans can only be compared with ==, !=, in and not in',
    )
  })

  it('names what it expected when the selector ends early', () => {
    expect(failure('country ==').message).toBe(
      'expected a string, number, true or false, found the end of the selector',
    )
    expect(failure('country == "US" and').message).toBe(
      'expected a field name or facts["key"], found the end of the selector',
    )
  })

  it('refuses malformed numbers', () => {
    expect(failure('facts["k"] == 01').message).toBe('"01" is not a number')
    expect(failure('facts["k"] == 1.').message).toBe('"1." is not a number')
  })
})

describe('evaluate', () => {
  it('compares columns and facts', () => {
    expect(matches('country == "US" and os_name in ["debian", "ubuntu"]')).toBe(true)
    expect(matches('facts["dusk.device.memory_bytes"] >= 4294967296')).toBe(true)
    expect(matches('facts["acme.beta"] == true')).toBe(true)
  })

  it('compares versions as semver', () => {
    expect(matches('dusk_version < "0.2.0"')).toBe(true)
    expect(matches('dusk_version < "0.1.1-rc.1"')).toBe(false)
    expect(matches('semver(facts["dusk.os.locale"]) > "1.0.0"')).toBe(false)
  })

  it('uses two-valued logic for absent values', () => {
    expect(matches('tenant == "acme"')).toBe(false)
    expect(matches('not tenant == "acme"')).toBe(true)
    expect(matches('tenant not in ["acme"]')).toBe(false)
    expect(matches('facts["missing"] != "x"')).toBe(false)
    expect(matches('reported_version < "1.0.0"')).toBe(false)
  })

  it('is false when the fact has another type than the literal', () => {
    expect(matches('facts["dusk.device.memory_bytes"] == "8589934592"')).toBe(false)
  })

  it('checks presence with has', () => {
    expect(matches('has(facts["dusk.os.locale"])')).toBe(true)
    expect(matches('has(tenant)')).toBe(false)
    expect(matches('has(country)')).toBe(true)
  })
})

describe('semverKey', () => {
  it('ranks a prerelease below its release', () => {
    expect(semverKey('0.2.2-rc.1')).toEqual([0, 2, 2, 0])
    expect(semverKey('0.2.2')).toEqual([0, 2, 2, 1])
    expect(semverKey('v0.2.2')).toBeNull()
  })
})
