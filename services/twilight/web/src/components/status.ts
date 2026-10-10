import type { Icon } from '@tabler/icons-react'
import {
  IconAlertCircle,
  IconAlertOctagon,
  IconAlertTriangle,
  IconArchive,
  IconArrowsSplit2,
  IconBan,
  IconCircleCheck,
  IconCircleDot,
  IconCircleX,
  IconClock,
  IconCloudOff,
  IconCopy,
  IconFilterOff,
  IconFlag,
  IconHandStop,
  IconHourglassHigh,
  IconInfoCircle,
  IconLock,
  IconPencil,
  IconPlayerPause,
  IconPlayerPlay,
  IconPlugConnectedX,
  IconQuestionMark,
  IconRecycle,
  IconRefresh,
  IconSearch,
  IconSend,
  IconShield,
  IconShieldCheck,
  IconShieldX,
  IconUserPlus,
} from '@tabler/icons-react'
import type { CampaignStatus, Lifecycle, NodeState, ProcessStatus, Severity } from '../api/types'

export interface StatusStyle {
  label: string
  color: string
  icon: Icon
  description: string
}

export type CampaignDisplayStatus = CampaignStatus | 'converging'

export type GateDisplay = 'pass' | 'hold' | 'fail' | 'degraded' | 'inactive' | 'converging'

export const campaignStatusStyles: Record<CampaignDisplayStatus, StatusStyle> = {
  draft: {
    label: 'Draft',
    color: 'gray',
    icon: IconPencil,
    description: 'Not started; nothing is dispatched.',
  },
  running: {
    label: 'Running',
    color: 'dusk',
    icon: IconPlayerPlay,
    description: 'Dispatching to nodes in the open phases.',
  },
  converging: {
    label: 'Converging',
    color: 'teal',
    icon: IconRefresh,
    description: 'Every phase passed; desired state is enforced until you complete or abort.',
  },
  paused: {
    label: 'Paused',
    color: 'yellow',
    icon: IconPlayerPause,
    description: 'No new dispatch; processes already on nodes finish.',
  },
  completed: {
    label: 'Completed',
    color: 'green',
    icon: IconCircleCheck,
    description: 'Finished.',
  },
  aborted: {
    label: 'Aborted',
    color: 'orange',
    icon: IconHandStop,
    description: 'Stopped by an operator.',
  },
  failed: {
    label: 'Failed',
    color: 'red',
    icon: IconCircleX,
    description: 'Stopped by its policy.',
  },
  archived: {
    label: 'Archived',
    color: 'gray',
    icon: IconArchive,
    description: 'Finished and its node rows dropped.',
  },
}

export const nodeStateStyles: Record<NodeState, StatusStyle> = {
  pending: {
    label: 'Pending',
    color: 'gray',
    icon: IconClock,
    description: 'Waiting for a token and an online session.',
  },
  dispatching: {
    label: 'Dispatching',
    color: 'dusk',
    icon: IconSend,
    description: 'Its pid is recorded as intended; calling dawn.',
  },
  dispatched: {
    label: 'Dispatched',
    color: 'dusk',
    icon: IconSend,
    description: 'Dawn accepted the dispatch.',
  },
  delivered: {
    label: 'Delivered',
    color: 'blue',
    icon: IconCircleDot,
    description: 'The process started on the node.',
  },
  verifying: {
    label: 'Verifying',
    color: 'cyan',
    icon: IconSearch,
    description: 'The process succeeded; waiting for the node to report the desired state.',
  },
  backoff: {
    label: 'Backoff',
    color: 'yellow',
    icon: IconHourglassHigh,
    description: 'Failed; retrying after a backoff.',
  },
  succeeded: { label: 'Succeeded', color: 'green', icon: IconCircleCheck, description: 'Done.' },
  failed: {
    label: 'Failed',
    color: 'red',
    icon: IconCircleX,
    description: 'Failed after every allowed attempt.',
  },
  unknown: {
    label: 'Unknown',
    color: 'orange',
    icon: IconQuestionMark,
    description: 'Delivered but no result arrived; resolve by hand or retry.',
  },
  excluded: {
    label: 'Excluded',
    color: 'gray',
    icon: IconFilterOff,
    description: 'No longer matches the selector; returns if it matches again.',
  },
  conflict: {
    label: 'Conflict',
    color: 'grape',
    icon: IconArrowsSplit2,
    description: 'An earlier campaign owns this key on the node.',
  },
  cancelled: {
    label: 'Cancelled',
    color: 'gray',
    icon: IconBan,
    description: 'Stopped before finishing: the campaign ended or passed its deadline.',
  },
}

export const processStatusStyles: Record<ProcessStatus, StatusStyle> = {
  started: {
    label: 'Started',
    color: 'blue',
    icon: IconCircleDot,
    description: 'The shell server at the pid accepted the script.',
  },
  succeeded: {
    label: 'Succeeded',
    color: 'green',
    icon: IconCircleCheck,
    description: 'The script ended without an error.',
  },
  failed: {
    label: 'Failed',
    color: 'red',
    icon: IconCircleX,
    description: 'The script ended with an error.',
  },
  duplicate: {
    label: 'Duplicate',
    color: 'grape',
    icon: IconCopy,
    description: "The pid was already in the node's process table, so nothing ran again.",
  },
  running: {
    label: 'Running',
    color: 'cyan',
    icon: IconPlayerPlay,
    description: 'Still running on the node, or its end could not be learned.',
  },
  ended: {
    label: 'Ended',
    color: 'orange',
    icon: IconFlag,
    description: 'The script finished, but whether it succeeded could not be learned.',
  },
  already_satisfied: {
    label: 'Already satisfied',
    color: 'green',
    icon: IconCircleCheck,
    description: 'The node already reported the desired version; nothing ran.',
  },
  unreachable: {
    label: 'Unreachable',
    color: 'gray',
    icon: IconPlugConnectedX,
    description: 'dawn could not reach the node; nothing was delivered.',
  },
  timed_out: {
    label: 'Timed out',
    color: 'orange',
    icon: IconHourglassHigh,
    description: 'No end within the timeout.',
  },
  denied: {
    label: 'Denied',
    color: 'red',
    icon: IconShieldX,
    description: 'nightfall refused the call.',
  },
  reaped: {
    label: 'Reaped',
    color: 'gray',
    icon: IconRecycle,
    description: "Killed and taken out of the node's process table; the pid no longer marks it.",
  },
  error: {
    label: 'Error',
    color: 'red',
    icon: IconAlertCircle,
    description: 'dawn hit an error it could not classify.',
  },
}

export const processAwaitingStyle: StatusStyle = {
  label: 'No result yet',
  color: 'gray',
  icon: IconClock,
  description: 'dawn has not reported on this process yet.',
}

export const lifecycleStyles: Record<Lifecycle, StatusStyle> = {
  enrolled: {
    label: 'Enrolled',
    color: 'dusk',
    icon: IconUserPlus,
    description: 'Holds a certificate; never connected.',
  },
  active: {
    label: 'Active',
    color: 'green',
    icon: IconCircleCheck,
    description: 'Connected at least once.',
  },
  quarantined: {
    label: 'Quarantined',
    color: 'yellow',
    icon: IconLock,
    description: 'Connects, but callers may only do what the quarantine policy allows.',
  },
  retired: {
    label: 'Retired',
    color: 'gray',
    icon: IconArchive,
    description: 'Refused at the handshake; kept for history.',
  },
  revoked: {
    label: 'Revoked',
    color: 'red',
    icon: IconShieldX,
    description: 'Refused at the handshake.',
  },
}

export const severityStyles: Record<Severity, StatusStyle> = {
  critical: { label: 'Critical', color: 'red', icon: IconAlertOctagon, description: 'Act now.' },
  high: { label: 'High', color: 'orange', icon: IconAlertTriangle, description: 'Act today.' },
  medium: {
    label: 'Medium',
    color: 'yellow',
    icon: IconAlertCircle,
    description: 'Look when you can.',
  },
  low: { label: 'Low', color: 'gray', icon: IconInfoCircle, description: 'For the record.' },
}

export type DeliveryDisplay = 'delivered' | 'retrying' | 'queued' | 'failed'

export const deliveryStyles: Record<DeliveryDisplay, StatusStyle> = {
  delivered: {
    label: 'Delivered',
    color: 'green',
    icon: IconCircleCheck,
    description: 'The receiver accepted the notification.',
  },
  retrying: {
    label: 'Retrying',
    color: 'orange',
    icon: IconRefresh,
    description: 'The last attempt failed; twilight tries again.',
  },
  queued: { label: 'Queued', color: 'gray', icon: IconClock, description: 'Not sent yet.' },
  failed: {
    label: 'Failed',
    color: 'red',
    icon: IconCircleX,
    description: 'twilight gave up on this notification.',
  },
}

export const gateVerdictStyles: Record<GateDisplay, StatusStyle> = {
  pass: {
    label: 'Passing',
    color: 'green',
    icon: IconShieldCheck,
    description: 'Rates are under their thresholds.',
  },
  hold: {
    label: 'Holding',
    color: 'yellow',
    icon: IconHourglassHigh,
    description: 'Waiting for bake time or sample.',
  },
  fail: {
    label: 'Failing',
    color: 'red',
    icon: IconShieldX,
    description: 'A rate is over its threshold.',
  },
  degraded: {
    label: 'Degraded view',
    color: 'orange',
    icon: IconCloudOff,
    description: 'The online view is incomplete; gates hold.',
  },
  converging: {
    label: 'Enforcing',
    color: 'teal',
    icon: IconRefresh,
    description: 'Every phase passed; the desired state is enforced on matching nodes.',
  },
  inactive: {
    label: 'Not evaluated',
    color: 'gray',
    icon: IconShield,
    description: 'The campaign is not running.',
  },
}
