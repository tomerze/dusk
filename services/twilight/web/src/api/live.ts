import type { QueryClient } from '@tanstack/react-query'
import { useQueryClient } from '@tanstack/react-query'
import { useEffect, useState } from 'react'
import { storedToken } from './client'
import type { Campaign, FeedEvent, Overview, Page } from './types'

export type LiveState = 'connecting' | 'live' | 'reconnecting'

export function parseEventStream(buffer: string): { events: string[]; rest: string } {
  const normalized = buffer.replace(/\r\n/g, '\n')
  const blocks = normalized.split('\n\n')
  const rest = blocks.pop() ?? ''
  const events: string[] = []
  for (const block of blocks) {
    const data = block
      .split('\n')
      .filter((line) => line.startsWith('data:'))
      .map((line) => line.slice(5).replace(/^ /, ''))
    if (data.length > 0) {
      events.push(data.join('\n'))
    }
  }
  return { events, rest }
}

export function applyFeedEvent(client: QueryClient, event: FeedEvent) {
  switch (event.kind) {
    case 'counters': {
      const updates = new Map(event.data.map((update) => [update.campaign_id, update]))
      const time = Date.parse(event.time)
      const moved = new Set<string>()
      const patch = (campaign: Campaign): Campaign => {
        const update = updates.get(campaign.id)
        if (update === undefined || time < Date.parse(campaign.updated_at)) {
          return campaign
        }
        if (update.status !== campaign.status || update.phase !== campaign.current_phase) {
          moved.add(campaign.id)
          return campaign
        }
        return { ...campaign, counters: update.counters }
      }
      for (const update of event.data) {
        client.setQueryData<Campaign>(['campaign', update.campaign_id], (old) =>
          old === undefined ? old : patch(old),
        )
      }
      client.setQueriesData<Page<Campaign>>({ queryKey: ['campaigns'] }, (old) =>
        old === undefined ? old : { ...old, items: old.items.map(patch) },
      )
      client.setQueryData<Overview>(['overview'], (old) =>
        old === undefined ? old : { ...old, campaigns: old.campaigns.map(patch) },
      )
      if (moved.size > 0) {
        for (const campaignId of moved) {
          void client.invalidateQueries({ queryKey: ['campaign', campaignId] })
        }
        void client.invalidateQueries({ queryKey: ['campaigns'] })
        void client.invalidateQueries({ queryKey: ['overview'] })
      }
      break
    }
    case 'presence':
      client.setQueryData<Overview>(['overview'], (old) =>
        old === undefined
          ? old
          : {
              ...old,
              degraded: event.data.degraded,
              nodes: { ...old.nodes, online: event.data.online },
            },
      )
      break
    case 'alerts':
      client.setQueryData<Overview>(['overview'], (old) =>
        old === undefined
          ? old
          : { ...old, alerts: event.data.open, alerts_unacknowledged: event.data.unacknowledged },
      )
      void client.invalidateQueries({ queryKey: ['alerts'] })
      break
  }
}

export const idleTimeoutMilliseconds = 45_000

export const steadyMilliseconds = 30_000

export function useLiveStream(enabled: boolean): LiveState {
  const client = useQueryClient()
  const [state, setState] = useState<LiveState>('connecting')

  useEffect(() => {
    if (!enabled) {
      return
    }
    const controller = new AbortController()
    let attempt = 0
    let timer: ReturnType<typeof setTimeout> | null = null

    const connect = async () => {
      const idle = new AbortController()
      let idleTimer: ReturnType<typeof setTimeout> | null = null
      const arm = () => {
        if (idleTimer !== null) {
          clearTimeout(idleTimer)
        }
        idleTimer = setTimeout(
          () =>
            idle.abort(new Error(`nothing arrived for ${idleTimeoutMilliseconds / 1000} seconds`)),
          idleTimeoutMilliseconds,
        )
      }
      let opened: number | null = null
      try {
        const headers: Record<string, string> = { Accept: 'text/event-stream' }
        const token = storedToken()
        if (token !== null) {
          headers.Authorization = `Bearer ${token}`
        }
        arm()
        const response = await fetch('/api/v1/stream', {
          headers,
          credentials: 'same-origin',
          signal: AbortSignal.any([controller.signal, idle.signal]),
        })
        if (!response.ok || response.body === null) {
          throw new Error(`stream answered ${response.status}`)
        }
        setState('live')
        opened = Date.now()
        const reader = response.body.pipeThrough(new TextDecoderStream()).getReader()
        let buffer = ''
        for (;;) {
          const { value, done } = await reader.read()
          if (done) {
            break
          }
          arm()
          buffer += value
          if (buffer.length > 1_048_576) {
            throw new Error('stream event exceeded 1 MiB')
          }
          const parsed = parseEventStream(buffer)
          buffer = parsed.rest
          for (const data of parsed.events) {
            attempt = 0
            try {
              applyFeedEvent(client, JSON.parse(data) as FeedEvent)
            } catch (failure) {
              console.warn('twilight: dropped a malformed stream event', failure)
            }
          }
        }
        throw new Error('stream closed')
      } catch (failure) {
        if (controller.signal.aborted) {
          return
        }
        setState('reconnecting')
        if (opened !== null && Date.now() - opened >= steadyMilliseconds) {
          attempt = 0
        }
        attempt += 1
        const ceiling = Math.min(30_000, 1000 * 2 ** Math.min(attempt, 10))
        const wait = Math.random() * ceiling
        console.warn(
          `twilight: live updates interrupted, retrying in ${Math.round(wait)} ms`,
          idle.signal.aborted ? idle.signal.reason : failure,
        )
        timer = setTimeout(() => void connect(), wait)
      } finally {
        if (idleTimer !== null) {
          clearTimeout(idleTimer)
        }
      }
    }
    void connect()
    return () => {
      controller.abort()
      if (timer !== null) {
        clearTimeout(timer)
      }
    }
  }, [client, enabled])

  return state
}
