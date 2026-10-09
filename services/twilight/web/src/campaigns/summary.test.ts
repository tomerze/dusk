import { describe, expect, it } from 'vitest'
import type { CampaignDefinition } from '../api/types'
import { defaultPolicy, emptyDefinition } from './defaults'
import { summarizeCampaign } from './summary'

function definition(patch: Partial<CampaignDefinition> = {}): CampaignDefinition {
  return {
    ...emptyDefinition(),
    name: 'Rotate store Wi-Fi credentials',
    selector: 'tenant == "acme-retail"',
    action: { kind: 'run_script', script: 'kvs set acme.wifi.profile 2026-10' },
    ...patch,
  }
}

describe('summarizeCampaign', () => {
  it('reads like the specification example for the defaults', () => {
    const policy = defaultPolicy('run_script')
    policy.gates.breakdown = ['os_build']
    expect(summarizeCampaign(definition({ policy }), 12408).headline).toBe(
      'Run on 12,408 nodes in 4 phases (1% → 10% → 50% → 100%), at most 50 nodes/s, pausing if more than 5% fail in any OS build.',
    )
  })

  it('lists every breakdown field', () => {
    expect(summarizeCampaign(definition(), 3).headline).toContain(
      'in any OS build, hardware class, dusk version or country.',
    )
  })

  it('says overall when nothing is broken down, and aborting when the policy aborts', () => {
    const policy = defaultPolicy('run_script')
    policy.gates.breakdown = []
    policy.abort.on_gate_failure = 'abort'
    expect(summarizeCampaign(definition({ policy }), 1).headline).toBe(
      'Run on 1 node in 4 phases (1% → 10% → 50% → 100%), at most 50 nodes/s, aborting if more than 5% fail overall.',
    )
  })

  it('names the version for ensure_version and keeps enforcing it', () => {
    const policy = defaultPolicy('ensure_version')
    policy.phases = [{ name: 'all', percent: 100, bake_seconds: 3600 }]
    policy.rate.per_second = 1
    const summary = summarizeCampaign(
      definition({
        policy,
        action: {
          kind: 'ensure_version',
          version: '0.2.1',
          version_key: 'dusk.version',
          script: 'x',
        },
      }),
      null,
    )
    expect(summary.headline).toBe(
      'Bring the matching nodes to dusk.version 0.2.1 all at once, at most 1 node/s, pausing if more than 5% fail in any OS build, hardware class, dusk version or country.',
    )
    expect(summary.details).toContain(
      'After the last phase passes it keeps enforcing the desired state on matching nodes until you complete or abort it.',
    )
    expect(summary.details).toContain(
      'Nodes that report a failure are retried up to 4 more times, backing off from 1m to 1h.',
    )
    expect(summary.details).toContain(
      'A node with no result 15m after delivery is asked again under the same pid when it is next seen; the script does not run twice, and the state the node reports is read.',
    )
  })

  it('says every node for an empty selector', () => {
    expect(summarizeCampaign(definition({ selector: ' ' }), 6000).headline).toMatch(
      /^Run on all 6,000 nodes in 4 phases/,
    )
    expect(summarizeCampaign(definition({ selector: '' }), null).headline).toMatch(
      /^Run on every node in 4 phases/,
    )
  })

  it('pauses at the first failure when no failures are allowed', () => {
    const policy = defaultPolicy('run_script')
    policy.gates.max_failure_rate = 0
    expect(summarizeCampaign(definition({ policy }), 10).headline).toMatch(
      /, pausing at the first failure\.$/,
    )
  })

  it('explains one-shot timeouts, files, logs and the failure cap', () => {
    const policy = defaultPolicy('run_script')
    policy.abort.max_total_failures = 25
    const summary = summarizeCampaign(
      definition({
        policy,
        action: {
          kind: 'run_script',
          script: 'echo',
          collect_files: ['/var/log/acme/pos.log'],
          stream_logs: { level: 'debug', duration_seconds: 900 },
        },
      }),
      40,
    )
    expect(summary.details).toEqual([
      'Each phase waits for at least 10 results and bakes 1h, 2h, 4h and 4h before the next opens.',
      'Also pauses if more than 5% of nodes go silent within 10m of reporting success.',
      'Nodes that report a failure are not retried.',
      'A node with no result 15m after delivery is marked unknown for you to resolve; it is never rerun on its own.',
      'Pauses once more than 25 nodes have failed.',
      'Collects 1 file from each node: /var/log/acme/pos.log.',
      'Streams debug logs from each node for 15m.',
    ])
  })

  it('names the minimum sample when the gates need one, and aborts on the cap when told to', () => {
    const policy = defaultPolicy('run_script')
    policy.gates.min_sample = 50
    policy.abort.on_gate_failure = 'abort'
    policy.abort.max_total_failures = 1
    const summary = summarizeCampaign(definition({ policy }), 40)
    expect(summary.details).toContain(
      'Each phase waits for at least 50 results and bakes 1h, 2h, 4h and 4h before the next opens.',
    )
    expect(summary.details).toContain('Aborts once more than 1 node has failed.')
  })

  it('describes the quarantine script rule', () => {
    const lenient = summarizeCampaign(
      definition({
        action: { kind: 'quarantine', script: 'kvs set x 1', require_script_success: false },
      }),
      2,
    )
    expect(lenient.headline).toMatch(/^Quarantine 2 nodes/)
    expect(lenient.details).toContain(
      'Runs the script first on nodes that are online; quarantines every node either way.',
    )
    const strict = summarizeCampaign(
      definition({
        action: { kind: 'quarantine', script: 'kvs set x 1', require_script_success: true },
      }),
      2,
    )
    expect(strict.details).toContain(
      'Runs the script first and quarantines only the nodes where it succeeds.',
    )
  })
})
