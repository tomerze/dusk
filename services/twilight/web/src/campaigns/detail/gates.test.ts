import { describe, expect, it } from 'vitest'
import type { Gates } from '../../api/types'
import { dimensionsOf, failing, gateDisplay, judge, splitGroup, waitingForSample } from './gates'

const thresholds = {
  min_sample: 20,
  max_failure_rate: 0.05,
  max_silent_rate: 0.02,
  degraded: false,
}

function gates(patch: Partial<Gates> = {}): Gates {
  return {
    overall: { succeeded: 180, failed: 20, eligible: 150, silent: 1 },
    groups: {},
    verdict: 'pass',
    reason: '',
    failing_group: '',
    degraded: false,
    min_sample: 20,
    required: 20,
    max_failure_rate: 0.05,
    max_silent_rate: 0.02,
    phase: 1,
    phase_name: 'early',
    bake_seconds: 7200,
    bake_accrued_seconds: 1800,
    converging: false,
    ...patch,
  }
}

describe('judge', () => {
  it('fails a group whose failure rate is over the limit once it has the sample', () => {
    const judgement = judge({ succeeded: 25, failed: 41, eligible: 0, silent: 0 }, thresholds)
    expect(judgement.sample).toBe(66)
    expect(judgement.failureRate).toBeCloseTo(41 / 66)
    expect(judgement.failureExceeded).toBe(true)
    expect(failing(judgement)).toBe(true)
  })

  it('never fails a group below the sample, however bad its rate', () => {
    const judgement = judge({ succeeded: 1, failed: 9, eligible: 0, silent: 0 }, thresholds)
    expect(judgement.failureJudged).toBe(false)
    expect(failing(judgement)).toBe(false)
  })

  it('judges every group with a result when the sample is zero', () => {
    const judgement = judge(
      { succeeded: 0, failed: 1, eligible: 0, silent: 0 },
      { ...thresholds, min_sample: 0 },
    )
    expect(judgement.failureExceeded).toBe(true)
    expect(judge(emptyTally(), { ...thresholds, min_sample: 0 }).failureJudged).toBe(false)
  })

  it('does not judge the silent rate while the online view is degraded', () => {
    const tally = { succeeded: 100, failed: 0, eligible: 100, silent: 30 }
    expect(judge(tally, thresholds).silentExceeded).toBe(true)
    expect(judge(tally, { ...thresholds, degraded: true }).silentJudged).toBe(false)
  })

  it('has no rate when nothing was counted', () => {
    const judgement = judge(emptyTally(), thresholds)
    expect(judgement.failureRate).toBeNull()
    expect(judgement.silentRate).toBeNull()
  })
})

function emptyTally() {
  return { succeeded: 0, failed: 0, eligible: 0, silent: 0 }
}

describe('dimensionsOf', () => {
  it('orders dimensions as the policy lists them and keeps unexpected ones after', () => {
    const report = gates({
      groups: {
        country: { US: emptyTally() },
        os_build: { '22631.4317': emptyTally(), '26100.2033': emptyTally() },
        rack: { a: emptyTally() },
      },
    })
    const dimensions = dimensionsOf(report, ['os_build', 'hardware_class', 'country'])
    expect(dimensions.map((dimension) => dimension.field)).toEqual(['os_build', 'country', 'rack'])
    expect(dimensions[0]?.groups.map((group) => group.value)).toEqual(['22631.4317', '26100.2033'])
  })
})

describe('splitGroup', () => {
  it('splits the failing group at the first equals sign', () => {
    expect(splitGroup('os_build=22631.4317')).toEqual({ field: 'os_build', value: '22631.4317' })
    expect(splitGroup('hardware_class=x86_64-8c-16g')).toEqual({
      field: 'hardware_class',
      value: 'x86_64-8c-16g',
    })
    expect(splitGroup('country=')).toEqual({ field: 'country', value: '' })
  })

  it('has no group for an overall breach', () => {
    expect(splitGroup('')).toBeNull()
  })
})

describe('gateDisplay', () => {
  it('is not evaluated once the campaign stopped', () => {
    expect(gateDisplay({ status: 'completed' }, gates({ verdict: 'fail' }))).toBe('inactive')
    expect(gateDisplay({ status: 'draft' }, gates())).toBe('inactive')
  })

  it('names converging over a pass', () => {
    expect(gateDisplay({ status: 'running' }, gates({ converging: true }))).toBe('converging')
  })

  it('shows a degraded hold as degraded but keeps a failure a failure', () => {
    expect(
      gateDisplay(
        { status: 'running' },
        gates({ verdict: 'hold', degraded: true, reason: 'the online view is degraded' }),
      ),
    ).toBe('degraded')
    expect(gateDisplay({ status: 'paused' }, gates({ verdict: 'fail', degraded: true }))).toBe(
      'fail',
    )
  })
})

describe('waitingForSample', () => {
  it('recognises both sample holds', () => {
    expect(
      waitingForSample(gates({ verdict: 'hold', reason: 'waiting for sample 10 of 20' })),
    ).toBe(true)
    expect(
      waitingForSample(
        gates({ verdict: 'hold', reason: 'waiting for silent-window sample 4 of 20' }),
      ),
    ).toBe(true)
    expect(
      waitingForSample(gates({ verdict: 'hold', reason: 'the online view is degraded' })),
    ).toBe(false)
  })
})
