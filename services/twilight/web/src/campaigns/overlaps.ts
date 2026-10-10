import { useQueries } from '@tanstack/react-query'
import { pageLimit, queryKeys, useCampaigns, validateSelector } from '../api/queries'
import type { Campaign, CampaignDefinition } from '../api/types'

export interface Overlap {
  campaign: Campaign
  nodes: number
}

export function intersection(left: string, right: string): string {
  return [left, right]
    .filter((selector) => selector.trim().length > 0)
    .map((selector) => `(${selector})`)
    .join(' and ')
}

export function overlapCandidates(
  definition: Pick<CampaignDefinition, 'action'>,
  campaigns: Campaign[],
  excluded: string | null,
): Campaign[] {
  const { action } = definition
  if (action.kind !== 'ensure_version' && action.kind !== 'ensure_config') {
    return []
  }
  return campaigns.filter((campaign) => {
    if (campaign.id === excluded) {
      return false
    }
    if (campaign.status !== 'running' && campaign.status !== 'paused') {
      return false
    }
    if (campaign.action.kind !== action.kind) {
      return false
    }
    return (
      action.kind !== 'ensure_version' ||
      (campaign.action.kind === 'ensure_version' &&
        campaign.action.version_key === action.version_key)
    )
  })
}

export function useOverlaps(
  definition: CampaignDefinition,
  selectorValid: boolean,
  excluded: string | null,
): { overlaps: Overlap[]; checking: boolean } {
  const converging =
    definition.action.kind === 'ensure_version' || definition.action.kind === 'ensure_config'
  const active = useCampaigns({ status: 'active', limit: pageLimit, cursor: null }, converging)
  const candidates = overlapCandidates(definition, active.data?.items ?? [], excluded)
  const counts = useQueries({
    queries: candidates.map((campaign) => {
      const selector = intersection(definition.selector, campaign.selector)
      return {
        queryKey: queryKeys.selectorCount(selector),
        queryFn: ({ signal }: { signal: AbortSignal }) => validateSelector(selector, signal),
        enabled: selectorValid,
        staleTime: 60_000,
        retry: false,
      }
    }),
  })
  const overlaps: Overlap[] = []
  candidates.forEach((campaign, index) => {
    const result = counts[index]?.data
    if (result?.ok === true && result.matched > 0) {
      overlaps.push({ campaign, nodes: result.matched })
    }
  })
  return {
    overlaps,
    checking: (converging && active.isPending) || counts.some((count) => count.isFetching),
  }
}
