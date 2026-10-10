import type { CampaignDefinition, Phase, Policy } from '../api/types'
import { formatDuration } from '../format'

export interface Issue {
  path: string
  message: string
}

export function processTimeoutSeconds({
  action,
  policy,
}: Pick<CampaignDefinition, 'action' | 'policy'>): number {
  if (action.kind !== 'run_script') {
    return policy.node_timeout_seconds
  }
  return (
    policy.node_timeout_seconds * (1 + (action.collect_files ?? []).length) +
    (action.stream_logs?.duration_seconds ?? 0)
  )
}

export function validatePhases(
  phases: Phase[],
  silentWindowSeconds: number,
  nodeTimeoutSeconds: number,
): Issue[] {
  if (phases.length === 0) {
    return [{ path: 'phases', message: 'Add at least one phase.' }]
  }
  if (phases.length > maximumPhases) {
    return [{ path: 'phases', message: `Use at most ${maximumPhases} phases.` }]
  }
  const issues: Issue[] = []
  const minimumBake = Math.max(silentWindowSeconds, nodeTimeoutSeconds)
  const seen = new Set<string>()
  let previous = 0
  phases.forEach((phase, index) => {
    const name = phase.name.trim()
    if (name.length === 0) {
      issues.push({ path: `phases.${index}.name`, message: 'Name the phase.' })
    } else if (name.length > 100) {
      issues.push({ path: `phases.${index}.name`, message: 'Keep the name to 100 characters.' })
    } else if (seen.has(name)) {
      issues.push({
        path: `phases.${index}.name`,
        message: `Another phase is already named ${name}.`,
      })
    }
    seen.add(name)
    if (!Number.isFinite(phase.percent) || phase.percent <= 0 || phase.percent > 100) {
      issues.push({
        path: `phases.${index}.percent`,
        message: 'Enter a share above 0% and at most 100%.',
      })
    } else if (Math.round(phase.percent * 1000) !== phase.percent * 1000) {
      issues.push({
        path: `phases.${index}.percent`,
        message: 'Use at most three decimal places.',
      })
    } else if (phase.percent <= previous) {
      issues.push({
        path: `phases.${index}.percent`,
        message: `Phases are cumulative: cover more than the ${previous}% before this one.`,
      })
    } else if (index === phases.length - 1 && phase.percent !== 100) {
      issues.push({
        path: `phases.${index}.percent`,
        message: 'The last phase must reach 100% of the matched nodes.',
      })
    }
    if (Number.isFinite(phase.percent)) {
      previous = Math.max(previous, phase.percent)
    }
    if (!Number.isInteger(phase.bake_seconds) || phase.bake_seconds < 0) {
      issues.push({
        path: `phases.${index}.bake_seconds`,
        message: 'Enter a bake time in whole seconds.',
      })
    } else if (phase.bake_seconds > 30 * 86400) {
      issues.push({
        path: `phases.${index}.bake_seconds`,
        message: 'Bake for at most 30 days.',
      })
    } else if (phase.bake_seconds < minimumBake) {
      const limit = silentWindowSeconds >= nodeTimeoutSeconds ? 'silent window' : 'node timeout'
      issues.push({
        path: `phases.${index}.bake_seconds`,
        message: `Bake at least ${formatDuration(minimumBake)}, the ${limit}.`,
      })
    }
  })
  return issues
}

const maximumPhases = 20

const wholeAtLeast = (value: number, minimum: number) => Number.isInteger(value) && value >= minimum

const wholeBetween = (value: number, minimum: number, maximum: number) =>
  wholeAtLeast(value, minimum) && value <= maximum

const fraction = (value: number) => Number.isFinite(value) && value >= 0 && value <= 1

export function validatePolicy(policy: Policy, now = Date.now()): Issue[] {
  const issues: Issue[] = []
  const add = (path: string, message: string) => issues.push({ path: `policy.${path}`, message })
  if (
    !Number.isFinite(policy.rate.per_second) ||
    policy.rate.per_second <= 0 ||
    policy.rate.per_second > 100_000
  ) {
    add('rate.per_second', 'Dispatch more than 0 and at most 100,000 nodes per second.')
  }
  if (!wholeBetween(policy.rate.burst, 1, 1_000_000)) {
    add('rate.burst', 'Burst is a whole number from 1 to 1,000,000.')
  }
  if (!wholeBetween(policy.gates.min_sample, 1, 10_000_000)) {
    add('gates.min_sample', 'Enter a whole number of results, 1 or more.')
  }
  if (!fraction(policy.gates.max_failure_rate)) {
    add('gates.max_failure_rate', 'Enter a failure rate from 0% to 100%.')
  }
  if (!fraction(policy.gates.max_silent_rate)) {
    add('gates.max_silent_rate', 'Enter a silent rate from 0% to 100%.')
  }
  if (!wholeBetween(policy.gates.silent_window_seconds, 0, 7 * 86400)) {
    add('gates.silent_window_seconds', 'Enter a silent window in whole seconds, at most 7 days.')
  }
  if (!wholeBetween(policy.node_timeout_seconds, 1, 7 * 86400)) {
    add('node_timeout_seconds', 'Enter a node timeout in whole seconds, at most 7 days.')
  }
  if (!wholeBetween(policy.retry.max_attempts, 1, 100)) {
    add('retry.max_attempts', 'Allow 1 to 100 attempts.')
  }
  if (!wholeBetween(policy.retry.initial_backoff_seconds, 1, 86400)) {
    add('retry.initial_backoff_seconds', 'Back off first for a whole number of seconds, 1s to 24h.')
  }
  if (
    !wholeBetween(policy.retry.max_backoff_seconds, 1, 7 * 86400) ||
    policy.retry.max_backoff_seconds < policy.retry.initial_backoff_seconds
  ) {
    add(
      'retry.max_backoff_seconds',
      'The longest backoff is a whole number of seconds, at least the first and at most 7 days.',
    )
  }
  if (
    !Number.isFinite(policy.retry.multiplier) ||
    policy.retry.multiplier < 1 ||
    policy.retry.multiplier > 10
  ) {
    add('retry.multiplier', 'Use a multiplier from 1 to 10.')
  }
  if (
    policy.abort.max_total_failures !== null &&
    !wholeAtLeast(policy.abort.max_total_failures, 1)
  ) {
    add('abort.max_total_failures', 'Enter a whole number of failures, at least 1.')
  }
  if (policy.deadline !== null && Number.isNaN(Date.parse(policy.deadline))) {
    add('deadline', 'Enter a valid date and time.')
  } else if (policy.deadline !== null && Date.parse(policy.deadline) <= now) {
    add('deadline', 'Pick a deadline in the future.')
  }
  for (const issue of validatePhases(
    policy.phases,
    policy.gates.silent_window_seconds,
    policy.node_timeout_seconds,
  )) {
    add(issue.path, issue.message)
  }
  return issues
}

export const maximumCollectedFiles = 16

export function validateDefinition(definition: CampaignDefinition, now = Date.now()): Issue[] {
  const issues: Issue[] = []
  const add = (path: string, message: string) => issues.push({ path, message })
  if (definition.name.trim().length === 0) {
    add('name', 'Name the campaign.')
  } else if (definition.name.length > 200) {
    add('name', 'Keep the name to 200 characters.')
  }
  if (definition.description.length > 10_000) {
    add('description', 'Keep the description to 10,000 characters.')
  }
  if (definition.selector.trim().length === 0) {
    add('selector', 'Say which nodes it targets. For every node, use has(device_id).')
  }
  const action = definition.action
  if (action.kind !== 'quarantine' && action.script.trim().length === 0) {
    add('action.script', 'Write the script the nodes run.')
  }
  if (
    action.kind === 'quarantine' &&
    action.require_script_success &&
    (action.script ?? '').trim().length === 0
  ) {
    add('action.script', 'Requiring the script to succeed needs a script.')
  }
  if (action.kind === 'ensure_version') {
    if (action.version.trim().length === 0) {
      add('action.version', 'Enter the version the nodes should report.')
    }
    if (action.version_key.trim().length === 0) {
      add('action.version_key', 'Name the key that reports the version.')
    }
  }
  if (action.kind === 'ensure_config' && !/^\S+$/.test(action.config_hash)) {
    add('action.config_hash', 'Enter the config hash the nodes should report, without spaces.')
  }
  if (action.kind === 'run_script') {
    const files = action.collect_files ?? []
    if (files.length > maximumCollectedFiles) {
      add('action.collect_files', `Collect at most ${maximumCollectedFiles} files per node.`)
    }
    files.forEach((file, index) => {
      if (!file.startsWith('/') && !/^[A-Za-z]:[\\/]/.test(file)) {
        add(`action.collect_files.${index}`, 'Use an absolute path on the node.')
      } else if (files.indexOf(file) !== index) {
        add(`action.collect_files.${index}`, 'This path is already listed.')
      }
    })
    if (action.stream_logs !== null && action.stream_logs !== undefined) {
      const duration = action.stream_logs.duration_seconds
      if (!wholeBetween(duration, 1, 86400)) {
        add('action.stream_logs.duration_seconds', 'Stream logs for 1s to 24h.')
      }
    }
  }
  issues.push(...validatePolicy(definition.policy, now))
  return issues
}

export function issuesUnder(issues: Issue[], prefix: string): Issue[] {
  return issues.filter((issue) => issue.path === prefix || issue.path.startsWith(`${prefix}.`))
}

export function issueAt(issues: Issue[], path: string): string | undefined {
  return issues.find((issue) => issue.path === path)?.message
}
