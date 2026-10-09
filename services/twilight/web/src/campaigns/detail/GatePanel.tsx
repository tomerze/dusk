import { Alert, Group, SegmentedControl, Skeleton, Stack, Text } from '@mantine/core'
import {
  IconCircleCheck,
  IconCircleDashed,
  IconCircleX,
  IconCloudOff,
  IconHourglassHigh,
  IconInfoCircle,
} from '@tabler/icons-react'
import { useMemo, useState } from 'react'
import type { BreakdownField, Campaign, Gates } from '../../api/types'
import { DataTable } from '../../components/DataTable'
import { QueryError } from '../../components/QueryError'
import { RelativeTime } from '../../components/RelativeTime'
import { StatusPill } from '../../components/StatusPill'
import { gateVerdictStyles } from '../../components/status'
import type { StatusStyle } from '../../components/status'
import type { TableColumn } from '../../components/table'
import { columnHelper } from '../../components/table'
import { formatDuration, formatNumber, formatPercent, plural } from '../../format'
import { breakdownLabels } from '../summary'
import type { Dimension, GroupJudgement, Thresholds } from './gates'
import { dimensionsOf, failing, gateDisplay, judge, splitGroup, waitingForSample } from './gates'
import classes from './GatePanel.module.css'

type GroupVerdict = 'failing' | 'sampling' | 'passing'

const groupVerdicts: Record<GroupVerdict, StatusStyle> = {
  failing: {
    label: 'Failing',
    color: 'red',
    icon: IconCircleX,
    description: 'A rate in this group is over its threshold.',
  },
  sampling: {
    label: 'Below sample',
    color: 'gray',
    icon: IconCircleDashed,
    description: 'Too few results in this group to judge it; it cannot fail the gate yet.',
  },
  passing: {
    label: 'Passing',
    color: 'green',
    icon: IconCircleCheck,
    description: 'Both rates are under their thresholds.',
  },
}

function groupVerdict(group: GroupJudgement): GroupVerdict {
  if (failing(group)) {
    return 'failing'
  }
  return group.failureJudged || group.silentJudged ? 'passing' : 'sampling'
}

function groupName(value: string): string {
  return value.length === 0 ? 'not reported' : value
}

interface MeterProperties {
  label: string
  value: number | null
  threshold: number
  exceeded: boolean
  judged: boolean
  detail: string
}

function RateMeter({ label, value, threshold, exceeded, judged, detail }: MeterProperties) {
  const scale = Math.max(threshold * 2, value ?? 0, 0.01)
  const tone = !judged || value === null ? 'neutral' : exceeded ? 'danger' : 'ok'
  return (
    <div className={classes.metric}>
      <Text size="xs" c="dimmed">
        {label}
      </Text>
      <Group gap={6} align="baseline" wrap="nowrap">
        <Text className={classes.metricValue} data-tone={tone}>
          {formatPercent(value, threshold)}
        </Text>
        <Text size="xs" c="dimmed" className="tabular">
          limit {formatPercent(threshold)}
        </Text>
      </Group>
      <div
        className={classes.meter}
        role="meter"
        aria-label={label}
        aria-valuemin={0}
        aria-valuemax={1}
        aria-valuenow={value ?? 0}
        aria-valuetext={`${formatPercent(value, threshold)}, limit ${formatPercent(threshold)}`}
      >
        <span
          className={classes.meterFill}
          data-tone={tone}
          style={{ width: `${Math.min(100, ((value ?? 0) / scale) * 100)}%` }}
        />
        <span className={classes.meterLimit} style={{ left: `${(threshold / scale) * 100}%` }} />
      </div>
      <Text size="xs" c="dimmed" mt={6} className="tabular">
        {detail}
      </Text>
    </div>
  )
}

function ProgressMetric({
  label,
  value,
  goal,
  text,
  detail,
}: {
  label: string
  value: number
  goal: number
  text: string
  detail: string
}) {
  const share = goal <= 0 ? 1 : Math.min(1, value / goal)
  return (
    <div className={classes.metric}>
      <Text size="xs" c="dimmed">
        {label}
      </Text>
      <Text className={classes.metricValue} data-tone={share >= 1 ? 'ok' : 'neutral'}>
        {text}
      </Text>
      <div
        className={classes.meter}
        role="meter"
        aria-label={label}
        aria-valuemin={0}
        aria-valuemax={Math.max(goal, 1)}
        aria-valuenow={Math.min(value, Math.max(goal, 1))}
        aria-valuetext={detail}
      >
        <span
          className={classes.meterFill}
          data-tone={share >= 1 ? 'ok' : 'progress'}
          style={{ width: `${share * 100}%` }}
        />
      </div>
      <Text size="xs" c="dimmed" mt={6} className="tabular">
        {detail}
      </Text>
    </div>
  )
}

function InlineRate({
  value,
  threshold,
  exceeded,
  judged,
}: {
  value: number | null
  threshold: number
  exceeded: boolean
  judged: boolean
}) {
  const scale = Math.max(threshold * 2, value ?? 0, 0.01)
  const tone = !judged || value === null ? 'neutral' : exceeded ? 'danger' : 'ok'
  return (
    <div className={classes.inlineRate}>
      <Text size="sm" className="tabular" data-tone={tone} fw={exceeded ? 700 : 500}>
        {formatPercent(value, threshold)}
      </Text>
      <div className={classes.inlineMeter} aria-hidden="true">
        <span
          className={classes.meterFill}
          data-tone={tone}
          style={{ width: `${Math.min(100, ((value ?? 0) / scale) * 100)}%` }}
        />
        <span className={classes.meterLimit} style={{ left: `${(threshold / scale) * 100}%` }} />
      </div>
    </div>
  )
}

const helper = columnHelper<GroupJudgement>()

function capitalize(text: string): string {
  return text.charAt(0).toUpperCase() + text.slice(1)
}

function breakdownColumns(thresholds: Thresholds, tripped: string | null) {
  const columns: TableColumn<GroupJudgement>[] = [
    helper.display({
      id: 'value',
      header: 'Group',
      meta: { label: 'Group', mobile: 'title' },
      cell: ({ row }) => (
        <Stack gap={0}>
          <Text size="sm" className="mono" fw={600}>
            {groupName(row.original.value)}
          </Text>
          {tripped === row.original.value && (
            <Text size="xs" c="red">
              Tripped the gate
            </Text>
          )}
        </Stack>
      ),
    }),
    helper.display({
      id: 'verdict',
      header: 'Verdict',
      meta: { label: 'Verdict', width: 130 },
      cell: ({ row }) => (
        <StatusPill status={groupVerdicts[groupVerdict(row.original)]} size="xs" />
      ),
    }),
    helper.display({
      id: 'results',
      header: 'Results',
      meta: { label: 'Results', align: 'right', width: 90 },
      cell: ({ row }) => (
        <Text size="sm" className="tabular">
          {formatNumber(row.original.sample)}
        </Text>
      ),
    }),
    helper.display({
      id: 'failed',
      header: 'Failed',
      meta: { label: 'Failed', align: 'right', width: 80 },
      cell: ({ row }) => (
        <Text size="sm" className="tabular" c={row.original.tally.failed > 0 ? 'red' : undefined}>
          {formatNumber(row.original.tally.failed)}
        </Text>
      ),
    }),
    helper.display({
      id: 'failure_rate',
      header: `Failure rate (limit ${formatPercent(thresholds.max_failure_rate)})`,
      meta: { label: 'Failure rate', width: 210 },
      cell: ({ row }) => (
        <InlineRate
          value={row.original.failureRate}
          threshold={thresholds.max_failure_rate}
          exceeded={row.original.failureExceeded}
          judged={row.original.failureJudged}
        />
      ),
    }),
    helper.display({
      id: 'silent',
      header: 'Silent',
      meta: { label: 'Silent', align: 'right', width: 110 },
      cell: ({ row }) => (
        <Text size="sm" className="tabular">
          {formatNumber(row.original.tally.silent)} of {formatNumber(row.original.tally.eligible)}
        </Text>
      ),
    }),
    helper.display({
      id: 'silent_rate',
      header: `Silent rate (limit ${formatPercent(thresholds.max_silent_rate)})`,
      meta: { label: 'Silent rate', width: 210 },
      cell: ({ row }) => (
        <InlineRate
          value={row.original.silentRate}
          threshold={thresholds.max_silent_rate}
          exceeded={row.original.silentExceeded}
          judged={row.original.silentJudged}
        />
      ),
    }),
  ]
  return columns
}

const verdictOrder: Record<GroupVerdict, number> = { failing: 0, passing: 1, sampling: 2 }

function Breakdown({ gates, order }: { gates: Gates; order: readonly BreakdownField[] }) {
  const dimensions = useMemo(() => dimensionsOf(gates, order), [gates, order])
  const tripped = splitGroup(gates.failing_group)
  const failingField =
    tripped?.field ??
    dimensions.find((dimension) => dimension.groups.some((group) => failing(group)))?.field
  const [chosen, setChosen] = useState<string | null>(null)
  const field = chosen ?? failingField ?? dimensions[0]?.field
  const dimension: Dimension | undefined = dimensions.find((candidate) => candidate.field === field)
  const trippedValue = tripped !== null && tripped.field === field ? tripped.value : null
  const columns = useMemo(() => breakdownColumns(gates, trippedValue), [gates, trippedValue])
  if (field === undefined || dimension === undefined) {
    return (
      <Text size="sm" c="dimmed">
        {order.length === 0
          ? 'This campaign does not break its gate down by any inventory fact.'
          : 'No node has a result yet, so there are no groups to compare.'}
      </Text>
    )
  }
  const groups = dimension.groups
    .filter((group) => group.sample > 0 || group.tally.eligible > 0)
    .sort(
      (left, right) =>
        Number(right.value === trippedValue) - Number(left.value === trippedValue) ||
        verdictOrder[groupVerdict(left)] - verdictOrder[groupVerdict(right)] ||
        right.sample - left.sample,
    )
  const failingCount = groups.filter((group) => groupVerdict(group) === 'failing').length
  const label = (name: string) =>
    breakdownLabels[name as BreakdownField] ?? name.replaceAll('_', ' ')
  return (
    <Stack gap="sm">
      <Group justify="space-between" gap="sm" wrap="wrap">
        <SegmentedControl
          size="xs"
          aria-label="Break down by"
          value={field}
          onChange={setChosen}
          data={dimensions.map((candidate) => ({
            value: candidate.field,
            label: `${capitalize(label(candidate.field))}${
              candidate.groups.some((group) => failing(group)) ? ' (failing)' : ''
            }`,
          }))}
        />
        <Text size="xs" c="dimmed">
          {plural(groups.length, 'group')}
          {failingCount > 0 ? `, ${failingCount} failing` : ''}.{' '}
          {gates.min_sample > 0
            ? `Groups below ${plural(gates.min_sample, 'result')} cannot fail the gate.`
            : 'Every group with a result is judged.'}
        </Text>
      </Group>
      <DataTable
        label={`Gate breakdown by ${label(field)}`}
        columns={columns}
        data={groups}
        getRowId={(group) => group.value}
        rowTone={(group) => (groupVerdict(group) === 'failing' ? 'danger' : undefined)}
        maxHeight="420px"
        dense
        empty={
          <Text size="sm" c="dimmed" ta="center">
            No results yet in any group.
          </Text>
        }
      />
    </Stack>
  )
}

interface GatePanelProperties {
  campaign: Campaign
  gates: Gates | undefined
  error: unknown
  loading: boolean
  onRetry: () => void
  updatedAt: number
}

export function GatePanel({
  campaign,
  gates,
  error,
  loading,
  onRetry,
  updatedAt,
}: GatePanelProperties) {
  if (error !== null && error !== undefined) {
    return <QueryError error={error} what="the gate" onRetry={onRetry} />
  }
  if (loading || gates === undefined) {
    return (
      <Stack gap="md" role="status" aria-busy="true" aria-label="Loading the gate">
        <Skeleton height={20} width={260} />
        <Skeleton height={96} />
        <Skeleton height={220} />
      </Stack>
    )
  }
  const display = gateDisplay(campaign, gates)
  const overall = judge(gates.overall, { ...gates, min_sample: gates.required })
  const silentWindow = campaign.policy.gates.silent_window_seconds
  const maximumFailures = campaign.policy.abort.max_total_failures
  const ready = Math.min(overall.sample, gates.overall.eligible)
  return (
    <Stack gap="md">
      <Group justify="space-between" gap="sm" wrap="wrap" align="center">
        <Group gap="sm" wrap="wrap">
          <StatusPill status={gateVerdictStyles[display]} size="md" />
          <Text size="sm" fw={600}>
            {`Cumulative up to phase ${gates.phase + 1} (${gates.phase_name})`}
          </Text>
        </Group>
        {updatedAt > 0 && (
          <Text size="xs" c="dimmed">
            Updated <RelativeTime value={new Date(updatedAt).toISOString()} size="xs" c="dimmed" />
          </Text>
        )}
      </Group>
      {gates.degraded && display !== 'inactive' && (
        <Alert
          color="orange"
          variant="light"
          icon={<IconCloudOff size={18} aria-hidden="true" />}
          title="Holding: the online view is degraded"
        >
          At least one nightfall instance has not published a full census, so twilight cannot tell
          which nodes went silent. No phase advances until the view recovers; nothing is lost
          meanwhile.
        </Alert>
      )}
      {display === 'fail' && (
        <Alert color="red" variant="light" title="The gate is failing">
          <Text size="sm" className="mono">
            {gates.reason}
          </Text>
        </Alert>
      )}
      {display === 'hold' && (
        <Alert
          color="yellow"
          variant="light"
          icon={<IconHourglassHigh size={18} aria-hidden="true" />}
          title={waitingForSample(gates) ? 'Waiting for sample' : 'Holding'}
        >
          {waitingForSample(gates)
            ? `The gate needs ${plural(gates.required, 'result')} and ${plural(gates.required, 'node')} past the ${formatDuration(silentWindow)} silent window before it can judge this phase (${gates.reason}). The phase holds until then.`
            : gates.reason}
        </Alert>
      )}
      {display === 'inactive' && (
        <Alert color="gray" variant="light" icon={<IconInfoCircle size={18} aria-hidden="true" />}>
          The campaign is {campaign.status}; gates are no longer evaluated. The numbers below count
          its rows as they stand.
        </Alert>
      )}
      {campaign.status === 'paused' && (
        <Alert color="gray" variant="light" icon={<IconInfoCircle size={18} aria-hidden="true" />}>
          Gates are still evaluated while the campaign is paused, but bake time does not accrue, so
          no phase opens until it resumes.
        </Alert>
      )}
      <div className={classes.metrics}>
        <RateMeter
          label="Failure rate"
          value={overall.failureRate}
          threshold={gates.max_failure_rate}
          exceeded={overall.failureExceeded}
          judged={overall.failureJudged}
          detail={`${formatNumber(gates.overall.failed)} failed of ${plural(overall.sample, 'result')}`}
        />
        <RateMeter
          label="Silent rate"
          value={overall.silentRate}
          threshold={gates.max_silent_rate}
          exceeded={overall.silentExceeded}
          judged={overall.silentJudged}
          detail={`${formatNumber(gates.overall.silent)} silent of ${formatNumber(gates.overall.eligible)} past ${formatDuration(silentWindow)}`}
        />
        <ProgressMetric
          label="Sample"
          value={ready}
          goal={gates.required}
          text={
            ready >= gates.required
              ? 'Enough'
              : `${formatNumber(ready)} of ${formatNumber(gates.required)}`
          }
          detail={`${plural(overall.sample, 'result')} and ${formatNumber(gates.overall.eligible)} past the silent window, ${formatNumber(gates.required)} of each needed${gates.required < gates.min_sample ? `: one per node, as the open phases hold fewer than ${plural(gates.min_sample, 'node')}` : ''}`}
        />
        <ProgressMetric
          label="Bake"
          value={gates.bake_accrued_seconds}
          goal={gates.bake_seconds}
          text={`${formatDuration(Math.min(gates.bake_accrued_seconds, gates.bake_seconds))} of ${formatDuration(gates.bake_seconds)}`}
          detail="Paused time does not count."
        />
      </div>
      <Text size="xs" c="dimmed">
        Unreachable nodes, replaced sessions, undelivered timeouts and denials are retried or
        reported on the Nodes tab; they never count as failures here.
        {maximumFailures !== null &&
          ` The gate fails once more than ${plural(maximumFailures, 'node')} ${maximumFailures === 1 ? 'has' : 'have'} failed; ${formatNumber(gates.overall.failed)} so far.`}
      </Text>
      <Breakdown gates={gates} order={campaign.policy.gates.breakdown} />
    </Stack>
  )
}
