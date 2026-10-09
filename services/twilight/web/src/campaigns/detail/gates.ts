import type { Campaign, Gates, Tally } from '../../api/types'
import type { GateDisplay } from '../../components/status'

export interface Judgement {
  tally: Tally
  sample: number
  failureRate: number | null
  silentRate: number | null
  failureJudged: boolean
  silentJudged: boolean
  failureExceeded: boolean
  silentExceeded: boolean
}

export type Thresholds = Pick<Gates, 'min_sample' | 'max_failure_rate' | 'max_silent_rate'> & {
  degraded: boolean
}

export function judge(tally: Tally, thresholds: Thresholds): Judgement {
  const sample = tally.succeeded + tally.failed
  const failureRate = sample === 0 ? null : tally.failed / sample
  const silentRate = tally.eligible === 0 ? null : tally.silent / tally.eligible
  const failureJudged = sample >= thresholds.min_sample && sample > 0
  const silentJudged =
    !thresholds.degraded && tally.eligible >= thresholds.min_sample && tally.eligible > 0
  return {
    tally,
    sample,
    failureRate,
    silentRate,
    failureJudged,
    silentJudged,
    failureExceeded:
      failureJudged && failureRate !== null && failureRate > thresholds.max_failure_rate,
    silentExceeded: silentJudged && silentRate !== null && silentRate > thresholds.max_silent_rate,
  }
}

export function failing(judgement: Judgement): boolean {
  return judgement.failureExceeded || judgement.silentExceeded
}

export interface GroupJudgement extends Judgement {
  value: string
}

export interface Dimension {
  field: string
  groups: GroupJudgement[]
}

export function dimensionsOf(gates: Gates, order: readonly string[]): Dimension[] {
  const known = order.filter((field) => field in gates.groups)
  const extra = Object.keys(gates.groups)
    .filter((field) => !order.includes(field))
    .sort()
  return [...known, ...extra].map((field) => ({
    field,
    groups: Object.entries(gates.groups[field] ?? {}).map(([value, tally]) => ({
      value,
      ...judge(tally, gates),
    })),
  }))
}

export function splitGroup(group: string): { field: string; value: string } | null {
  const separator = group.indexOf('=')
  if (separator <= 0) {
    return null
  }
  return { field: group.slice(0, separator), value: group.slice(separator + 1) }
}

export function gateDisplay(campaign: Pick<Campaign, 'status'>, gates: Gates): GateDisplay {
  if (campaign.status !== 'running' && campaign.status !== 'paused') {
    return 'inactive'
  }
  if (gates.converging) {
    return 'converging'
  }
  if (gates.verdict === 'hold' && gates.degraded) {
    return 'degraded'
  }
  return gates.verdict
}

export function waitingForSample(gates: Gates): boolean {
  return gates.verdict === 'hold' && gates.reason.startsWith('waiting for')
}
