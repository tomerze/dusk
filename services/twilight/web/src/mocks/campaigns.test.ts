import { describe, expect, it } from 'vitest'
import type { Policy, Tally } from '../api/types'
import { countersOf, gatesOf, generateCampaigns, judgeGates } from './campaigns'
import { generateNodes } from './nodes'

const gates: Policy['gates'] = {
  min_sample: 20,
  max_failure_rate: 0.05,
  max_silent_rate: 0.1,
  silent_window_seconds: 1800,
  breakdown: ['os_build', 'country'],
}

function tally(patch: Partial<Tally>): Tally {
  return { succeeded: 0, failed: 0, eligible: 0, silent: 0, ...patch }
}

describe('judgeGates, as twilight judges', () => {
  it('waits for the result sample, then for the silent-window sample', () => {
    expect(
      judgeGates(gates, null, tally({ succeeded: 10, eligible: 10 }), {}, 1000, false),
    ).toEqual({
      verdict: 'hold',
      reason: 'waiting for sample 10 of 20',
      failing_group: '',
      required: 20,
    })
    expect(
      judgeGates(gates, null, tally({ succeeded: 30, eligible: 12 }), {}, 1000, false).reason,
    ).toBe('waiting for silent-window sample 12 of 20')
    expect(
      judgeGates(gates, null, tally({ succeeded: 30, eligible: 30 }), {}, 1000, false).verdict,
    ).toBe('pass')
  })

  it('fails on a group that has enough sample and names it', () => {
    const groups = {
      os_build: {
        '22631.4317': tally({ succeeded: 25, failed: 41, eligible: 25 }),
        '19045.5011': tally({ succeeded: 400, failed: 2, eligible: 400 }),
        rare: tally({ succeeded: 1, failed: 3, eligible: 1 }),
      },
    }
    expect(
      judgeGates(
        gates,
        null,
        tally({ succeeded: 1000, failed: 46, eligible: 1000 }),
        groups,
        1000,
        false,
      ),
    ).toEqual({
      verdict: 'fail',
      reason: 'failure rate 0.62 in os_build=22631.4317, 41 of 66',
      failing_group: 'os_build=22631.4317',
      required: 20,
    })
  })

  it('judges silence only while the view is whole', () => {
    const groups = { country: { US: tally({ succeeded: 50, eligible: 30, silent: 9 }) } }
    const overall = tally({ succeeded: 100, eligible: 80, silent: 9 })
    expect(judgeGates(gates, null, overall, groups, 1000, false).reason).toBe(
      'silent rate 0.30 in country=US, 9 of 30',
    )
    expect(judgeGates(gates, null, overall, groups, 1000, true)).toEqual({
      verdict: 'hold',
      reason: 'the online view is degraded',
      failing_group: '',
      required: 20,
    })
    expect(
      judgeGates(gates, null, tally({ succeeded: 50, failed: 50, eligible: 50 }), {}, 1000, true)
        .reason,
    ).toBe('failure rate 0.50 overall, 50 of 100')
  })

  it('needs a result from every node of open phases that hold fewer nodes than min_sample', () => {
    const small = { ...gates, min_sample: 5 }
    expect(judgeGates(small, null, tally({}), {}, 1, false)).toEqual({
      verdict: 'hold',
      reason: 'waiting for sample 0 of 1',
      failing_group: '',
      required: 1,
    })
    expect(judgeGates(small, null, tally({ succeeded: 1 }), {}, 1, false).reason).toBe(
      'waiting for silent-window sample 0 of 1',
    )
    expect(
      judgeGates(small, null, tally({ succeeded: 1, eligible: 1 }), {}, 1, false).verdict,
    ).toBe('pass')
    expect(judgeGates(small, null, tally({ failed: 1 }), {}, 1, false).reason).toBe(
      'failure rate 1.00 overall, 1 of 1',
    )
    expect(judgeGates(gates, null, tally({}), {}, 0, false)).toEqual({
      verdict: 'pass',
      reason: '',
      failing_group: '',
      required: 0,
    })
  })

  it('keeps min_sample for breakdown groups in a small phase', () => {
    const small = { ...gates, min_sample: 5, max_silent_rate: 0.3 }
    const groups = {
      os_build: {
        a: tally({ succeeded: 3, eligible: 3 }),
        b: tally({ succeeded: 1, eligible: 1, silent: 1 }),
      },
    }
    expect(
      judgeGates(small, null, tally({ succeeded: 4, eligible: 4, silent: 1 }), groups, 4, false)
        .verdict,
    ).toBe('pass')
  })

  it('stops at max_total_failures', () => {
    const open = { ...gates, max_failure_rate: 1 }
    expect(
      judgeGates(open, 3, tally({ succeeded: 1000, failed: 4, eligible: 1000 }), {}, 1000, false)
        .reason,
    ).toBe('4 failures, more than max_total_failures 3')
    expect(
      judgeGates(open, 3, tally({ succeeded: 1000, failed: 3, eligible: 1000 }), {}, 1000, false)
        .verdict,
    ).toBe('pass')
  })
})

describe('the generated fleet', () => {
  const now = Date.parse('2026-10-09T12:00:00Z')
  const nodes = generateNodes(20261007, 12000, now)
  const campaigns = generateCampaigns(20261009, nodes, now)

  it('has a campaign in every status', () => {
    expect(new Set(campaigns.map((entry) => entry.campaign.status))).toEqual(
      new Set(['draft', 'running', 'paused', 'completed', 'aborted', 'failed', 'archived']),
    )
  })

  it('pauses the printer campaign on a failing OS build, with the reason its gate gives', () => {
    const paused = campaigns.find((entry) => entry.campaign.pause_kind === 'gate')
    expect(paused).toBeDefined()
    const report = gatesOf(paused!, now, false)
    expect(report.required).toBe(
      Math.min(
        report.min_sample,
        paused!.rows.filter((row) => row.phase <= paused!.campaign.current_phase).length,
      ),
    )
    expect(report.verdict).toBe('fail')
    expect(report.failing_group).toBe('os_build=22631.4317')
    expect(paused?.campaign.pause_reason).toMatch(/in os_build=22631\.4317, \d+ of \d+$/)
  })

  it('counts every row once in the counters', () => {
    for (const entry of campaigns) {
      const counted = countersOf(entry.rows).reduce((sum, counter) => sum + counter.count, 0)
      expect(counted).toBe(entry.rows.length)
    }
  })

  it('reports converging only for the ensure campaign whose last phase passed', () => {
    const converging = campaigns.filter((entry) => gatesOf(entry, now, false).converging)
    expect(converging.map((entry) => entry.campaign.name)).toEqual([
      'Telemetry exporter config v14',
    ])
  })
})
