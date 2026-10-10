import {
  Alert,
  Anchor,
  Button,
  EmptyState,
  Group,
  SimpleGrid,
  Skeleton,
  Stack,
  Text,
} from '@mantine/core'
import {
  IconCloudOff,
  IconPlus,
  IconRocket,
  IconServer2,
  IconShieldCheck,
} from '@tabler/icons-react'
import { Link } from 'react-router'
import { alertKindLabel, alertMessage } from '../alerts/kinds'
import { usePermissions } from '../api/permissions'
import { useAlerts, useOverview } from '../api/queries'
import type { Alert as AlertRecord, Lifecycle, Overview, Severity } from '../api/types'
import { lifecycles, severities } from '../api/types'
import { CampaignTable } from '../campaigns/CampaignTable'
import { PageHeader } from '../components/PageHeader'
import { QueryError } from '../components/QueryError'
import { RelativeTime } from '../components/RelativeTime'
import { Section } from '../components/Section'
import { StatusPill } from '../components/StatusPill'
import { lifecycleStyles, severityStyles } from '../components/status'
import { formatNumber, formatPercent, joinWords } from '../format'
import classes from './Overview.module.css'

function FleetSummary({ overview }: { overview: Overview }) {
  const { total, online } = overview.nodes
  const count = (lifecycle: Lifecycle) => overview.nodes.by_lifecycle[lifecycle] ?? 0
  const present = lifecycles.filter((lifecycle) => count(lifecycle) > 0)
  return (
    <div className={classes.fleet}>
      <div className={classes.headline}>
        <Text className={classes.bigNumber} component="p">
          <span className="tabular">{formatNumber(online)}</span>
          <Text component="span" className={classes.bigUnit}>
            {' '}
            online of {formatNumber(total)}
          </Text>
        </Text>
        <Text size="sm" c="dimmed">
          {total === 0
            ? 'No node has enrolled yet.'
            : `${formatPercent(online / total)} of the inventory has a live session right now.`}
        </Text>
      </div>
      {total > 0 && (
        <div className={classes.composition}>
          <div
            className={classes.lifecycleBar}
            role="img"
            aria-label={present
              .map((lifecycle) => `${formatNumber(count(lifecycle))} ${lifecycle}`)
              .join(', ')}
          >
            {present.map((lifecycle) => (
              <span
                key={lifecycle}
                className={classes.lifecycleSegment}
                style={{
                  flexGrow: count(lifecycle),
                  background: `var(--mantine-color-${lifecycleStyles[lifecycle].color}-filled)`,
                }}
              />
            ))}
          </div>
          <ul className={classes.legend}>
            {lifecycles.map((lifecycle: Lifecycle) => {
              const style = lifecycleStyles[lifecycle]
              const LifecycleIcon = style.icon
              return (
                <li key={lifecycle}>
                  <Anchor
                    component={Link}
                    to={`/nodes?selector=${encodeURIComponent(`lifecycle == "${lifecycle}"`)}`}
                    className={classes.legendItem}
                  >
                    <LifecycleIcon
                      size={14}
                      stroke={2}
                      aria-hidden="true"
                      color={`var(--mantine-color-${style.color}-filled)`}
                    />
                    <span>{style.label}</span>
                    <span className={`${classes.legendCount} tabular`}>
                      {formatNumber(count(lifecycle))}
                    </span>
                  </Anchor>
                </li>
              )
            })}
          </ul>
        </div>
      )}
    </div>
  )
}

function AlertList({ alerts, loading }: { alerts: AlertRecord[]; loading: boolean }) {
  if (loading) {
    return (
      <Stack gap="sm" role="status" aria-busy="true" aria-label="Loading open alerts">
        <Skeleton height={48} />
        <Skeleton height={48} />
      </Stack>
    )
  }
  if (alerts.length === 0) {
    return (
      <EmptyState
        size="sm"
        icon={<IconShieldCheck size={22} aria-hidden="true" />}
        title="No open alerts"
        description="Reconcile, enrollment and revocation checks are quiet."
      />
    )
  }
  return (
    <ul className={classes.alerts}>
      {alerts.map((alert) => (
        <li key={alert.id} className={classes.alert}>
          <Group justify="space-between" gap="xs" wrap="nowrap" align="flex-start">
            <StatusPill status={severityStyles[alert.severity]} size="xs" />
            <RelativeTime value={alert.last_seen_at} size="xs" c="dimmed" />
          </Group>
          <Text size="sm" mt={6} className={classes.alertMessage}>
            {alertMessage(alert)}
          </Text>
          <Text size="xs" c="dimmed" mt={2}>
            {alertKindLabel(alert.kind)}
            {alert.acknowledged_by !== null ? `, acknowledged by ${alert.acknowledged_by}` : ''}
          </Text>
        </li>
      ))}
    </ul>
  )
}

function OverviewSkeleton() {
  return (
    <Stack gap="lg" role="status" aria-busy="true" aria-label="Loading the overview">
      <Skeleton height={132} radius="md" />
      <SimpleGrid cols={{ base: 1, lg: 3 }} spacing="lg">
        <Skeleton height={280} radius="md" className={classes.wide} />
        <Skeleton height={280} radius="md" />
      </SimpleGrid>
    </Stack>
  )
}

export function OverviewPage() {
  const overview = useOverview()
  const { canOperate } = usePermissions()
  const data = overview.data
  const openAlerts = Object.values(data?.alerts ?? {}).reduce((sum, count) => sum + count, 0)
  const recent = useAlerts({ state: 'open', limit: 5, cursor: null }, openAlerts > 0)
  const bySeverity = severities
    .filter((severity: Severity) => (data?.alerts[severity] ?? 0) > 0)
    .map(
      (severity) =>
        `${formatNumber(data?.alerts[severity] ?? 0)} ${severityStyles[severity].label.toLowerCase()}`,
    )
  return (
    <>
      <PageHeader
        title="Overview"
        documentTitle="Overview"
        description="What the fleet looks like right now and what is changing it."
        actions={
          canOperate && (
            <Button
              component={Link}
              to="/campaigns/new"
              leftSection={<IconPlus size={16} aria-hidden="true" />}
            >
              New campaign
            </Button>
          )
        }
      />
      {overview.isPending && <OverviewSkeleton />}
      {overview.isError && (
        <QueryError
          error={overview.error}
          what="the overview"
          onRetry={() => void overview.refetch()}
        />
      )}
      {data !== undefined && (
        <Stack gap="lg">
          {data.degraded && (
            <Alert
              color="orange"
              variant="light"
              icon={<IconCloudOff size={18} aria-hidden="true" />}
              title="The online view is degraded"
            >
              <Text size="sm">
                A nightfall instance has stopped publishing its census, or twilight skipped ahead in
                the connection stream and is waiting for the next full census.
              </Text>
              <Text size="sm" mt={4}>
                Online counts may be stale, and health gates hold every campaign until the view
                recovers.
              </Text>
            </Alert>
          )}
          <Section title="Fleet" padded>
            {data.nodes.total === 0 ? (
              <EmptyState
                size="md"
                icon={<IconServer2 size={24} aria-hidden="true" />}
                title="No nodes yet"
                description="Nodes appear here once they enroll with nightfall. Build a node with an init script that runs nightfall -c, and start it."
              />
            ) : (
              <FleetSummary overview={data} />
            )}
          </Section>
          <div className={classes.columns}>
            <Section
              title={`Active campaigns (${formatNumber(data.campaigns.length)})`}
              padded={false}
              actions={
                <Anchor component={Link} to="/campaigns" size="xs">
                  All campaigns
                </Anchor>
              }
            >
              <CampaignTable
                label="Active campaigns"
                campaigns={data.campaigns}
                loading={false}
                compact
                framed={false}
                empty={
                  <EmptyState
                    size="sm"
                    icon={<IconRocket size={22} aria-hidden="true" />}
                    title="Nothing is rolling out"
                    description="Running and paused campaigns show here with their progress."
                  >
                    {canOperate && (
                      <EmptyState.Actions>
                        <Button component={Link} to="/campaigns/new" size="xs" variant="light">
                          New campaign
                        </Button>
                      </EmptyState.Actions>
                    )}
                  </EmptyState>
                }
              />
            </Section>
            <Section
              title={`Open alerts (${formatNumber(openAlerts)})`}
              description={bySeverity.length === 0 ? undefined : `${joinWords(bySeverity, 'and')}.`}
              actions={
                <Anchor component={Link} to="/alerts" size="xs">
                  All alerts
                </Anchor>
              }
            >
              <AlertList
                alerts={openAlerts > 0 ? (recent.data?.items ?? []) : []}
                loading={openAlerts > 0 && recent.isPending}
              />
            </Section>
          </div>
        </Stack>
      )}
    </>
  )
}
