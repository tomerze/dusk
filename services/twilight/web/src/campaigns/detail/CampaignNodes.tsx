import {
  Anchor,
  Button,
  Checkbox,
  EmptyState,
  Group,
  Radio,
  Select,
  Stack,
  Text,
  Tooltip,
} from '@mantine/core'
import { notifications } from '@mantine/notifications'
import {
  IconChecks,
  IconDatabaseOff,
  IconRotateClockwise,
  IconUsersGroup,
} from '@tabler/icons-react'
import { useCallback, useMemo, useState } from 'react'
import { Link, useNavigate, useSearchParams } from 'react-router'
import { operatorOnly, usePermissions } from '../../api/permissions'
import type { RetryInput } from '../../api/queries'
import {
  useCampaignNodes,
  useEveryCampaignNode,
  useHostnames,
  useResolveCampaignNodes,
  useRetryCampaignNodes,
} from '../../api/queries'
import type { Campaign, CampaignNode, NodeState } from '../../api/types'
import { nodeStates } from '../../api/types'
import { ConfirmDialog } from '../../components/ConfirmDialog'
import { CursorPagination } from '../../components/CursorPagination'
import { DataTable } from '../../components/DataTable'
import { MonoId } from '../../components/MonoId'
import { countOf, stateCounts } from '../../components/progress'
import { QueryError } from '../../components/QueryError'
import { RelativeTime } from '../../components/RelativeTime'
import { StatusPill } from '../../components/StatusPill'
import { nodeStateStyles } from '../../components/status'
import type { TableColumn } from '../../components/table'
import { columnHelper } from '../../components/table'
import { formatNumber, middleEllipsis, plural } from '../../format'
import { useCursorPages } from '../../hooks/useCursorPages'
import { isActive } from '../display'

const retryableStates: NodeState[] = ['failed', 'unknown']

const maximumRowsPerRetry = 10_000

const recordedInHistory = 'Recorded in the campaign history.'

function nodeKey(node: Pick<CampaignNode, 'device_id' | 'installation_id'>) {
  return `${node.device_id}/${node.installation_id}`
}

const helper = columnHelper<CampaignNode>()

function buildColumns(
  campaign: Campaign,
  hostnames: Map<string, string>,
  selection: Set<string>,
  toggle: (row: CampaignNode) => void,
  selectable: boolean,
): TableColumn<CampaignNode>[] {
  const columns: TableColumn<CampaignNode>[] = []
  if (selectable) {
    columns.push(
      helper.display({
        id: 'select',
        header: '',
        meta: { label: '', width: 36 },
        cell: ({ row }) => (
          <Checkbox
            size="xs"
            aria-label={`Select ${hostnames.get(nodeKey(row.original)) ?? row.original.device_id}`}
            checked={selection.has(nodeKey(row.original))}
            disabled={!retryableStates.includes(row.original.state)}
            onChange={() => toggle(row.original)}
          />
        ),
      }),
    )
  }
  columns.push(
    helper.display({
      id: 'node',
      header: 'Node',
      meta: { label: 'Node', mobile: 'title' },
      cell: ({ row }) => (
        <Stack gap={0} style={{ minWidth: 0 }}>
          <Anchor
            component={Link}
            to={`/nodes/${row.original.device_id}/${row.original.installation_id}`}
            size="sm"
            fw={600}
            c="var(--twilight-text-strong)"
            onClick={(event) => event.stopPropagation()}
          >
            {hostnames.get(nodeKey(row.original)) ?? middleEllipsis(row.original.device_id, 14)}
          </Anchor>
          <Text size="xs" c="dimmed" className="mono">
            {middleEllipsis(row.original.device_id, 14)}
          </Text>
        </Stack>
      ),
    }),
    helper.display({
      id: 'state',
      header: 'State',
      meta: { label: 'State', width: 130 },
      cell: ({ row }) => (
        <Stack gap={2} align="flex-start">
          <StatusPill status={nodeStateStyles[row.original.state]} />
          {row.original.last_status.length > 0 && (
            <Text size="xs" c="dimmed">
              Last result: {row.original.last_status.replaceAll('_', ' ')}
            </Text>
          )}
          {row.original.unreached > 0 && (
            <Text size="xs" c="dimmed">
              {plural(row.original.unreached, 'dispatch', 'dispatches')} did not reach the node
            </Text>
          )}
        </Stack>
      ),
    }),
    helper.display({
      id: 'phase',
      header: 'Phase',
      meta: { label: 'Phase', width: 110 },
      cell: ({ row }) => (
        <Text size="sm" style={{ whiteSpace: 'nowrap' }}>
          {row.original.phase + 1}. {campaign.policy.phases[row.original.phase]?.name ?? ''}
        </Text>
      ),
    }),
    helper.display({
      id: 'attempt',
      header: 'Attempt',
      meta: { label: 'Attempt', align: 'right', width: 80 },
      cell: ({ row }) => (
        <Text size="sm" className="tabular">
          {row.original.attempt} of {campaign.policy.retry.max_attempts}
        </Text>
      ),
    }),
    helper.display({
      id: 'pid',
      header: 'Pid',
      meta: { label: 'Pid', width: 210 },
      cell: ({ row }) =>
        row.original.pid === null ? (
          <Text size="sm" c="dimmed">
            not sent
          </Text>
        ) : (
          <Stack gap={0}>
            <MonoId value={row.original.pid} label="pid" maximum={20} />
            {row.original.reaped_at !== null && (
              <Text size="xs" c="dimmed">
                reaped <RelativeTime value={row.original.reaped_at} size="xs" c="dimmed" />
              </Text>
            )}
          </Stack>
        ),
    }),
    helper.display({
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
            <Text size="sm" className="mono" truncate="end" maw={320} c="red">
              {row.original.last_error}
            </Text>
          </Tooltip>
        ),
    }),
    helper.display({
      id: 'when',
      header: 'Updated',
      meta: { label: 'Updated', width: 110 },
      cell: ({ row }) => {
        const node = row.original
        if (node.state === 'backoff' && node.next_attempt_at !== null) {
          return (
            <Text size="sm" component="span">
              retry <RelativeTime value={node.next_attempt_at} />
            </Text>
          )
        }
        return (
          <RelativeTime
            value={node.finished_at ?? node.delivered_at ?? node.dispatched_at}
            fallback="not sent"
          />
        )
      },
    }),
  )
  return columns
}

export function CampaignNodes({
  campaign,
  matched,
}: {
  campaign: Campaign
  matched: number | null
}) {
  const navigate = useNavigate()
  const { canOperate } = usePermissions()
  const [searchParameters, setSearchParameters] = useSearchParams()
  const stateText = searchParameters.get('state')
  const state = nodeStates.find((candidate) => candidate === stateText) ?? null
  const phaseText = searchParameters.get('phase')
  const phase =
    phaseText !== null && campaign.policy.phases[Number(phaseText)] !== undefined
      ? Number(phaseText)
      : null
  const pages = useCursorPages(`${campaign.id}/${state}/${phase}`, 50)
  const nodes = useCampaignNodes(campaign.id, {
    state,
    phase,
    limit: pages.pageSize,
    cursor: pages.cursor,
  })
  const [selection, setSelection] = useState<Set<string>>(new Set())
  const [dialog, setDialog] = useState<'retry' | 'resolve' | null>(null)
  const [outcome, setOutcome] = useState<'succeeded' | 'failed'>('succeeded')
  const retry = useRetryCampaignNodes(campaign.id)
  const resolve = useResolveCampaignNodes(campaign.id)
  const rows = useMemo(() => nodes.data?.items ?? [], [nodes.data])
  const hostnames = useHostnames(rows)
  const selectedRows = rows.filter((row) => selection.has(nodeKey(row)))
  const retryStates = state !== null && retryableStates.includes(state) ? [state] : retryableStates
  const namesFiltered = selectedRows.length === 0 && (state !== null || phase !== null)
  const targets = useEveryCampaignNode(
    campaign.id,
    retryStates,
    phase,
    maximumRowsPerRetry,
    dialog === 'retry' && namesFiltered,
  )
  const selectable = canOperate && campaign.status !== 'archived' && campaign.status !== 'draft'
  const toggle = useCallback(
    (row: CampaignNode) =>
      setSelection((previous) => {
        const next = new Set(previous)
        const key = nodeKey(row)
        if (next.has(key)) {
          next.delete(key)
        } else {
          next.add(key)
        }
        return next
      }),
    [setSelection],
  )
  const columns = useMemo(
    () => buildColumns(campaign, hostnames, selection, toggle, selectable),
    [campaign, hostnames, selection, toggle, selectable],
  )

  const setFilter = (key: 'state' | 'phase', value: string | null) => {
    setSelection(new Set())
    setSearchParameters(
      (previous) => {
        const next = new URLSearchParams(previous)
        if (value === null) {
          next.delete(key)
        } else {
          next.set(key, value)
        }
        return next
      },
      { replace: true },
    )
  }

  const counts = stateCounts(campaign.counters, phase ?? undefined)
  const stateOptions = [
    { value: 'any', label: `Any state (${formatNumber(countOf(counts))})` },
    ...nodeStates
      .filter((candidate) => (counts[candidate] ?? 0) > 0 || candidate === state)
      .map((candidate) => ({
        value: candidate,
        label: `${nodeStateStyles[candidate].label} (${formatNumber(counts[candidate] ?? 0)})`,
      })),
  ]
  const phaseOptions = [
    { value: 'any', label: 'Any phase' },
    ...campaign.policy.phases.map((definition, index) => ({
      value: String(index),
      label: `${index + 1}. ${definition.name} (${definition.percent}%)`,
    })),
  ]

  const countedRetry = retryStates.reduce((sum, candidate) => sum + (counts[candidate] ?? 0), 0)
  const retryCount = selectedRows.length > 0 ? selectedRows.length : countedRetry
  const namedRows =
    selectedRows.length > 0
      ? selectedRows
      : namesFiltered
        ? (targets.data?.items.slice(0, maximumRowsPerRetry) ?? null)
        : null
  const retryInput: RetryInput = {
    states: selectedRows.length > 0 ? retryableStates : retryStates,
    ...(namedRows === null
      ? {}
      : {
          nodes: namedRows.map((row) => ({
            device_id: row.device_id,
            installation_id: row.installation_id,
          })),
        }),
    reason: '',
  }
  const counting = namesFiltered && (targets.data === undefined || targets.isFetching)
  const dialogCount = namedRows?.length ?? countedRetry
  const stateWords = retryStates
    .map((candidate) => nodeStateStyles[candidate].label.toLowerCase())
    .join(' and ')
  const phaseWords =
    phase === null ? '' : ` in phase ${phase + 1} (${campaign.policy.phases[phase]?.name ?? ''})`
  const unknownSelected = selectedRows.filter((row) => row.state === 'unknown')
  const oneShot = campaign.kind === 'run_script' || campaign.kind === 'quarantine'

  if (campaign.status === 'archived') {
    return (
      <EmptyState
        size="md"
        icon={<IconDatabaseOff size={24} aria-hidden="true" />}
        title="Node rows were dropped when the campaign was archived"
        description="The counters stay with the campaign. Every command sent to a node stays in the nightfall ledger."
      />
    )
  }
  if (campaign.status === 'draft') {
    return (
      <EmptyState
        size="md"
        icon={<IconUsersGroup size={24} aria-hidden="true" />}
        title="No node has a row yet"
        description={`A node gets a row the first time it matches while its phase is open. ${
          matched === null ? '' : `${plural(matched, 'node')} match the selector now.`
        }`}
      >
        <EmptyState.Actions>
          <Button
            component={Link}
            to={`/nodes?selector=${encodeURIComponent(campaign.selector)}`}
            size="xs"
            variant="light"
          >
            See the matching nodes
          </Button>
        </EmptyState.Actions>
      </EmptyState>
    )
  }

  return (
    <Stack gap="sm">
      <Group justify="space-between" gap="sm" wrap="wrap" align="flex-end">
        <Group gap="sm" wrap="wrap">
          <Select
            aria-label="State"
            data={stateOptions}
            value={state ?? 'any'}
            onChange={(value) => setFilter('state', value === 'any' ? null : value)}
            w={200}
          />
          <Select
            aria-label="Phase"
            data={phaseOptions}
            value={phase === null ? 'any' : String(phase)}
            onChange={(value) => setFilter('phase', value === 'any' ? null : value)}
            w={200}
          />
        </Group>
        <Group gap="xs" wrap="wrap">
          {canOperate ? (
            <>
              <Tooltip
                label="Only a running or paused campaign retries nodes."
                disabled={isActive(campaign)}
              >
                <span tabIndex={isActive(campaign) ? undefined : 0}>
                  <Button
                    variant="default"
                    leftSection={<IconRotateClockwise size={16} aria-hidden="true" />}
                    disabled={retryCount === 0 || !isActive(campaign)}
                    onClick={() => setDialog('retry')}
                  >
                    {selectedRows.length > 0
                      ? `Retry ${plural(selectedRows.length, 'node')}`
                      : `Retry ${state === 'unknown' ? 'unknown' : state === 'failed' ? 'failed' : 'failed and unknown'}`}
                  </Button>
                </span>
              </Tooltip>
              <Button
                variant="default"
                leftSection={<IconChecks size={16} aria-hidden="true" />}
                disabled={unknownSelected.length === 0}
                onClick={() => setDialog('resolve')}
              >
                {unknownSelected.length > 0
                  ? `Resolve ${plural(unknownSelected.length, 'unknown node')}`
                  : 'Resolve unknown'}
              </Button>
            </>
          ) : (
            <Tooltip label={operatorOnly}>
              <Text size="xs" c="dimmed" tabIndex={0}>
                Retry and resolve need the operator role.
              </Text>
            </Tooltip>
          )}
        </Group>
      </Group>
      {nodes.isError ? (
        <QueryError
          error={nodes.error}
          what="the campaign's nodes"
          onRetry={() => void nodes.refetch()}
        />
      ) : (
        <>
          <DataTable
            label="Campaign nodes"
            columns={columns}
            maxHeight="max(420px, calc(100dvh - 240px))"
            data={rows}
            getRowId={nodeKey}
            loading={nodes.isPending}
            onOpen={(row) => void navigate(`/nodes/${row.device_id}/${row.installation_id}`)}
            rowTone={(row) =>
              row.state === 'failed' ? 'danger' : row.state === 'unknown' ? 'warning' : undefined
            }
            empty={
              <Text size="sm" c="dimmed" ta="center">
                {state === null && phase === null
                  ? 'No node has a row yet. Rows appear as nodes in open phases are dispatched.'
                  : 'No node is in this state and phase.'}
              </Text>
            }
          />
          {nodes.data !== undefined && (rows.length > 0 || pages.pageIndex > 0) && (
            <CursorPagination
              pages={pages}
              shown={rows.length}
              total={state === null ? countOf(counts) : (counts[state] ?? null)}
              nextCursor={nodes.data.next_cursor}
              noun="nodes"
            />
          )}
        </>
      )}
      <ConfirmDialog
        opened={dialog === 'retry'}
        onClose={() => setDialog(null)}
        title={counting ? 'Retry nodes' : `Retry ${plural(dialogCount, 'node')}`}
        confirmLabel="Retry"
        reason="required"
        reasonDescription={recordedInHistory}
        confirmDisabled={counting || (namesFiltered && targets.isError) || dialogCount === 0}
        consequences={
          <Stack gap="xs">
            {namesFiltered && targets.isError ? (
              <QueryError
                error={targets.error}
                what="the nodes to retry"
                onRetry={() => void targets.refetch()}
              />
            ) : counting ? (
              <Text size="sm" c="dimmed">
                Counting the {stateWords} nodes{phaseWords}…
              </Text>
            ) : dialogCount === 0 ? (
              <Text size="sm">
                No {stateWords} node{phaseWords} is left to retry.
              </Text>
            ) : (
              <Text size="sm">
                {selectedRows.length > 0
                  ? `The ${plural(dialogCount, 'selected node')} ${dialogCount === 1 ? 'gets' : 'get'} a new attempt at a new pid.`
                  : namedRows !== null
                    ? `The ${plural(dialogCount, `${stateWords} node`)}${phaseWords} ${dialogCount === 1 ? 'gets' : 'get'} a new attempt at a new pid.`
                    : `Every ${stateWords} node gets a new attempt at a new pid.`}{' '}
                They go back to pending and are dispatched at the campaign's rate while their phase
                is open
                {campaign.status === 'paused' ? ', once the campaign resumes' : ''}.
                {namedRows === null && countedRetry > maximumRowsPerRetry
                  ? ` twilight retries at most ${formatNumber(maximumRowsPerRetry)} nodes per request, so ${formatNumber(maximumRowsPerRetry)} of them get a new attempt now; retry again for the rest.`
                  : ''}
                {namesFiltered && targets.data?.truncated === true
                  ? ` More than ${formatNumber(maximumRowsPerRetry)} match; twilight retries at most ${formatNumber(maximumRowsPerRetry)} nodes per request, so retry again for the rest.`
                  : ''}
              </Text>
            )}
            {oneShot && (
              <Text size="sm" c="orange">
                This is a one-shot action. A node marked unknown may already have run it; retrying
                runs it again there.
              </Text>
            )}
          </Stack>
        }
        onConfirm={async (reason) => {
          const result = await retry.mutateAsync({ ...retryInput, reason })
          setSelection(new Set())
          notifications.show({
            color: 'green',
            message: `${plural(result.count, 'node')} queued for another attempt.`,
          })
        }}
      />
      <ConfirmDialog
        opened={dialog === 'resolve'}
        onClose={() => setDialog(null)}
        title={`Resolve ${plural(unknownSelected.length, 'unknown node')}`}
        confirmLabel="Resolve"
        reason="required"
        reasonDescription={recordedInHistory}
        consequences={
          <Stack gap="xs">
            <Text size="sm">
              Use this when you know what happened on these nodes, for example from their logs.
              Their rows close with the outcome you pick and count in the gate like a reported
              result: as successes, or as failures toward the failure rate. Nothing is sent to the
              nodes.
            </Text>
            {campaign.kind === 'quarantine' && (
              <Text size="sm">
                A node you resolve as succeeded is still quarantined; its row closes once the
                quarantine is recorded.
              </Text>
            )}
          </Stack>
        }
        onConfirm={async (reason) => {
          const result = await resolve.mutateAsync({
            nodes: unknownSelected.map((row) => ({
              device_id: row.device_id,
              installation_id: row.installation_id,
            })),
            outcome,
            reason,
          })
          setSelection(new Set())
          notifications.show({
            color: 'green',
            message: `${plural(result.count, 'node')} resolved as ${outcome}.`,
          })
        }}
      >
        <Radio.Group
          label="Outcome"
          value={outcome}
          onChange={(value) => setOutcome(value as 'succeeded' | 'failed')}
        >
          <Group gap="lg" mt={6}>
            <Radio value="succeeded" label="Succeeded" />
            <Radio value="failed" label="Failed" />
          </Group>
        </Radio.Group>
      </ConfirmDialog>
    </Stack>
  )
}
