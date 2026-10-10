import { Text } from '@mantine/core'
import {
  IconCheck,
  IconCircleDashed,
  IconFlag,
  IconHandStop,
  IconPlayerPauseFilled,
  IconPointFilled,
} from '@tabler/icons-react'
import type { CSSProperties } from 'react'
import type { Campaign, Gates } from '../../api/types'
import { SegmentedProgress } from '../../components/SegmentedProgress'
import { formatDuration, plural } from '../../format'
import { useNow } from '../../hooks/useNow'
import { waitingForSample } from './gates'
import type { PhaseView } from './phases'
import { expectedNodes, phaseViews } from './phases'
import classes from './PhaseTimeline.module.css'

interface PhaseContext {
  campaign: Campaign
  gates: Gates | undefined
  converging: boolean
}

function openPhaseState({ campaign, gates, converging }: PhaseContext, view: PhaseView): string {
  if (campaign.status === 'paused') {
    return campaign.pause_kind === 'gate' ? 'Paused by its gate' : 'Paused'
  }
  if (converging) {
    return 'Converging'
  }
  const remaining = Math.max(0, view.bakeSeconds - (view.bakeAccruedSeconds ?? 0))
  if (gates?.verdict === 'fail') {
    return 'Gate failing'
  }
  if (gates?.verdict === 'hold' && gates.degraded && !waitingForSample(gates)) {
    return 'Holding: online view degraded'
  }
  if (gates !== undefined && waitingForSample(gates)) {
    return `Holding: ${gates.reason}`
  }
  if (remaining > 0) {
    return `Baking, ${formatDuration(remaining)} left`
  }
  return view.index === campaign.policy.phases.length - 1 ? 'Finishing' : 'Advancing'
}

function phaseState(context: PhaseContext, view: PhaseView): string {
  const { campaign } = context
  switch (view.status) {
    case 'passed':
      return 'Passed'
    case 'open':
      return openPhaseState(context, view)
    case 'ended':
      return 'Completed here'
    case 'stopped':
      return campaign.status === 'failed' ? 'Stopped by its policy' : 'Stopped here'
    case 'skipped':
      return 'Never opened'
    case 'upcoming':
      return campaign.status === 'draft' ? 'Planned' : 'Not open yet'
  }
}

function Marker({ view, campaign }: { view: PhaseView; campaign: Campaign }) {
  if (view.status === 'passed') {
    return <IconCheck size={14} stroke={3} aria-hidden="true" />
  }
  if (view.status === 'open') {
    return campaign.status === 'paused' ? (
      <IconPlayerPauseFilled size={12} aria-hidden="true" />
    ) : (
      <IconPointFilled size={16} aria-hidden="true" />
    )
  }
  if (view.status === 'stopped') {
    return <IconHandStop size={13} stroke={2.4} aria-hidden="true" />
  }
  if (view.status === 'ended') {
    return <IconFlag size={13} stroke={2.4} aria-hidden="true" />
  }
  return <IconCircleDashed size={14} stroke={1.6} aria-hidden="true" />
}

interface PhaseTimelineProperties {
  campaign: Campaign
  gates: Gates | undefined
  matched: number | null
  converging: boolean
}

export function PhaseTimeline({ campaign, gates, matched, converging }: PhaseTimelineProperties) {
  const now = useNow()
  const views = phaseViews(campaign, now)
  return (
    <ol
      className={classes.timeline}
      aria-label="Phases"
      style={{ '--phase-count': views.length } as CSSProperties}
    >
      {views.map((view) => {
        const expected = expectedNodes(view, matched)
        const state = phaseState({ campaign, gates, converging }, view)
        const tone =
          view.status === 'open' &&
          (gates?.verdict === 'fail' ||
            (campaign.status === 'paused' && campaign.pause_kind === 'gate'))
            ? 'danger'
            : view.status
        const bakeShare =
          view.status === 'open' && view.bakeSeconds > 0
            ? Math.min(1, (view.bakeAccruedSeconds ?? 0) / view.bakeSeconds)
            : null
        return (
          <li
            key={view.index}
            className={classes.phase}
            data-status={view.status}
            data-tone={tone}
            aria-current={view.status === 'open' ? 'step' : undefined}
          >
            <div className={classes.rail}>
              <span className={classes.marker}>
                <Marker view={view} campaign={campaign} />
              </span>
              <span className={classes.connector} aria-hidden="true" />
            </div>
            <div className={classes.body}>
              <div className={classes.title}>
                <Text component="span" fw={650} size="sm" className={classes.name}>
                  {view.name}
                </Text>
                <Text component="span" size="sm" c="dimmed" className="tabular">
                  {view.percent}%
                </Text>
              </div>
              <Text size="xs" c="dimmed" className="tabular">
                {view.status === 'upcoming' || view.status === 'skipped'
                  ? expected === null
                    ? `Phase ${view.index + 1}`
                    : `about ${plural(expected, 'node')}`
                  : plural(view.nodes, 'node')}
                {`, bakes ${formatDuration(view.bakeSeconds)}`}
              </Text>
              <Text size="sm" className={classes.state} data-tone={tone}>
                {state}
              </Text>
              {(view.status === 'passed' ||
                view.status === 'open' ||
                view.status === 'ended' ||
                view.status === 'stopped') && (
                <div className={classes.progress}>
                  <SegmentedProgress counts={view.counts} expected={expected} size="sm" />
                </div>
              )}
              {bakeShare !== null && campaign.status === 'running' && (
                <div
                  className={classes.bake}
                  role="meter"
                  aria-label="Bake time accrued"
                  aria-valuemin={0}
                  aria-valuemax={view.bakeSeconds}
                  aria-valuenow={view.bakeAccruedSeconds ?? 0}
                  aria-valuetext={`${formatDuration(view.bakeAccruedSeconds ?? 0)} of ${formatDuration(view.bakeSeconds)}`}
                >
                  <span style={{ width: `${bakeShare * 100}%` }} />
                </div>
              )}
            </div>
          </li>
        )
      })}
    </ol>
  )
}
