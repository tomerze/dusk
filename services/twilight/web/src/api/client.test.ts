import { delay, http, HttpResponse } from 'msw'
import { setupServer } from 'msw/node'
import { afterAll, afterEach, beforeAll, describe, expect, it, vi } from 'vitest'
import { ApiError, apiRequest, storeToken } from './client'

const server = setupServer()

beforeAll(() => server.listen({ onUnhandledFrame: 'error' }))
afterEach(() => {
  server.resetHandlers()
  vi.restoreAllMocks()
  document.cookie = 'twilight_csrf=; max-age=0'
})
afterAll(() => server.close())

function capture() {
  const seen: Headers[] = []
  server.use(
    http.all('/api/v1/probe', ({ request }) => {
      seen.push(request.headers)
      return HttpResponse.json({ ok: true })
    }),
  )
  return seen
}

describe('apiRequest', () => {
  it('sends the stored token as Bearer and the CSRF cookie only on changes', async () => {
    const seen = capture()
    storeToken('secret-token')
    document.cookie = 'twilight_csrf=abc%2Fdef'
    await apiRequest('GET', '/api/v1/probe')
    await apiRequest('POST', '/api/v1/probe', {})
    expect(seen[0]?.get('Authorization')).toBe('Bearer secret-token')
    expect(seen[0]?.get('X-CSRF-Token')).toBeNull()
    expect(seen[1]?.get('X-CSRF-Token')).toBe('abc/def')
    expect(seen[1]?.get('Content-Type')).toBe('application/json')
  })

  it('sends no Authorization header without a stored token', async () => {
    const seen = capture()
    await apiRequest('GET', '/api/v1/probe')
    expect(seen[0]?.get('Authorization')).toBeNull()
  })

  it("turns twilight's error body into an ApiError with its code and details", async () => {
    server.use(
      http.get('/api/v1/probe', () =>
        HttpResponse.json(
          {
            error: {
              code: 'unauthenticated',
              message: 'log in, or send an API token as Authorization: Bearer',
              details: { login: '/api/v1/auth/login' },
            },
          },
          { status: 401 },
        ),
      ),
    )
    const failure = await apiRequest('GET', '/api/v1/probe').catch((error: unknown) => error)
    expect(failure).toBeInstanceOf(ApiError)
    expect(failure).toMatchObject({
      status: 401,
      code: 'unauthenticated',
      message: 'log in, or send an API token as Authorization: Bearer',
      loginPath: '/api/v1/auth/login',
    })
  })

  it('names the status when the body is not an error body', async () => {
    server.use(
      http.get(
        '/api/v1/probe',
        () => new HttpResponse('<html>bad gateway</html>', { status: 502 }),
      ),
    )
    await expect(apiRequest('GET', '/api/v1/probe')).rejects.toMatchObject({
      status: 502,
      code: 'http',
    })
  })

  it('reports a network failure as status 0', async () => {
    server.use(http.get('/api/v1/probe', () => HttpResponse.error()))
    await expect(apiRequest('GET', '/api/v1/probe')).rejects.toMatchObject({
      status: 0,
      code: 'network',
    })
  })

  it('returns nothing for 204', async () => {
    server.use(http.post('/api/v1/probe', () => new HttpResponse(null, { status: 204 })))
    await expect(apiRequest('POST', '/api/v1/probe', {})).resolves.toBeUndefined()
  })

  it('gives up when twilight does not answer before the deadline', async () => {
    server.use(
      http.get('/api/v1/probe', async () => {
        await delay('infinite')
        return HttpResponse.json({})
      }),
    )
    const deadline = new AbortController()
    vi.spyOn(AbortSignal, 'timeout').mockReturnValue(deadline.signal)
    const pending = apiRequest('GET', '/api/v1/probe')
    deadline.abort(new DOMException('signal timed out', 'TimeoutError'))
    await expect(pending).rejects.toMatchObject({ status: 0, code: 'timeout' })
  })

  it("rethrows the caller's own cancellation unchanged", async () => {
    server.use(
      http.get('/api/v1/probe', async () => {
        await delay('infinite')
        return HttpResponse.json({})
      }),
    )
    const caller = new AbortController()
    const pending = apiRequest('GET', '/api/v1/probe', undefined, caller.signal)
    caller.abort()
    const failure = await pending.catch((error: unknown) => error)
    expect(failure).not.toBeInstanceOf(ApiError)
    expect(failure).toMatchObject({ name: 'AbortError' })
  })
})
