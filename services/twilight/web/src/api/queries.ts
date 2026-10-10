import {
  keepPreviousData,
  useMutation,
  useQueries,
  useQuery,
  useQueryClient,
} from '@tanstack/react-query'
import type { UseQueryResult } from '@tanstack/react-query'
import { apiRequest, errorMessage, queryString } from './client'
import type {
  Alert,
  Campaign,
  CampaignDefinition,
  CampaignEvent,
  CampaignNode,
  CampaignStatus,
  CampaignUpdate,
  CountResult,
  DeviceLifecycle,
  FileUploadAccepted,
  Gates,
  InteractiveSession,
  Lifecycle,
  LogLevel,
  LogStreamAccepted,
  Me,
  NodeDetail,
  NodeKey,
  NodeState,
  NodeSummary,
  Overlap,
  Overview,
  Page,
  SelectorValidation,
} from './types'

export interface NodeListParameters {
  selector: string
  online: 'any' | 'online' | 'offline'
  sort: string
  limit: number
  cursor: string | null
}

export interface CampaignListParameters {
  status: CampaignStatus | 'active' | 'all'
  limit: number
  cursor: string | null
}

export interface CampaignNodeParameters {
  state: NodeState | null
  phase: number | null
  limit: number
  cursor: string | null
}

export interface AlertListParameters {
  state: 'open' | 'all'
  limit: number
  cursor: string | null
}

export const queryKeys = {
  me: ['me'] as const,
  overview: ['overview'] as const,
  nodes: (parameters: NodeListParameters) => ['nodes', parameters] as const,
  node: (deviceId: string, installationId: string) => ['node', deviceId, installationId] as const,
  campaigns: (parameters: CampaignListParameters) => ['campaigns', parameters] as const,
  campaign: (campaignId: string) => ['campaign', campaignId] as const,
  campaignNodes: (campaignId: string, parameters: CampaignNodeParameters) =>
    ['campaign', campaignId, 'nodes', parameters] as const,
  everyCampaignNode: (campaignId: string, states: NodeState[], phase: number | null) =>
    ['campaign', campaignId, 'nodes', 'every', states, phase] as const,
  campaignEvents: (campaignId: string) => ['campaign', campaignId, 'events'] as const,
  campaignGates: (campaignId: string) => ['campaign', campaignId, 'gates'] as const,
  campaignOverlap: (campaignId: string) => ['campaign', campaignId, 'overlap'] as const,
  alerts: (parameters: AlertListParameters) => ['alerts', parameters] as const,
  everyCampaign: (status: CampaignListParameters['status']) =>
    ['campaigns', 'every', status] as const,
  everyAlert: (state: AlertListParameters['state']) => ['alerts', 'every', state] as const,
  alert: (alertId: number) => ['alerts', 'one', alertId] as const,
  criticalAlerts: ['alerts', 'critical'] as const,
  selectorCount: (selector: string) => ['selector-count', selector] as const,
  hostnames: (installationIds: string[]) => ['nodes', 'hostnames', installationIds] as const,
}

export function useMe() {
  return useQuery({
    queryKey: queryKeys.me,
    queryFn: ({ signal }) => apiRequest<Me>('GET', '/api/v1/me', undefined, signal),
    retry: false,
    staleTime: 60_000,
  })
}

export function useOverview() {
  return useQuery({
    queryKey: queryKeys.overview,
    queryFn: ({ signal }) => apiRequest<Overview>('GET', '/api/v1/overview', undefined, signal),
    refetchInterval: 30_000,
  })
}

export function useNodes(parameters: NodeListParameters) {
  return useQuery({
    queryKey: queryKeys.nodes(parameters),
    queryFn: ({ signal }) =>
      apiRequest<Page<NodeSummary>>(
        'GET',
        `/api/v1/nodes${queryString({
          selector: parameters.selector,
          online:
            parameters.online === 'any' ? null : parameters.online === 'online' ? 'true' : 'false',
          sort: parameters.sort,
          limit: parameters.limit,
          cursor: parameters.cursor,
        })}`,
        undefined,
        signal,
      ),
    placeholderData: keepPreviousData,
    refetchInterval: 30_000,
  })
}

function nodePath(deviceId: string, installationId: string) {
  return `/api/v1/nodes/${encodeURIComponent(deviceId)}/${encodeURIComponent(installationId)}`
}

export function useNode(deviceId: string, installationId: string) {
  return useQuery({
    queryKey: queryKeys.node(deviceId, installationId),
    queryFn: ({ signal }) =>
      apiRequest<NodeDetail>('GET', nodePath(deviceId, installationId), undefined, signal),
    refetchInterval: 30_000,
  })
}

export const pageLimit = 500

export const searchLimit = 2000

export interface Collected<Item> {
  items: Item[]
  truncated: boolean
}

export async function collectPages<Item>(
  path: (cursor: string | null) => string,
  maximum: number,
  signal?: AbortSignal,
  found: (item: Item) => boolean = () => false,
): Promise<Collected<Item>> {
  const items: Item[] = []
  let cursor: string | null = null
  do {
    const page: Page<Item> = await apiRequest<Page<Item>>('GET', path(cursor), undefined, signal)
    items.push(...page.items)
    cursor = page.next_cursor
  } while (cursor !== null && items.length < maximum && !items.some(found))
  return { items, truncated: cursor !== null }
}

function campaignStatusParameter(status: CampaignListParameters['status']) {
  return status === 'all' ? null : status === 'active' ? 'running,paused' : status
}

export function useCampaigns(parameters: CampaignListParameters, enabled = true) {
  return useQuery({
    queryKey: queryKeys.campaigns(parameters),
    queryFn: ({ signal }) =>
      apiRequest<Page<Campaign>>(
        'GET',
        `/api/v1/campaigns${queryString({
          status: campaignStatusParameter(parameters.status),
          limit: parameters.limit,
          cursor: parameters.cursor,
        })}`,
        undefined,
        signal,
      ),
    enabled,
    placeholderData: keepPreviousData,
    refetchInterval: 30_000,
  })
}

export function useEveryCampaign(status: CampaignListParameters['status'], enabled: boolean) {
  return useQuery({
    queryKey: queryKeys.everyCampaign(status),
    queryFn: ({ signal }) =>
      collectPages<Campaign>(
        (cursor) =>
          `/api/v1/campaigns${queryString({
            status: campaignStatusParameter(status),
            limit: pageLimit,
            cursor,
          })}`,
        searchLimit,
        signal,
      ),
    enabled,
    placeholderData: keepPreviousData,
  })
}

export function useCampaign(campaignId: string) {
  return useQuery({
    queryKey: queryKeys.campaign(campaignId),
    queryFn: ({ signal }) =>
      apiRequest<Campaign>(
        'GET',
        `/api/v1/campaigns/${encodeURIComponent(campaignId)}`,
        undefined,
        signal,
      ),
    refetchInterval: 15_000,
  })
}

export function useCampaignGates(campaignId: string, enabled: boolean) {
  return useQuery({
    queryKey: queryKeys.campaignGates(campaignId),
    queryFn: ({ signal }) =>
      apiRequest<Gates>(
        'GET',
        `/api/v1/campaigns/${encodeURIComponent(campaignId)}/gates`,
        undefined,
        signal,
      ),
    enabled,
    refetchInterval: 15_000,
  })
}

export function useCampaignOverlap(campaignId: string, enabled: boolean) {
  return useQuery({
    queryKey: queryKeys.campaignOverlap(campaignId),
    queryFn: ({ signal }) =>
      apiRequest<Overlap>(
        'GET',
        `/api/v1/campaigns/${encodeURIComponent(campaignId)}/overlap`,
        undefined,
        signal,
      ),
    enabled,
  })
}

export function useCampaignNodes(campaignId: string, parameters: CampaignNodeParameters) {
  return useQuery({
    queryKey: queryKeys.campaignNodes(campaignId, parameters),
    queryFn: ({ signal }) =>
      apiRequest<Page<CampaignNode>>(
        'GET',
        `/api/v1/campaigns/${encodeURIComponent(campaignId)}/nodes${queryString({
          state: parameters.state,
          phase: parameters.phase,
          limit: parameters.limit,
          cursor: parameters.cursor,
        })}`,
        undefined,
        signal,
      ),
    placeholderData: keepPreviousData,
    refetchInterval: 15_000,
  })
}

export function useEveryCampaignNode(
  campaignId: string,
  states: NodeState[],
  phase: number | null,
  maximum: number,
  enabled: boolean,
) {
  return useQuery({
    queryKey: queryKeys.everyCampaignNode(campaignId, states, phase),
    queryFn: ({ signal }) =>
      collectPages<CampaignNode>(
        (cursor) =>
          `/api/v1/campaigns/${encodeURIComponent(campaignId)}/nodes${queryString({
            state: states.join(','),
            phase,
            limit: pageLimit,
            cursor,
          })}`,
        maximum,
        signal,
      ),
    enabled,
  })
}

export const historyWindow = 2000

export interface EventWindow {
  events: CampaignEvent[]
  dropped: boolean
}

export function useCampaignEvents(campaignId: string) {
  return useQuery({
    queryKey: queryKeys.campaignEvents(campaignId),
    queryFn: async ({ client, queryKey, signal }): Promise<EventWindow> => {
      const previous = client.getQueryData<EventWindow>(queryKey)
      let events = previous?.events ?? []
      let dropped = previous?.dropped ?? false
      for (;;) {
        const page = await apiRequest<Page<CampaignEvent>>(
          'GET',
          `/api/v1/campaigns/${encodeURIComponent(campaignId)}/events${queryString({
            after: events.at(-1)?.id,
            limit: pageLimit,
          })}`,
          undefined,
          signal,
        )
        events = [...events, ...page.items]
        if (events.length > historyWindow) {
          events = events.slice(-historyWindow)
          dropped = true
        }
        if (page.next_cursor === null) {
          return { events, dropped }
        }
      }
    },
    refetchInterval: 15_000,
  })
}

export function useAlerts(parameters: AlertListParameters, enabled = true) {
  return useQuery({
    enabled,
    queryKey: queryKeys.alerts(parameters),
    queryFn: ({ signal }) =>
      apiRequest<Page<Alert>>(
        'GET',
        `/api/v1/alerts${queryString({
          state: parameters.state,
          limit: parameters.limit,
          cursor: parameters.cursor,
        })}`,
        undefined,
        signal,
      ),
    placeholderData: keepPreviousData,
    refetchInterval: 30_000,
  })
}

export function useAlert(alertId: number | null) {
  return useQuery({
    enabled: alertId !== null,
    queryKey: queryKeys.alert(alertId ?? 0),
    queryFn: ({ signal }) =>
      apiRequest<Alert>('GET', `/api/v1/alerts/${alertId}`, undefined, signal),
    refetchInterval: 15_000,
  })
}

export function useEveryAlert(state: AlertListParameters['state'], enabled: boolean) {
  return useQuery({
    queryKey: queryKeys.everyAlert(state),
    queryFn: ({ signal }) =>
      collectPages<Alert>(
        (cursor) => `/api/v1/alerts${queryString({ state, limit: pageLimit, cursor })}`,
        searchLimit,
        signal,
      ),
    enabled,
    placeholderData: keepPreviousData,
  })
}

export function waitsForAcknowledgement(alert: Alert): boolean {
  return alert.severity === 'critical' && alert.acknowledged_at === null
}

export function useCriticalAlerts(enabled: boolean) {
  return useQuery({
    queryKey: queryKeys.criticalAlerts,
    queryFn: ({ signal }) =>
      collectPages<Alert>(
        (cursor) => `/api/v1/alerts${queryString({ state: 'open', limit: pageLimit, cursor })}`,
        searchLimit,
        signal,
        waitsForAcknowledgement,
      ),
    enabled,
  })
}

export function useMatchedCount(selector: string, enabled: boolean) {
  const query = useQuery({
    queryKey: queryKeys.selectorCount(selector),
    queryFn: ({ signal }) => validateSelector(selector, signal),
    enabled,
    staleTime: 60_000,
    refetchInterval: 60_000,
  })
  const validation = query.data
  return {
    matched: validation?.ok === true ? validation.matched : null,
    failure:
      query.error !== null
        ? errorMessage(query.error)
        : validation?.ok === false
          ? (validation.error?.message ?? 'the selector is not valid')
          : null,
  }
}

export const hostnameChunk = 64

function hostnamesOf(results: UseQueryResult<Page<NodeSummary>>[]): Map<string, string> {
  const names = new Map<string, string>()
  for (const result of results) {
    for (const node of result.data?.items ?? []) {
      if (node.hostname !== null) {
        names.set(`${node.device_id}/${node.installation_id}`, node.hostname)
      }
    }
  }
  return names
}

export function useHostnames(nodes: NodeKey[]) {
  const installations = [...new Set(nodes.map((node) => node.installation_id))].sort()
  const chunks: string[][] = []
  for (let start = 0; start < installations.length; start += hostnameChunk) {
    chunks.push(installations.slice(start, start + hostnameChunk))
  }
  return useQueries({
    queries: chunks.map((chunk) => ({
      queryKey: queryKeys.hostnames(chunk),
      queryFn: ({ signal }: { signal: AbortSignal }) =>
        apiRequest<Page<NodeSummary>>(
          'GET',
          `/api/v1/nodes${queryString({
            selector: `installation_id in [${chunk.map((installation) => JSON.stringify(installation)).join(', ')}]`,
            limit: pageLimit,
          })}`,
          undefined,
          signal,
        ),
      staleTime: 5 * 60_000,
    })),
    combine: hostnamesOf,
  })
}

export function useCampaignNames(campaignIds: string[]) {
  const unique = [...new Set(campaignIds)]
  const results = useQueries({
    queries: unique.map((campaignId) => ({
      queryKey: queryKeys.campaign(campaignId),
      queryFn: ({ signal }: { signal: AbortSignal }) =>
        apiRequest<Campaign>(
          'GET',
          `/api/v1/campaigns/${encodeURIComponent(campaignId)}`,
          undefined,
          signal,
        ),
      staleTime: 60_000,
    })),
  })
  const campaigns = new Map<string, Campaign>()
  for (const result of results) {
    if (result.data !== undefined) {
      campaigns.set(result.data.id, result.data)
    }
  }
  return campaigns
}

export function validateSelector(selector: string, signal?: AbortSignal) {
  return apiRequest<SelectorValidation>('POST', '/api/v1/selectors/validate', { selector }, signal)
}

export type CampaignCommand = 'start' | 'pause' | 'resume' | 'abort' | 'complete' | 'archive'

export interface CampaignCommandInput {
  command: CampaignCommand
  reason?: string
  override_gate?: boolean
}

export function useCampaignCommand(campaignId: string) {
  const client = useQueryClient()
  return useMutation({
    mutationFn: ({ command, ...body }: CampaignCommandInput) =>
      apiRequest<Campaign>(
        'POST',
        `/api/v1/campaigns/${encodeURIComponent(campaignId)}/${command}`,
        body,
      ),
    onSuccess: (campaign) => {
      client.setQueryData(queryKeys.campaign(campaignId), campaign)
      void client.invalidateQueries({ queryKey: ['campaign', campaignId] })
      void client.invalidateQueries({ queryKey: ['campaigns'] })
      void client.invalidateQueries({ queryKey: queryKeys.overview })
    },
  })
}

export function useCreateCampaign() {
  const client = useQueryClient()
  return useMutation({
    mutationFn: (definition: CampaignDefinition) =>
      apiRequest<Campaign>('POST', '/api/v1/campaigns', definition),
    onSuccess: (campaign) => {
      client.setQueryData(queryKeys.campaign(campaign.id), campaign)
      void client.invalidateQueries({ queryKey: ['campaigns'] })
    },
  })
}

export function useUpdateCampaign(campaignId: string) {
  const client = useQueryClient()
  return useMutation({
    mutationFn: (update: CampaignUpdate) =>
      apiRequest<Campaign>('PUT', `/api/v1/campaigns/${encodeURIComponent(campaignId)}`, update),
    onSuccess: (campaign) => {
      client.setQueryData(queryKeys.campaign(campaign.id), campaign)
      void client.invalidateQueries({ queryKey: ['campaigns'] })
    },
  })
}

export function startCampaign(campaignId: string) {
  return apiRequest<Campaign>(
    'POST',
    `/api/v1/campaigns/${encodeURIComponent(campaignId)}/start`,
    {},
  )
}

export interface RetryInput {
  states: NodeState[]
  nodes?: NodeKey[]
  reason: string
}

export interface ResolveInput {
  nodes: NodeKey[]
  outcome: 'succeeded' | 'failed'
  reason: string
}

export function useRetryCampaignNodes(campaignId: string) {
  const client = useQueryClient()
  return useMutation({
    mutationFn: (input: RetryInput) =>
      apiRequest<CountResult>(
        'POST',
        `/api/v1/campaigns/${encodeURIComponent(campaignId)}/nodes/retry`,
        input,
      ),
    onSuccess: () => void client.invalidateQueries({ queryKey: ['campaign', campaignId] }),
  })
}

export function useResolveCampaignNodes(campaignId: string) {
  const client = useQueryClient()
  return useMutation({
    mutationFn: (input: ResolveInput) =>
      apiRequest<CountResult>(
        'POST',
        `/api/v1/campaigns/${encodeURIComponent(campaignId)}/nodes/resolve`,
        input,
      ),
    onSuccess: () => void client.invalidateQueries({ queryKey: ['campaign', campaignId] }),
  })
}

export function useLifecycleChange(deviceId: string, installationId: string) {
  const client = useQueryClient()
  return useMutation({
    mutationFn: (input: { lifecycle: Exclude<Lifecycle, 'enrolled'>; reason: string }) =>
      apiRequest<NodeDetail>('POST', `${nodePath(deviceId, installationId)}/lifecycle`, input),
    onSuccess: (node) => {
      client.setQueryData(queryKeys.node(deviceId, installationId), node)
      void client.invalidateQueries({ queryKey: ['nodes'] })
      void client.invalidateQueries({ queryKey: queryKeys.overview })
    },
  })
}

export function useDeviceLifecycleChange(deviceId: string) {
  const client = useQueryClient()
  return useMutation({
    mutationFn: (input: { lifecycle: DeviceLifecycle['lifecycle']; reason: string }) =>
      apiRequest<DeviceLifecycle>(
        'POST',
        `/api/v1/devices/${encodeURIComponent(deviceId)}/lifecycle`,
        input,
      ),
    onSuccess: () => {
      void client.invalidateQueries({ queryKey: ['node', deviceId] })
      void client.invalidateQueries({ queryKey: ['nodes'] })
      void client.invalidateQueries({ queryKey: queryKeys.overview })
    },
  })
}

export function useOpenSession(deviceId: string, installationId: string) {
  return useMutation({
    mutationFn: (input: { reason: string; ttl_seconds: number }) =>
      apiRequest<InteractiveSession>(
        'POST',
        `${nodePath(deviceId, installationId)}/sessions`,
        input,
      ),
  })
}

export function useStreamLogs(deviceId: string, installationId: string) {
  return useMutation({
    mutationFn: (input: { level: LogLevel; duration_seconds: number }) =>
      apiRequest<LogStreamAccepted>('POST', `${nodePath(deviceId, installationId)}/logs`, input),
  })
}

export function useCollectFile(deviceId: string, installationId: string) {
  return useMutation({
    mutationFn: (input: { path: string }) =>
      apiRequest<FileUploadAccepted>('POST', `${nodePath(deviceId, installationId)}/files`, input),
  })
}

export function useAlertCommand() {
  const client = useQueryClient()
  return useMutation({
    mutationFn: ({ alertId, command }: { alertId: number; command: 'acknowledge' | 'resolve' }) =>
      apiRequest<Alert>('POST', `/api/v1/alerts/${alertId}/${command}`, {}),
    onSuccess: () => {
      void client.invalidateQueries({ queryKey: ['alerts'] })
      void client.invalidateQueries({ queryKey: queryKeys.overview })
    },
  })
}
