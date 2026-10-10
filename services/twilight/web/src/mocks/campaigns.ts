import type {
  Action,
  Campaign,
  CampaignEvent,
  CampaignNode,
  CampaignStatus,
  Counter,
  Gates,
  JsonValue,
  NodeState,
  PauseKind,
  Policy,
  Tally,
} from '../api/types'
import { defaultPolicy } from '../campaigns/defaults'
import { bakeAccruedSeconds } from '../campaigns/detail/phases'
import type { MockNode } from './nodes'
import { selectableFields } from './nodes'
import { bucketOf, Random } from './random'
import { evaluate, parseSelector } from './selector'

export interface MockCampaign {
  campaign: Omit<Campaign, 'counters'>
  salt: string
  rows: CampaignNode[]
  events: CampaignEvent[]
}

const hour = 3600

type Mix = 'progressing' | 'finished' | 'aborted' | 'policy_failed' | 'sampling' | 'gate_failing'

interface Scenario {
  id: string
  name: string
  description: string
  selector: string
  action: Action
  status: CampaignStatus
  currentPhase: number
  startedHoursAgo: number | null
  phaseOpenedMinutesAgo: number
  createdBy: string
  policy?: (policy: Policy) => Policy
  pause?: { kind: PauseKind; reason: string | null; actor: string; minutesAgo: number }
  abortReason?: string
  finishedHoursAgo?: number
  mix: Mix
}

const updateScript = `cp :/mnt/updates/dusk-node-0.2.1 :/var/lib/dusk/dusk-node.next
kvs set dusk.update.pending 0.2.1`

export const scenarios: readonly Scenario[] = [
  {
    id: '01929b3e-7c4a-7d1e-9f3a-5b8c2d4e6f01',
    name: 'Dusk 0.2.1 to retail stores',
    description: 'Moves every active Acme store device to dusk 0.2.1 before the holiday freeze.',
    selector: 'tenant == "acme-retail" and lifecycle == "active"',
    action: {
      kind: 'ensure_version',
      version: '0.2.1',
      version_key: 'dusk.version',
      script: updateScript,
    },
    status: 'running',
    currentPhase: 2,
    startedHoursAgo: 7,
    phaseOpenedMinutesAgo: 82,
    createdBy: 'release-bot',
    mix: 'progressing',
  },
  {
    id: '01929b41-02aa-7b6e-8d11-0c7f3e9a2b02',
    name: 'Printer profile v7 on Windows registers',
    description: 'Switches store and depot Windows machines to receipt printer profile v7.',
    selector: 'os_name == "windows" and tenant in ["acme-retail", "northwind-logistics"]',
    action: {
      kind: 'run_script',
      script:
        'kvs set acme.printer.profile v7\ncp :C:/ProgramData/Acme/printer/v7.json :C:/ProgramData/Acme/printer/active.json',
    },
    status: 'paused',
    currentPhase: 1,
    startedHoursAgo: 3,
    phaseOpenedMinutesAgo: 64,
    createdBy: 'windows-platform',
    policy: (policy) => ({
      ...policy,
      gates: { ...policy.gates, min_sample: 20, breakdown: ['os_build', 'hardware_class'] },
    }),
    pause: { kind: 'gate', reason: null, actor: 'twilight', minutesAgo: 18 },
    mix: 'gate_failing',
  },
  {
    id: '01929a77-5d10-7e44-a2c9-1f6b0e8d3c03',
    name: 'Telemetry exporter config v14',
    description: 'Points every Linux node at the regional OTLP collectors.',
    selector: 'lifecycle == "active" and impl == "nix"',
    action: {
      kind: 'ensure_config',
      config_hash: 'b41d9a0c3e5f7a2d6c8e1f0a9b7c5d3e2f1a0b9c8d7e6f5a4b3c2d1e0f9a8b7c',
      script:
        'kvs set logs.export.endpoint otlp://collector.eu.internal:4318\nkvs set dusk.config.hash b41d9a0c3e5f7a2d6c8e1f0a9b7c5d3e2f1a0b9c8d7e6f5a4b3c2d1e0f9a8b7c',
    },
    status: 'running',
    currentPhase: 3,
    startedHoursAgo: 52,
    phaseOpenedMinutesAgo: 600,
    createdBy: 'observability',
    mix: 'finished',
  },
  {
    id: '01929c02-9e3b-7a55-b6d0-4c2e8f1a7d04',
    name: 'Restart stale shell servers',
    description: 'Kills shell servers older than a week on depot scanners.',
    selector: 'tenant == "northwind-logistics" and os_name == "android"',
    action: { kind: 'run_script', script: 'ps\nkill 7' },
    status: 'running',
    currentPhase: 0,
    startedHoursAgo: 0.4,
    phaseOpenedMinutesAgo: 24,
    createdBy: 'depot-ops',
    policy: (policy) => ({
      ...policy,
      phases: [
        { name: 'canary', percent: 20, bake_seconds: hour },
        { name: 'all', percent: 100, bake_seconds: 2 * hour },
      ],
    }),
    mix: 'sampling',
  },
  {
    id: '01929c11-1a2b-7c3d-8e4f-5a6b7c8d9e05',
    name: 'Dusk 0.2.2-rc.1 beta ring',
    description: 'Release candidate for stores enrolled in the beta ring.',
    selector: 'tenant == "acme-retail" and facts["acme.ring"] == "beta"',
    action: {
      kind: 'ensure_version',
      version: '0.2.2-rc.1',
      version_key: 'dusk.version',
      script:
        'cp :/mnt/updates/dusk-node-0.2.2-rc.1 :/var/lib/dusk/dusk-node.next\nkvs set dusk.update.pending 0.2.2-rc.1',
    },
    status: 'running',
    currentPhase: 0,
    startedHoursAgo: 5,
    phaseOpenedMinutesAgo: 290,
    createdBy: 'release-bot',
    policy: (policy) => ({
      ...policy,
      phases: [{ name: 'ring', percent: 100, bake_seconds: 6 * hour }],
      allow_overlap: true,
    }),
    mix: 'progressing',
  },
  {
    id: '01929a10-4b5c-7d6e-9f01-2a3b4c5d6e06',
    name: 'Northwind fleet dusk 0.2.1',
    description: 'Same release for Northwind trucks and depots.',
    selector: 'tenant == "northwind-logistics" and lifecycle == "active"',
    action: {
      kind: 'ensure_version',
      version: '0.2.1',
      version_key: 'dusk.version',
      script: updateScript,
    },
    status: 'paused',
    currentPhase: 1,
    startedHoursAgo: 30,
    phaseOpenedMinutesAgo: 900,
    createdBy: 'depot-ops',
    pause: {
      kind: 'operator',
      reason: 'Holding for the depot change freeze until Monday',
      actor: 'depot-ops',
      minutesAgo: 340,
    },
    mix: 'progressing',
  },
  {
    id: '01929d00-6f7a-7b8c-9d0e-1f2a3b4c5d07',
    name: 'Debug logging for EU stores',
    description:
      'Streams debug logs from EU stores for fifteen minutes while support reproduces a printer issue.',
    selector: 'country in ["DE", "FR", "NL", "ES", "IT", "SE"] and tenant == "acme-retail"',
    action: {
      kind: 'run_script',
      script: 'kvs set logs.level debug',
      collect_files: [],
      stream_logs: { level: 'debug', duration_seconds: 900 },
    },
    status: 'draft',
    currentPhase: 0,
    startedHoursAgo: null,
    phaseOpenedMinutesAgo: 0,
    createdBy: 'support-eu',
    mix: 'progressing',
  },
  {
    id: '019299f0-7a8b-7c9d-8e0f-1a2b3c4d5e08',
    name: 'Collect POS crash dumps',
    description: 'Uploads crash dumps from registers still on POS 4.12.0.',
    selector: 'tenant == "acme-retail" and facts["acme.pos.version"] == "4.12.0"',
    action: {
      kind: 'run_script',
      script: 'echo collecting',
      collect_files: ['/var/log/acme/pos/crash.dmp', '/var/log/acme/pos/pos.log'],
      stream_logs: null,
    },
    status: 'completed',
    currentPhase: 3,
    startedHoursAgo: 96,
    phaseOpenedMinutesAgo: 4000,
    finishedHoursAgo: 70,
    createdBy: 'pos-team',
    mix: 'finished',
  },
  {
    id: '019299a0-8b9c-7d0e-9f1a-2b3c4d5e6f09',
    name: 'Quarantine cloned kiosk images',
    description: 'Quarantines devices booted from the leaked img-7f3a disk image.',
    selector: 'facts["acme.image_id"] == "img-7f3a"',
    action: {
      kind: 'quarantine',
      script: 'kvs set acme.kiosk.locked true',
      require_script_success: false,
    },
    status: 'completed',
    currentPhase: 0,
    startedHoursAgo: 150,
    phaseOpenedMinutesAgo: 9000,
    finishedHoursAgo: 149,
    createdBy: 'security',
    policy: (policy) => ({
      ...policy,
      phases: [{ name: 'all', percent: 100, bake_seconds: 1800 }],
      rate: { per_second: 200, burst: 200 },
    }),
    mix: 'finished',
  },
  {
    id: '01929950-9cad-7e0f-8a1b-3c4d5e6f7a10',
    name: 'Pilot log shipper 2',
    description: 'Replaces the vendor log shipper on contoso imaging hosts.',
    selector: 'tenant == "contoso-health" and os_name in ["ubuntu", "debian"]',
    action: {
      kind: 'run_script',
      script: 'sh -d "logs stream otlp://collector.internal:4318 -l info"',
    },
    status: 'aborted',
    currentPhase: 1,
    startedHoursAgo: 120,
    phaseOpenedMinutesAgo: 6900,
    finishedHoursAgo: 115,
    abortReason: 'Vendor pulled the release',
    createdBy: 'contoso-it',
    mix: 'aborted',
  },
  {
    id: '01929940-adbe-7f1a-9b2c-4d5e6f7a8b11',
    name: 'glibc compatibility shim',
    description: 'Installs the glibc 2.36 shim on older Debian and Ubuntu nodes.',
    selector: 'os_name in ["debian", "ubuntu"] and dusk_version < "0.2.0"',
    action: {
      kind: 'run_script',
      script: 'cp :/mnt/updates/glibc-shim.so :/usr/local/lib/glibc-shim.so',
    },
    status: 'failed',
    currentPhase: 1,
    startedHoursAgo: 200,
    phaseOpenedMinutesAgo: 11900,
    finishedHoursAgo: 198,
    createdBy: 'platform',
    policy: (policy) => ({
      ...policy,
      abort: { on_gate_failure: 'abort', max_total_failures: 200 },
    }),
    mix: 'policy_failed',
  },
  {
    id: '01929900-bedf-7a2b-8c3d-5e6f7a8b9c12',
    name: 'Rotate store Wi-Fi credentials',
    description: 'Writes the October Wi-Fi credentials into every store device.',
    selector: 'tenant == "acme-retail"',
    action: { kind: 'run_script', script: 'kvs set acme.wifi.profile 2026-10' },
    status: 'completed',
    currentPhase: 3,
    startedHoursAgo: 260,
    phaseOpenedMinutesAgo: 15000,
    finishedHoursAgo: 240,
    createdBy: 'store-network',
    mix: 'finished',
  },
  {
    id: '019298a0-cfe0-7b3c-9d4e-6f7a8b9c0d13',
    name: 'Dusk 0.1.1 security patch',
    description: 'Patched the shell parser issue fleet-wide.',
    selector: 'dusk_version < "0.1.1"',
    action: {
      kind: 'ensure_version',
      version: '0.1.1',
      version_key: 'dusk.version',
      script:
        'cp :/mnt/updates/dusk-node-0.1.1 :/var/lib/dusk/dusk-node.next\nkvs set dusk.update.pending 0.1.1',
    },
    status: 'archived',
    currentPhase: 3,
    startedHoursAgo: 900,
    phaseOpenedMinutesAgo: 50000,
    finishedHoursAgo: 800,
    createdBy: 'security',
    mix: 'finished',
  },
]

function stateFor(
  random: Random,
  mix: Scenario['mix'],
  phase: number,
  currentPhase: number,
  failing: boolean,
  kind: Action['kind'],
): NodeState {
  const ensure = kind === 'ensure_version' || kind === 'ensure_config'
  if (mix === 'finished') {
    return random.weighted<NodeState>([
      ['succeeded', 98.2],
      ['failed', 0.7],
      ['unknown', ensure ? 0 : 0.3],
      ['excluded', 0.8],
    ])
  }
  if (mix === 'aborted') {
    return random.weighted<NodeState>([
      ['succeeded', phase < currentPhase ? 90 : 30],
      ['failed', 6],
      ['cancelled', phase < currentPhase ? 0 : 60],
    ])
  }
  if (mix === 'policy_failed') {
    return random.weighted<NodeState>([
      ['succeeded', 45],
      ['failed', 30],
      ['cancelled', 25],
    ])
  }
  if (mix === 'sampling') {
    return random.weighted<NodeState>([
      ['succeeded', 28],
      ['delivered', 18],
      ['dispatched', 10],
      ['pending', 40],
      ['failed', 1],
    ])
  }
  if (mix === 'gate_failing' && failing) {
    return random.weighted<NodeState>([
      ['failed', phase < currentPhase ? 20 : 60],
      ['succeeded', phase < currentPhase ? 80 : 35],
      ['pending', phase < currentPhase ? 0 : 5],
    ])
  }
  if (phase < currentPhase) {
    return random.weighted<NodeState>([
      ['succeeded', 97],
      ['failed', 0.5],
      ['unknown', ensure ? 0 : 0.2],
      ['backoff', ensure ? 0.2 : 0],
      ['verifying', ensure ? 0.4 : 0],
      ['excluded', 0.8],
      ['conflict', kind === 'ensure_version' ? 0.4 : 0],
    ])
  }
  return random.weighted<NodeState>([
    ['succeeded', 52],
    ['delivered', 7],
    ['dispatched', 4],
    ['dispatching', 0.5],
    ['pending', 30],
    ['failed', 0.4],
    ['backoff', ensure ? 0.5 : 0],
    ['verifying', ensure ? 2 : 0],
    ['unknown', ensure ? 0 : 0.2],
    ['excluded', 0.6],
    ['conflict', kind === 'ensure_version' ? 0.5 : 0],
  ])
}

function failuresFor(campaign: MockCampaign['campaign']): string[] {
  const script = campaign.action.script ?? ''
  const messages: string[] = []
  const copy = /cp :(\S+) :(\S+)/.exec(script)
  if (copy !== null) {
    messages.push(`cp: ${copy[1]}: no such file`)
    messages.push(
      /^[A-Za-z]:/.test(copy[2] ?? '')
        ? `cp: ${copy[2]}: access is denied`
        : `cp: ${copy[2]}: no space left on device`,
    )
  }
  if (script.includes('kvs set')) {
    messages.push('kvs: the store refused the write')
  }
  if (messages.length === 0) {
    messages.push('the script failed')
  }
  messages.push('the process was interrupted: the node restarted while it ran')
  return messages
}

export function phaseOf(percents: number[], bucket: number): number {
  for (let index = 0; index < percents.length; index += 1) {
    if ((percents[index] ?? 100) / 100 > bucket) {
      return index
    }
  }
  return percents.length - 1
}

const terminalForFailures: readonly NodeState[] = ['succeeded', 'cancelled', 'excluded', 'conflict']

export function buildRows(
  random: Random,
  campaign: MockCampaign['campaign'],
  salt: string,
  nodes: MockNode[],
  mix: Mix,
  now: number,
): CampaignNode[] {
  const parsed = parseSelector(campaign.selector)
  if (!parsed.ok || campaign.status === 'draft') {
    return []
  }
  const percents = campaign.policy.phases.map((phase) => phase.percent)
  const rows: CampaignNode[] = []
  for (const node of nodes) {
    if (!evaluate(parsed.expression, { fields: selectableFields(node), facts: node.facts })) {
      continue
    }
    const phase = phaseOf(percents, bucketOf(`${node.device_id}/${node.installation_id}/${salt}`))
    if (phase > campaign.current_phase) {
      continue
    }
    const failing =
      (mix === 'gate_failing' && node.os_build === '22631.4317') ||
      (mix === 'policy_failed' && node.dusk_version === '0.1.0')
    const state = stateFor(random, mix, phase, campaign.current_phase, failing, campaign.kind)
    rows.push(rowFor(random, node, phase, state, now, campaign, failing))
  }
  return rows
}

export function rowFor(
  random: Random,
  node: MockNode,
  phase: number,
  state: NodeState,
  now: number,
  campaign: MockCampaign['campaign'],
  failing: boolean,
): CampaignNode {
  const timeoutSeconds = campaign.policy.node_timeout_seconds
  const maximumAttempts = campaign.policy.retry.max_attempts
  const dispatched =
    !['pending', 'excluded', 'conflict', 'cancelled'].includes(state) || random.chance(0.1)
  const dispatchedAt = dispatched ? now - random.integer(60, 6 * 3600) * 1000 : null
  const finished = ['succeeded', 'failed', 'unknown', 'cancelled'].includes(state)
  const finishedAt =
    finished && dispatchedAt !== null
      ? Math.min(now - 1000, dispatchedAt + random.integer(4, 600) * 1000)
      : null
  const session = node.sessions[0]
  const attempt =
    state === 'backoff'
      ? random.integer(1, Math.max(1, Math.min(3, maximumAttempts - 1)))
      : state === 'failed'
        ? maximumAttempts
        : 1
  const failures = state === 'failed' || state === 'backoff' ? attempt : 0
  const ensure = campaign.kind === 'ensure_version' || campaign.kind === 'ensure_config'
  const statusByState: Partial<Record<NodeState, string>> = {
    succeeded: ensure && random.chance(0.1) ? 'duplicate' : 'succeeded',
    failed: 'failed',
    unknown: random.chance(0.5) ? 'ended' : 'started',
    backoff: 'failed',
    delivered: random.chance(0.2) ? 'running' : 'started',
    verifying: 'succeeded',
  }
  const lastError =
    state === 'failed' || state === 'backoff'
      ? random.pick(failuresFor(campaign))
      : state === 'unknown'
        ? 'no result within the node deadline; the process may have run'
        : state === 'conflict'
          ? 'an earlier campaign of the same kind owns this node'
          : ''
  const silent = state === 'succeeded' ? failing && random.chance(0.05) : null
  const reapedAt =
    dispatched && finishedAt !== null && (state === 'succeeded' || state === 'failed')
      ? finishedAt +
        (campaign.policy.retry.max_backoff_seconds + campaign.policy.node_timeout_seconds) * 1000
      : null
  return {
    campaign_id: campaign.id,
    device_id: node.device_id,
    installation_id: node.installation_id,
    phase,
    state,
    attempt,
    failures,
    unreached: state === 'pending' && dispatched ? random.integer(1, 3) : 0,
    pid: dispatched ? random.pid() : null,
    epoch: dispatched ? (session?.epoch ?? now * 1000) : 0,
    namespace_id: dispatched ? (session?.namespace_id ?? random.hex(16)) : '',
    dispatched_at: dispatchedAt === null ? null : new Date(dispatchedAt).toISOString(),
    delivered_at:
      dispatchedAt === null || state === 'dispatched' || state === 'dispatching'
        ? null
        : new Date(dispatchedAt + random.integer(1, 5) * 1000).toISOString(),
    deadline_at:
      dispatchedAt === null
        ? null
        : new Date(dispatchedAt + (timeoutSeconds + 60) * 1000).toISOString(),
    finished_at: finishedAt === null ? null : new Date(finishedAt).toISOString(),
    next_attempt_at:
      state === 'backoff' ? new Date(now + random.integer(60, 1800) * 1000).toISOString() : null,
    last_error: lastError,
    last_status: statusByState[state] ?? '',
    event_at:
      silent === true && finishedAt !== null ? new Date(finishedAt + 60_000).toISOString() : null,
    back_at: null,
    silent,
    reaped_at: reapedAt !== null && reapedAt < now ? new Date(reapedAt).toISOString() : null,
    breakdown: dispatched
      ? {
          os_build: node.os_build ?? '',
          hardware_class: node.hardware_class ?? '',
          dusk_version: node.dusk_version ?? '',
          country: node.country ?? '',
        }
      : {},
  }
}

export function countersOf(rows: CampaignNode[]): Counter[] {
  const counts = new Map<string, Counter>()
  for (const row of rows) {
    const key = `${row.phase}/${row.state}`
    const counter = counts.get(key)
    if (counter === undefined) {
      counts.set(key, { phase: row.phase, state: row.state, count: 1 })
    } else {
      counter.count += 1
    }
  }
  return [...counts.values()].sort(
    (left, right) => left.phase - right.phase || left.state.localeCompare(right.state),
  )
}

function emptyTally(): Tally {
  return { succeeded: 0, failed: 0, eligible: 0, silent: 0 }
}

function count(tally: Tally, row: CampaignNode, windowEnd: number) {
  if (row.state === 'succeeded') {
    tally.succeeded += 1
    if (row.finished_at !== null && Date.parse(row.finished_at) <= windowEnd) {
      tally.eligible += 1
      if (row.silent === true) {
        tally.silent += 1
      }
    }
  } else if (
    row.state === 'unknown' ||
    (row.failures > 0 && !terminalForFailures.includes(row.state))
  ) {
    tally.failed += 1
  }
}

interface Breach {
  group: string
  reason: string
  rate: number
}

export function judgeGates(
  gates: Policy['gates'],
  maximumFailures: number | null,
  overall: Tally,
  groups: Record<string, Record<string, Tally>>,
  rows: number,
  degraded: boolean,
): Pick<Gates, 'verdict' | 'reason' | 'failing_group' | 'required'> {
  const minimum = gates.min_sample
  const required = Math.min(minimum, rows)
  const breaches: Breach[] = []
  const check = (group: string, tally: Tally, needed: number) => {
    const where = group === '' ? 'overall' : `in ${group}`
    const sample = tally.succeeded + tally.failed
    const failureRate = sample === 0 ? 0 : tally.failed / sample
    const silentRate = tally.eligible === 0 ? 0 : tally.silent / tally.eligible
    if (sample >= needed && sample > 0 && failureRate > gates.max_failure_rate) {
      breaches.push({
        group,
        reason: `failure rate ${failureRate.toFixed(2)} ${where}, ${tally.failed} of ${sample}`,
        rate: failureRate,
      })
    }
    if (
      !degraded &&
      tally.eligible >= needed &&
      tally.eligible > 0 &&
      silentRate > gates.max_silent_rate
    ) {
      breaches.push({
        group,
        reason: `silent rate ${silentRate.toFixed(2)} ${where}, ${tally.silent} of ${tally.eligible}`,
        rate: silentRate,
      })
    }
  }
  check('', overall, required)
  for (const dimension of Object.keys(groups).sort()) {
    const values = groups[dimension] ?? {}
    for (const value of Object.keys(values).sort()) {
      check(`${dimension}=${value}`, values[value] ?? emptyTally(), minimum)
    }
  }
  if (maximumFailures !== null && overall.failed > maximumFailures) {
    breaches.push({
      group: '',
      reason: `${overall.failed} failures, more than max_total_failures ${maximumFailures}`,
      rate: 2,
    })
  }
  const worst = breaches.reduce<Breach | null>(
    (current, candidate) =>
      current === null || candidate.rate > current.rate ? candidate : current,
    null,
  )
  if (worst !== null) {
    return { verdict: 'fail', reason: worst.reason, failing_group: worst.group, required }
  }
  const sample = overall.succeeded + overall.failed
  if (sample < required) {
    return {
      verdict: 'hold',
      reason: `waiting for sample ${sample} of ${required}`,
      failing_group: '',
      required,
    }
  }
  if (overall.eligible < required) {
    return {
      verdict: 'hold',
      reason: `waiting for silent-window sample ${overall.eligible} of ${required}`,
      failing_group: '',
      required,
    }
  }
  if (degraded) {
    return { verdict: 'hold', reason: 'the online view is degraded', failing_group: '', required }
  }
  return { verdict: 'pass', reason: '', failing_group: '', required }
}

export function gatesOf(entry: MockCampaign, now: number, degraded: boolean): Gates {
  const { campaign, rows } = entry
  const gates = campaign.policy.gates
  const windowEnd = now - gates.silent_window_seconds * 1000
  const override =
    campaign.gate_override_after === null ? null : Date.parse(campaign.gate_override_after)
  const overall = emptyTally()
  const groups: Record<string, Record<string, Tally>> = {}
  for (const row of rows) {
    if (row.phase > campaign.current_phase) {
      continue
    }
    if (
      override !== null &&
      (row.dispatched_at === null || Date.parse(row.dispatched_at) < override)
    ) {
      continue
    }
    count(overall, row, windowEnd)
    for (const dimension of gates.breakdown) {
      const value = row.breakdown[dimension] ?? ''
      const byValue = (groups[dimension] ??= {})
      count((byValue[value] ??= emptyTally()), row, windowEnd)
    }
  }
  const judged = judgeGates(
    gates,
    campaign.policy.abort.max_total_failures,
    overall,
    groups,
    rows.filter((row) => row.phase <= campaign.current_phase).length,
    degraded,
  )
  const phaseIndex = Math.min(campaign.current_phase, campaign.policy.phases.length - 1)
  const phase = campaign.policy.phases[phaseIndex]
  const bakeSeconds = phase?.bake_seconds ?? 0
  const accrued = bakeAccruedSeconds(campaign, now) ?? 0
  const converging =
    (campaign.kind === 'ensure_version' || campaign.kind === 'ensure_config') &&
    campaign.status === 'running' &&
    campaign.current_phase >= campaign.policy.phases.length - 1 &&
    judged.verdict === 'pass' &&
    accrued >= bakeSeconds
  return {
    overall,
    groups,
    ...judged,
    degraded,
    min_sample: gates.min_sample,
    max_failure_rate: gates.max_failure_rate,
    max_silent_rate: gates.max_silent_rate,
    phase: campaign.current_phase,
    phase_name: phase?.name ?? '',
    bake_seconds: bakeSeconds,
    bake_accrued_seconds: accrued,
    converging,
  }
}

export function eventFactory(events: CampaignEvent[]) {
  return (time: number, kind: string, actor: string, detail: Record<string, JsonValue>) => {
    events.push({ id: events.length + 1, time: new Date(time).toISOString(), kind, actor, detail })
  }
}

function overallOf(rows: CampaignNode[], phase: number): Record<string, JsonValue> {
  const tally = emptyTally()
  for (const row of rows) {
    if (row.phase <= phase) {
      count(tally, row, Number.POSITIVE_INFINITY)
    }
  }
  return { ...tally }
}

function eventsFor(scenario: Scenario, entry: MockCampaign, now: number): CampaignEvent[] {
  const { campaign, rows } = entry
  const events: CampaignEvent[] = []
  const add = eventFactory(events)
  add(Date.parse(campaign.created_at), 'created', scenario.createdBy, { name: campaign.name })
  if (campaign.started_at === null) {
    return events
  }
  let time = Date.parse(campaign.started_at)
  add(time, 'started', scenario.createdBy, {})
  const phases = campaign.policy.phases
  const minimum = campaign.policy.gates.min_sample
  for (let phase = 0; phase < campaign.current_phase; phase += 1) {
    const definition = phases[phase]
    const next = phases[phase + 1]
    if (definition === undefined || next === undefined) {
      break
    }
    if (phase === 0 && minimum > 0) {
      add(time + 120_000, 'gate_holding', 'twilight', {
        reason: `waiting for sample 12 of ${minimum}`,
        phase,
      })
    }
    time += definition.bake_seconds * 1000 + 180_000
    add(time, 'phase_advanced', 'twilight', {
      from: definition.name,
      to: next.name,
      percent: next.percent,
      overall: overallOf(rows, phase),
    })
  }
  if (rows.some((row) => row.state === 'conflict')) {
    const conflicted = rows.find((row) => row.state === 'conflict')
    add(time + 600_000, 'conflict', 'twilight', {
      campaign_id: campaign.id,
      device_id: conflicted?.device_id ?? '',
      installation_id: conflicted?.installation_id ?? '',
      message:
        "an earlier campaign of the same kind owns some of this campaign's nodes; their rows are in conflict",
    })
  }
  if (campaign.status === 'paused' && scenario.pause !== undefined) {
    add(now - scenario.pause.minutesAgo * 60_000, 'paused', scenario.pause.actor, {
      ...(campaign.pause_reason === null ? {} : { reason: campaign.pause_reason }),
      pause_kind: scenario.pause.kind,
    })
  }
  if (campaign.status === 'completed' || campaign.status === 'archived') {
    add(
      Date.parse(campaign.finished_at ?? campaign.started_at),
      'completed',
      scenario.createdBy,
      {},
    )
  }
  if (campaign.status === 'aborted') {
    add(Date.parse(campaign.finished_at ?? campaign.started_at), 'aborted', scenario.createdBy, {
      reason: campaign.abort_reason ?? '',
    })
  }
  if (campaign.status === 'failed') {
    add(Date.parse(campaign.finished_at ?? campaign.started_at), 'failed', 'twilight', {
      reason: campaign.abort_reason ?? '',
      pause_kind: 'gate',
      group: /in (\S+=\S+),/.exec(campaign.abort_reason ?? '')?.[1] ?? '',
      overall: overallOf(rows, campaign.current_phase),
    })
  }
  return events
    .sort((left, right) => Date.parse(left.time) - Date.parse(right.time))
    .map((event, index) => ({ ...event, id: index + 1 }))
}

export function generateCampaigns(seed: number, nodes: MockNode[], now: number): MockCampaign[] {
  const random = new Random(seed)
  return scenarios.map((scenario) => {
    const base = defaultPolicy(scenario.action.kind)
    const policy = (scenario.policy ?? ((value: Policy) => value))({
      ...base,
      gates: { ...base.gates, min_sample: 50 },
    })
    const startedAt =
      scenario.startedHoursAgo === null ? null : now - scenario.startedHoursAgo * 3600_000
    const finishedAt =
      scenario.finishedHoursAgo === undefined ? null : now - scenario.finishedHoursAgo * 3600_000
    const pausedAt = scenario.pause === undefined ? null : now - scenario.pause.minutesAgo * 60_000
    const createdAt = (startedAt ?? now - 2 * 3600_000) - 25 * 60_000
    const campaign: MockCampaign['campaign'] = {
      id: scenario.id,
      name: scenario.name,
      description: scenario.description,
      tenant: null,
      status: scenario.status,
      kind: scenario.action.kind,
      selector: scenario.selector,
      action: scenario.action,
      policy,
      created_by: scenario.createdBy,
      created_at: new Date(createdAt).toISOString(),
      updated_at: new Date(finishedAt ?? pausedAt ?? startedAt ?? createdAt).toISOString(),
      started_at: startedAt === null ? null : new Date(startedAt).toISOString(),
      paused_at: pausedAt === null ? null : new Date(pausedAt).toISOString(),
      finished_at: finishedAt === null ? null : new Date(finishedAt).toISOString(),
      current_phase: scenario.currentPhase,
      phase_started_at:
        startedAt === null
          ? null
          : new Date(now - scenario.phaseOpenedMinutesAgo * 60_000).toISOString(),
      phase_paused_seconds: 0,
      gate_override_after: null,
      pause_kind: scenario.pause?.kind ?? null,
      pause_reason: scenario.pause?.reason ?? null,
      abort_reason: scenario.abortReason ?? null,
      last_dispatch_at: null,
      version: 3,
    }
    const salt = random.hex(32)
    const entry: MockCampaign = { campaign, salt, rows: [], events: [] }
    entry.rows = buildRows(random, campaign, salt, nodes, scenario.mix, now)
    const lastDispatch = Math.max(
      0,
      ...entry.rows.map((row) => (row.dispatched_at === null ? 0 : Date.parse(row.dispatched_at))),
    )
    campaign.last_dispatch_at =
      lastDispatch === 0
        ? null
        : new Date(
            finishedAt === null ? lastDispatch : Math.min(lastDispatch, finishedAt),
          ).toISOString()
    if (scenario.pause?.kind === 'gate' || scenario.mix === 'policy_failed') {
      const verdict = gatesOf(entry, pausedAt ?? finishedAt ?? now, false)
      if (scenario.pause?.kind === 'gate') {
        campaign.pause_reason = verdict.reason
      } else {
        campaign.abort_reason = verdict.reason
      }
    }
    entry.events = eventsFor(scenario, entry, now)
    return entry
  })
}
