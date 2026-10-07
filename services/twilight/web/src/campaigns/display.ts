import type { ActionKind, Campaign } from '../api/types'
import type { CampaignDisplayStatus } from '../components/status'

export function displayStatus(
  campaign: Pick<Campaign, 'status'>,
  converging: boolean,
): CampaignDisplayStatus {
  return campaign.status === 'running' && converging ? 'converging' : campaign.status
}

export const actionLabels: Record<ActionKind, string> = {
  run_script: 'Run script',
  ensure_version: 'Ensure version',
  ensure_config: 'Ensure config',
  quarantine: 'Quarantine',
}

export function phaseLabel(
  campaign: Pick<Campaign, 'status' | 'current_phase' | 'policy'>,
): string {
  if (campaign.status === 'draft') {
    return `${campaign.policy.phases.length} planned`
  }
  const phase = campaign.policy.phases[campaign.current_phase]
  const position = `${campaign.current_phase + 1} of ${campaign.policy.phases.length}`
  return phase === undefined ? position : `${position}, ${phase.percent}%`
}

export function estimatedTotal(
  campaign: Pick<Campaign, 'status' | 'current_phase' | 'policy'>,
  rows: number,
): number | null {
  if (campaign.status !== 'running' && campaign.status !== 'paused') {
    return null
  }
  const percent = campaign.policy.phases[campaign.current_phase]?.percent ?? 100
  if (rows === 0 || percent <= 0 || percent >= 100) {
    return null
  }
  return Math.round((rows * 100) / percent)
}

export function matchesSearch(
  campaign: Campaign,
  kind: ActionKind | 'all',
  search: string,
): boolean {
  if (kind !== 'all' && campaign.kind !== kind) {
    return false
  }
  const needle = search.trim().toLowerCase()
  return (
    needle.length === 0 ||
    [campaign.name, campaign.created_by, campaign.selector, campaign.description].some((text) =>
      text.toLowerCase().includes(needle),
    )
  )
}

export function isActive(campaign: Pick<Campaign, 'status'>): boolean {
  return campaign.status === 'running' || campaign.status === 'paused'
}
