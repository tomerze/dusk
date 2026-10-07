import {
  Button,
  CloseButton,
  EmptyState,
  Group,
  Select,
  Stack,
  Text,
  TextInput,
} from '@mantine/core'
import { useDebouncedValue } from '@mantine/hooks'
import { IconFilterOff, IconPlus, IconRocket, IconSearch } from '@tabler/icons-react'
import { useState } from 'react'
import { Link, useSearchParams } from 'react-router'
import { usePermissions } from '../api/permissions'
import type { CampaignListParameters } from '../api/queries'
import { searchLimit, useCampaigns, useEveryCampaign } from '../api/queries'
import type { ActionKind, CampaignStatus } from '../api/types'
import { campaignStatuses } from '../api/types'
import { CursorPagination } from '../components/CursorPagination'
import { PageHeader } from '../components/PageHeader'
import { QueryError } from '../components/QueryError'
import { campaignStatusStyles } from '../components/status'
import { formatNumber } from '../format'
import { useCursorPages } from '../hooks/useCursorPages'
import { CampaignTable } from './CampaignTable'
import { actionLabels, matchesSearch } from './display'

type StatusFilter = CampaignListParameters['status']
type KindFilter = ActionKind | 'all'

const statusOptions: { value: StatusFilter; label: string }[] = [
  { value: 'all', label: 'Any status' },
  { value: 'active', label: 'Running or paused' },
  ...campaignStatuses.map((status: CampaignStatus) => ({
    value: status,
    label: campaignStatusStyles[status].label,
  })),
]

const kindOptions: { value: KindFilter; label: string }[] = [
  { value: 'all', label: 'Any action' },
  ...(Object.keys(actionLabels) as ActionKind[]).map((kind) => ({
    value: kind,
    label: actionLabels[kind],
  })),
]

function parseOption<Value extends string>(
  text: string | null,
  options: { value: Value }[],
  fallback: Value,
): Value {
  return options.find((option) => option.value === text)?.value ?? fallback
}

export function CampaignsPage() {
  const { canOperate } = usePermissions()
  const [searchParameters, setSearchParameters] = useSearchParams()
  const status = parseOption(searchParameters.get('status'), statusOptions, 'all')
  const kind = parseOption(searchParameters.get('kind'), kindOptions, 'all')
  const [search, setSearch] = useState(searchParameters.get('search') ?? '')
  const [debouncedSearch] = useDebouncedValue(search.trim(), 250)
  const pages = useCursorPages(`${status}/${kind}/${debouncedSearch}`, 25)
  const searching = kind !== 'all' || debouncedSearch.length > 0
  const server = useCampaigns({ status, limit: pages.pageSize, cursor: pages.cursor }, !searching)
  const every = useEveryCampaign(status, searching)
  const campaigns = searching ? every : server
  const offset = searching ? Number(pages.cursor ?? 0) : 0
  const found = every.data?.items.filter((campaign) =>
    matchesSearch(campaign, kind, debouncedSearch),
  )
  const items = searching
    ? (found ?? []).slice(offset, offset + pages.pageSize)
    : (server.data?.items ?? [])
  const nextCursor = searching
    ? found !== undefined && offset + pages.pageSize < found.length
      ? String(offset + pages.pageSize)
      : null
    : (server.data?.next_cursor ?? null)

  const update = (key: string, value: string, fallback: string) => {
    setSearchParameters(
      (previous) => {
        const next = new URLSearchParams(previous)
        if (value === fallback) {
          next.delete(key)
        } else {
          next.set(key, value)
        }
        return next
      },
      { replace: true },
    )
  }
  const filtered = status !== 'all' || kind !== 'all' || debouncedSearch.length > 0
  const clearFilters = () => {
    setSearch('')
    setSearchParameters(new URLSearchParams(), { replace: true })
  }

  return (
    <>
      <PageHeader
        title="Campaigns"
        documentTitle="Campaigns"
        description="Each campaign sets the desired state of the nodes its selector matches, and rolls it out in phases behind health gates."
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
      <Stack gap="md">
        <Group gap="sm" wrap="wrap" align="flex-end" role="search" aria-label="Filter campaigns">
          <TextInput
            aria-label="Search campaigns"
            placeholder="Search by name, owner or selector"
            leftSection={<IconSearch size={16} aria-hidden="true" />}
            value={search}
            onChange={(event) => {
              setSearch(event.currentTarget.value)
              update('search', event.currentTarget.value.trim(), '')
            }}
            rightSection={
              search.length > 0 ? (
                <CloseButton
                  size="sm"
                  aria-label="Clear the search"
                  onClick={() => {
                    setSearch('')
                    update('search', '', '')
                  }}
                />
              ) : null
            }
            style={{ flex: '1 1 280px', maxWidth: 420 }}
          />
          <Select
            aria-label="Status"
            data={statusOptions}
            value={status}
            onChange={(value) => update('status', value ?? 'all', 'all')}
            w={190}
          />
          <Select
            aria-label="Action"
            data={kindOptions}
            value={kind}
            onChange={(value) => update('kind', value ?? 'all', 'all')}
            w={170}
          />
          {filtered && (
            <Button
              variant="subtle"
              color="gray"
              leftSection={<IconFilterOff size={16} aria-hidden="true" />}
              onClick={clearFilters}
            >
              Clear filters
            </Button>
          )}
        </Group>
        {campaigns.isError ? (
          <QueryError
            error={campaigns.error}
            what="campaigns"
            onRetry={() => void campaigns.refetch()}
          />
        ) : (
          <>
            {searching && every.data?.truncated === true && (
              <Text size="xs" c="dimmed">
                Searched the {formatNumber(searchLimit)} newest campaigns. Pick a status to search
                further back.
              </Text>
            )}
            <CampaignTable
              label="Campaigns"
              campaigns={items}
              loading={campaigns.isPending}
              empty={
                filtered ? (
                  <EmptyState
                    size="sm"
                    icon={<IconFilterOff size={22} aria-hidden="true" />}
                    title="No campaign matches these filters"
                    description="Change the search or the filters, or clear them to see every campaign."
                  >
                    <EmptyState.Actions>
                      <Button size="xs" variant="light" onClick={clearFilters}>
                        Clear filters
                      </Button>
                    </EmptyState.Actions>
                  </EmptyState>
                ) : (
                  <EmptyState
                    size="md"
                    icon={<IconRocket size={24} aria-hidden="true" />}
                    title="No campaigns yet"
                    description="A campaign picks nodes with a selector, says what they should do, and rolls it out in phases that pause when health gates fail."
                  >
                    {canOperate && (
                      <EmptyState.Actions>
                        <Button component={Link} to="/campaigns/new" size="xs">
                          Create the first campaign
                        </Button>
                      </EmptyState.Actions>
                    )}
                  </EmptyState>
                )
              }
            />
            {campaigns.data !== undefined && (items.length > 0 || pages.pageIndex > 0) && (
              <CursorPagination
                pages={pages}
                shown={items.length}
                total={found === undefined || !searching ? null : found.length}
                nextCursor={nextCursor}
                noun="campaigns"
                pageSizes={[25, 50, 100]}
              />
            )}
          </>
        )}
      </Stack>
    </>
  )
}
