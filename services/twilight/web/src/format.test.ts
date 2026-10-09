import { describe, expect, it } from 'vitest'
import { formatPercent } from './format'

describe('formatPercent', () => {
  it('writes whole percents without decimals', () => {
    expect(formatPercent(0)).toBe('0%')
    expect(formatPercent(0.05)).toBe('5%')
    expect(formatPercent(0.29)).toBe('29%')
    expect(formatPercent(1)).toBe('100%')
  })

  it('never rounds a share above zero to 0% or one below a whole to 100%', () => {
    expect(formatPercent(0.0004)).toBe('0.04%')
    expect(formatPercent(0.000001)).toBe('0.0001%')
    expect(formatPercent(0.9996)).toBe('99.96%')
    expect(formatPercent(0.0999)).toBe('9.99%')
  })

  it('keeps two decimals below 1% and one above', () => {
    expect(formatPercent(0.0026)).toBe('0.26%')
    expect(formatPercent(0.1234)).toBe('12.3%')
  })

  it('shows a rate apart from the limit it is compared with', () => {
    expect(formatPercent(0.002501, 0.0025)).toBe('0.2501%')
    expect(formatPercent(0.25005, 0.25)).toBe('25.005%')
    expect(formatPercent(0.0025, 0.0025)).toBe('0.25%')
  })

  it('writes a dash when there is no rate', () => {
    expect(formatPercent(null)).toBe('–')
    expect(formatPercent(Number.NaN)).toBe('–')
  })
})
