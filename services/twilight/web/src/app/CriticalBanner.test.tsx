import { screen, waitFor, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { describe, expect, it } from 'vitest'
import type { Alert } from '../api/types'
import { renderApp } from '../test/app'
import { serveMockApi } from '../test/server'

const api = serveMockApi()

function newerOpenAlert(index: number): Alert {
  const time = new Date(Date.now() - index * 1000).toISOString()
  return {
    id: 20_000 + index,
    time,
    last_seen_at: time,
    occurrences: 1,
    severity: 'high',
    kind: 'process_after_deadline',
    fingerprint: `process_after_deadline:probe:${index}`,
    tenant: null,
    detail: { message: 'calls arrived under a pid after its deadline' },
    acknowledged_by: null,
    acknowledged_at: null,
    resolved_by: null,
    resolved_at: null,
  }
}

async function findBanner() {
  const title = await screen.findByText('Critical alert')
  const banner = title.closest('[role="alert"]')
  if (!(banner instanceof HTMLElement)) {
    throw new Error('the critical alert title is not inside the banner')
  }
  return banner
}

describe('the critical alert banner', () => {
  it('finds a critical alert waiting behind a full page of newer open alerts', async () => {
    const alerts = api.current().alerts
    alerts.push(...Array.from(Array(600).keys(), newerOpenAlert))
    renderApp('/')
    const banner = await findBanner()
    expect(
      within(banner).getByText('A process was created at a pid twilight never intended'),
    ).toBeVisible()
  })

  it('goes away once the alert is acknowledged', async () => {
    const user = userEvent.setup()
    renderApp('/')
    const banner = await findBanner()
    await user.click(within(banner).getByRole('button', { name: 'Acknowledge' }))
    await waitFor(() => expect(screen.queryByText('Critical alert')).toBeNull())
    expect(
      api.current().alerts.find((alert) => alert.kind === 'process_without_intent')
        ?.acknowledged_at,
    ).not.toBeNull()
  })
})
