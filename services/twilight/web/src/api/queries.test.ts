import { QueryClientProvider } from '@tanstack/react-query'
import { renderHook, waitFor } from '@testing-library/react'
import { http, HttpResponse } from 'msw'
import { setupServer } from 'msw/node'
import type { ReactNode } from 'react'
import { createElement } from 'react'
import { afterAll, afterEach, beforeAll, describe, expect, it } from 'vitest'
import { testQueryClient } from '../test/render'
import { historyWindow, useCampaignEvents } from './queries'
import type { CampaignEvent } from './types'

const server = setupServer()

beforeAll(() => server.listen({ onUnhandledFrame: 'error' }))
afterEach(() => server.resetHandlers())
afterAll(() => server.close())

function providers() {
  const client = testQueryClient()
  return ({ children }: { children: ReactNode }) =>
    createElement(QueryClientProvider, { client }, children)
}

describe('useCampaignEvents', () => {
  const campaignId = '01929a77-5d10-7e44-a2c9-1f6b0e8d3c03'
  let events: CampaignEvent[] = []
  let requested: (string | null)[] = []

  function serveEvents(count: number) {
    events = Array.from(Array(count).keys(), (index) => ({
      id: index + 1,
      time: '2026-10-09T12:00:00Z',
      kind: 'gate_holding',
      actor: 'twilight',
      detail: { reason: `waiting for sample ${index} of 10000` },
    }))
    requested = []
    server.use(
      http.get(`/api/v1/campaigns/${campaignId}/events`, ({ request }) => {
        const url = new URL(request.url)
        const after = Number(url.searchParams.get('after') ?? 0)
        const limit = Number(url.searchParams.get('limit') ?? 100)
        requested.push(url.searchParams.get('after'))
        const items = events.filter((event) => event.id > after).slice(0, limit)
        return HttpResponse.json({
          items,
          next_cursor: items.length === limit ? String(items[items.length - 1]?.id) : null,
        })
      }),
    )
  }

  it('keeps the newest events of a long history and says older ones were left out', async () => {
    serveEvents(historyWindow + 600)
    const { result } = renderHook(() => useCampaignEvents(campaignId), { wrapper: providers() })
    await waitFor(() => expect(result.current.isSuccess).toBe(true))
    expect(result.current.data?.events).toHaveLength(historyWindow)
    expect(result.current.data?.events[0]?.id).toBe(601)
    expect(result.current.data?.events.at(-1)?.id).toBe(historyWindow + 600)
    expect(result.current.data?.dropped).toBe(true)
  })

  it('fetches only the events after the newest one it holds', async () => {
    serveEvents(3)
    const { result } = renderHook(() => useCampaignEvents(campaignId), { wrapper: providers() })
    await waitFor(() =>
      expect(result.current.data?.events.map((event) => event.id)).toEqual([1, 2, 3]),
    )
    events.push({ id: 4, time: '2026-10-09T12:00:15Z', kind: 'paused', actor: 'dev', detail: {} })
    await result.current.refetch()
    await waitFor(() =>
      expect(result.current.data?.events.map((event) => event.id)).toEqual([1, 2, 3, 4]),
    )
    expect(requested).toEqual([null, '3'])
    expect(result.current.data?.dropped).toBe(false)
  })
})
