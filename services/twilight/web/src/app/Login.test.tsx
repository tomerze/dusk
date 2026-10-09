import { screen } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { afterEach, describe, expect, it } from 'vitest'
import { storedToken, storeToken } from '../api/client'
import { renderApp } from '../test/app'
import { serveMockApi } from '../test/server'

serveMockApi({ scenario: 'signed-out' })

afterEach(() => window.history.replaceState(null, '', '/'))

describe('signing in', () => {
  it('sends single sign-on back to the page that asked for it', async () => {
    window.history.replaceState(null, '', '/campaigns?status=running')
    renderApp('/campaigns?status=running')
    expect(await screen.findByRole('link', { name: 'Sign in' })).toHaveAttribute(
      'href',
      '/api/v1/auth/login?return_to=%2Fcampaigns%3Fstatus%3Drunning',
    )
  })

  it('forgets a stored token when single sign-on is chosen instead', async () => {
    const user = userEvent.setup()
    renderApp('/')
    const link = await screen.findByRole('link', { name: 'Sign in' })
    storeToken('left-over-token')
    link.addEventListener('click', (event) => event.preventDefault())
    await user.click(link)
    expect(storedToken()).toBeNull()
  })

  it('signs in with an API token', async () => {
    const user = userEvent.setup()
    renderApp('/')
    await user.type(await screen.findByLabelText('API token'), 'twilight-token')
    await user.click(screen.getByRole('button', { name: 'Sign in with token' }))
    expect(await screen.findByRole('link', { name: 'Campaigns' })).toBeVisible()
    expect(storedToken()).toBe('twilight-token')
  })
})
