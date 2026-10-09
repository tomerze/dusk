import { Alert, Anchor, Badge, Group, Skeleton, Stack, Tabs, Text } from '@mantine/core'
import {
  IconAlertOctagon,
  IconHandStop,
  IconPlayerPause,
  IconRefresh,
  IconShieldX,
} from '@tabler/icons-react'
import { Link, useParams, useSearchParams } from 'react-router'
import { ApiError } from '../../api/client'
import { useCampaign, useCampaignGates, useMatchedCount } from '../../api/queries'
import type { Campaign, NodeState } from '../../api/types'
import { nodeStates } from '../../api/types'
import { PageHeader } from '../../components/PageHeader'
import { QueryError } from '../../components/QueryError'
import { RelativeTime } from '../../components/RelativeTime'
import { Section } from '../../components/Section'
import type { StateCounts } from '../../components/progress'
import { countOf, stateCounts } from '../../components/progress'
import { SegmentedProgress } from '../../components/SegmentedProgress'
import { StatusPill } from '../../components/StatusPill'
import { campaignStatusStyles, nodeStateStyles } from '../../components/status'
import { formatNumber, formatPercent } from '../../format'
import { NotFound } from '../../app/RouteError'
import { actionLabels, displayStatus, isActive } from '../display'
import { CampaignActions } from './CampaignActions'
import { CampaignDefinitionView } from './CampaignDefinition'
import { CampaignNodes } from './CampaignNodes'
import { EventHistory } from './EventHistory'
import { GatePanel } from './GatePanel'
import { PhaseTimeline } from './PhaseTimeline'
import classes from './CampaignPage.module.css'

const tabs = ['gates', 'nodes', 'history', 'definition'] as const
type Tab = (typeof tabs)[number]

function actionDetail(campaign: Campaign): string {
  const action = campaign.action
  switch (action.kind) {
    case 'ensure_version':
      return `${actionLabels[action.kind]} ${action.version_key} ${action.version}`
    case 'ensure_config':
      return `${actionLabels[action.kind]} ${action.config_hash.slice(0, 12)}`
    default:
      return actionLabels[action.kind]
  }
}

function StateLegend({ counts, expected }: { counts: StateCounts; expected: number | null }) {
  const present: NodeState[] = nodeStates.filter((state) => (counts[state] ?? 0) > 0)
  const notReached = Math.max(0, (expected ?? 0) - countOf(counts))
  return (
    <ul className={classes.legend}>
      {present.map((state) => {
        const style = nodeStateStyles[state]
        const StateIcon = style.icon
        return (
          <li key={state}>
            <Anchor
              component={Link}
              to={`?tab=nodes&state=${state}`}
              className={classes.legendItem}
            >
              <StateIcon
                size={14}
                stroke={2}
                aria-hidden="true"
                color={`var(--mantine-color-${style.color}-text)`}
              />
              <span>{style.label}</span>
              <span className={`${classes.legendCount} tabular`}>
                {formatNumber(counts[state] ?? 0)}
              </span>
            </Anchor>
          </li>
        )
      })}
      {notReached > 0 && (
        <li className={classes.legendItem}>
          <span className={classes.notReachedSwatch} aria-hidden="true" />
          <span>Not reached yet</span>
          <span className={`${classes.legendCount} tabular`}>{formatNumber(notReached)}</span>
        </li>
      )}
    </ul>
  )
}

function StatusBanner({ campaign, converging }: { campaign: Campaign; converging: boolean }) {
  if (campaign.status === 'paused') {
    const gate = campaign.pause_kind === 'gate'
    return (
      <Alert
        color={gate ? 'red' : 'yellow'}
        variant="light"
        icon={
          gate ? (
            <IconShieldX size={18} aria-hidden="true" />
          ) : (
            <IconPlayerPause size={18} aria-hidden="true" />
          )
        }
        title={
          <Group gap={6} wrap="wrap">
            <span>
              {gate
                ? 'Paused; its health gate failed'
                : campaign.pause_kind === 'permission'
                  ? 'Paused after nightfall denied a dispatch'
                  : 'Paused by an operator'}
            </span>
            {campaign.paused_at !== null && (
              <RelativeTime value={campaign.paused_at} size="sm" c="dimmed" />
            )}
          </Group>
        }
      >
        <Text size="sm" className={gate ? 'mono' : undefined}>
          {campaign.pause_reason === null || campaign.pause_reason.length === 0
            ? 'No reason was given.'
            : campaign.pause_reason}
        </Text>
        {gate && (
          <Text size="sm" mt={4}>
            Look at the failing group under Gates before resuming; resuming needs a gate override
            and a reason.
          </Text>
        )}
      </Alert>
    )
  }
  if (
    (campaign.status === 'aborted' || campaign.status === 'failed') &&
    campaign.abort_reason !== null
  ) {
    const failed = campaign.status === 'failed'
    return (
      <Alert
        color={failed ? 'red' : 'orange'}
        variant="light"
        icon={
          failed ? (
            <IconAlertOctagon size={18} aria-hidden="true" />
          ) : (
            <IconHandStop size={18} aria-hidden="true" />
          )
        }
        title={failed ? 'Stopped by its policy' : 'Aborted by an operator'}
      >
        <Text size="sm" className={failed ? 'mono' : undefined}>
          {campaign.abort_reason}
        </Text>
      </Alert>
    )
  }
  if (campaign.status === 'running' && converging) {
    return (
      <Alert
        color="teal"
        variant="light"
        icon={<IconRefresh size={18} aria-hidden="true" />}
        title="Converging"
      >
        Every phase passed. The campaign keeps sending its action to matching nodes that report
        anything other than the desired state, until you complete or abort it.
      </Alert>
    )
  }
  return null
}

function CampaignSkeleton() {
  return (
    <Stack gap="lg" role="status" aria-busy="true" aria-label="Loading the campaign">
      <Skeleton height={28} width="40%" />
      <Skeleton height={16} width="60%" />
      <Skeleton height={120} radius="md" />
      <Skeleton height={140} radius="md" />
      <Skeleton height={320} radius="md" />
    </Stack>
  )
}

export function CampaignPage() {
  const { campaignId = '' } = useParams()
  const [searchParameters, setSearchParameters] = useSearchParams()
  const tabText = searchParameters.get('tab')
  const tab: Tab = tabs.find((candidate) => candidate === tabText) ?? 'gates'
  const campaignQuery = useCampaign(campaignId)
  const campaign = campaignQuery.data
  const gatesQuery = useCampaignGates(
    campaignId,
    campaign !== undefined && campaign.status !== 'draft',
  )
  const { matched, failure: countFailure } = useMatchedCount(
    campaign?.selector ?? '',
    campaign !== undefined && (campaign.status === 'draft' || isActive(campaign)),
  )

  if (campaignQuery.isError) {
    if (campaignQuery.error instanceof ApiError && campaignQuery.error.status === 404) {
      return <NotFound />
    }
    return (
      <QueryError
        error={campaignQuery.error}
        what="the campaign"
        onRetry={() => void campaignQuery.refetch()}
      />
    )
  }
  if (campaign === undefined) {
    return <CampaignSkeleton />
  }
  const converging = gatesQuery.data?.converging ?? false
  const status = displayStatus(campaign, converging)
  const counts = stateCounts(campaign.counters)
  const rows = countOf(counts)
  const succeeded = counts.succeeded ?? 0
  const expected = isActive(campaign) ? matched : null
  const target = Math.max(expected ?? 0, rows)

  return (
    <>
      <PageHeader
        trail={[{ label: 'Campaigns', to: '/campaigns' }]}
        title={campaign.name}
        documentTitle={campaign.name}
        status={<StatusPill status={campaignStatusStyles[status]} size="lg" />}
        description={campaign.description.length > 0 ? campaign.description : undefined}
        actions={<CampaignActions campaign={campaign} />}
      />
      <Stack gap="lg">
        <dl className={classes.meta}>
          <div>
            <dt>Action</dt>
            <dd className="mono">{actionDetail(campaign)}</dd>
          </div>
          <div>
            <dt>Owner</dt>
            <dd>{campaign.created_by}</dd>
          </div>
          <div>
            <dt>{campaign.started_at === null ? 'Created' : 'Started'}</dt>
            <dd>
              <RelativeTime value={campaign.started_at ?? campaign.created_at} />
            </dd>
          </div>
          {campaign.finished_at !== null && (
            <div>
              <dt>Finished</dt>
              <dd>
                <RelativeTime value={campaign.finished_at} />
              </dd>
            </div>
          )}
          <div>
            <dt>Rate</dt>
            <dd className="tabular">{formatNumber(campaign.policy.rate.per_second)} nodes/s</dd>
          </div>
          {campaign.tenant !== null && (
            <div>
              <dt>Tenant</dt>
              <dd>{campaign.tenant}</dd>
            </div>
          )}
        </dl>
        <StatusBanner campaign={campaign} converging={converging} />
        <Section
          title="Progress"
          description={
            campaign.status === 'draft'
              ? `${countFailure !== null ? `Could not count the nodes that match: ${countFailure}` : matched === null ? 'Counting the nodes that match' : `${formatNumber(matched)} nodes match the selector now`}. Nothing is dispatched until the campaign starts.`
              : expected === null
                ? `${formatNumber(succeeded)} of ${formatNumber(rows)} nodes with a row succeeded (${formatPercent(rows === 0 ? null : succeeded / rows)}).${isActive(campaign) && countFailure !== null ? ` Could not count the nodes the selector matches now: ${countFailure}.` : ''}`
                : `${formatNumber(succeeded)} of ${formatNumber(target)} nodes succeeded (${formatPercent(target === 0 ? null : succeeded / target)}), counting every node the selector matches now.`
          }
        >
          <Stack gap="sm">
            <SegmentedProgress
              counts={counts}
              expected={expected}
              phases={campaign.policy.phases.map((entry) => entry.percent)}
              size="lg"
            />
            <StateLegend counts={counts} expected={expected} />
          </Stack>
        </Section>
        <Section
          title="Phases"
          description="Each phase covers a cumulative share of the matched nodes and opens after the one before it bakes and passes its gate."
        >
          <PhaseTimeline
            campaign={campaign}
            gates={gatesQuery.data}
            matched={matched}
            converging={converging}
          />
        </Section>
        <Tabs
          value={tab}
          onChange={(value) =>
            setSearchParameters(value === null || value === 'gates' ? {} : { tab: value }, {
              replace: true,
            })
          }
          keepMounted={false}
          className={classes.tabs}
        >
          <Tabs.List>
            <Tabs.Tab value="gates">
              Gates
              {gatesQuery.data?.verdict === 'fail' && (
                <Badge size="xs" color="red" variant="filled" ml={6}>
                  failing
                </Badge>
              )}
            </Tabs.Tab>
            <Tabs.Tab value="nodes">
              Nodes{' '}
              <Text component="span" size="xs" c="dimmed" className="tabular">
                {formatNumber(rows)}
              </Text>
            </Tabs.Tab>
            <Tabs.Tab value="history">History</Tabs.Tab>
            <Tabs.Tab value="definition">Definition</Tabs.Tab>
          </Tabs.List>
          <Tabs.Panel value="gates" pt="md">
            {campaign.status === 'draft' ? (
              <Text size="sm" c="dimmed">
                Gates are evaluated once the campaign starts.
              </Text>
            ) : (
              <GatePanel
                campaign={campaign}
                gates={gatesQuery.data}
                error={gatesQuery.error}
                loading={gatesQuery.isPending}
                onRetry={() => void gatesQuery.refetch()}
                updatedAt={gatesQuery.dataUpdatedAt}
              />
            )}
          </Tabs.Panel>
          <Tabs.Panel value="nodes" pt="md">
            <CampaignNodes campaign={campaign} matched={matched} />
          </Tabs.Panel>
          <Tabs.Panel value="history" pt="md">
            <EventHistory campaignId={campaign.id} status={campaign.status} />
          </Tabs.Panel>
          <Tabs.Panel value="definition" pt="md">
            <CampaignDefinitionView definition={campaign} matched={matched} />
          </Tabs.Panel>
        </Tabs>
      </Stack>
    </>
  )
}
