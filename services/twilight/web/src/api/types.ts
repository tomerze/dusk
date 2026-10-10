export type Role = 'viewer' | 'operator' | 'admin'

export type AuthenticationMethod = 'oidc' | 'token' | 'dev' | 'mtls'

export interface Me {
  subject: string
  name: string | null
  role: Role
  authentication: AuthenticationMethod
  expires_at: string | null
}

export type JsonValue =
  string | number | boolean | null | JsonValue[] | { [key: string]: JsonValue }

export interface ApiErrorBody {
  error: {
    code: string
    message: string
    details: Record<string, JsonValue>
  }
}

export type Lifecycle = 'enrolled' | 'active' | 'quarantined' | 'retired' | 'revoked'

export const lifecycles: readonly Lifecycle[] = [
  'enrolled',
  'active',
  'quarantined',
  'retired',
  'revoked',
]

export type CampaignStatus =
  'draft' | 'running' | 'paused' | 'completed' | 'aborted' | 'failed' | 'archived'

export const campaignStatuses: readonly CampaignStatus[] = [
  'draft',
  'running',
  'paused',
  'completed',
  'aborted',
  'failed',
  'archived',
]

export type ActionKind = 'run_script' | 'ensure_version' | 'ensure_config' | 'quarantine'

export type NodeState =
  | 'pending'
  | 'dispatching'
  | 'dispatched'
  | 'delivered'
  | 'verifying'
  | 'backoff'
  | 'succeeded'
  | 'failed'
  | 'unknown'
  | 'excluded'
  | 'conflict'
  | 'cancelled'

export const nodeStates: readonly NodeState[] = [
  'succeeded',
  'failed',
  'unknown',
  'backoff',
  'verifying',
  'delivered',
  'dispatched',
  'dispatching',
  'pending',
  'conflict',
  'excluded',
  'cancelled',
]

export type Severity = 'critical' | 'high' | 'medium' | 'low'

export const severities: readonly Severity[] = ['critical', 'high', 'medium', 'low']

export type LogLevel = 'trace' | 'debug' | 'info' | 'warn' | 'error'

export type BreakdownField = 'os_build' | 'hardware_class' | 'dusk_version' | 'country'

export const breakdownFields: readonly BreakdownField[] = [
  'os_build',
  'hardware_class',
  'dusk_version',
  'country',
]

export interface StreamLogs {
  level: LogLevel
  duration_seconds: number
}

export type Action =
  | {
      kind: 'run_script'
      script: string
      collect_files?: string[]
      stream_logs?: StreamLogs | null
    }
  | { kind: 'ensure_version'; version: string; script: string; version_key: string }
  | { kind: 'ensure_config'; config_hash: string; script: string }
  | { kind: 'quarantine'; script?: string; require_script_success?: boolean }

export interface Phase {
  name: string
  percent: number
  bake_seconds: number
}

export interface Policy {
  rate: { per_second: number; burst: number }
  phases: Phase[]
  gates: {
    min_sample: number
    max_failure_rate: number
    max_silent_rate: number
    silent_window_seconds: number
    breakdown: BreakdownField[]
  }
  abort: { on_gate_failure: 'pause' | 'abort'; max_total_failures: number | null }
  node_timeout_seconds: number
  retry: {
    max_attempts: number
    initial_backoff_seconds: number
    max_backoff_seconds: number
    multiplier: number
  }
  deadline: string | null
  allow_overlap: boolean
}

export interface CampaignDefinition {
  name: string
  description: string
  tenant?: string | null
  selector: string
  action: Action
  policy: Policy
}

export interface CampaignUpdate extends CampaignDefinition {
  version: number
}

export interface Counter {
  phase: number
  state: NodeState
  count: number
}

export type PauseKind = 'operator' | 'gate' | 'permission'

export interface Campaign {
  id: string
  name: string
  description: string
  tenant: string | null
  status: CampaignStatus
  kind: ActionKind
  selector: string
  action: Action
  policy: Policy
  created_by: string
  created_at: string
  updated_at: string
  started_at: string | null
  paused_at: string | null
  finished_at: string | null
  current_phase: number
  phase_started_at: string | null
  phase_paused_seconds: number
  gate_override_after: string | null
  pause_kind: PauseKind | null
  pause_reason: string | null
  abort_reason: string | null
  last_dispatch_at: string | null
  version: number
  counters: Counter[]
}

export interface Page<Item> {
  items: Item[]
  next_cursor: string | null
}

export interface NodeKey {
  device_id: string
  installation_id: string
}

export interface NodeSummary extends NodeKey {
  cert_fingerprint: string | null
  lifecycle: Lifecycle
  lifecycle_reason: string | null
  lifecycle_changed_at: string | null
  country: string | null
  os_name: string | null
  os_version: string | null
  os_build: string | null
  dusk_version: string | null
  hardware_class: string | null
  tenant: string | null
  locale: string | null
  hostname: string | null
  impl: string | null
  target_arch: string | null
  facts: Record<string, JsonValue>
  reported_version: string | null
  reported_config_hash: string | null
  reported_services: string[] | null
  reported_at: string | null
  facts_namespace_id: string | null
  facts_read_at: string | null
  enrolled_at: string | null
  first_seen_at: string
  updated_at: string
  online: boolean
  last_seen_at: string | null
}

export interface PresenceSession {
  namespace_id: string
  epoch: number
  instance: string
  inner_address: string
  connected_at: string
  last_seen: string
}

export interface CampaignNode extends NodeKey {
  campaign_id: string
  phase: number
  state: NodeState
  attempt: number
  failures: number
  unreached: number
  pid: string | null
  epoch: number
  namespace_id: string
  dispatched_at: string | null
  delivered_at: string | null
  deadline_at: string | null
  finished_at: string | null
  next_attempt_at: string | null
  last_error: string
  last_status: string
  event_at: string | null
  back_at: string | null
  silent: boolean | null
  reaped_at: string | null
  breakdown: Record<string, string>
}

export type ProcessStatus =
  | 'started'
  | 'succeeded'
  | 'failed'
  | 'duplicate'
  | 'running'
  | 'ended'
  | 'already_satisfied'
  | 'unreachable'
  | 'timed_out'
  | 'denied'
  | 'reaped'
  | 'error'

export interface DeviceLifecycle {
  device_id: string
  lifecycle: 'active' | 'retired' | 'revoked'
  reason: string
  changed_at: string
  actor: string
}

export interface NodeDetail extends NodeSummary {
  device: DeviceLifecycle | null
  sessions: PresenceSession[]
  executions: CampaignNode[]
}

export interface NodeReference extends NodeKey {
  namespace_id: string
  nightfall: string | null
}

export interface InteractiveSession {
  pid: string
  node: NodeReference
}

export interface LogStreamAccepted {
  stream_id: string
}

export interface FileUploadAccepted {
  upload_id: string
}

export interface SelectorError {
  position: number
  end: number
  message: string
}

export interface SelectorValidation {
  ok: boolean
  error: SelectorError | null
  matched: number
  sample: NodeSummary[]
}

export interface CampaignEvent {
  id: number
  time: string
  kind: string
  actor: string
  detail: Record<string, JsonValue>
}

export interface Tally {
  succeeded: number
  failed: number
  eligible: number
  silent: number
}

export interface Gates {
  overall: Tally
  groups: Record<string, Record<string, Tally>>
  verdict: 'pass' | 'hold' | 'fail'
  reason: string
  failing_group: string
  degraded: boolean
  min_sample: number
  required: number
  max_failure_rate: number
  max_silent_rate: number
  phase: number
  phase_name: string
  bake_seconds: number
  bake_accrued_seconds: number
  converging: boolean
}

export interface Alert {
  id: number
  time: string
  last_seen_at: string
  occurrences: number
  severity: Severity
  kind: string
  fingerprint: string
  detail: Record<string, JsonValue>
  acknowledged_by: string | null
  acknowledged_at: string | null
  resolved_by: string | null
  resolved_at: string | null
}

export type SeverityCounts = Partial<Record<Severity, number>>

export interface Overview {
  nodes: {
    total: number
    online: number
    by_lifecycle: Partial<Record<Lifecycle, number>>
  }
  campaigns: Campaign[]
  alerts: SeverityCounts
  alerts_unacknowledged: SeverityCounts
  degraded: boolean
  leader: boolean
}

export interface CountResult {
  count: number
}

export interface Overlap {
  count: number
  campaigns: string[]
}

export interface CampaignCounters {
  campaign_id: string
  status: CampaignStatus
  phase: number
  counters: Counter[]
}

export type FeedEvent =
  | { kind: 'presence'; time: string; data: { online: number; degraded: boolean } }
  | { kind: 'counters'; time: string; data: CampaignCounters[] }
  | {
      kind: 'alerts'
      time: string
      data: { open: SeverityCounts; unacknowledged: SeverityCounts; latest: Alert | null }
    }
