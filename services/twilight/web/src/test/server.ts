import { setupServer } from 'msw/node'
import { afterAll, afterEach, beforeAll } from 'vitest'
import { createHandlers } from '../mocks/handlers'
import type { MockScenario, MockState } from '../mocks/state'
import { createMockState } from '../mocks/state'

export function serveMockApi(options: { scenario?: MockScenario; nodeCount?: number } = {}) {
  const create = () =>
    createMockState({ scenario: options.scenario, nodeCount: options.nodeCount ?? 600 })
  const holder: { state: MockState } = { state: create() }
  const server = setupServer(...createHandlers(holder.state))
  beforeAll(() => server.listen({ onUnhandledFrame: 'error' }))
  afterEach(() => {
    holder.state = create()
    server.resetHandlers(...createHandlers(holder.state))
  })
  afterAll(() => server.close())
  return { server, current: () => holder.state }
}
