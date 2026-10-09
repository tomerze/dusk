import type { Icon } from '@tabler/icons-react'
import {
  IconArchive,
  IconArrowsSplit2,
  IconChecks,
  IconCircleCheck,
  IconCircleX,
  IconFlag,
  IconHandStop,
  IconHourglassHigh,
  IconPencil,
  IconPlayerPause,
  IconPlayerPlay,
  IconPlus,
  IconPoint,
  IconRotateClockwise,
  IconShieldX,
  IconUserCheck,
} from '@tabler/icons-react'
import type { CampaignEvent, JsonValue } from '../../api/types'
import { formatNumber, plural } from '../../format'

export type EventGroup = 'lifecycle' | 'gate' | 'operator' | 'nodes'

export interface EventStyle {
  label: string
  icon: Icon
  color: string
  group: EventGroup
}

const knownStyles: Record<string, EventStyle> = {
  created: { label: 'Created', icon: IconPlus, color: 'gray', group: 'operator' },
  updated: { label: 'Definition updated', icon: IconPencil, color: 'gray', group: 'operator' },
  started: { label: 'Started', icon: IconPlayerPlay, color: 'dusk', group: 'operator' },
  phase_advanced: { label: 'Phase opened', icon: IconFlag, color: 'dusk', group: 'lifecycle' },
  gate_failed: {
    label: 'Gate failed while paused',
    icon: IconShieldX,
    color: 'red',
    group: 'gate',
  },
  gate_holding: {
    label: 'Gate holding',
    icon: IconHourglassHigh,
    color: 'yellow',
    group: 'gate',
  },
  paused: { label: 'Paused', icon: IconPlayerPause, color: 'yellow', group: 'operator' },
  resumed: { label: 'Resumed', icon: IconPlayerPlay, color: 'dusk', group: 'operator' },
  conflict: { label: 'Conflict', icon: IconArrowsSplit2, color: 'grape', group: 'nodes' },
  nodes_retried: {
    label: 'Nodes retried',
    icon: IconRotateClockwise,
    color: 'blue',
    group: 'operator',
  },
  nodes_resolved: { label: 'Nodes resolved', icon: IconChecks, color: 'blue', group: 'operator' },
  completed: { label: 'Completed', icon: IconCircleCheck, color: 'green', group: 'lifecycle' },
  aborted: { label: 'Aborted', icon: IconHandStop, color: 'orange', group: 'operator' },
  failed: { label: 'Failed', icon: IconCircleX, color: 'red', group: 'gate' },
  archived: { label: 'Archived', icon: IconArchive, color: 'gray', group: 'operator' },
}

const gatePause: EventStyle = {
  label: 'Paused by its gate',
  icon: IconShieldX,
  color: 'red',
  group: 'gate',
}

const permissionPause: EventStyle = {
  label: 'Paused, dispatch denied',
  icon: IconShieldX,
  color: 'red',
  group: 'nodes',
}

const overrideResume: EventStyle = {
  label: 'Gate overridden',
  icon: IconUserCheck,
  color: 'orange',
  group: 'operator',
}

function text(detail: Record<string, JsonValue>, key: string): string | null {
  const value = detail[key]
  return typeof value === 'string' && value.length > 0 ? value : null
}

function count(detail: Record<string, JsonValue>, key: string): number | null {
  const value = detail[key]
  return typeof value === 'number' && Number.isFinite(value) ? value : null
}

function tally(detail: Record<string, JsonValue>): { succeeded: number; failed: number } | null {
  const overall = detail.overall
  if (typeof overall !== 'object' || overall === null || Array.isArray(overall)) {
    return null
  }
  const succeeded = count(overall, 'succeeded')
  const failed = count(overall, 'failed')
  return succeeded === null || failed === null ? null : { succeeded, failed }
}

function capitalize(sentence: string): string {
  return sentence.charAt(0).toUpperCase() + sentence.slice(1)
}

function joined(parts: (string | null)[]): string {
  return capitalize(
    parts.filter((part): part is string => part !== null && part.length > 0).join('; '),
  )
}

export function eventStyle(event: CampaignEvent): EventStyle {
  if (event.kind === 'paused') {
    const kind = text(event.detail, 'pause_kind')
    if (kind === 'gate') {
      return gatePause
    }
    if (kind === 'permission') {
      return permissionPause
    }
  }
  if (event.kind === 'resumed' && event.detail.override_gate === true) {
    return overrideResume
  }
  return (
    knownStyles[event.kind] ?? {
      label: capitalize(event.kind.replaceAll('_', ' ')),
      icon: IconPoint,
      color: 'gray',
      group: 'lifecycle',
    }
  )
}

export function summarizeEvent(event: CampaignEvent): string {
  const { detail } = event
  const reason = text(detail, 'reason')
  switch (event.kind) {
    case 'created': {
      const name = text(detail, 'name')
      return name === null ? 'Saved as a draft' : `Saved as a draft named ${name}`
    }
    case 'updated':
      return 'The draft definition changed'
    case 'started':
      return 'Dispatch began in the first phase'
    case 'phase_advanced': {
      const from = text(detail, 'from')
      const to = text(detail, 'to')
      const percent = count(detail, 'percent')
      const so = tally(detail)
      return joined([
        to === null
          ? 'the next phase opened'
          : `${to} opened${percent === null ? '' : ` at ${percent}%`}${from === null ? '' : ` after ${from} passed its gate`}`,
        so === null
          ? null
          : `${formatNumber(so.succeeded)} succeeded and ${formatNumber(so.failed)} failed so far`,
      ])
    }
    case 'gate_failed':
      return joined([
        'the gate failed while the campaign was paused; resuming needs a gate override',
        reason,
      ])
    case 'gate_holding': {
      const phase = count(detail, 'phase')
      return joined([phase === null ? 'the gate holds' : `phase ${phase + 1} holds`, reason])
    }
    case 'paused': {
      const kind = text(detail, 'pause_kind')
      if (kind === 'gate') {
        return joined(['dispatch stopped by a health gate', reason])
      }
      if (kind === 'permission') {
        return joined(['dispatch stopped after nightfall denied a dispatch', reason])
      }
      return joined(['dispatch stopped', reason])
    }
    case 'resumed':
      return detail.override_gate === true
        ? joined([
            'dispatch resumed past a failed gate; the gate now counts only nodes dispatched from here on',
            reason === null ? null : `reason: ${reason}`,
          ])
        : 'Dispatch resumed'
    case 'conflict':
      return (
        text(detail, 'message') ??
        "An earlier campaign of the same kind owns some of this campaign's nodes"
      )
    case 'nodes_retried': {
      const retried = count(detail, 'count')
      return joined([
        retried === null
          ? 'nodes were sent again'
          : `${plural(retried, 'node')} sent again at a new pid`,
        reason,
      ])
    }
    case 'nodes_resolved': {
      const resolved = count(detail, 'count')
      const outcome = text(detail, 'outcome')
      return joined([
        resolved === null
          ? 'nodes were resolved by hand'
          : `${plural(resolved, 'node')} marked ${outcome ?? 'resolved'} by hand`,
        reason,
      ])
    }
    case 'completed':
      return joined(['the campaign finished', reason])
    case 'aborted':
      return joined(['stopped by an operator; nodes not in flight were cancelled', reason])
    case 'failed':
      return joined(['stopped by its policy; nodes not in flight were cancelled', reason])
    case 'archived':
      return 'Node rows and history were dropped; counters remain'
    default:
      return reason ?? text(detail, 'message') ?? ''
  }
}
