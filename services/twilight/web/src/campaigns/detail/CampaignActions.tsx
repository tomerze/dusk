import { Badge, Button, Group, Menu, Stack, Text, Tooltip } from '@mantine/core'
import { notifications } from '@mantine/notifications'
import {
  IconArchive,
  IconChevronDown,
  IconCircleCheck,
  IconCopy,
  IconEye,
  IconHandStop,
  IconPencil,
  IconPlayerPause,
  IconPlayerPlay,
  IconTrash,
} from '@tabler/icons-react'
import type { ReactNode } from 'react'
import { useState } from 'react'
import { Link, useNavigate } from 'react-router'
import { operatorOnly, usePermissions } from '../../api/permissions'
import type { CampaignCommand } from '../../api/queries'
import { useCampaignCommand, useCampaignOverlap, useMatchedCount } from '../../api/queries'
import type { Campaign, Overlap } from '../../api/types'
import { ConfirmDialog } from '../../components/ConfirmDialog'
import { stateCounts } from '../../components/progress'
import { formatAbsolute, formatDuration, formatNumber, plural } from '../../format'
import { processTimeoutSeconds } from '../validation'

interface DialogSpecification {
  title: string
  confirmLabel: string
  done: string
  color: string
  reason: 'required' | 'optional' | 'none'
  reasonLabel?: string
  body: ReactNode
  overrideGate?: boolean
  blocked?: boolean
}

function expectedInFirstPhase(campaign: Campaign, matched: number | null): string {
  const first = campaign.policy.phases[0]
  if (first === undefined) {
    return 'the first phase'
  }
  const nodes = matched === null ? null : Math.round((matched * first.percent) / 100)
  return `phase 1 (${first.name}, ${first.percent}% of the matched nodes${nodes === null ? '' : `, about ${plural(nodes, 'node')}`})`
}

function dialogFor(
  campaign: Campaign,
  command: CampaignCommand,
  matched: number | null,
  overlap: Overlap | null,
): DialogSpecification {
  const ensure = campaign.kind === 'ensure_version' || campaign.kind === 'ensure_config'
  const rate = `${formatNumber(campaign.policy.rate.per_second)} nodes/s`
  const counts = stateCounts(campaign.counters)
  const cancellable =
    (counts.pending ?? 0) +
    (counts.backoff ?? 0) +
    (counts.excluded ?? 0) +
    (counts.conflict ?? 0) +
    (counts.verifying ?? 0)
  const archiveWait = processTimeoutSeconds(campaign) + 3600
  const overlapping = overlap !== null && overlap.count > 0
  switch (command) {
    case 'start':
      return {
        title: `Start ${campaign.name}`,
        confirmLabel: 'Start campaign',
        done: 'Campaign started',
        color: 'dusk',
        reason: 'none',
        blocked: overlapping && !campaign.policy.allow_overlap,
        body: (
          <Stack gap="xs">
            <Text size="sm">
              Dispatch begins now to {expectedInFirstPhase(campaign, matched)}, at most {rate}. Each
              later phase opens only after the one before it has baked and passed its gate.
            </Text>
            <Text size="sm">
              The definition is fixed from here on. To change it later, duplicate the campaign.
            </Text>
            {overlapping && (
              <Text size="sm" c={campaign.policy.allow_overlap ? 'yellow' : 'red'}>
                {plural(overlap.count, 'node')} {overlap.count === 1 ? 'is' : 'are'} also targeted
                by {plural(overlap.campaigns.length, 'running or paused campaign')} of the same
                kind.{' '}
                {campaign.policy.allow_overlap
                  ? ''
                  : 'twilight refuses to start this one until you edit the draft to narrow its selector or allow overlap.'}
              </Text>
            )}
            {campaign.policy.allow_overlap && (
              <Text size="sm" c="yellow">
                Overlap is allowed: where another campaign for the same key started first, this
                campaign&apos;s rows become conflicts and do nothing.
              </Text>
            )}
          </Stack>
        ),
      }
    case 'pause':
      return {
        title: `Pause ${campaign.name}`,
        confirmLabel: 'Pause campaign',
        done: 'Campaign paused',
        color: 'yellow',
        reason: 'optional',
        body: (
          <Text size="sm">
            Nothing new is dispatched, including retries and resends. Processes already on nodes
            finish and their results are recorded. Gates are still evaluated, but bake time stops
            accruing until you resume. If the gate fails meanwhile, resuming needs a gate override
            and a reason.
          </Text>
        ),
      }
    case 'resume':
      if (campaign.pause_kind === 'gate') {
        return {
          title: `Override the gate and resume ${campaign.name}`,
          confirmLabel: 'Override and resume',
          done: 'Campaign resumed past the failed gate',
          color: 'orange',
          reason: 'required',
          reasonLabel: 'Why is it safe to continue?',
          overrideGate: true,
          body: (
            <Stack gap="xs">
              <Text size="sm">This campaign&apos;s health gate failed:</Text>
              <Text size="sm" className="mono" fw={600}>
                {campaign.pause_reason}
              </Text>
              <Text size="sm">
                Resuming overrides it. For the current phase the gate then counts only the nodes
                dispatched after now, so the same group fails it again if the problem is still
                there. The override and your reason go into the campaign history.
              </Text>
            </Stack>
          ),
        }
      }
      return {
        title: `Resume ${campaign.name}`,
        confirmLabel: 'Resume campaign',
        done: 'Campaign resumed',
        color: 'dusk',
        reason: 'none',
        body: (
          <Text size="sm">
            Dispatch continues in phase {campaign.current_phase + 1} at most {rate}, and bake time
            accrues again.
          </Text>
        ),
      }
    case 'abort':
      if (campaign.status === 'draft') {
        return {
          title: `Discard ${campaign.name}`,
          confirmLabel: 'Discard draft',
          done: 'Draft discarded',
          color: 'red',
          reason: 'required',
          body: (
            <Text size="sm">
              The draft becomes aborted: it can no longer be edited or started, and you can archive
              it afterwards. Nothing was dispatched, so no node is affected.
            </Text>
          ),
        }
      }
      return {
        title: `Abort ${campaign.name}`,
        confirmLabel: 'Abort campaign',
        done: 'Campaign aborted',
        color: 'red',
        reason: 'required',
        body: (
          <Text size="sm">
            {cancellable > 0
              ? `${plural(cancellable, 'node')} that ${cancellable === 1 ? 'is' : 'are'} not in flight ${cancellable === 1 ? 'is' : 'are'} cancelled and nothing new is dispatched. `
              : 'Nothing new is dispatched. '}
            Processes already on nodes finish and their results are recorded. An aborted campaign
            cannot be resumed; to try again, duplicate it.
          </Text>
        ),
      }
    case 'complete':
      return {
        title: `Complete ${campaign.name}`,
        confirmLabel: 'Complete campaign',
        done: 'Campaign completed',
        color: 'green',
        reason: 'optional',
        body: (
          <Text size="sm">
            {ensure
              ? 'The campaign stops enforcing the desired state: nodes that drift later are no longer corrected. '
              : ''}
            {cancellable > 0
              ? `${plural(cancellable, 'node')} that ${cancellable === 1 ? 'is' : 'are'} not in flight ${cancellable === 1 ? 'is' : 'are'} cancelled. `
              : ''}
            Processes already on nodes finish and their results are recorded.
          </Text>
        ),
      }
    case 'archive':
      return {
        title: `Archive ${campaign.name}`,
        confirmLabel: 'Archive campaign',
        done: 'Campaign archived',
        color: 'gray',
        reason: 'none',
        body: (
          <Text size="sm">
            The per-node rows and the history of this campaign are dropped; the campaign, its
            definition and its counters stay. twilight refuses until {formatDuration(archiveWait)}{' '}
            after the last dispatch, so no result can still be arriving
            {campaign.last_dispatch_at === null
              ? '.'
              : `: from ${formatAbsolute(new Date(Date.parse(campaign.last_dispatch_at) + archiveWait * 1000).toISOString())}.`}
          </Text>
        ),
      }
  }
}

export function CampaignActions({ campaign }: { campaign: Campaign }) {
  const { canOperate } = usePermissions()
  const navigate = useNavigate()
  const command = useCampaignCommand(campaign.id)
  const [open, setOpen] = useState<CampaignCommand | null>(null)
  const { matched } = useMatchedCount(campaign.selector, open === 'start')
  const overlap = useCampaignOverlap(
    campaign.id,
    open === 'start' && (campaign.kind === 'ensure_version' || campaign.kind === 'ensure_config'),
  )
  const specification =
    open === null ? null : dialogFor(campaign, open, matched, overlap.data ?? null)

  const duplicate = (
    <Menu.Item
      leftSection={<IconCopy size={16} aria-hidden="true" />}
      onClick={() => void navigate(`/campaigns/new?from=${campaign.id}`)}
    >
      Duplicate as a new draft
    </Menu.Item>
  )

  if (!canOperate) {
    return (
      <Tooltip label={operatorOnly}>
        <Badge
          variant="light"
          color="gray"
          size="lg"
          leftSection={<IconEye size={14} aria-hidden="true" />}
          tabIndex={0}
        >
          View only
        </Badge>
      </Tooltip>
    )
  }

  const status = campaign.status
  const gatePaused = status === 'paused' && campaign.pause_kind === 'gate'
  return (
    <Group gap="xs" wrap="wrap">
      {status === 'draft' && (
        <>
          <Button
            variant="default"
            component={Link}
            to={`/campaigns/${campaign.id}/edit`}
            leftSection={<IconPencil size={16} aria-hidden="true" />}
          >
            Edit
          </Button>
          <Button
            leftSection={<IconPlayerPlay size={16} aria-hidden="true" />}
            onClick={() => setOpen('start')}
          >
            Start
          </Button>
        </>
      )}
      {status === 'running' && (
        <Button
          variant="default"
          leftSection={<IconPlayerPause size={16} aria-hidden="true" />}
          onClick={() => setOpen('pause')}
        >
          Pause
        </Button>
      )}
      {status === 'paused' && (
        <Button
          color={gatePaused ? 'orange' : undefined}
          leftSection={<IconPlayerPlay size={16} aria-hidden="true" />}
          onClick={() => setOpen('resume')}
        >
          {gatePaused ? 'Override and resume' : 'Resume'}
        </Button>
      )}
      {(status === 'running' || status === 'paused') && (
        <Button
          variant="default"
          color="red"
          leftSection={<IconHandStop size={16} aria-hidden="true" />}
          onClick={() => setOpen('abort')}
        >
          Abort
        </Button>
      )}
      <Menu position="bottom-end" withinPortal>
        <Menu.Target>
          <Button variant="default" rightSection={<IconChevronDown size={14} aria-hidden="true" />}>
            More
          </Button>
        </Menu.Target>
        <Menu.Dropdown>
          {(status === 'running' || status === 'paused') && (
            <Menu.Item
              leftSection={<IconCircleCheck size={16} aria-hidden="true" />}
              onClick={() => setOpen('complete')}
            >
              Complete
            </Menu.Item>
          )}
          {status === 'draft' && (
            <Menu.Item
              color="red"
              leftSection={<IconTrash size={16} aria-hidden="true" />}
              onClick={() => setOpen('abort')}
            >
              Discard draft
            </Menu.Item>
          )}
          {(status === 'completed' || status === 'aborted' || status === 'failed') && (
            <Menu.Item
              leftSection={<IconArchive size={16} aria-hidden="true" />}
              onClick={() => setOpen('archive')}
            >
              Archive
            </Menu.Item>
          )}
          {duplicate}
        </Menu.Dropdown>
      </Menu>
      {specification !== null && open !== null && (
        <ConfirmDialog
          opened
          onClose={() => setOpen(null)}
          title={specification.title}
          confirmLabel={specification.confirmLabel}
          color={specification.color}
          reason={specification.reason}
          reasonLabel={specification.reasonLabel}
          reasonDescription="Recorded in the campaign history."
          consequences={specification.body}
          confirmDisabled={specification.blocked === true}
          onConfirm={async (reason) => {
            await command.mutateAsync({
              command: open,
              ...(reason.length > 0 ? { reason } : {}),
              ...(specification.overrideGate === true ? { override_gate: true } : {}),
            })
            notifications.show({ color: 'green', message: specification.done })
          }}
        />
      )}
    </Group>
  )
}
