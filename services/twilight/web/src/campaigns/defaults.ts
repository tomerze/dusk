import type { Action, ActionKind, CampaignDefinition, Policy } from '../api/types'

const hour = 3600

export function defaultPolicy(kind: ActionKind): Policy {
  return {
    rate: { per_second: 50, burst: 100 },
    phases: [
      { name: 'canary', percent: 1, bake_seconds: hour },
      { name: 'early', percent: 10, bake_seconds: 2 * hour },
      { name: 'half', percent: 50, bake_seconds: 4 * hour },
      { name: 'all', percent: 100, bake_seconds: 4 * hour },
    ],
    gates: {
      min_sample: 10,
      max_failure_rate: 0.05,
      max_silent_rate: 0.05,
      silent_window_seconds: 600,
      breakdown: ['os_build', 'hardware_class', 'dusk_version', 'country'],
    },
    abort: { on_gate_failure: 'pause', max_total_failures: null },
    node_timeout_seconds: 15 * 60,
    retry: {
      max_attempts: kind === 'run_script' || kind === 'quarantine' ? 1 : 5,
      initial_backoff_seconds: 60,
      max_backoff_seconds: hour,
      multiplier: 2,
    },
    deadline: null,
    allow_overlap: false,
  }
}

export function defaultAction(kind: ActionKind): Action {
  switch (kind) {
    case 'run_script':
      return { kind, script: '', collect_files: [], stream_logs: null }
    case 'ensure_version':
      return { kind, version: '', version_key: 'dusk.version', script: '' }
    case 'ensure_config':
      return { kind, config_hash: '', script: '' }
    case 'quarantine':
      return { kind, script: '', require_script_success: false }
  }
}

export function emptyDefinition(): CampaignDefinition {
  return {
    name: '',
    description: '',
    selector: '',
    action: defaultAction('run_script'),
    policy: defaultPolicy('run_script'),
  }
}
