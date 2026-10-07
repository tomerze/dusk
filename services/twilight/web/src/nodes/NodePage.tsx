import {
  ActionIcon,
  Alert,
  Anchor,
  Badge,
  CloseButton,
  CopyButton,
  EmptyState,
  Group,
  Skeleton,
  Stack,
  Tabs,
  Text,
  TextInput,
  Tooltip,
} from '@mantine/core'
import {
  IconArchive,
  IconCheck,
  IconCopy,
  IconHistory,
  IconLock,
  IconSearch,
  IconShieldX,
} from '@tabler/icons-react'
import type { ReactNode } from 'react'
import { useState } from 'react'
import { Link, useParams, useSearchParams } from 'react-router'
import { ApiError } from '../api/client'
import { useCampaignNames, useNode } from '../api/queries'
import type {
  Campaign,
  CampaignNode,
  DeviceLifecycle,
  JsonValue,
  Lifecycle,
  NodeDetail,
  ProcessStatus,
} from '../api/types'
import { NotFound } from '../app/RouteError'
import { actionLabels } from '../campaigns/display'
import { DataTable } from '../components/DataTable'
import { MonoId } from '../components/MonoId'
import { PageHeader } from '../components/PageHeader'
import { Presence } from '../components/Presence'
import { QueryError } from '../components/QueryError'
import { RelativeTime } from '../components/RelativeTime'
import { Section } from '../components/Section'
import { StatusPill } from '../components/StatusPill'
import {
  lifecycleStyles,
  nodeStateStyles,
  processAwaitingStyle,
  processStatusStyles,
} from '../components/status'
import type { TableColumn } from '../components/table'
import { columnHelper } from '../components/table'
import { formatNumber, middleEllipsis, plural } from '../format'
import { NodeActions } from './NodeActions'
import classes from './NodePage.module.css'

const byteUnits = ['B', 'KiB', 'MiB', 'GiB', 'TiB']

function formatBytes(value: number): string {
  let amount = value
  let unit = 0
  while (amount >= 1024 && unit < byteUnits.length - 1) {
    amount /= 1024
    unit += 1
  }
  return `${Number.isInteger(amount) ? amount : amount.toFixed(1)} ${byteUnits[unit]}`
}

function factText(value: JsonValue): string {
  return typeof value === 'string' ? value : JSON.stringify(value)
}

function Field({ label, children }: { label: string; children: ReactNode }) {
  return (
    <div className={classes.field}>
      <dt>{label}</dt>
      <dd>{children}</dd>
    </div>
  )
}

function maybe(value: string | null, mono = false): ReactNode {
  return value === null ? (
    <Text component="span" size="sm" c="dimmed">
      unknown
    </Text>
  ) : (
    <span className={mono ? 'mono' : undefined}>{value}</span>
  )
}

const processHelper = columnHelper<CampaignNode>()

function processColumns(campaigns: Map<string, Campaign>): TableColumn<CampaignNode>[] {
  return [
    processHelper.display({
      id: 'pid',
      header: 'Pid',
      meta: { label: 'Pid', width: 210 },
      cell: ({ row }) =>
        row.original.pid === null ? (
          <Text size="sm" c="dimmed">
            not sent
          </Text>
        ) : (
          <MonoId value={row.original.pid} label="pid" maximum={20} />
        ),
    }),
    processHelper.display({
      id: 'campaign',
      header: 'Campaign',
      meta: { label: 'Campaign', mobile: 'title' },
      cell: ({ row }) => {
        const campaign = campaigns.get(row.original.campaign_id)
        return (
          <Stack gap={0}>
            <Anchor
              component={Link}
              to={`/campaigns/${row.original.campaign_id}`}
              size="sm"
              fw={600}
              c="var(--twilight-text-strong)"
              onClick={(event) => event.stopPropagation()}
            >
              {campaign?.name ?? middleEllipsis(row.original.campaign_id, 18)}
            </Anchor>
            <Text size="xs" c="dimmed">
              {campaign === undefined
                ? `Phase ${row.original.phase + 1}, attempt ${row.original.attempt}`
                : `${actionLabels[campaign.kind]}, attempt ${row.original.attempt}`}
            </Text>
          </Stack>
        )
      },
    }),
    processHelper.display({
      id: 'state',
      header: 'Row',
      meta: { label: 'Row', width: 130 },
      cell: ({ row }) => <StatusPill status={nodeStateStyles[row.original.state]} />,
    }),
    processHelper.display({
      id: 'status',
      header: 'Process',
      meta: { label: 'Process', width: 160 },
      cell: ({ row }) => {
        const status = row.original.last_status
        if (status.length === 0) {
          return row.original.pid === null ? (
            <Text size="sm" c="dimmed">
              not sent
            </Text>
          ) : (
            <StatusPill status={processAwaitingStyle} />
          )
        }
        return (
          <StatusPill
            status={
              processStatusStyles[status as ProcessStatus] ?? {
                ...processAwaitingStyle,
                label: status.replaceAll('_', ' '),
                description: '',
              }
            }
          />
        )
      },
    }),
    processHelper.display({
      id: 'error',
      header: 'Last error',
      meta: { label: 'Last error' },
      cell: ({ row }) =>
        row.original.last_error.length === 0 ? (
          <Text size="sm" c="dimmed">
            none
          </Text>
        ) : (
          <Tooltip label={row.original.last_error}>
            <Text size="sm" className="mono" c="red" truncate="end" maw={320}>
              {row.original.last_error}
            </Text>
          </Tooltip>
        ),
    }),
    processHelper.display({
      id: 'dispatched',
      header: 'Sent',
      meta: { label: 'Sent', width: 110 },
      cell: ({ row }) => <RelativeTime value={row.original.dispatched_at} fallback="not sent" />,
    }),
    processHelper.display({
      id: 'reaped',
      header: 'Reaped',
      meta: { label: 'Reaped', width: 110 },
      cell: ({ row }) => <RelativeTime value={row.original.reaped_at} fallback="not yet" />,
    }),
  ]
}

function Facts({ node }: { node: NodeDetail }) {
  const [filter, setFilter] = useState('')
  const entries = Object.entries(node.facts).sort(([left], [right]) => left.localeCompare(right))
  const needle = filter.trim().toLowerCase()
  const shown = entries.filter(
    ([key, value]) =>
      needle.length === 0 ||
      key.toLowerCase().includes(needle) ||
      factText(value).toLowerCase().includes(needle),
  )
  return (
    <Stack gap="sm">
      <Group justify="space-between" gap="sm" wrap="wrap">
        <TextInput
          aria-label="Filter facts"
          placeholder="Filter by key or value"
          leftSection={<IconSearch size={16} aria-hidden="true" />}
          value={filter}
          onChange={(event) => setFilter(event.currentTarget.value)}
          rightSection={
            filter.length > 0 ? (
              <CloseButton size="sm" aria-label="Clear the filter" onClick={() => setFilter('')} />
            ) : null
          }
          style={{ flex: '1 1 260px', maxWidth: 420 }}
        />
        <Text size="xs" c="dimmed">
          {node.facts_read_at === null ? (
            'Never read'
          ) : (
            <>
              Read <RelativeTime value={node.facts_read_at} size="xs" c="dimmed" />
              {node.facts_namespace_id !== null && (
                <>
                  {' '}
                  from namespace <span className="mono">{node.facts_namespace_id}</span>
                </>
              )}
            </>
          )}
        </Text>
      </Group>
      {entries.length === 0 ? (
        <Text size="sm" c="dimmed">
          The node has not reported any facts yet. They are read when it first connects.
        </Text>
      ) : shown.length === 0 ? (
        <Text size="sm" c="dimmed">
          No fact matches {filter.trim()}.
        </Text>
      ) : (
        <div className={classes.factsFrame} tabIndex={0} role="region" aria-label="Facts">
          <table className={classes.facts} aria-label="Facts">
            <thead>
              <tr>
                <th scope="col">Key</th>
                <th scope="col">Value</th>
                <th scope="col">
                  <span className={classes.hidden}>Copy</span>
                </th>
              </tr>
            </thead>
            <tbody>
              {shown.map(([key, value]) => (
                <tr key={key}>
                  <td className="mono">{key}</td>
                  <td>
                    <span className={`mono ${classes.factValue}`}>{factText(value)}</span>
                    {typeof value === 'number' && key.endsWith('_bytes') && value >= 1024 && (
                      <Text component="span" size="xs" c="dimmed" ml={8}>
                        {formatBytes(value)}
                      </Text>
                    )}
                  </td>
                  <td>
                    <CopyButton value={factText(value)} timeout={1500}>
                      {({ copied, copy }) => (
                        <ActionIcon
                          variant="subtle"
                          color={copied ? 'green' : 'gray'}
                          size="sm"
                          aria-label={`Copy ${key}`}
                          onClick={copy}
                        >
                          {copied ? (
                            <IconCheck size={14} aria-hidden="true" />
                          ) : (
                            <IconCopy size={14} aria-hidden="true" />
                          )}
                        </ActionIcon>
                      )}
                    </CopyButton>
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
    </Stack>
  )
}

function LifecycleBanner({
  lifecycle,
  changedAt,
  reason,
  device = null,
}: {
  lifecycle: Lifecycle
  changedAt: string | null
  reason: string | null
  device?: DeviceLifecycle | null
}) {
  if (lifecycle === 'active' || lifecycle === 'enrolled') {
    return null
  }
  const icon =
    lifecycle === 'quarantined' ? (
      <IconLock size={18} aria-hidden="true" />
    ) : lifecycle === 'retired' ? (
      <IconArchive size={18} aria-hidden="true" />
    ) : (
      <IconShieldX size={18} aria-hidden="true" />
    )
  const title =
    device !== null
      ? `Device ${lifecycle}: every installation of this machine is refused at every handshake`
      : lifecycle === 'quarantined'
        ? 'Quarantined: callers may only do what the quarantine policy allows'
        : lifecycle === 'retired'
          ? 'Retired: refused at every handshake'
          : 'Revoked: refused at every handshake'
  return (
    <Alert
      color={lifecycleStyles[lifecycle].color}
      variant="light"
      icon={icon}
      title={
        <Group gap={6} wrap="wrap">
          <span>{title}</span>
          {changedAt !== null && <RelativeTime value={changedAt} size="sm" c="dimmed" />}
        </Group>
      }
    >
      <Text size="sm">{reason ?? 'No reason was recorded.'}</Text>
      {device !== null && (
        <Text size="xs" c="dimmed" mt={4}>
          Changed by {device.actor}
        </Text>
      )}
    </Alert>
  )
}

function NodeSkeleton() {
  return (
    <Stack gap="lg" role="status" aria-busy="true" aria-label="Loading the node">
      <Skeleton height={28} width="30%" />
      <Skeleton height={16} width="50%" />
      <Group grow align="flex-start">
        <Skeleton height={220} radius="md" />
        <Skeleton height={220} radius="md" />
        <Skeleton height={220} radius="md" />
      </Group>
      <Skeleton height={320} radius="md" />
    </Stack>
  )
}

const tabs = ['facts', 'executions'] as const
type Tab = (typeof tabs)[number]

export function NodePage() {
  const { deviceId = '', installationId = '' } = useParams()
  const [searchParameters, setSearchParameters] = useSearchParams()
  const tabText = searchParameters.get('tab')
  const tab: Tab = tabs.find((candidate) => candidate === tabText) ?? 'facts'
  const query = useNode(deviceId, installationId)
  const campaigns = useCampaignNames(
    (query.data?.executions ?? []).map((execution) => execution.campaign_id),
  )

  if (query.isError) {
    if (query.error instanceof ApiError && query.error.status === 404) {
      return <NotFound />
    }
    return <QueryError error={query.error} what="the node" onRetry={() => void query.refetch()} />
  }
  const node = query.data
  if (node === undefined) {
    return <NodeSkeleton />
  }
  const name = node.hostname ?? node.device_id

  return (
    <>
      <PageHeader
        trail={[{ label: 'Nodes', to: '/nodes' }]}
        title={<span className="mono">{name}</span>}
        documentTitle={name}
        status={
          <Group gap="sm" wrap="nowrap">
            <StatusPill status={lifecycleStyles[node.lifecycle]} size="lg" />
            <Presence online={node.online} lastSeen={node.last_seen_at} />
          </Group>
        }
        actions={<NodeActions node={node} />}
      />
      <Stack gap="lg">
        {node.device !== null && (
          <LifecycleBanner
            lifecycle={node.device.lifecycle}
            changedAt={node.device.changed_at}
            reason={node.device.reason}
            device={node.device}
          />
        )}
        <LifecycleBanner
          lifecycle={node.lifecycle}
          changedAt={node.lifecycle_changed_at}
          reason={node.lifecycle_reason}
        />
        <div className={classes.grid}>
          <Section title="Identity">
            <dl className={classes.fields}>
              <Field label="Device">
                <MonoId value={node.device_id} label="device id" maximum={20} />
              </Field>
              <Field label="Installation">
                <MonoId value={node.installation_id} label="installation id" maximum={20} />
              </Field>
              <Field label="Certificate">
                {node.cert_fingerprint === null ? (
                  maybe(null)
                ) : (
                  <MonoId
                    value={node.cert_fingerprint}
                    label="certificate fingerprint"
                    maximum={20}
                  />
                )}
              </Field>
              <Field label="Tenant">{maybe(node.tenant)}</Field>
              <Field label="Enrolled">
                <RelativeTime value={node.enrolled_at} fallback="not recorded" />
              </Field>
              <Field label="First seen">
                <RelativeTime value={node.first_seen_at} />
              </Field>
            </dl>
          </Section>
          <Section
            title="Presence"
            description={
              node.online
                ? `${plural(node.sessions.length, 'live session')}. Dispatch goes to the newest epoch.`
                : 'No live session.'
            }
          >
            {node.online ? (
              <Stack gap="md">
                {node.sessions.map((session) => (
                  <dl key={session.namespace_id} className={classes.fields}>
                    <Field label="Namespace">
                      <MonoId value={session.namespace_id} label="namespace id" maximum={20} />
                    </Field>
                    <Field label="Epoch">
                      <span className="mono">{session.epoch}</span>
                    </Field>
                    <Field label="nightfall">
                      <Tooltip label={session.inner_address}>
                        <span className="mono">{session.instance}</span>
                      </Tooltip>
                    </Field>
                    <Field label="Connected">
                      <RelativeTime value={session.connected_at} />
                    </Field>
                    <Field label="Last heard">
                      <RelativeTime value={session.last_seen} />
                    </Field>
                  </dl>
                ))}
              </Stack>
            ) : (
              <dl className={classes.fields}>
                <Field label="Last seen">
                  <RelativeTime value={node.last_seen_at} fallback="never" />
                </Field>
              </dl>
            )}
          </Section>
          <Section title="Reported state">
            <dl className={classes.fields}>
              <Field label="Version">{maybe(node.reported_version, true)}</Field>
              <Field label="Config hash">
                {node.reported_config_hash === null ? (
                  maybe(null)
                ) : (
                  <MonoId value={node.reported_config_hash} label="config hash" maximum={20} />
                )}
              </Field>
              <Field label="Running">
                {node.reported_services === null || node.reported_services.length === 0 ? (
                  maybe(null)
                ) : (
                  <Group gap={4}>
                    {node.reported_services.map((service) => (
                      <Badge key={service} variant="default" size="sm" tt="none" className="mono">
                        {service}
                      </Badge>
                    ))}
                  </Group>
                )}
              </Field>
              <Field label="Reported">
                <RelativeTime value={node.reported_at} fallback="never" />
              </Field>
              <Field label="OS">
                {maybe(
                  node.os_name === null ? null : `${node.os_name} ${node.os_version ?? ''}`.trim(),
                )}
              </Field>
              <Field label="Hardware">{maybe(node.hardware_class, true)}</Field>
            </dl>
          </Section>
        </div>
        <Tabs
          value={tab}
          onChange={(value) =>
            setSearchParameters(value === null || value === 'facts' ? {} : { tab: value }, {
              replace: true,
            })
          }
          keepMounted={false}
        >
          <Tabs.List>
            <Tabs.Tab value="facts">
              Facts{' '}
              <Text component="span" size="xs" c="dimmed" className="tabular">
                {formatNumber(Object.keys(node.facts).length)}
              </Text>
            </Tabs.Tab>
            <Tabs.Tab value="executions">
              Processes{' '}
              <Text component="span" size="xs" c="dimmed" className="tabular">
                {formatNumber(node.executions.length)}
              </Text>
            </Tabs.Tab>
          </Tabs.List>
          <Tabs.Panel value="facts" pt="md">
            <Facts node={node} />
          </Tabs.Panel>
          <Tabs.Panel value="executions" pt="md">
            <DataTable
              label="Campaign processes on this node"
              columns={processColumns(campaigns)}
              data={node.executions}
              getRowId={(execution) => execution.campaign_id}
              rowTone={(execution) =>
                execution.state === 'failed'
                  ? 'danger'
                  : execution.state === 'unknown'
                    ? 'warning'
                    : undefined
              }
              empty={
                <EmptyState
                  size="sm"
                  icon={<IconHistory size={22} aria-hidden="true" />}
                  title="No campaign has reached this node"
                  description="A row appears here when a campaign's phase that holds this node opens; its pid appears once the campaign sends it."
                />
              }
            />
          </Tabs.Panel>
        </Tabs>
      </Stack>
    </>
  )
}
