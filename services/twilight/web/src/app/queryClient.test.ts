import { describe, expect, it, vi } from 'vitest'
import { ApiError, storedToken, storeToken } from '../api/client'
import { createQueryClient } from './queryClient'

const unauthenticated = new ApiError(401, 'unauthenticated', 'the API token is unknown or revoked')

describe('createQueryClient', () => {
  it('forgets a stored token that twilight refuses, so the session can sign in', async () => {
    const client = createQueryClient()
    storeToken('revoked-token')
    await client
      .fetchQuery({
        queryKey: ['me'],
        queryFn: () => Promise.reject(unauthenticated),
        retry: false,
      })
      .catch(() => undefined)
    expect(storedToken()).toBeNull()
  })

  it('asks who is signed in again when another call comes back unauthenticated', async () => {
    const client = createQueryClient()
    client.setQueryData(['me'], { subject: 'dev' })
    await client
      .fetchQuery({
        queryKey: ['overview'],
        queryFn: () => Promise.reject(unauthenticated),
        retry: false,
      })
      .catch(() => undefined)
    expect(client.getQueryState(['me'])?.isInvalidated).toBe(true)
  })

  it('keeps the token on other failures', async () => {
    const client = createQueryClient()
    storeToken('good-token')
    await client
      .fetchQuery({
        queryKey: ['overview'],
        queryFn: () => Promise.reject(new ApiError(503, 'unavailable', 'try again shortly')),
        retry: false,
      })
      .catch(() => undefined)
    expect(storedToken()).toBe('good-token')
  })

  it('backs off with full jitter under a growing ceiling', () => {
    const retryDelay = createQueryClient().getDefaultOptions().queries?.retryDelay
    if (typeof retryDelay !== 'function') {
      throw new Error('the query client has no retry delay function')
    }
    const wait = (attempt: number) => retryDelay(attempt, unauthenticated)
    vi.spyOn(Math, 'random').mockReturnValue(0.999)
    expect(wait(1)).toBeCloseTo(1998)
    expect(wait(2)).toBeCloseTo(3996)
    expect(wait(10)).toBeCloseTo(29970)
    vi.spyOn(Math, 'random').mockReturnValue(0)
    expect(wait(3)).toBe(0)
    vi.restoreAllMocks()
  })

  it('waits at least as long as a busy twilight asks', () => {
    const retryDelay = createQueryClient().getDefaultOptions().queries?.retryDelay
    if (typeof retryDelay !== 'function') {
      throw new Error('the query client has no retry delay function')
    }
    const busy = new ApiError(
      503,
      'busy',
      'this instance is answering as many requests as it may; try again shortly',
      { retry_after_seconds: 5 },
    )
    expect(busy.retryAfterSeconds).toBe(5)
    vi.spyOn(Math, 'random').mockReturnValue(0)
    expect(retryDelay(1, busy)).toBe(5000)
    vi.spyOn(Math, 'random').mockReturnValue(0.999)
    expect(retryDelay(10, busy)).toBeCloseTo(29970)
    vi.restoreAllMocks()
  })
})
