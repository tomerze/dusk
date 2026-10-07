import type { BreakdownField, CampaignDefinition } from '../api/types'
import {
  formatAbsolute,
  formatDuration,
  formatNumber,
  formatPercent,
  joinWords,
  middleEllipsis,
  plural,
} from '../format'

export const breakdownLabels: Record<BreakdownField, string> = {
  os_build: 'OS build',
  hardware_class: 'hardware class',
  dusk_version: 'dusk version',
  country: 'country',
}

export interface CampaignSummary {
  headline: string
  details: string[]
}

function headlineAction(definition: CampaignDefinition, matched: number | null): string {
  const everyNode = definition.selector.trim().length === 0
  const target = () => {
    if (matched === null) {
      return everyNode ? 'every node' : 'the matching nodes'
    }
    return everyNode ? `all ${plural(matched, 'node')}` : plural(matched, 'node')
  }
  const action = definition.action
  switch (action.kind) {
    case 'run_script':
      return `Run on ${target()}`
    case 'ensure_version':
      return `Bring ${target()} to ${action.version_key} ${action.version}`
    case 'ensure_config':
      return `Apply config ${middleEllipsis(action.config_hash, 12)} on ${target()}`
    case 'quarantine':
      return `Quarantine ${target()}`
  }
}

function sameValues(values: number[]): boolean {
  return values.every((value) => value === values[0])
}

export function summarizeCampaign(
  definition: CampaignDefinition,
  matched: number | null,
): CampaignSummary {
  const { policy, action } = definition
  const phases = policy.phases
  const phasePart =
    phases.length <= 1
      ? 'all at once'
      : `in ${phases.length} phases (${phases.map((phase) => `${phase.percent}%`).join(' → ')})`
  const ratePart = `at most ${formatNumber(policy.rate.per_second)} ${policy.rate.per_second === 1 ? 'node' : 'nodes'}/s`
  const verb = policy.abort.on_gate_failure === 'pause' ? 'pausing' : 'aborting'
  const fields = policy.gates.breakdown.map((field) => breakdownLabels[field])
  const scope = fields.length === 0 ? 'overall' : `in any ${joinWords(fields, 'or')}`
  const gatePart =
    policy.gates.max_failure_rate === 0
      ? `${verb} at the first failure`
      : `${verb} if more than ${formatPercent(policy.gates.max_failure_rate)} fail ${scope}`
  const headline = `${headlineAction(definition, matched)} ${phasePart}, ${ratePart}, ${gatePart}.`

  const details: string[] = []
  const bakes = phases.map((phase) => phase.bake_seconds)
  if (phases.length > 0) {
    const bakeText = sameValues(bakes)
      ? `bakes ${formatDuration(bakes[0] ?? 0)}`
      : `bakes ${joinWords(bakes.map(formatDuration), 'and')}`
    const sample =
      policy.gates.min_sample > 0
        ? `waits for at least ${plural(policy.gates.min_sample, 'result')} and ${bakeText}`
        : bakeText
    details.push(
      phases.length === 1
        ? `It ${sample} before it can finish.`
        : `Each phase ${sample} before the next opens.`,
    )
  }
  details.push(
    `Also ${verb === 'pausing' ? 'pauses' : 'aborts'} if more than ${formatPercent(policy.gates.max_silent_rate)} of nodes go silent within ${formatDuration(policy.gates.silent_window_seconds)} of reporting success.`,
  )
  const oneShot = action.kind === 'run_script' || action.kind === 'quarantine'
  if (policy.retry.max_attempts <= 1) {
    details.push('Nodes that report a failure are not retried.')
  } else {
    details.push(
      `Nodes that report a failure are retried up to ${plural(policy.retry.max_attempts - 1, 'more time')}, backing off from ${formatDuration(policy.retry.initial_backoff_seconds)} to ${formatDuration(policy.retry.max_backoff_seconds)}.`,
    )
  }
  details.push(
    oneShot
      ? `A node with no result ${formatDuration(policy.node_timeout_seconds)} after delivery is marked unknown for you to resolve; it is never rerun on its own.`
      : `A node with no result ${formatDuration(policy.node_timeout_seconds)} after delivery is asked again under the same pid when it is next seen; the script does not run twice, and the state the node reports is read.`,
  )
  if (policy.abort.max_total_failures !== null) {
    details.push(
      `${verb === 'pausing' ? 'Pauses' : 'Aborts'} once more than ${plural(policy.abort.max_total_failures, 'node')} ${policy.abort.max_total_failures === 1 ? 'has' : 'have'} failed.`,
    )
  }
  if (action.kind === 'run_script' && (action.collect_files?.length ?? 0) > 0) {
    const files = action.collect_files ?? []
    details.push(`Collects ${plural(files.length, 'file')} from each node: ${files.join(', ')}.`)
  }
  if (action.kind === 'run_script' && action.stream_logs) {
    details.push(
      `Streams ${action.stream_logs.level} logs from each node for ${formatDuration(action.stream_logs.duration_seconds)}.`,
    )
  }
  if (action.kind === 'quarantine' && (action.script ?? '').trim().length > 0) {
    details.push(
      action.require_script_success
        ? 'Runs the script first and quarantines only the nodes where it succeeds.'
        : 'Runs the script first on nodes that are online; quarantines every node either way.',
    )
  }
  if (action.kind === 'ensure_version' || action.kind === 'ensure_config') {
    details.push(
      'After the last phase passes it keeps enforcing the desired state on matching nodes until you complete or abort it.',
    )
  }
  if (policy.deadline !== null) {
    details.push(
      `Stops at ${formatAbsolute(policy.deadline)}; nodes that are not in flight then are cancelled.`,
    )
  }
  if (policy.allow_overlap) {
    details.push('May start while another campaign for the same key targets some of these nodes.')
  }
  return { headline, details }
}
