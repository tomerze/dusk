import { screen, within } from '@testing-library/react'
import { describe, expect, it } from 'vitest'
import { renderApp } from '../test/app'
import { serveMockApi } from '../test/server'

const api = serveMockApi()

function newest(kind: string) {
  const alert = api.current().alerts.find((candidate) => candidate.kind === kind)
  if (alert === undefined) {
    throw new Error(`the mock has no ${kind} alert`)
  }
  return alert
}

describe('alert notifications', () => {
  it("shows each receiver's latest notification on the alert's row", async () => {
    renderApp('/alerts')
    const list = await screen.findByRole('list', { name: 'Alerts' })
    const row = within(list).getByText('Process nobody intended').closest('li')
    if (row === null) {
      throw new Error('the alert has no row')
    }
    expect(within(row).getByText('on-call delivered')).toBeVisible()
    expect(within(row).getByText('siem retrying')).toBeVisible()
  })

  it('opens one alert from the link in its notification, with every notification and why one fails', async () => {
    const alert = newest('process_without_intent')
    renderApp(`/alerts?alert=${alert.id}`)
    const notifications = await screen.findByRole('list', { name: 'Notifications' })
    expect(within(notifications).getAllByRole('listitem')).toHaveLength(2)
    expect(within(notifications).getByText(/HTTP 503 upstream unavailable/)).toBeVisible()
    expect(within(notifications).getByText('Retrying')).toBeVisible()
    expect(screen.getByRole('link', { name: 'All alerts' })).toHaveAttribute('href', '/alerts')
    expect(screen.getByText(alert.tenant ?? 'no tenant')).toBeVisible()
  })

  it('says so when the alert of a link does not exist', async () => {
    renderApp('/alerts?alert=123456789')
    expect(await screen.findByRole('link', { name: 'All alerts' })).toBeVisible()
    expect(await screen.findByText(/alert not found|not found/i)).toBeVisible()
  })
})
