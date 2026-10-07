import { network } from 'virtual:msw'
import { createHandlers, createLiveStream } from './handlers'
import type { MockScenario } from './state'
import { createMockState, mockScenarios } from './state'

const scenarioStorageKey = 'twilight.mock.scenario'

function resolveScenario(): MockScenario {
  const requested = new URLSearchParams(window.location.search).get('scenario')
  try {
    if (requested !== null && (mockScenarios as readonly string[]).includes(requested)) {
      window.sessionStorage.setItem(scenarioStorageKey, requested)
      return requested as MockScenario
    }
    const stored = window.sessionStorage.getItem(scenarioStorageKey)
    if (stored !== null && (mockScenarios as readonly string[]).includes(stored)) {
      return stored as MockScenario
    }
  } catch {
    return 'default'
  }
  return 'default'
}

export async function startMocks(): Promise<void> {
  const state = createMockState({ scenario: resolveScenario() })
  network.configure({
    handlers: createHandlers(state, { stream: createLiveStream(state) }),
    onUnhandledFrame: 'bypass',
  })
  await network.enable()
}
