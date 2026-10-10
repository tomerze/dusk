import {
  Anchor,
  Button,
  Code,
  EmptyState,
  Group,
  SegmentedControl,
  Select,
  Skeleton,
  Stack,
  Text,
  UnstyledButton,
} from '@mantine/core'
import { notifications } from '@mantine/notifications'
import { IconChevronRight, IconFilterOff, IconShieldCheck } from '@tabler/icons-react'
import type { ReactNode } from 'react'
import { useState } from 'react'
import { Link, useSearchParams } from 'react-router'
import { errorMessage } from '../api/client'
import { usePermissions } from '../api/permissions'
import { searchLimit, useAlertCommand, useAlerts, useEveryAlert } from '../api/queries'
import type { Alert, JsonValue, Severity } from '../api/types'
import { severities } from '../api/types'
import { CursorPagination } from '../components/CursorPagination'
import { MonoId } from '../components/MonoId'
import { PageHeader } from '../components/PageHeader'
import { QueryError } from '../components/QueryError'
import { RelativeTime } from '../components/RelativeTime'
import { StatusPill } from '../components/StatusPill'
import { severityStyles } from '../components/status'
import { formatNumber } from '../format'
import { useCursorPages } from '../hooks/useCursorPages'
import { alertKindLabel, alertKinds, alertMessage, alertSubject } from './kinds'
import classes from './AlertsPage.module.css'

const identifierKeys = new Set(['device_id', 'installation_id', 'pid', 'fingerprint'])

function detailValue(key: string, value: JsonValue, alert: Alert): ReactNode {
  const deviceId = alert.detail.device_id
  const installationId = alert.detail.installation_id
  if (key === 'device_id' && typeof deviceId === 'string' && typeof installationId === 'string') {
    return (
      <MonoId
        value={deviceId}
        label="device id"
        maximum={20}
        to={`/nodes/${deviceId}/${installationId}`}
      />
    )
  }
  if (key.endsWith('campaign_id') && typeof value === 'string') {
    return (
      <MonoId
        value={value}
        label={key.replaceAll('_', ' ')}
        maximum={20}
        to={`/campaigns/${value}`}
      />
    )
  }
  if (identifierKeys.has(key) && typeof value === 'string') {
    return <MonoId value={value} label={key.replaceAll('_', ' ')} maximum={24} />
  }
  return (
    <Code className={classes.value}>
      {typeof value === 'string' ? value : JSON.stringify(value)}
    </Code>
  )
}

function AlertState({ alert }: { alert: Alert }) {
  if (alert.resolved_at !== null) {
    return (
      <Text size="xs" c="dimmed">
        Resolved <RelativeTime value={alert.resolved_at} size="xs" c="dimmed" />
      </Text>
    )
  }
  if (alert.acknowledged_at !== null) {
    return (
      <Text size="xs" c="dimmed">
        Acknowledged by {alert.acknowledged_by ?? 'someone'}
      </Text>
    )
  }
  return (
    <Text size="xs" fw={600} c="orange">
      Waiting for acknowledgement
    </Text>
  )
}

function AlertRow({ alert, canOperate }: { alert: Alert; canOperate: boolean }) {
  const [open, setOpen] = useState(false)
  const command = useAlertCommand()
  const kind = alertKinds[alert.kind]
  const detailId = `alert-${alert.id}-detail`
  const run = (name: 'acknowledge' | 'resolve') =>
    command.mutate(
      { alertId: alert.id, command: name },
      {
        onSuccess: () =>
          notifications.show({
            color: 'green',
            message: name === 'acknowledge' ? 'Alert acknowledged' : 'Alert resolved',
          }),
        onError: (error) =>
          notifications.show({
            color: 'red',
            title: name === 'acknowledge' ? 'Could not acknowledge' : 'Could not resolve',
            message: errorMessage(error),
          }),
      },
    )
  return (
    <li
      className={classes.row}
      data-open={open || undefined}
      data-severity={alert.severity}
      data-resolved={alert.resolved_at !== null || undefined}
    >
      <div className={classes.line}>
        <UnstyledButton
          className={classes.summary}
          onClick={() => setOpen((value) => !value)}
          aria-expanded={open}
          aria-controls={detailId}
        >
          <IconChevronRight size={14} className={classes.chevron} aria-hidden="true" />
          <span className={classes.severity}>
            <StatusPill status={severityStyles[alert.severity]} size="xs" />
          </span>
          <span className={classes.text}>
            <Text component="span" size="sm" fw={650} className={classes.kind}>
              {alertKindLabel(alert.kind)}
            </Text>
            <Text component="span" size="sm" className={classes.message}>
              {alertMessage(alert)}
              {alertSubject(alert) !== null && (
                <Text component="span" size="xs" c="dimmed" className="mono">
                  {' '}
                  {alertSubject(alert)}
                </Text>
              )}
            </Text>
          </span>
          <span className={classes.when}>
            <Text component="span" size="xs">
              <RelativeTime value={alert.last_seen_at} size="xs" />
              {alert.occurrences > 1 && (
                <Text component="span" size="xs" c="dimmed" className="tabular">
                  {' '}
                  ×{formatNumber(alert.occurrences)}
                </Text>
              )}
            </Text>
            <AlertState alert={alert} />
          </span>
        </UnstyledButton>
        {canOperate && (
          <Group gap={6} wrap="nowrap" justify="flex-end" className={classes.actions}>
            {alert.resolved_at === null && alert.acknowledged_at === null && (
              <Button
                size="xs"
                variant="default"
                loading={command.isPending && command.variables?.command === 'acknowledge'}
                onClick={() => run('acknowledge')}
              >
                Acknowledge
              </Button>
            )}
            {alert.resolved_at === null && (
              <Button
                size="xs"
                variant="subtle"
                color="gray"
                loading={command.isPending && command.variables?.command === 'resolve'}
                onClick={() => run('resolve')}
              >
                Resolve
              </Button>
            )}
          </Group>
        )}
      </div>
      {open && (
        <div className={classes.detail} id={detailId}>
          {kind !== undefined && (
            <Text size="sm" mb="sm" maw="80ch">
              {kind.meaning}
            </Text>
          )}
          <dl className={classes.fields}>
            <div className={classes.field}>
              <dt>kind</dt>
              <dd>
                <Code className={classes.value}>{alert.kind}</Code>
              </dd>
            </div>
            {Object.entries(alert.detail)
              .filter(([key]) => key !== 'message')
              .map(([key, value]) => (
                <div key={key} className={classes.field}>
                  <dt>{key}</dt>
                  <dd>{detailValue(key, value, alert)}</dd>
                </div>
              ))}
            <div className={classes.field}>
              <dt>raised</dt>
              <dd>
                <RelativeTime value={alert.time} size="sm" />
              </dd>
            </div>
            {alert.occurrences > 1 && (
              <div className={classes.field}>
                <dt>seen</dt>
                <dd>
                  <Text component="span" size="sm" className="tabular">
                    {formatNumber(alert.occurrences)} times, last{' '}
                  </Text>
                  <RelativeTime value={alert.last_seen_at} size="sm" />
                </dd>
              </div>
            )}
            {alert.acknowledged_at !== null && (
              <div className={classes.field}>
                <dt>acknowledged</dt>
                <dd>
                  <RelativeTime value={alert.acknowledged_at} size="sm" />
                  <Text component="span" size="sm" c="dimmed">
                    {' '}
                    by {alert.acknowledged_by ?? 'someone'}
                  </Text>
                </dd>
              </div>
            )}
            {alert.resolved_at !== null && (
              <div className={classes.field}>
                <dt>resolved</dt>
                <dd>
                  <RelativeTime value={alert.resolved_at} size="sm" />
                  <Text component="span" size="sm" c="dimmed">
                    {' '}
                    by {alert.resolved_by ?? 'someone'}
                  </Text>
                </dd>
              </div>
            )}
          </dl>
        </div>
      )}
    </li>
  )
}

const kindOptions = [
  { value: 'any', label: 'Any kind' },
  ...Object.entries(alertKinds)
    .map(([value, kind]) => ({ value, label: kind.label }))
    .sort((left, right) => left.label.localeCompare(right.label)),
]

export function AlertsPage() {
  const { canOperate } = usePermissions()
  const [searchParameters, setSearchParameters] = useSearchParams()
  const state = searchParameters.get('state') === 'all' ? 'all' : 'open'
  const severityText = searchParameters.get('severity')
  const severity = severities.find((candidate) => candidate === severityText) ?? null
  const kindText = searchParameters.get('kind')
  const kind = kindText !== null && kindText in alertKinds ? kindText : null
  const pages = useCursorPages(`${state}/${severity}/${kind}`, 50)
  const filtered = severity !== null || kind !== null
  const server = useAlerts({ state, limit: pages.pageSize, cursor: pages.cursor }, !filtered)
  const every = useEveryAlert(state, filtered)
  const alerts = filtered ? every : server
  const offset = filtered ? Number(pages.cursor ?? 0) : 0
  const found = every.data?.items.filter(
    (alert) =>
      (severity === null || alert.severity === severity) && (kind === null || alert.kind === kind),
  )
  const setParameter = (key: string, value: string | null) =>
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
  const items = filtered
    ? (found ?? []).slice(offset, offset + pages.pageSize)
    : (server.data?.items ?? [])
  const nextCursor = filtered
    ? found !== undefined && offset + pages.pageSize < found.length
      ? String(offset + pages.pageSize)
      : null
    : (server.data?.next_cursor ?? null)

  return (
    <>
      <PageHeader
        title="Alerts"
        documentTitle="Alerts"
        description="What twilight's checks found: processes nobody intended, broken ledger chains, revocations not enforced, enrollment spikes and campaign conflicts."
      />
      <Stack gap="md">
        <Group gap="sm" wrap="wrap" role="search" aria-label="Filter alerts">
          <SegmentedControl
            aria-label="Which alerts"
            value={state}
            onChange={(value) => setParameter('state', value === 'all' ? 'all' : null)}
            data={[
              { value: 'open', label: 'Open' },
              { value: 'all', label: 'All' },
            ]}
          />
          <Select
            aria-label="Severity"
            value={severity ?? 'any'}
            onChange={(value) => setParameter('severity', value === 'any' ? null : value)}
            data={[
              { value: 'any', label: 'Any severity' },
              ...severities.map((entry: Severity) => ({
                value: entry,
                label: severityStyles[entry].label,
              })),
            ]}
            w={160}
          />
          <Select
            aria-label="Kind"
            value={kind ?? 'any'}
            onChange={(value) => setParameter('kind', value === 'any' ? null : value)}
            data={kindOptions}
            w={240}
          />
          {filtered && (
            <Button
              variant="subtle"
              color="gray"
              leftSection={<IconFilterOff size={16} aria-hidden="true" />}
              onClick={() =>
                setSearchParameters(state === 'all' ? { state: 'all' } : {}, { replace: true })
              }
            >
              Clear filters
            </Button>
          )}
        </Group>
        {filtered && every.data?.truncated === true && (
          <Text size="xs" c="dimmed">
            Filtered the {formatNumber(searchLimit)} newest alerts.
          </Text>
        )}
        {alerts.isError ? (
          <QueryError error={alerts.error} what="alerts" onRetry={() => void alerts.refetch()} />
        ) : alerts.isPending ? (
          <Stack gap={6} role="status" aria-busy="true" aria-label="Loading alerts">
            {Array.from(Array(6).keys(), (index) => (
              <Skeleton key={index} height={52} radius="md" />
            ))}
          </Stack>
        ) : items.length === 0 ? (
          <EmptyState
            size="md"
            icon={
              filtered ? (
                <IconFilterOff size={24} aria-hidden="true" />
              ) : (
                <IconShieldCheck size={24} aria-hidden="true" />
              )
            }
            title={
              filtered
                ? 'No alert matches these filters'
                : state === 'open'
                  ? 'No open alerts'
                  : 'No alerts yet'
            }
            description={
              filtered
                ? 'Change or clear the filters.'
                : 'Reconcile compares every call in the nightfall ledger with the processes twilight intended; anything that does not match lands here.'
            }
          >
            {state === 'open' && !filtered && (
              <EmptyState.Actions>
                <Anchor component={Link} to="/alerts?state=all" size="sm">
                  See resolved alerts
                </Anchor>
              </EmptyState.Actions>
            )}
          </EmptyState>
        ) : (
          <ol className={classes.list} aria-label="Alerts">
            {items.map((alert) => (
              <AlertRow key={alert.id} alert={alert} canOperate={canOperate} />
            ))}
          </ol>
        )}
        {alerts.data !== undefined && (items.length > 0 || pages.pageIndex > 0) && (
          <CursorPagination
            pages={pages}
            shown={items.length}
            total={filtered && found !== undefined ? found.length : null}
            nextCursor={nextCursor}
            noun="alerts"
            pageSizes={[50, 100, 200]}
          />
        )}
      </Stack>
    </>
  )
}
