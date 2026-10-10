import type { Alert, Campaign, JsonValue, Role, Severity } from '../api/types'
import type { MockCampaign } from './campaigns'
import { countersOf, generateCampaigns } from './campaigns'
import type { MockNode } from './nodes'
import { generateNodes } from './nodes'
import { Random } from './random'

export type MockScenario =
  'default' | 'empty' | 'degraded' | 'errors' | 'viewer' | 'operator' | 'signed-out' | 'slow'

export const mockScenarios: readonly MockScenario[] = [
  'default',
  'empty',
  'degraded',
  'errors',
  'viewer',
  'operator',
  'signed-out',
  'slow',
]

export interface MockState {
  scenario: MockScenario
  now: number
  random: Random
  nodes: MockNode[]
  nodeIndex: Map<string, MockNode>
  campaigns: MockCampaign[]
  alerts: Alert[]
  degraded: boolean
  role: Role
}

export function nodeKey(deviceId: string, installationId: string) {
  return `${deviceId}/${installationId}`
}

export function withCounters(entry: MockCampaign): Campaign {
  return { ...entry.campaign, counters: countersOf(entry.rows) }
}

function applyCampaignsToNodes(state: MockState) {
  for (const entry of state.campaigns) {
    const { campaign } = entry
    if (campaign.status === 'archived') {
      continue
    }
    for (const row of entry.rows) {
      const node = state.nodeIndex.get(nodeKey(row.device_id, row.installation_id))
      if (node === undefined) {
        continue
      }
      node.executions.push(row)
      if (row.state === 'succeeded' && campaign.action.kind === 'ensure_version') {
        node.dusk_version = campaign.action.version
        node.reported_version = campaign.action.version
        node.facts['dusk.version'] = campaign.action.version
      }
      if (row.state === 'succeeded' && campaign.action.kind === 'quarantine') {
        node.lifecycle = 'quarantined'
        node.lifecycle_reason = `campaign ${campaign.name}`
        node.lifecycle_changed_at = row.finished_at
      }
    }
  }
  for (const node of state.nodes) {
    node.executions.sort((left, right) =>
      (right.dispatched_at ?? '').localeCompare(left.dispatched_at ?? ''),
    )
    node.executions = node.executions.slice(0, 100)
  }
}

interface AlertSeed {
  minutesAgo: number
  severity: Severity
  kind: string
  fingerprint: string
  detail: Record<string, JsonValue>
  occurrences?: number
  lastSeenMinutesAgo?: number
  acknowledged?: { by: string; minutesAgo: number }
  resolved?: { by: string; minutesAgo: number }
}

function generateAlerts(state: MockState): Alert[] {
  const node =
    state.nodes.find((candidate) => candidate.tenant === 'acme-retail' && candidate.online) ??
    state.nodes[0]
  const minutes = (count: number) => new Date(state.now - count * 60_000).toISOString()
  const deviceId = node?.device_id ?? '0'.repeat(32)
  const installationId = node?.installation_id ?? '0'.repeat(32)
  const namespaceId = node?.sessions[0]?.namespace_id ?? '0'.repeat(16)
  const command = (
    message: string,
    pid: string,
    action: string,
    principal: string,
    instance: number,
    sequence: number,
    minutesAgo: number,
  ): Record<string, JsonValue> => ({
    message,
    pid,
    session_id: state.random.uuid(4),
    call_id: state.random.uuid(4),
    principal,
    device_id: deviceId,
    installation_id: installationId,
    namespace_id: namespaceId,
    action,
    instance: `nightfall-${instance}`,
    partition: instance,
    sequence,
    time: minutes(minutesAgo),
  })
  const seeds: AlertSeed[] = [
    {
      minutesAgo: 7,
      severity: 'critical',
      kind: 'process_without_intent',
      fingerprint: 'process_without_intent:13873190457734094641',
      detail: command(
        'a process was created at a pid twilight never intended',
        '13873190457734094641',
        'Dusk.process',
        'dawn-2',
        3,
        18820441,
        7,
      ),
      occurrences: 3,
      lastSeenMinutesAgo: 4,
    },
    {
      minutesAgo: 26,
      severity: 'high',
      kind: 'enrollment_rate',
      fingerprint: 'enrollment_rate:2026-10-09T09:34:00Z',
      detail: {
        minute: minutes(26),
        threshold: 600,
        message: 'more enrollments in one minute than alerts.enrollment_rate_per_minute',
      },
    },
    {
      minutesAgo: 41,
      severity: 'medium',
      kind: 'campaign_conflict',
      fingerprint: 'campaign_conflict:01929c11-1a2b-7c3d-8e4f-5a6b7c8d9e05',
      detail: {
        campaign_id: '01929c11-1a2b-7c3d-8e4f-5a6b7c8d9e05',
        device_id: deviceId,
        installation_id: installationId,
        message:
          "an earlier campaign of the same kind owns some of this campaign's nodes; their rows are in conflict",
      },
      acknowledged: { by: 'release-bot', minutesAgo: 35 },
    },
    {
      minutesAgo: 95,
      severity: 'high',
      kind: 'process_after_result',
      fingerprint: 'process_after_result:8615092217736154119',
      detail: {
        message: 'calls arrived under a pid more than a minute after its final result',
        pid: '8615092217736154119',
        sessions: [state.random.uuid(4)],
        device_id: deviceId,
        installation_id: installationId,
        action_kind: 'run_script',
        campaign_id: '01929b41-02aa-7b6e-8d11-0c7f3e9a2b02',
        last_call_at: minutes(94),
        result_at: minutes(98),
        result_status: 'succeeded',
      },
    },
    {
      minutesAgo: 180,
      severity: 'critical',
      kind: 'default_shell_without_intent',
      fingerprint: 'default_shell_without_intent:dawn-1',
      detail: command(
        "a shell command ran in the node's default shell while no process twilight intended for the node was open",
        '17505437192229758416',
        'ShPortal.sh',
        'dawn-1',
        1,
        9912007,
        180,
      ),
      occurrences: 12,
      lastSeenMinutesAgo: 150,
      acknowledged: { by: 'security', minutesAgo: 150 },
    },
    {
      minutesAgo: 60 * 26,
      severity: 'critical',
      kind: 'revocation_not_enforced',
      fingerprint: `revocation_not_enforced:${deviceId}/${installationId}`,
      detail: {
        device_id: deviceId,
        installation_id: installationId,
        lifecycle: 'revoked',
        changed_at: minutes(60 * 27),
        instance: 'nightfall-1',
        namespace_id: namespaceId,
      },
      acknowledged: { by: 'security', minutesAgo: 60 * 25 },
      resolved: { by: 'security', minutesAgo: 60 * 24 },
    },
    {
      minutesAgo: 60 * 30,
      severity: 'high',
      kind: 'quarantine_override',
      fingerprint: 'quarantine_override:5170373391855270882',
      detail: command(
        'a principal allowed past quarantine created a process on a quarantined node',
        '5170373391855270882',
        'Dusk.process',
        'incident-response-1',
        2,
        4410093,
        60 * 30,
      ),
      acknowledged: { by: 'security', minutesAgo: 60 * 29 },
      resolved: { by: 'security', minutesAgo: 60 * 28 },
    },
    {
      minutesAgo: 60 * 50,
      severity: 'high',
      kind: 'process_after_deadline',
      fingerprint: 'process_after_deadline:10224930664915006353',
      detail: {
        ...command(
          'calls arrived under a pid after its deadline',
          '10224930664915006353',
          'KvsPortal.get',
          'dawn-0',
          0,
          30199,
          60 * 50,
        ),
        expires_at: minutes(60 * 50 + 2),
      },
      acknowledged: { by: 'security', minutesAgo: 60 * 49 },
      resolved: { by: 'security', minutesAgo: 60 * 48 },
    },
    {
      minutesAgo: 60 * 24 * 9,
      severity: 'critical',
      kind: 'ledger_chain_broken',
      fingerprint: 'ledger_chain_broken:nightfall-4/4',
      detail: {
        message: 'the ledger chain is broken: sequence 991305 follows 991201',
        break: 'sequence_gap',
        instance: 'nightfall-4',
        partition: 4,
        sequence: 991305,
      },
      acknowledged: { by: 'security', minutesAgo: 60 * 24 * 9 - 20 },
      resolved: { by: 'security', minutesAgo: 60 * 24 * 8 },
    },
  ]
  const history: [Severity, string, string][] = [
    [
      'high',
      'enrollment_rate',
      'more enrollments in one minute than alerts.enrollment_rate_per_minute',
    ],
    [
      'medium',
      'campaign_conflict',
      "an earlier campaign of the same kind owns some of this campaign's nodes; their rows are in conflict",
    ],
    [
      'critical',
      'default_shell_without_intent',
      "a shell command ran in the node's default shell while no process twilight intended for the node was open",
    ],
    [
      'high',
      'quarantine_override',
      'a principal allowed past quarantine created a process on a quarantined node',
    ],
    ['high', 'process_after_deadline', 'calls arrived under a pid after its deadline'],
    [
      'high',
      'process_shape',
      'a run_script process ran 3 scripts, more than the 2 twilight intended for it',
    ],
  ]
  for (let index = 0; index < 36; index += 1) {
    const [severity, kind, message] = history[index % history.length] ?? history[0]!
    const host = state.nodes[(index * 97) % Math.max(1, state.nodes.length)]
    const hours = 60 + index * 19
    seeds.push({
      minutesAgo: hours * 60,
      severity,
      kind,
      fingerprint: `${kind}:${host?.device_id ?? index}:${index}`,
      detail: {
        message,
        device_id: host?.device_id ?? deviceId,
        installation_id: host?.installation_id ?? installationId,
      },
      acknowledged: {
        by: index % 3 === 0 ? 'security' : 'release-bot',
        minutesAgo: hours * 60 - 12,
      },
      resolved: { by: index % 3 === 0 ? 'security' : 'release-bot', minutesAgo: hours * 60 - 90 },
    })
  }
  const tenantOf = (detail: Record<string, JsonValue>): string | null => {
    const device = detail.device_id
    const installation = detail.installation_id
    if (typeof device !== 'string' || typeof installation !== 'string') {
      return null
    }
    return state.nodeIndex.get(nodeKey(device, installation))?.tenant ?? null
  }
  return seeds.map((seed, index) => ({
    id: 9000 + seeds.length - index,
    time: minutes(seed.minutesAgo),
    last_seen_at: minutes(seed.lastSeenMinutesAgo ?? seed.minutesAgo),
    occurrences: seed.occurrences ?? 1,
    severity: seed.severity,
    kind: seed.kind,
    fingerprint: seed.fingerprint,
    tenant: tenantOf(seed.detail),
    detail: seed.detail,
    acknowledged_by: seed.acknowledged?.by ?? null,
    acknowledged_at: seed.acknowledged === undefined ? null : minutes(seed.acknowledged.minutesAgo),
    resolved_by: seed.resolved?.by ?? null,
    resolved_at: seed.resolved === undefined ? null : minutes(seed.resolved.minutesAgo),
  }))
}

export function createMockState(
  options: { scenario?: MockScenario; nodeCount?: number; seed?: number; now?: number } = {},
): MockState {
  const scenario = options.scenario ?? 'default'
  const now = options.now ?? Date.now()
  const seed = options.seed ?? 20261007
  const empty = scenario === 'empty'
  const nodes = empty ? [] : generateNodes(seed, options.nodeCount ?? 12000, now)
  const state: MockState = {
    scenario,
    now,
    random: new Random(seed + 1),
    nodes,
    nodeIndex: new Map(nodes.map((node) => [nodeKey(node.device_id, node.installation_id), node])),
    campaigns: [],
    alerts: [],
    degraded: scenario === 'degraded',
    role: scenario === 'viewer' ? 'viewer' : scenario === 'operator' ? 'operator' : 'admin',
  }
  if (!empty) {
    state.campaigns = generateCampaigns(seed + 2, nodes, now)
    applyCampaignsToNodes(state)
    state.alerts = generateAlerts(state)
  }
  return state
}
