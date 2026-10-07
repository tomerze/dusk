import { Anchor, Group, Stack, Text, Tooltip } from '@mantine/core'
import type { ReactNode } from 'react'
import { useMemo } from 'react'
import { Link, useNavigate } from 'react-router'
import type { Campaign } from '../api/types'
import { DataTable } from '../components/DataTable'
import { RelativeTime } from '../components/RelativeTime'
import { countOf, stateCounts } from '../components/progress'
import { SegmentedProgress } from '../components/SegmentedProgress'
import { StatusPill } from '../components/StatusPill'
import { campaignStatusStyles } from '../components/status'
import type { TableColumn } from '../components/table'
import { columnHelper } from '../components/table'
import { formatNumber } from '../format'
import { actionLabels, estimatedTotal, phaseLabel } from './display'

const helper = columnHelper<Campaign>()

function buildColumns(compact: boolean): TableColumn<Campaign>[] {
  const columns: TableColumn<Campaign>[] = [
    helper.display({
      id: 'name',
      header: 'Campaign',
      meta: { label: 'Campaign', mobile: 'title' },
      cell: ({ row }) => (
        <Stack gap={2} style={{ minWidth: 180 }}>
          <Anchor
            component={Link}
            to={`/campaigns/${row.original.id}`}
            fw={600}
            size="sm"
            c="var(--twilight-text-strong)"
            onClick={(event) => event.stopPropagation()}
          >
            {row.original.name}
          </Anchor>
          <Text size="xs" c="dimmed">
            {actionLabels[row.original.kind]}
            {row.original.action.kind === 'ensure_version' ? ` ${row.original.action.version}` : ''}
          </Text>
        </Stack>
      ),
    }),
    helper.display({
      id: 'status',
      header: 'Status',
      meta: { label: 'Status', width: 130 },
      cell: ({ row }) => {
        return (
          <StatusPill
            status={campaignStatusStyles[row.original.status]}
            detail={row.original.pause_reason ?? row.original.abort_reason ?? undefined}
          />
        )
      },
    }),
    helper.display({
      id: 'progress',
      header: 'Progress',
      meta: { label: 'Progress', width: 240, mobile: 'block' },
      cell: ({ row }) => {
        const campaign = row.original
        const counts = stateCounts(campaign.counters)
        const rows = countOf(counts)
        const succeeded = counts.succeeded ?? 0
        const failed = counts.failed ?? 0
        const estimate = estimatedTotal(campaign, rows)
        return (
          <Stack gap={4} style={{ minWidth: 160 }}>
            <SegmentedProgress
              counts={counts}
              expected={estimate}
              phases={campaign.policy.phases.map((phase) => phase.percent)}
              size="sm"
            />
            <Group gap={8} wrap="wrap" className="tabular">
              <Text size="xs" c="dimmed" style={{ whiteSpace: 'nowrap' }}>
                {campaign.status === 'draft'
                  ? 'Not started'
                  : `${formatNumber(succeeded)} of ${estimate === null ? '' : 'about '}${formatNumber(estimate ?? rows)} succeeded`}
              </Text>
              {failed > 0 && (
                <Text size="xs" c="red" style={{ whiteSpace: 'nowrap' }}>
                  {`${formatNumber(failed)} failed`}
                </Text>
              )}
            </Group>
          </Stack>
        )
      },
    }),
    helper.display({
      id: 'phase',
      header: 'Phase',
      meta: { label: 'Phase', width: 110 },
      cell: ({ row }) => (
        <Text size="sm" className="tabular" style={{ whiteSpace: 'nowrap' }}>
          {phaseLabel(row.original)}
        </Text>
      ),
    }),
  ]
  if (!compact) {
    columns.push(
      helper.display({
        id: 'selector',
        header: 'Selector',
        meta: { label: 'Selector', mobile: 'hidden' },
        cell: ({ row }) => (
          <Tooltip
            label={
              <Text ff="monospace" size="xs">
                {row.original.selector}
              </Text>
            }
          >
            <Text ff="monospace" size="xs" c="dimmed" className="mono" truncate="end" maw={220}>
              {row.original.selector.length === 0 ? 'every node' : row.original.selector}
            </Text>
          </Tooltip>
        ),
      }),
      helper.display({
        id: 'rate',
        header: 'Rate',
        meta: { label: 'Rate', align: 'right', width: 80 },
        cell: ({ row }) => (
          <Text size="sm" className="tabular" style={{ whiteSpace: 'nowrap' }}>
            {formatNumber(row.original.policy.rate.per_second)}/s
          </Text>
        ),
      }),
    )
  }
  columns.push(
    helper.display({
      id: 'started',
      header: 'Started',
      meta: { label: 'Started', width: 110 },
      cell: ({ row }) => <RelativeTime value={row.original.started_at} fallback="not started" />,
    }),
  )
  if (compact) {
    return columns
  }
  columns.push(
    helper.display({
      id: 'owner',
      header: 'Owner',
      meta: { label: 'Owner', width: 130 },
      cell: ({ row }) => (
        <Text size="sm" truncate="end" maw={120}>
          {row.original.created_by}
        </Text>
      ),
    }),
  )
  return columns
}

interface CampaignTableProperties {
  campaigns: Campaign[]
  loading: boolean
  empty: ReactNode
  compact?: boolean
  framed?: boolean
  label: string
}

export function CampaignTable({
  campaigns,
  loading,
  empty,
  compact = false,
  framed = true,
  label,
}: CampaignTableProperties) {
  const navigate = useNavigate()
  const columns = useMemo(() => buildColumns(compact), [compact])
  return (
    <DataTable
      label={label}
      columns={columns}
      data={campaigns}
      getRowId={(campaign) => campaign.id}
      loading={loading}
      skeletonRows={compact ? 3 : 8}
      empty={empty}
      onOpen={(campaign) => void navigate(`/campaigns/${campaign.id}`)}
      rowTone={(campaign) =>
        campaign.status === 'paused' && campaign.pause_kind === 'gate' ? 'danger' : undefined
      }
      framed={framed}
    />
  )
}
