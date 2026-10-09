import { describe, expect, it } from 'vitest'
import type { CampaignDefinition } from '../api/types'
import { defaultAction, defaultPolicy, emptyDefinition } from './defaults'
import { issueAt, validateDefinition, validatePhases, validatePolicy } from './validation'

function definition(patch: Partial<CampaignDefinition> = {}): CampaignDefinition {
  return {
    ...emptyDefinition(),
    name: 'Rotate store Wi-Fi credentials',
    selector: 'tenant == "acme-retail"',
    action: { kind: 'run_script', script: 'kvs set acme.wifi.profile 2026-10' },
    ...patch,
  }
}

describe('validatePhases', () => {
  it('accepts the default phases', () => {
    expect(validatePhases(defaultPolicy('run_script').phases, 1800, 900)).toEqual([])
  })

  it('requires at least one phase', () => {
    expect(validatePhases([], 1800, 900)).toEqual([
      { path: 'phases', message: 'Add at least one phase.' },
    ])
  })

  it('requires each phase to cover more than the one before it', () => {
    const issues = validatePhases(
      [
        { name: 'canary', percent: 10, bake_seconds: 3600 },
        { name: 'early', percent: 10, bake_seconds: 3600 },
        { name: 'all', percent: 100, bake_seconds: 3600 },
      ],
      1800,
      900,
    )
    expect(issues).toEqual([
      {
        path: 'phases.1.percent',
        message: 'Phases are cumulative: cover more than the 10% before this one.',
      },
    ])
  })

  it('keeps comparing against the highest share seen so far', () => {
    const issues = validatePhases(
      [
        { name: 'one', percent: 50, bake_seconds: 3600 },
        { name: 'two', percent: 20, bake_seconds: 3600 },
        { name: 'three', percent: 40, bake_seconds: 3600 },
        { name: 'four', percent: 100, bake_seconds: 3600 },
      ],
      1800,
      900,
    )
    expect(issues.map((issue) => issue.path)).toEqual(['phases.1.percent', 'phases.2.percent'])
    expect(issues[1]?.message).toContain('50%')
  })

  it('requires the last phase to reach 100%', () => {
    const issues = validatePhases([{ name: 'all', percent: 90, bake_seconds: 3600 }], 1800, 900)
    expect(issueAt(issues, 'phases.0.percent')).toBe(
      'The last phase must reach 100% of the matched nodes.',
    )
  })

  it('refuses shares outside 0 to 100', () => {
    const issues = validatePhases(
      [
        { name: 'none', percent: 0, bake_seconds: 3600 },
        { name: 'too many', percent: 120, bake_seconds: 3600 },
      ],
      1800,
      900,
    )
    expect(issueAt(issues, 'phases.0.percent')).toBe('Enter a share above 0% and at most 100%.')
    expect(issueAt(issues, 'phases.1.percent')).toBe('Enter a share above 0% and at most 100%.')
  })

  it('requires the bake to cover the longer of the silent window and the node timeout', () => {
    const issues = validatePhases([{ name: 'all', percent: 100, bake_seconds: 600 }], 1800, 900)
    expect(issueAt(issues, 'phases.0.bake_seconds')).toBe('Bake at least 30m, the silent window.')
    const timeout = validatePhases([{ name: 'all', percent: 100, bake_seconds: 600 }], 300, 900)
    expect(issueAt(timeout, 'phases.0.bake_seconds')).toBe('Bake at least 15m, the node timeout.')
  })

  it('requires distinct, non-empty names', () => {
    const issues = validatePhases(
      [
        { name: ' ', percent: 10, bake_seconds: 3600 },
        { name: 'all', percent: 50, bake_seconds: 3600 },
        { name: 'all', percent: 100, bake_seconds: 3600 },
      ],
      1800,
      900,
    )
    expect(issueAt(issues, 'phases.0.name')).toBe('Name the phase.')
    expect(issueAt(issues, 'phases.2.name')).toBe('Another phase is already named all.')
  })
})

describe('validatePolicy', () => {
  it('accepts the defaults for every action kind', () => {
    for (const kind of ['run_script', 'ensure_version', 'ensure_config', 'quarantine'] as const) {
      expect(validatePolicy(defaultPolicy(kind))).toEqual([])
    }
  })

  it('prefixes every path with policy', () => {
    const policy = defaultPolicy('run_script')
    policy.rate.per_second = 0
    policy.retry.max_backoff_seconds = 10
    policy.phases = []
    expect(validatePolicy(policy).map((issue) => issue.path)).toEqual([
      'policy.rate.per_second',
      'policy.retry.max_backoff_seconds',
      'policy.phases',
    ])
  })

  it('checks rates are fractions', () => {
    const policy = defaultPolicy('run_script')
    policy.gates.max_failure_rate = 1.5
    expect(issueAt(validatePolicy(policy), 'policy.gates.max_failure_rate')).toBe(
      'Enter a failure rate from 0% to 100%.',
    )
  })

  it('checks the deadline parses and lies ahead', () => {
    const policy = defaultPolicy('run_script')
    policy.deadline = 'next tuesday'
    expect(issueAt(validatePolicy(policy), 'policy.deadline')).toBe('Enter a valid date and time.')
    const now = Date.parse('2026-10-09T12:00:00Z')
    policy.deadline = '2026-10-09T11:59:00Z'
    expect(issueAt(validatePolicy(policy, now), 'policy.deadline')).toBe(
      'Pick a deadline in the future.',
    )
    policy.deadline = '2026-10-10T12:00:00Z'
    expect(validatePolicy(policy, now)).toEqual([])
  })

  it('holds the limits twilight enforces', () => {
    const policy = defaultPolicy('run_script')
    policy.rate.per_second = 100_001
    policy.rate.burst = 0
    policy.gates.min_sample = 0
    policy.gates.silent_window_seconds = 0
    policy.node_timeout_seconds = 8 * 86400
    expect(
      validatePolicy(policy)
        .map((issue) => issue.path)
        .filter((path) => !path.startsWith('policy.phases')),
    ).toEqual([
      'policy.rate.per_second',
      'policy.rate.burst',
      'policy.gates.min_sample',
      'policy.node_timeout_seconds',
    ])
  })

  it('holds the retry limits twilight enforces', () => {
    const within = defaultPolicy('ensure_version')
    within.retry = {
      max_attempts: 100,
      initial_backoff_seconds: 86400,
      max_backoff_seconds: 7 * 86400,
      multiplier: 10,
    }
    expect(validatePolicy(within).filter((issue) => issue.path.startsWith('policy.retry'))).toEqual(
      [],
    )
    const beyond = defaultPolicy('ensure_version')
    beyond.retry = {
      max_attempts: 101,
      initial_backoff_seconds: 1.5,
      max_backoff_seconds: 7 * 86400 + 1,
      multiplier: 10.5,
    }
    expect(
      validatePolicy(beyond)
        .filter((issue) => issue.path.startsWith('policy.retry'))
        .map((issue) => issue.path),
    ).toEqual([
      'policy.retry.max_attempts',
      'policy.retry.initial_backoff_seconds',
      'policy.retry.max_backoff_seconds',
      'policy.retry.multiplier',
    ])
    const backwards = defaultPolicy('ensure_version')
    backwards.retry.initial_backoff_seconds = 600
    backwards.retry.max_backoff_seconds = 300
    expect(issueAt(validatePolicy(backwards), 'policy.retry.max_backoff_seconds')).toBe(
      'The longest backoff is a whole number of seconds, at least the first and at most 7 days.',
    )
  })

  it('refuses more than 20 phases and more than three decimals', () => {
    const many = Array.from(Array(21).keys(), (index) => ({
      name: `phase ${index}`,
      percent: index === 20 ? 100 : index + 1,
      bake_seconds: 3600,
    }))
    expect(validatePhases(many, 1800, 900)).toEqual([
      { path: 'phases', message: 'Use at most 20 phases.' },
    ])
    expect(
      issueAt(
        validatePhases(
          [
            { name: 'canary', percent: 0.0005, bake_seconds: 3600 },
            { name: 'all', percent: 100, bake_seconds: 3600 },
          ],
          1800,
          900,
        ),
        'phases.0.percent',
      ),
    ).toBe('Use at most three decimal places.')
  })
})

describe('validateDefinition', () => {
  it('accepts a complete run_script campaign', () => {
    expect(validateDefinition(definition())).toEqual([])
  })

  it('refuses an empty selector and names the whole-fleet form', () => {
    expect(validateDefinition(definition({ selector: ' ' }))).toEqual([
      {
        path: 'selector',
        message: 'Say which nodes it targets. For every node, use has(device_id).',
      },
    ])
    expect(validateDefinition(definition({ selector: 'has(device_id)' }))).toEqual([])
  })

  it('requires a name and a script', () => {
    const issues = validateDefinition(
      definition({ name: '  ', action: { kind: 'run_script', script: ' ' } }),
    )
    expect(issueAt(issues, 'name')).toBe('Name the campaign.')
    expect(issueAt(issues, 'action.script')).toBe('Write the script the nodes run.')
  })

  it('lets a quarantine run without a script unless it must succeed', () => {
    const quarantine = defaultAction('quarantine')
    expect(validateDefinition(definition({ action: quarantine }))).toEqual([])
    const strict = { ...quarantine, require_script_success: true }
    expect(issueAt(validateDefinition(definition({ action: strict })), 'action.script')).toBe(
      'Requiring the script to succeed needs a script.',
    )
  })

  it('requires a version and its key for ensure_version', () => {
    const issues = validateDefinition(
      definition({
        action: { kind: 'ensure_version', version: '', version_key: '', script: 'kvs get dusk' },
      }),
    )
    expect(issueAt(issues, 'action.version')).toBe('Enter the version the nodes should report.')
    expect(issueAt(issues, 'action.version_key')).toBe('Name the key that reports the version.')
  })

  it('accepts versions that are not semver', () => {
    const action = {
      kind: 'ensure_version' as const,
      version: '24B83',
      version_key: 'acme.build',
      script: 'x',
    }
    expect(validateDefinition(definition({ action }))).toEqual([])
  })

  it('caps collected files and requires absolute paths', () => {
    const files = Array.from(Array(17).keys(), (index) => `/var/log/${index}.log`)
    const many = validateDefinition(
      definition({ action: { kind: 'run_script', script: 'echo', collect_files: files } }),
    )
    expect(issueAt(many, 'action.collect_files')).toBe('Collect at most 16 files per node.')
    const relative = validateDefinition(
      definition({
        action: {
          kind: 'run_script',
          script: 'echo',
          collect_files: ['logs/pos.log', 'C:\\ProgramData\\pos.log', '/var/log/pos.log'],
        },
      }),
    )
    expect(relative).toEqual([
      { path: 'action.collect_files.0', message: 'Use an absolute path on the node.' },
    ])
    const twice = validateDefinition(
      definition({
        action: {
          kind: 'run_script',
          script: 'echo',
          collect_files: ['/var/log/pos.log', '/var/log/pos.log'],
        },
      }),
    )
    expect(twice).toEqual([
      { path: 'action.collect_files.1', message: 'This path is already listed.' },
    ])
  })
})
