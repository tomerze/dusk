import { describe, expect, it } from 'vitest'
import type { Action, Campaign, CampaignStatus } from '../api/types'
import { defaultPolicy } from './defaults'
import { intersection, overlapCandidates } from './overlaps'

function campaign(id: string, action: Action, status: CampaignStatus = 'running'): Campaign {
  return {
    id,
    name: id,
    description: '',
    tenant: null,
    status,
    kind: action.kind,
    selector: 'tenant == "acme-retail"',
    action,
    policy: defaultPolicy(action.kind),
    created_by: 'release-bot',
    created_at: '2026-10-09T08:00:00Z',
    updated_at: '2026-10-09T08:00:00Z',
    started_at: '2026-10-09T08:10:00Z',
    paused_at: null,
    finished_at: null,
    current_phase: 0,
    phase_started_at: '2026-10-09T08:10:00Z',
    phase_paused_seconds: 0,
    gate_override_after: null,
    pause_kind: null,
    pause_reason: null,
    abort_reason: null,
    last_dispatch_at: null,
    version: 2,
    counters: [],
  }
}

const version = (key: string): Action => ({
  kind: 'ensure_version',
  version: '0.2.1',
  version_key: key,
  script: 'x',
})

describe('intersection', () => {
  it('wraps both selectors so their precedence survives', () => {
    expect(intersection('a == "1" or b == "2"', 'c == "3"')).toBe(
      '(a == "1" or b == "2") and (c == "3")',
    )
  })

  it('drops an empty side', () => {
    expect(intersection('', 'c == "3"')).toBe('(c == "3")')
    expect(intersection('  ', '')).toBe('')
  })
})

describe('overlapCandidates', () => {
  const campaigns = [
    campaign('same-key', version('dusk.version')),
    campaign('other-key', version('acme.build')),
    campaign('paused', version('dusk.version'), 'paused'),
    campaign('finished', version('dusk.version'), 'completed'),
    campaign('config', { kind: 'ensure_config', config_hash: 'b41d', script: 'x' }),
    campaign('script', { kind: 'run_script', script: 'x' }),
  ]

  it('keeps running and paused campaigns that set the same version key', () => {
    expect(
      overlapCandidates({ action: version('dusk.version') }, campaigns, null).map(
        (entry) => entry.id,
      ),
    ).toEqual(['same-key', 'paused'])
  })

  it('compares ensure_config with ensure_config only', () => {
    expect(
      overlapCandidates(
        { action: { kind: 'ensure_config', config_hash: 'c0ff', script: 'x' } },
        campaigns,
        null,
      ).map((entry) => entry.id),
    ).toEqual(['config'])
  })

  it('never compares a campaign with itself, nor one-shot actions with anything', () => {
    expect(
      overlapCandidates({ action: version('dusk.version') }, campaigns, 'same-key').map(
        (entry) => entry.id,
      ),
    ).toEqual(['paused'])
    expect(
      overlapCandidates({ action: { kind: 'run_script', script: 'x' } }, campaigns, null),
    ).toEqual([])
  })
})
