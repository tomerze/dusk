import { delay, http, HttpResponse, sse } from 'msw'
import type {
  Campaign,
  CampaignCounters,
  CampaignDefinition,
  CampaignNode,
  CampaignUpdate,
  DeviceLifecycle,
  FeedEvent,
  JsonValue,
  Lifecycle,
  LogLevel,
  NodeState,
  Overlap,
  Overview,
  Page,
  SelectorValidation,
  SeverityCounts,
} from '../api/types'
import { campaignStatuses, lifecycles } from '../api/types'
import { intersection } from '../campaigns/overlaps'
import { processTimeoutSeconds, validateDefinition } from '../campaigns/validation'
import type { MockCampaign } from './campaigns'
import { buildRows, eventFactory, gatesOf } from './campaigns'
import type { MockNode } from './nodes'
import { selectableFields, toDetail, toSummary } from './nodes'
import { evaluate, parseSelector } from './selector'
import type { MockState } from './state'
import { nodeKey, withCounters } from './state'

function failure(
  status: number,
  code: string,
  message: string,
  details: Record<string, JsonValue> = {},
) {
  return HttpResponse.json({ error: { code, message, details } }, { status })
}

function limitOf(url: URL): number | null {
  const text = url.searchParams.get('limit')
  if (text === null) {
    return 100
  }
  const limit = Number(text)
  return Number.isInteger(limit) && limit >= 1 && limit <= 500 ? limit : null
}

const badLimit = () =>
  failure(400, 'invalid_argument', 'limit must be an integer between 1 and 500')

function paginate<Item>(items: Item[], url: URL, limit: number): Page<Item> {
  const offset = Math.max(0, Number(url.searchParams.get('cursor') ?? 0) || 0)
  return {
    items: items.slice(offset, offset + limit),
    next_cursor: offset + limit < items.length ? String(offset + limit) : null,
  }
}

function matching(state: MockState, selector: string) {
  const parsed = parseSelector(selector)
  if (!parsed.ok) {
    return parsed
  }
  return {
    ok: true as const,
    nodes: state.nodes.filter((node) =>
      evaluate(parsed.expression, { fields: selectableFields(node), facts: node.facts }),
    ),
  }
}

const sortableColumns = [
  'device_id',
  'hostname',
  'lifecycle',
  'country',
  'os_name',
  'os_version',
  'os_build',
  'dusk_version',
  'hardware_class',
  'tenant',
  'locale',
  'impl',
  'target_arch',
  'reported_version',
] as const

function sortNodes(nodes: MockNode[], sort: string): MockNode[] | null {
  const descending = sort.startsWith('-')
  const name = descending ? sort.slice(1) : sort
  const field = sortableColumns.find((candidate) => candidate === name)
  if (field === undefined) {
    return null
  }
  const sorted = [...nodes].sort((left, right) => {
    const leftValue = left[field] ?? ''
    const rightValue = right[field] ?? ''
    return (
      leftValue.localeCompare(rightValue, 'en', { sensitivity: 'base', numeric: true }) ||
      left.device_id.localeCompare(right.device_id)
    )
  })
  return descending ? sorted.reverse() : sorted
}

function openAlertCounts(state: MockState, unacknowledged = false): SeverityCounts {
  const counts: SeverityCounts = {}
  for (const alert of state.alerts) {
    if (alert.resolved_at === null && (!unacknowledged || alert.acknowledged_at === null)) {
      counts[alert.severity] = (counts[alert.severity] ?? 0) + 1
    }
  }
  return counts
}

function activeCampaigns(state: MockState): MockCampaign[] {
  return state.campaigns.filter(
    ({ campaign }) => campaign.status === 'running' || campaign.status === 'paused',
  )
}

function overview(state: MockState): Overview {
  const byLifecycle: Partial<Record<Lifecycle, number>> = {}
  let online = 0
  for (const node of state.nodes) {
    byLifecycle[node.lifecycle] = (byLifecycle[node.lifecycle] ?? 0) + 1
    if (node.online) {
      online += 1
    }
  }
  return {
    nodes: { total: state.nodes.length, online, by_lifecycle: byLifecycle },
    campaigns: activeCampaigns(state).map(withCounters),
    alerts: openAlertCounts(state),
    alerts_unacknowledged: openAlertCounts(state, true),
    degraded: state.degraded,
    leader: true,
  }
}

function overlapOf(state: MockState, entry: MockCampaign) {
  const { campaign } = entry
  if (campaign.kind !== 'ensure_version' && campaign.kind !== 'ensure_config') {
    return { count: 0, campaigns: [] as string[] }
  }
  let count = 0
  const campaigns: string[] = []
  for (const { campaign: other } of activeCampaigns(state)) {
    if (other.id === campaign.id || other.kind !== campaign.kind) {
      continue
    }
    if (
      campaign.action.kind === 'ensure_version' &&
      (other.action.kind !== 'ensure_version' ||
        other.action.version_key !== campaign.action.version_key)
    ) {
      continue
    }
    const both = matching(state, intersection(campaign.selector, other.selector))
    if (both.ok && both.nodes.length > 0) {
      count += both.nodes.length
      campaigns.push(other.id)
    }
  }
  return { count, campaigns }
}

const campaignCommands = ['start', 'pause', 'resume', 'abort', 'complete', 'archive'] as const
type CampaignCommand = (typeof campaignCommands)[number]

const allowed: Record<CampaignCommand, readonly Campaign['status'][]> = {
  start: ['draft'],
  pause: ['running'],
  resume: ['paused'],
  abort: ['draft', 'running', 'paused'],
  complete: ['running', 'paused'],
  archive: ['completed', 'aborted', 'failed'],
}

function cancelUnsent(entry: MockCampaign, now: number) {
  for (const row of entry.rows) {
    if (['pending', 'backoff', 'excluded', 'conflict', 'verifying'].includes(row.state)) {
      row.state = 'cancelled'
      row.finished_at = new Date(now).toISOString()
      row.next_attempt_at = null
    }
  }
}

function applyCommand(
  state: MockState,
  campaignId: string,
  command: CampaignCommand,
  body: { reason?: string; override_gate?: boolean },
  actor: string,
) {
  const entry = state.campaigns.find(({ campaign }) => campaign.id === campaignId)
  if (entry === undefined) {
    return failure(404, 'not_found', 'campaign not found')
  }
  const { campaign } = entry
  const now = Date.now()
  const at = new Date(now).toISOString()
  const add = eventFactory(entry.events)
  const reason = body.reason?.trim() ?? ''
  const detail: Record<string, JsonValue> = reason.length > 0 ? { reason } : {}
  if (!allowed[command].includes(campaign.status)) {
    return failure(
      409,
      'invalid_transition',
      `invalid campaign transition: cannot ${command} a ${campaign.status} campaign`,
    )
  }
  switch (command) {
    case 'start': {
      if (!campaign.policy.allow_overlap) {
        const overlap = overlapOf(state, entry)
        if (overlap.count > 0) {
          return failure(
            409,
            'overlap',
            `${overlap.count} nodes are also targeted by running campaigns of the same kind; start with policy.allow_overlap to accept it`,
            overlap,
          )
        }
      }
      campaign.status = 'running'
      campaign.started_at = at
      campaign.phase_started_at = at
      campaign.phase_paused_seconds = 0
      campaign.current_phase = 0
      entry.rows = buildRows(
        state.random,
        campaign,
        entry.salt,
        state.nodes,
        'progressing',
        now,
      ).map((row): CampaignNode => ({
        ...row,
        state: 'pending',
        attempt: 1,
        failures: 0,
        unreached: 0,
        pid: null,
        epoch: 0,
        namespace_id: '',
        dispatched_at: null,
        delivered_at: null,
        deadline_at: null,
        finished_at: null,
        next_attempt_at: null,
        last_error: '',
        last_status: '',
        silent: null,
        reaped_at: null,
        breakdown: {},
      }))
      add(now, 'started', actor, detail)
      break
    }
    case 'pause':
      campaign.status = 'paused'
      campaign.paused_at = at
      campaign.pause_kind = 'operator'
      campaign.pause_reason = reason
      add(now, 'paused', actor, { ...detail, pause_kind: 'operator' })
      break
    case 'resume': {
      const gate = campaign.pause_kind === 'gate'
      if (gate && (body.override_gate !== true || reason.length === 0)) {
        return failure(
          409,
          'gate_override_required',
          'the campaign was paused by a health gate; resume it with override_gate and a reason',
        )
      }
      if (gate) {
        campaign.gate_override_after = at
      }
      if (campaign.paused_at !== null) {
        campaign.phase_paused_seconds += Math.floor((now - Date.parse(campaign.paused_at)) / 1000)
      }
      campaign.status = 'running'
      campaign.paused_at = null
      campaign.pause_kind = null
      campaign.pause_reason = null
      add(now, 'resumed', actor, gate ? { ...detail, override_gate: true } : detail)
      break
    }
    case 'abort':
      cancelUnsent(entry, now)
      campaign.status = 'aborted'
      campaign.abort_reason = reason
      campaign.finished_at = at
      add(now, 'aborted', actor, detail)
      break
    case 'complete':
      cancelUnsent(entry, now)
      campaign.status = 'completed'
      campaign.finished_at = at
      add(now, 'completed', actor, detail)
      break
    case 'archive': {
      const lastDispatch =
        campaign.last_dispatch_at === null ? 0 : Date.parse(campaign.last_dispatch_at)
      const earliest = lastDispatch + (processTimeoutSeconds(campaign) + 3600) * 1000
      if (earliest > now) {
        return failure(
          409,
          'not_archivable',
          `the campaign cannot be archived yet: wait until ${new Date(earliest).toISOString().replace(/\.\d+Z$/, 'Z')}, an hour past the node timeout after the last dispatch`,
        )
      }
      campaign.status = 'archived'
      break
    }
  }
  campaign.version += 1
  campaign.updated_at = at
  return HttpResponse.json(withCounters(entry))
}

function definitionProblem(definition: CampaignDefinition) {
  const parsed = parseSelector(definition.selector)
  if (!parsed.ok) {
    return failure(400, 'invalid_argument', `selector: ${parsed.message}`, { field: 'selector' })
  }
  const issue = validateDefinition(definition)[0]
  if (issue !== undefined) {
    return failure(400, 'invalid_argument', `${issue.path}: ${issue.message}`, {
      field: issue.path,
    })
  }
  return null
}

export function feedEvents(state: MockState): FeedEvent[] {
  const time = new Date().toISOString()
  const counters: CampaignCounters[] = activeCampaigns(state).map((entry) => {
    const campaign = withCounters(entry)
    return {
      campaign_id: campaign.id,
      status: campaign.status,
      phase: campaign.current_phase,
      counters: campaign.counters,
    }
  })
  const latest =
    [...state.alerts]
      .filter((alert) => alert.resolved_at === null)
      .sort((left, right) => right.time.localeCompare(left.time))[0] ?? null
  return [
    {
      kind: 'presence',
      time,
      data: {
        online: state.nodes.filter((node) => node.online).length,
        degraded: state.degraded,
      },
    },
    { kind: 'counters', time, data: counters },
    {
      kind: 'alerts',
      time,
      data: { open: openAlertCounts(state), unacknowledged: openAlertCounts(state, true), latest },
    },
  ]
}

export function tick(state: MockState): boolean {
  let changed = false
  const now = Date.now()
  for (const entry of state.campaigns) {
    if (entry.campaign.status !== 'running') {
      continue
    }
    for (const row of entry.rows) {
      if (state.random.chance(0.97)) {
        continue
      }
      const next: Partial<Record<NodeState, NodeState>> = {
        pending: 'dispatched',
        dispatched: 'delivered',
        delivered: state.random.chance(0.03) ? 'backoff' : 'succeeded',
      }
      const target = next[row.state]
      if (target === undefined) {
        continue
      }
      row.state = target
      changed = true
      const at = new Date(now).toISOString()
      if (target === 'dispatched') {
        row.dispatched_at = at
        row.pid ??= state.random.pid()
        row.last_status = ''
        entry.campaign.last_dispatch_at = at
      }
      if (target === 'delivered') {
        row.delivered_at = at
        row.last_status = 'started'
      }
      if (target === 'succeeded') {
        row.finished_at = at
        row.last_status = 'succeeded'
        row.last_error = ''
      }
      if (target === 'backoff') {
        row.failures += 1
        row.last_status = 'failed'
        row.last_error = 'kvs: the store refused the write'
        row.next_attempt_at = new Date(now + 120_000).toISOString()
      }
    }
  }
  return changed
}

export function createHandlers(
  state: MockState,
  options: { stream?: (send: (event: FeedEvent) => void) => () => void } = {},
) {
  const stream = options.stream
  const actor = () => (state.role === 'admin' ? 'dev' : state.role)
  const guard = async (request: Request) => {
    if (state.scenario === 'slow') {
      await delay(1800)
    }
    if (state.scenario === 'signed-out' && !request.headers.get('Authorization')) {
      return failure(
        401,
        'unauthenticated',
        'log in, or send an API token as Authorization: Bearer',
        {
          login: '/api/v1/auth/login',
        },
      )
    }
    return null
  }
  const mutating = async (request: Request) => {
    const refused = await guard(request)
    if (refused !== null) {
      return refused
    }
    if (state.role === 'viewer') {
      return failure(403, 'forbidden', 'this needs the operator role; you have viewer')
    }
    return null
  }
  const unavailable = () =>
    state.scenario === 'errors'
      ? failure(503, 'unavailable', 'the inventory database is unreachable; try again shortly', {
          retry_after_seconds: 5,
        })
      : null
  const findNode = (parameters: Record<string, unknown>) =>
    state.nodeIndex.get(nodeKey(String(parameters.device), String(parameters.installation)))
  const findCampaign = (parameters: Record<string, unknown>) =>
    state.campaigns.find(({ campaign }) => campaign.id === parameters.id)
  const notFound = (what: string) => failure(404, 'not_found', `${what} not found`)
  const offline = () => failure(409, 'node_offline', 'the node is not online')

  return [
    http.get('/api/v1/me', async ({ request }) => {
      const refused = await guard(request)
      if (refused !== null) {
        return refused
      }
      return HttpResponse.json({
        subject: actor(),
        name: state.role === 'viewer' ? 'Read-only viewer' : null,
        role: state.role,
        authentication: state.scenario === 'signed-out' ? 'token' : 'dev',
        expires_at:
          state.scenario === 'signed-out' ? null : new Date(Date.now() + 3600_000).toISOString(),
      })
    }),
    http.post('/api/v1/auth/logout', () => new HttpResponse(null, { status: 204 })),
    http.get(
      '/api/v1/overview',
      async ({ request }) =>
        (await guard(request)) ?? unavailable() ?? HttpResponse.json(overview(state)),
    ),
    http.get('/api/v1/nodes', async ({ request }) => {
      const refused = (await guard(request)) ?? unavailable()
      if (refused !== null) {
        return refused
      }
      const url = new URL(request.url)
      const limit = limitOf(url)
      if (limit === null) {
        return badLimit()
      }
      const selector = url.searchParams.get('selector') ?? ''
      let nodes = state.nodes
      if (selector.trim().length > 0) {
        const selected = matching(state, selector)
        if (!selected.ok) {
          return failure(400, 'invalid_selector', selected.message, {
            position: selected.position,
            end: selected.end,
          })
        }
        nodes = selected.nodes
      }
      const online = url.searchParams.get('online')
      if (online === 'true' || online === 'false') {
        nodes = nodes.filter((node) => node.online === (online === 'true'))
      }
      const sorted = sortNodes(nodes, url.searchParams.get('sort') ?? 'device_id')
      if (sorted === null) {
        return failure(400, 'invalid_argument', 'sort names a column the inventory cannot sort by')
      }
      const page = paginate(sorted, url, limit)
      return HttpResponse.json({ ...page, items: page.items.map(toSummary) })
    }),
    http.get('/api/v1/nodes/:device/:installation', async ({ request, params: parameters }) => {
      const refused = (await guard(request)) ?? unavailable()
      if (refused !== null) {
        return refused
      }
      const node = findNode(parameters)
      return node === undefined ? notFound('node') : HttpResponse.json(toDetail(node))
    }),
    http.post(
      '/api/v1/nodes/:device/:installation/lifecycle',
      async ({ request, params: parameters }) => {
        const refused = await mutating(request)
        if (refused !== null) {
          return refused
        }
        const node = findNode(parameters)
        if (node === undefined) {
          return notFound('node')
        }
        const body = (await request.json()) as { lifecycle: Lifecycle; reason: string }
        if (!lifecycles.includes(body.lifecycle) || body.lifecycle === 'enrolled') {
          return failure(
            400,
            'invalid_argument',
            'lifecycle must be active, quarantined, retired or revoked',
          )
        }
        if (
          (body.lifecycle === 'retired' || body.lifecycle === 'revoked') &&
          state.role !== 'admin'
        ) {
          return failure(
            403,
            'forbidden',
            `setting a node ${body.lifecycle} needs the admin role; you have ${state.role}`,
          )
        }
        if (body.reason.trim().length === 0) {
          return failure(400, 'invalid_argument', 'a reason is required')
        }
        const now = new Date().toISOString()
        node.lifecycle = body.lifecycle
        node.lifecycle_reason = body.reason
        node.lifecycle_changed_at = now
        node.updated_at = now
        if (body.lifecycle === 'revoked' || body.lifecycle === 'retired') {
          node.online = false
          node.sessions = []
        }
        return HttpResponse.json(toDetail(node))
      },
    ),
    http.post('/api/v1/devices/:device/lifecycle', async ({ request, params: parameters }) => {
      const refused = await guard(request)
      if (refused !== null) {
        return refused
      }
      if (state.role !== 'admin') {
        return failure(403, 'forbidden', `this needs the admin role; you have ${state.role}`)
      }
      const body = (await request.json()) as {
        lifecycle: DeviceLifecycle['lifecycle']
        reason: string
      }
      if (!['active', 'retired', 'revoked'].includes(body.lifecycle)) {
        return failure(400, 'invalid_argument', 'lifecycle must be active, retired or revoked')
      }
      if (body.reason.trim().length === 0) {
        return failure(400, 'invalid_argument', 'a reason is required')
      }
      const changed: DeviceLifecycle = {
        device_id: String(parameters.device),
        lifecycle: body.lifecycle,
        reason: body.reason,
        changed_at: new Date().toISOString(),
        actor: actor(),
      }
      for (const node of state.nodes) {
        if (node.device_id === changed.device_id) {
          node.device = changed
          if (changed.lifecycle !== 'active') {
            node.online = false
            node.sessions = []
          }
        }
      }
      return HttpResponse.json(changed)
    }),
    http.post(
      '/api/v1/nodes/:device/:installation/sessions',
      async ({ request, params: parameters }) => {
        const refused = await mutating(request)
        if (refused !== null) {
          return refused
        }
        const node = findNode(parameters)
        if (node === undefined) {
          return notFound('node')
        }
        const session = node.sessions[0]
        if (!node.online || session === undefined) {
          return offline()
        }
        const body = (await request.json()) as { reason: string; ttl_seconds: number }
        if (body.reason.trim().length === 0) {
          return failure(400, 'invalid_argument', 'a reason is required')
        }
        if (body.ttl_seconds < 60 || body.ttl_seconds > 28800) {
          return failure(400, 'invalid_argument', 'ttl_seconds must be between 60 and 28800')
        }
        return HttpResponse.json(
          {
            pid: state.random.pid(),
            node: {
              device_id: node.device_id,
              installation_id: node.installation_id,
              namespace_id: session.namespace_id,
              nightfall: session.inner_address,
            },
          },
          { status: 201 },
        )
      },
    ),
    http.post(
      '/api/v1/nodes/:device/:installation/logs',
      async ({ request, params: parameters }) => {
        const refused = await mutating(request)
        if (refused !== null) {
          return refused
        }
        const node = findNode(parameters)
        if (node === undefined) {
          return notFound('node')
        }
        const body = (await request.json()) as { level: LogLevel; duration_seconds: number }
        if (body.duration_seconds < 1 || body.duration_seconds > 86400) {
          return failure(400, 'invalid_argument', 'duration_seconds must be between 1 and 86400')
        }
        if (!node.online) {
          return offline()
        }
        return HttpResponse.json({ stream_id: state.random.uuid(4) }, { status: 202 })
      },
    ),
    http.post(
      '/api/v1/nodes/:device/:installation/files',
      async ({ request, params: parameters }) => {
        const refused = await mutating(request)
        if (refused !== null) {
          return refused
        }
        const node = findNode(parameters)
        if (node === undefined) {
          return notFound('node')
        }
        const body = (await request.json()) as { path: string }
        if (body.path.length === 0 || body.path.length > 4096) {
          return failure(400, 'invalid_argument', 'path must be a non-empty absolute path')
        }
        if (!node.online) {
          return offline()
        }
        return HttpResponse.json({ upload_id: state.random.uuid(4) }, { status: 202 })
      },
    ),
    http.post('/api/v1/selectors/validate', async ({ request }) => {
      const refused = (await guard(request)) ?? unavailable()
      if (refused !== null) {
        return refused
      }
      const body = (await request.json()) as { selector: string }
      const selected = matching(state, body.selector)
      const response: SelectorValidation = selected.ok
        ? {
            ok: true,
            error: null,
            matched: selected.nodes.length,
            sample: selected.nodes.slice(0, 10).map(toSummary),
          }
        : {
            ok: false,
            error: { position: selected.position, end: selected.end, message: selected.message },
            matched: 0,
            sample: [],
          }
      return HttpResponse.json(response)
    }),
    http.get('/api/v1/campaigns', async ({ request }) => {
      const refused = (await guard(request)) ?? unavailable()
      if (refused !== null) {
        return refused
      }
      const url = new URL(request.url)
      const limit = limitOf(url)
      if (limit === null) {
        return badLimit()
      }
      const statuses = url.searchParams
        .getAll('status')
        .flatMap((value) => value.split(','))
        .filter((value) => value.length > 0)
      const unknown = statuses.find(
        (value) => !(campaignStatuses as readonly string[]).includes(value),
      )
      if (unknown !== undefined) {
        return failure(400, 'invalid_argument', `"${unknown}" is not a campaign status`)
      }
      const campaigns = state.campaigns
        .filter(({ campaign }) => statuses.length === 0 || statuses.includes(campaign.status))
        .sort((left, right) => right.campaign.created_at.localeCompare(left.campaign.created_at))
        .map(withCounters)
      return HttpResponse.json(paginate(campaigns, url, limit))
    }),
    http.post('/api/v1/campaigns', async ({ request }) => {
      const refused = await mutating(request)
      if (refused !== null) {
        return refused
      }
      const definition = (await request.json()) as CampaignDefinition
      const problem = definitionProblem(definition)
      if (problem !== null) {
        return problem
      }
      const now = new Date().toISOString()
      const campaign: MockCampaign['campaign'] = {
        id: state.random.uuid(4),
        name: definition.name.trim(),
        description: definition.description,
        tenant: definition.tenant ?? null,
        status: 'draft',
        kind: definition.action.kind,
        selector: definition.selector,
        action: definition.action,
        policy: definition.policy,
        created_by: actor(),
        created_at: now,
        updated_at: now,
        started_at: null,
        paused_at: null,
        finished_at: null,
        current_phase: 0,
        phase_started_at: null,
        phase_paused_seconds: 0,
        gate_override_after: null,
        pause_kind: null,
        pause_reason: null,
        abort_reason: null,
        last_dispatch_at: null,
        version: 1,
      }
      const entry: MockCampaign = { campaign, salt: state.random.hex(32), rows: [], events: [] }
      eventFactory(entry.events)(Date.now(), 'created', actor(), { name: campaign.name })
      state.campaigns.unshift(entry)
      return HttpResponse.json(withCounters(entry), {
        status: 201,
        headers: { Location: `/api/v1/campaigns/${campaign.id}` },
      })
    }),
    http.get('/api/v1/campaigns/:id', async ({ request, params: parameters }) => {
      const refused = (await guard(request)) ?? unavailable()
      if (refused !== null) {
        return refused
      }
      const entry = findCampaign(parameters)
      return entry === undefined ? notFound('campaign') : HttpResponse.json(withCounters(entry))
    }),
    http.put('/api/v1/campaigns/:id', async ({ request, params: parameters }) => {
      const refused = await mutating(request)
      if (refused !== null) {
        return refused
      }
      const entry = findCampaign(parameters)
      if (entry === undefined) {
        return notFound('campaign')
      }
      if (entry.campaign.status !== 'draft') {
        return failure(409, 'not_draft', 'only a draft campaign can be edited')
      }
      const update = (await request.json()) as CampaignUpdate
      if (update.version !== entry.campaign.version) {
        return failure(409, 'version_conflict', 'the campaign changed since it was read')
      }
      const problem = definitionProblem(update)
      if (problem !== null) {
        return problem
      }
      Object.assign(entry.campaign, {
        name: update.name.trim(),
        description: update.description,
        tenant: update.tenant ?? null,
        selector: update.selector,
        action: update.action,
        policy: update.policy,
        kind: update.action.kind,
        version: entry.campaign.version + 1,
        updated_at: new Date().toISOString(),
      })
      eventFactory(entry.events)(Date.now(), 'updated', actor(), {})
      return HttpResponse.json(withCounters(entry))
    }),
    ...campaignCommands.map((command) =>
      http.post(`/api/v1/campaigns/:id/${command}`, async ({ request, params: parameters }) => {
        const refused = await mutating(request)
        if (refused !== null) {
          return refused
        }
        const text = await request.text()
        const body =
          text.length > 0 ? (JSON.parse(text) as { reason?: string; override_gate?: boolean }) : {}
        return applyCommand(state, String(parameters.id), command, body, actor())
      }),
    ),
    http.get('/api/v1/campaigns/:id/nodes', async ({ request, params: parameters }) => {
      const refused = (await guard(request)) ?? unavailable()
      if (refused !== null) {
        return refused
      }
      const entry = findCampaign(parameters)
      if (entry === undefined) {
        return notFound('campaign')
      }
      const url = new URL(request.url)
      const limit = limitOf(url)
      if (limit === null) {
        return badLimit()
      }
      if (entry.campaign.status === 'archived') {
        return HttpResponse.json({ items: [], next_cursor: null })
      }
      const states = url.searchParams
        .getAll('state')
        .flatMap((value) => value.split(','))
        .filter((value) => value.length > 0)
      const phaseFilter = url.searchParams.get('phase')
      const rows = entry.rows.filter(
        (row) =>
          (states.length === 0 || states.includes(row.state)) &&
          (phaseFilter === null || row.phase === Number(phaseFilter)),
      )
      return HttpResponse.json(paginate(rows, url, limit))
    }),
    http.post('/api/v1/campaigns/:id/nodes/retry', async ({ request, params: parameters }) => {
      const refused = await mutating(request)
      if (refused !== null) {
        return refused
      }
      const entry = findCampaign(parameters)
      if (entry === undefined) {
        return notFound('campaign')
      }
      if (entry.campaign.status !== 'running' && entry.campaign.status !== 'paused') {
        return failure(
          409,
          'invalid_transition',
          'invalid campaign transition: only a running or paused campaign retries nodes',
        )
      }
      const body = (await request.json()) as {
        states?: NodeState[]
        nodes?: { device_id: string; installation_id: string }[]
        reason: string
      }
      if (body.reason.trim().length === 0) {
        return failure(400, 'invalid_argument', 'a reason is required')
      }
      const states = body.states ?? ['failed', 'unknown']
      const chosen =
        body.nodes === undefined
          ? null
          : new Set(body.nodes.map((node) => nodeKey(node.device_id, node.installation_id)))
      let count = 0
      for (const row of entry.rows) {
        if (
          count < 10_000 &&
          states.includes(row.state) &&
          ['failed', 'unknown', 'cancelled'].includes(row.state) &&
          (chosen === null || chosen.has(nodeKey(row.device_id, row.installation_id)))
        ) {
          row.state = 'pending'
          row.attempt += 1
          row.failures = 0
          row.unreached = 0
          row.pid = null
          row.reaped_at = null
          row.finished_at = null
          row.next_attempt_at = null
          row.deadline_at = null
          count += 1
        }
      }
      eventFactory(entry.events)(Date.now(), 'nodes_retried', actor(), {
        count,
        reason: body.reason,
      })
      return HttpResponse.json({ count })
    }),
    http.post('/api/v1/campaigns/:id/nodes/resolve', async ({ request, params: parameters }) => {
      const refused = await mutating(request)
      if (refused !== null) {
        return refused
      }
      const entry = findCampaign(parameters)
      if (entry === undefined) {
        return notFound('campaign')
      }
      const body = (await request.json()) as {
        nodes: { device_id: string; installation_id: string }[]
        outcome: 'succeeded' | 'failed'
        reason: string
      }
      if (body.reason.trim().length === 0) {
        return failure(400, 'invalid_argument', 'a reason is required')
      }
      if (body.nodes.length === 0) {
        return failure(400, 'invalid_argument', 'name the nodes to resolve')
      }
      const chosen = new Set(
        body.nodes.map((node) => nodeKey(node.device_id, node.installation_id)),
      )
      let count = 0
      for (const row of entry.rows) {
        if (row.state === 'unknown' && chosen.has(nodeKey(row.device_id, row.installation_id))) {
          row.state = body.outcome
          row.last_status = 'resolved'
          row.last_error = body.reason
          row.finished_at = new Date().toISOString()
          if (body.outcome === 'failed') {
            row.failures = Math.max(1, row.failures)
          }
          count += 1
        }
      }
      eventFactory(entry.events)(Date.now(), 'nodes_resolved', actor(), {
        count,
        outcome: body.outcome,
        reason: body.reason,
      })
      return HttpResponse.json({ count })
    }),
    http.get('/api/v1/campaigns/:id/events', async ({ request, params: parameters }) => {
      const refused = (await guard(request)) ?? unavailable()
      if (refused !== null) {
        return refused
      }
      const entry = findCampaign(parameters)
      if (entry === undefined) {
        return notFound('campaign')
      }
      if (entry.campaign.status === 'archived') {
        return HttpResponse.json({ items: [], next_cursor: null })
      }
      const url = new URL(request.url)
      const limit = limitOf(url)
      if (limit === null) {
        return badLimit()
      }
      const after = Number(url.searchParams.get('after') ?? 0) || 0
      const items = entry.events.filter((event) => event.id > after).slice(0, limit)
      const last = items[items.length - 1]
      return HttpResponse.json({
        items,
        next_cursor: items.length === limit && last !== undefined ? String(last.id) : null,
      })
    }),
    http.get('/api/v1/campaigns/:id/overlap', async ({ request, params: parameters }) => {
      const refused = (await guard(request)) ?? unavailable()
      if (refused !== null) {
        return refused
      }
      const entry = findCampaign(parameters)
      return entry === undefined
        ? notFound('campaign')
        : HttpResponse.json<Overlap>(overlapOf(state, entry))
    }),
    http.get('/api/v1/campaigns/:id/gates', async ({ request, params: parameters }) => {
      const refused = (await guard(request)) ?? unavailable()
      if (refused !== null) {
        return refused
      }
      const entry = findCampaign(parameters)
      if (entry === undefined) {
        return notFound('campaign')
      }
      return HttpResponse.json(gatesOf(entry, Date.now(), state.degraded))
    }),
    http.get('/api/v1/alerts', async ({ request }) => {
      const refused = (await guard(request)) ?? unavailable()
      if (refused !== null) {
        return refused
      }
      const url = new URL(request.url)
      const limit = limitOf(url)
      if (limit === null) {
        return badLimit()
      }
      const filter = url.searchParams.get('state') ?? 'open'
      const alerts = state.alerts
        .filter((alert) => filter === 'all' || alert.resolved_at === null)
        .sort((left, right) => right.time.localeCompare(left.time) || right.id - left.id)
      return HttpResponse.json(paginate(alerts, url, limit))
    }),
    http.get('/api/v1/alerts/:id', async ({ request, params: parameters }) => {
      const refused = (await guard(request)) ?? unavailable()
      if (refused !== null) {
        return refused
      }
      const alert = state.alerts.find((candidate) => String(candidate.id) === parameters.id)
      return alert === undefined ? notFound('alert') : HttpResponse.json(alert)
    }),
    http.post('/api/v1/alerts/:id/:command', async ({ request, params: parameters }) => {
      const refused = await mutating(request)
      if (refused !== null) {
        return refused
      }
      const alert = state.alerts.find((candidate) => String(candidate.id) === parameters.id)
      if (alert === undefined) {
        return notFound('alert')
      }
      const now = new Date().toISOString()
      const notify = (transition: 'acknowledged' | 'resolved') => {
        const receivers = new Set(
          alert.deliveries
            .filter(
              (delivery) =>
                delivery.transition === 'opened' || delivery.transition === 're_escalated',
            )
            .map((delivery) => delivery.receiver),
        )
        for (const receiver of receivers) {
          alert.deliveries.push({
            receiver,
            transition,
            state: 'delivered',
            attempts: 1,
            last_error: null,
            next_attempt_at: null,
            delivered_at: now,
          })
        }
      }
      if (parameters.command === 'acknowledge') {
        if (alert.acknowledged_at === null) {
          notify('acknowledged')
        }
        alert.acknowledged_at ??= now
        alert.acknowledged_by ??= actor()
      } else if (parameters.command === 'resolve') {
        if (alert.resolved_at === null) {
          notify('resolved')
        }
        alert.resolved_at ??= now
        alert.resolved_by ??= actor()
      } else {
        return notFound('route')
      }
      return HttpResponse.json(alert)
    }),
    ...(stream === undefined
      ? []
      : [
          sse<{ presence: string; counters: string; alerts: string }>(
            '/api/v1/stream',
            ({ client, request }) => {
              const stop = stream((event) =>
                client.send({ event: event.kind, data: JSON.stringify(event) }),
              )
              request.signal.addEventListener('abort', stop)
            },
          ),
        ]),
  ]
}

export function createLiveStream(state: MockState) {
  const listeners = new Set<(event: FeedEvent) => void>()
  let timer: ReturnType<typeof setInterval> | null = null
  const ensureTimer = () => {
    if (timer !== null) {
      return
    }
    timer = setInterval(() => {
      if (!tick(state)) {
        return
      }
      for (const event of feedEvents(state)) {
        for (const listener of listeners) {
          listener(event)
        }
      }
    }, 4000)
  }
  return (send: (event: FeedEvent) => void) => {
    listeners.add(send)
    for (const event of feedEvents(state)) {
      send(event)
    }
    ensureTimer()
    return () => {
      listeners.delete(send)
      if (listeners.size === 0 && timer !== null) {
        clearInterval(timer)
        timer = null
      }
    }
  }
}
