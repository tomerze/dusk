import { screen } from '@testing-library/react'
import { describe, expect, it } from 'vitest'
import { renderWithProviders } from '../test/render'
import { countOf, describeProgress, progressSegments, stateCounts } from './progress'
import { SegmentedProgress } from './SegmentedProgress'

describe('progressSegments', () => {
  it('orders states as the legend does and drops empty ones', () => {
    const segments = progressSegments({ pending: 30, succeeded: 60, failed: 10, backoff: 0 }, null)
    expect(segments.map((segment) => segment.state)).toEqual(['succeeded', 'failed', 'pending'])
    expect(segments.map((segment) => segment.fraction)).toEqual([0.6, 0.1, 0.3])
  })

  it('adds the expected nodes that have no row yet as a not-reached segment', () => {
    const segments = progressSegments({ succeeded: 50, pending: 50 }, 400)
    expect(segments.at(-1)).toEqual({ state: 'not_reached', count: 300, fraction: 0.75 })
    expect(segments.reduce((sum, segment) => sum + segment.fraction, 0)).toBeCloseTo(1)
  })

  it('never shows a negative remainder when rows outnumber the current match', () => {
    const segments = progressSegments({ succeeded: 120 }, 100)
    expect(segments).toEqual([{ state: 'succeeded', count: 120, fraction: 1 }])
  })

  it('is empty when nothing matched', () => {
    expect(progressSegments({}, 0)).toEqual([])
    expect(describeProgress([])).toBe('No nodes yet')
  })

  it('describes every segment with its count for assistive technology', () => {
    expect(describeProgress(progressSegments({ succeeded: 1200, failed: 3 }, 2000))).toBe(
      '1,200 succeeded, 3 failed, 797 not reached yet, of 2,000 nodes',
    )
  })
})

describe('stateCounts', () => {
  const counters = [
    { phase: 0, state: 'succeeded' as const, count: 40 },
    { phase: 0, state: 'failed' as const, count: 2 },
    { phase: 1, state: 'succeeded' as const, count: 100 },
    { phase: 1, state: 'pending' as const, count: 58 },
  ]

  it('sums every phase when no phase is named', () => {
    expect(stateCounts(counters)).toEqual({ succeeded: 140, failed: 2, pending: 58 })
    expect(countOf(stateCounts(counters))).toBe(200)
  })

  it('keeps only the named phase', () => {
    expect(stateCounts(counters, 1)).toEqual({ succeeded: 100, pending: 58 })
    expect(stateCounts(counters, 2)).toEqual({})
  })
})

describe('SegmentedProgress', () => {
  it('draws one segment per state, sized by its share', () => {
    renderWithProviders(
      <SegmentedProgress counts={{ succeeded: 75, failed: 5, delivered: 20 }} expected={200} />,
    )
    const bar = screen.getByRole('img', {
      name: '75 succeeded, 5 failed, 20 delivered, 100 not reached yet, of 200 nodes',
    })
    expect(bar).toBeVisible()
    expect(screen.getByTestId('segment-succeeded')).toHaveStyle({ flexGrow: '0.375' })
    expect(screen.getByTestId('segment-not_reached')).toHaveAttribute('data-count', '100')
  })

  it('marks the phase boundaries except the last', () => {
    const { container } = renderWithProviders(
      <SegmentedProgress counts={{ succeeded: 1 }} phases={[1, 10, 50, 100]} />,
    )
    const ticks = container.querySelectorAll('[aria-hidden="true"][style*="left"]')
    expect([...ticks].map((tick) => (tick as HTMLElement).style.left)).toEqual(['1%', '10%', '50%'])
  })
})
