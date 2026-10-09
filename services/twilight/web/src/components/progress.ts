import type { Counter, NodeState } from '../api/types'
import { nodeStates } from '../api/types'
import { formatNumber } from '../format'
import { nodeStateStyles } from './status'

export type StateCounts = Partial<Record<NodeState, number>>

export function stateCounts(counters: Counter[], phase?: number): StateCounts {
  const counts: StateCounts = {}
  for (const counter of counters) {
    if (phase === undefined || counter.phase === phase) {
      counts[counter.state] = (counts[counter.state] ?? 0) + counter.count
    }
  }
  return counts
}

export function countOf(counts: StateCounts): number {
  return Object.values(counts).reduce((sum, count) => sum + (count ?? 0), 0)
}

export interface Segment {
  state: NodeState | 'not_reached'
  count: number
  fraction: number
}

export function progressSegments(counts: StateCounts, expected: number | null): Segment[] {
  const present = nodeStates
    .map((state) => ({ state, count: counts[state] ?? 0 }))
    .filter((entry) => entry.count > 0)
  const rows = present.reduce((sum, entry) => sum + entry.count, 0)
  const total = Math.max(rows, expected ?? 0)
  if (total === 0) {
    return []
  }
  const segments: Segment[] = present.map((entry) => ({ ...entry, fraction: entry.count / total }))
  if (total > rows) {
    segments.push({ state: 'not_reached', count: total - rows, fraction: (total - rows) / total })
  }
  return segments
}

export function segmentLabel(segment: Segment): string {
  return segment.state === 'not_reached' ? 'Not reached yet' : nodeStateStyles[segment.state].label
}

export function describeProgress(segments: Segment[]): string {
  if (segments.length === 0) {
    return 'No nodes yet'
  }
  const total = segments.reduce((sum, segment) => sum + segment.count, 0)
  const parts = segments.map(
    (segment) => `${formatNumber(segment.count)} ${segmentLabel(segment).toLowerCase()}`,
  )
  return `${parts.join(', ')}, of ${formatNumber(total)} nodes`
}
