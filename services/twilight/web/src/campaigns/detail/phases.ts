import type { Campaign } from '../../api/types'
import type { StateCounts } from '../../components/progress'
import { countOf, stateCounts } from '../../components/progress'

export type PhaseStatus = 'passed' | 'open' | 'upcoming' | 'ended' | 'stopped' | 'skipped'

export interface PhaseView {
  index: number
  name: string
  percent: number
  share: number
  bakeSeconds: number
  status: PhaseStatus
  counts: StateCounts
  nodes: number
  bakeAccruedSeconds: number | null
}

export function bakeAccruedSeconds(
  campaign: Pick<Campaign, 'phase_started_at' | 'phase_paused_seconds' | 'paused_at' | 'status'>,
  now: number,
): number | null {
  if (campaign.phase_started_at === null) {
    return null
  }
  const started = Date.parse(campaign.phase_started_at)
  const pausedAt =
    campaign.status === 'paused' && campaign.paused_at !== null
      ? Date.parse(campaign.paused_at)
      : null
  const until = pausedAt === null ? now : Math.min(now, pausedAt)
  return Math.max(0, Math.floor((until - started) / 1000) - campaign.phase_paused_seconds)
}

function statusOf(campaign: Campaign, index: number): PhaseStatus {
  const current = campaign.current_phase
  switch (campaign.status) {
    case 'draft':
      return 'upcoming'
    case 'running':
    case 'paused':
      return index < current ? 'passed' : index === current ? 'open' : 'upcoming'
    case 'completed':
    case 'archived':
      return index < current ? 'passed' : index === current ? 'ended' : 'skipped'
    case 'aborted':
    case 'failed':
      return index < current ? 'passed' : index === current ? 'stopped' : 'skipped'
  }
}

export function phaseViews(campaign: Campaign, now: number): PhaseView[] {
  let previous = 0
  return campaign.policy.phases.map((phase, index) => {
    const counts = stateCounts(campaign.counters, index)
    const status = statusOf(campaign, index)
    const view: PhaseView = {
      index,
      name: phase.name,
      percent: phase.percent,
      share: Math.max(0, phase.percent - previous),
      bakeSeconds: phase.bake_seconds,
      status,
      counts,
      nodes: countOf(counts),
      bakeAccruedSeconds: status === 'open' ? bakeAccruedSeconds(campaign, now) : null,
    }
    previous = Math.max(previous, phase.percent)
    return view
  })
}

export function expectedNodes(view: PhaseView, matched: number | null): number | null {
  return matched === null ? null : Math.round((matched * view.share) / 100)
}
