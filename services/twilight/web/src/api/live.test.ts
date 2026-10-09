import { QueryClient } from '@tanstack/react-query'
import { describe, expect, it } from 'vitest'
import { generateCampaigns } from '../mocks/campaigns'
import { generateNodes } from '../mocks/nodes'
import { withCounters } from '../mocks/state'
import { applyFeedEvent, parseEventStream } from './live'
import type { Campaign, Overview, Page } from './types'

describe('parseEventStream', () => {
  it('reads named events, skips comments and retry hints, and keeps a partial event', () => {
    const parsed = parseEventStream(
      'retry: 5000\n\n: keepalive\n\nevent: presence\ndata: {"kind":"presence"}\n\nevent: counters\ndata: {"kind"',
    )
    expect(parsed.events).toEqual(['{"kind":"presence"}'])
    expect(parsed.rest).toBe('event: counters\ndata: {"kind"')
  })

  it('joins multi-line data and accepts CRLF line ends', () => {
    expect(parseEventStream('data: {"a":\r\ndata: 1}\r\n\r\n').events).toEqual(['{"a":\n1}'])
  })
})

describe('applyFeedEvent', () => {
  const now = Date.parse('2026-10-09T12:00:00Z')
  const nodes = generateNodes(7, 400, now)
  const running = withCounters(
    generateCampaigns(8, nodes, now).find((entry) => entry.campaign.status === 'running')!,
  )

  function seeded() {
    const client = new QueryClient()
    const overview: Overview = {
      nodes: { total: 400, online: 300, by_lifecycle: { active: 400 } },
      campaigns: [running],
      alerts: { high: 1 },
      alerts_unacknowledged: { high: 1 },
      degraded: false,
      leader: true,
    }
    client.setQueryData(['overview'], overview)
    client.setQueryData(['campaign', running.id], running)
    client.setQueryData<Page<Campaign>>(['campaigns', { status: 'all' }], {
      items: [running],
      next_cursor: null,
    })
    return client
  }

  it('patches every cached copy of a campaign with its new counters', () => {
    const client = seeded()
    const counters = [{ phase: 0, state: 'succeeded' as const, count: 999 }]
    applyFeedEvent(client, {
      kind: 'counters',
      time: '2026-10-09T12:00:04Z',
      data: [
        {
          campaign_id: running.id,
          status: running.status,
          phase: running.current_phase,
          counters,
        },
      ],
    })
    expect(client.getQueryData<Campaign>(['campaign', running.id])?.counters).toEqual(counters)
    expect(
      client.getQueryData<Page<Campaign>>(['campaigns', { status: 'all' }])?.items[0]?.counters,
    ).toEqual(counters)
    expect(client.getQueryData<Overview>(['overview'])?.campaigns[0]?.counters).toEqual(counters)
  })

  it('refetches a campaign whose status or phase moved instead of patching it', () => {
    const client = seeded()
    applyFeedEvent(client, {
      kind: 'counters',
      time: '2026-10-09T12:00:04Z',
      data: [
        { campaign_id: running.id, status: 'paused', phase: running.current_phase, counters: [] },
      ],
    })
    expect(client.getQueryData<Campaign>(['campaign', running.id])).toEqual(running)
    expect(client.getQueryState(['campaign', running.id])?.isInvalidated).toBe(true)
    expect(client.getQueryState(['campaigns', { status: 'all' }])?.isInvalidated).toBe(true)
    expect(client.getQueryState(['overview'])?.isInvalidated).toBe(true)
  })

  it('ignores counters read before the campaign last changed', () => {
    const client = seeded()
    applyFeedEvent(client, {
      kind: 'counters',
      time: new Date(Date.parse(running.updated_at) - 1000).toISOString(),
      data: [{ campaign_id: running.id, status: 'running', phase: 0, counters: [] }],
    })
    expect(client.getQueryData<Campaign>(['campaign', running.id])).toEqual(running)
    expect(client.getQueryState(['campaign', running.id])?.isInvalidated).toBe(false)
  })

  it('updates the online count, the degraded flag and the open alerts', () => {
    const client = seeded()
    applyFeedEvent(client, {
      kind: 'presence',
      time: '2026-10-09T12:00:04Z',
      data: { online: 287, degraded: true },
    })
    applyFeedEvent(client, {
      kind: 'alerts',
      time: '2026-10-09T12:00:04Z',
      data: { open: { critical: 1, high: 2 }, unacknowledged: { critical: 1 }, latest: null },
    })
    expect(client.getQueryData<Overview>(['overview'])).toMatchObject({
      nodes: { online: 287, total: 400 },
      degraded: true,
      alerts: { critical: 1, high: 2 },
      alerts_unacknowledged: { critical: 1 },
    })
  })
})
