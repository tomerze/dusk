import {
  Code,
  EmptyState,
  Group,
  SegmentedControl,
  Skeleton,
  Stack,
  Text,
  Tooltip,
  UnstyledButton,
} from '@mantine/core'
import { IconChevronRight, IconHistory } from '@tabler/icons-react'
import { useState } from 'react'
import { historyWindow, useCampaignEvents } from '../../api/queries'
import type { CampaignEvent, CampaignStatus } from '../../api/types'
import { QueryError } from '../../components/QueryError'
import { formatAbsolute, formatNumber, formatRelative } from '../../format'
import { useNow } from '../../hooks/useNow'
import type { EventGroup } from './events'
import { eventStyle, summarizeEvent } from './events'
import classes from './EventHistory.module.css'

type Filter = 'all' | EventGroup

const filters: { value: Filter; label: string }[] = [
  { value: 'all', label: 'All' },
  { value: 'gate', label: 'Gates' },
  { value: 'operator', label: 'Operators' },
  { value: 'lifecycle', label: 'Phases and status' },
  { value: 'nodes', label: 'Nodes' },
]

function detailValue(value: unknown): string {
  return typeof value === 'string' ? value : JSON.stringify(value)
}

function EventRow({ event, now }: { event: CampaignEvent; now: number }) {
  const [open, setOpen] = useState(false)
  const style = eventStyle(event)
  const EventIcon = style.icon
  const entries = Object.entries(event.detail)
  const summary = summarizeEvent(event)
  const detailId = `event-${event.id}-detail`
  return (
    <li className={classes.row} data-open={open || undefined}>
      <UnstyledButton
        className={classes.summary}
        onClick={() => setOpen((value) => !value)}
        aria-expanded={open}
        aria-controls={detailId}
      >
        <IconChevronRight size={14} className={classes.chevron} aria-hidden="true" />
        <Text component="span" size="xs" c="dimmed" className={`${classes.id} mono`}>
          {event.id}
        </Text>
        <Tooltip label={formatRelative(event.time, now)}>
          <Text
            component="time"
            dateTime={event.time}
            size="xs"
            className={`${classes.time} tabular`}
          >
            {formatAbsolute(event.time)}
          </Text>
        </Tooltip>
        <span className={classes.kind} data-color={style.color}>
          <EventIcon
            size={15}
            stroke={2}
            aria-hidden="true"
            color={`var(--mantine-color-${style.color}-text)`}
          />
          <Text component="span" size="sm" fw={600}>
            {style.label}
          </Text>
        </span>
        <Text component="span" size="sm" className={classes.text}>
          {summary}
          {event.actor.length > 0 && (
            <Text component="span" size="xs" c="dimmed">
              {' '}
              by {event.actor}
            </Text>
          )}
        </Text>
      </UnstyledButton>
      {open && (
        <div className={classes.detail} id={detailId}>
          {entries.length === 0 ? (
            <Text size="xs" c="dimmed">
              This event carries no further detail.
            </Text>
          ) : (
            <dl className={classes.fields}>
              {entries.map(([key, value]) => (
                <div key={key} className={classes.field}>
                  <dt>
                    <Text component="span" size="xs" c="dimmed" className="mono">
                      {key}
                    </Text>
                  </dt>
                  <dd>
                    <Code className={classes.value}>{detailValue(value)}</Code>
                  </dd>
                </div>
              ))}
            </dl>
          )}
        </div>
      )}
    </li>
  )
}

export function EventHistory({
  campaignId,
  status,
}: {
  campaignId: string
  status: CampaignStatus
}) {
  const events = useCampaignEvents(campaignId)
  const now = useNow()
  const [filter, setFilter] = useState<Filter>('all')
  const [order, setOrder] = useState<'newest' | 'oldest'>('newest')
  if (events.isError) {
    return (
      <QueryError error={events.error} what="the history" onRetry={() => void events.refetch()} />
    )
  }
  const all = status === 'archived' ? [] : (events.data?.events ?? [])
  const shown = all
    .filter((event) => filter === 'all' || eventStyle(event).group === filter)
    .sort((left, right) => (order === 'newest' ? right.id - left.id : left.id - right.id))
  return (
    <Stack gap="sm">
      <Group justify="space-between" gap="sm" wrap="wrap">
        <SegmentedControl
          size="xs"
          aria-label="Show events"
          value={filter}
          onChange={(value) => setFilter(value as Filter)}
          data={filters}
        />
        <SegmentedControl
          size="xs"
          aria-label="Order"
          value={order}
          onChange={(value) => setOrder(value as 'newest' | 'oldest')}
          data={[
            { value: 'newest', label: 'Newest first' },
            { value: 'oldest', label: 'Oldest first' },
          ]}
        />
      </Group>
      {events.data?.dropped === true && status !== 'archived' && (
        <Text size="xs" c="dimmed">
          Showing the newest {formatNumber(historyWindow)} events; older ones are not loaded.
        </Text>
      )}
      {events.isPending ? (
        <Stack gap={6} role="status" aria-busy="true" aria-label="Loading the history">
          {Array.from(Array(6).keys(), (index) => (
            <Skeleton key={index} height={38} />
          ))}
        </Stack>
      ) : shown.length === 0 ? (
        <EmptyState
          size="sm"
          icon={<IconHistory size={22} aria-hidden="true" />}
          title={
            status === 'archived'
              ? 'The history was dropped when the campaign was archived'
              : all.length === 0
                ? 'No events yet'
                : 'No events of this kind'
          }
          description={
            status === 'archived'
              ? 'Archiving keeps the campaign and its counters; the ledger in ClickHouse still holds every command sent to a node.'
              : all.length === 0
                ? 'Events appear as the campaign is started, opens phases, passes or fails gates, and as operators act on it.'
                : 'Pick another filter to see the rest of the history.'
          }
        />
      ) : (
        <ol className={classes.list} aria-label="Campaign history">
          {shown.map((event) => (
            <EventRow key={event.id} event={event} now={now} />
          ))}
        </ol>
      )}
    </Stack>
  )
}
