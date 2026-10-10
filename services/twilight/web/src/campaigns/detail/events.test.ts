import { describe, expect, it } from 'vitest'
import type { CampaignEvent, JsonValue } from '../../api/types'
import { eventStyle, summarizeEvent } from './events'

function event(kind: string, detail: Record<string, JsonValue> = {}, actor = 'twilight') {
  const value: CampaignEvent = { id: 1, time: '2026-10-09T10:00:00Z', kind, actor, detail }
  return value
}

describe('summarizeEvent', () => {
  it('names the phase that opened, the one that passed and the tally so far', () => {
    expect(
      summarizeEvent(
        event('phase_advanced', {
          from: 'canary',
          to: 'early',
          percent: 10,
          overall: { succeeded: 26, failed: 1, eligible: 20, silent: 0 },
        }),
      ),
    ).toBe('Early opened at 10% after canary passed its gate; 26 succeeded and 1 failed so far')
  })

  it('reads a gate pause with the reason the gate gave', () => {
    const paused = event('paused', {
      reason: 'failure rate 0.62 in os_build=22631.4317, 41 of 66',
      pause_kind: 'gate',
      group: 'os_build=22631.4317',
    })
    expect(summarizeEvent(paused)).toBe(
      'Dispatch stopped by a health gate; failure rate 0.62 in os_build=22631.4317, 41 of 66',
    )
    expect(eventStyle(paused)).toMatchObject({ label: 'Paused by its gate', group: 'gate' })
  })

  it('reads a gate that failed while the campaign was paused', () => {
    const failed = event('gate_failed', {
      reason: 'failure rate 0.62 in os_build=22631.4317, 41 of 66',
      pause_kind: 'gate',
    })
    expect(summarizeEvent(failed)).toBe(
      'The gate failed while the campaign was paused; resuming needs a gate override; failure rate 0.62 in os_build=22631.4317, 41 of 66',
    )
    expect(eventStyle(failed)).toMatchObject({ label: 'Gate failed while paused', group: 'gate' })
  })

  it('tells an operator pause from a denied dispatch', () => {
    expect(
      summarizeEvent(event('paused', { pause_kind: 'operator', reason: 'freeze' }, 'ops')),
    ).toBe('Dispatch stopped; freeze')
    expect(
      eventStyle(event('paused', { pause_kind: 'permission', reason: 'permission denied' })).group,
    ).toBe('nodes')
  })

  it('marks a resume past a failed gate as an override', () => {
    const resumed = event('resumed', { override_gate: true, reason: 'hotfix shipped' }, 'ops')
    expect(eventStyle(resumed).label).toBe('Gate overridden')
    expect(summarizeEvent(resumed)).toBe(
      'Dispatch resumed past a failed gate; the gate now counts only nodes dispatched from here on; reason: hotfix shipped',
    )
    expect(summarizeEvent(event('resumed', {}, 'ops'))).toBe('Dispatch resumed')
  })

  it('counts retried and resolved nodes', () => {
    expect(summarizeEvent(event('nodes_retried', { count: 12, reason: 'driver fixed' }))).toBe(
      '12 nodes sent again at a new pid; driver fixed',
    )
    expect(
      summarizeEvent(event('nodes_resolved', { count: 1, outcome: 'failed', reason: 'logs' })),
    ).toBe('1 node marked failed by hand; logs')
  })

  it('says why a campaign finished', () => {
    expect(
      summarizeEvent(event('completed', { reason: 'the campaign reached its deadline' })),
    ).toBe('The campaign finished; the campaign reached its deadline')
    expect(summarizeEvent(event('created', { name: 'Dusk 0.2.2' }, 'ops'))).toBe(
      'Saved as a draft named Dusk 0.2.2',
    )
  })

  it('shows an event kind it does not know by its name and reason', () => {
    const unknown = event('leader_changed', { reason: 'term 7' })
    expect(eventStyle(unknown)).toMatchObject({ label: 'Leader changed', group: 'lifecycle' })
    expect(summarizeEvent(unknown)).toBe('term 7')
  })
})
