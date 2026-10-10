import { screen, waitFor, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { http, HttpResponse } from 'msw'
import { describe, expect, it } from 'vitest'
import { renderApp } from '../../test/app'
import { serveMockApi } from '../../test/server'

const gatePaused = '01929b41-02aa-7b6e-8d11-0c7f3e9a2b02'
const gatePausedName = 'Printer profile v7 on Windows registers'

const api = serveMockApi({ nodeCount: 4000 })

function entry() {
  const found = api.current().campaigns.find(({ campaign }) => campaign.id === gatePaused)
  if (found === undefined) {
    throw new Error('the mock has no gate-paused campaign')
  }
  return found
}

describe('campaign actions', () => {
  it('resumes a gate-paused campaign only with an override and a reason', async () => {
    const user = userEvent.setup()
    const reason = entry().campaign.pause_reason ?? ''
    expect(reason).toMatch(/^failure rate 0\.\d+ in os_build=22631\.4317, \d+ of \d+$/)
    renderApp(`/campaigns/${gatePaused}`)
    expect(await screen.findByText('Paused; its health gate failed')).toBeVisible()
    await user.click(screen.getByRole('button', { name: 'Override and resume' }))
    const dialog = await screen.findByRole('dialog', {
      name: `Override the gate and resume ${gatePausedName}`,
    })
    expect(within(dialog).getByText(reason)).toBeVisible()
    const confirm = within(dialog).getByRole('button', { name: 'Override and resume' })
    expect(confirm).toBeDisabled()
    await user.type(
      within(dialog).getByRole('textbox', { name: 'Why is it safe to continue?' }),
      'Build 22631.4317 got the printer driver hotfix KB5031455',
    )
    await user.click(confirm)
    await waitFor(() => expect(entry().campaign.status).toBe('running'))
    expect(entry().campaign.gate_override_after).not.toBeNull()
    expect(entry().campaign.pause_kind).toBeNull()
    expect(entry().events.at(-1)).toMatchObject({
      kind: 'resumed',
      detail: {
        override_gate: true,
        reason: 'Build 22631.4317 got the printer driver hotfix KB5031455',
      },
    })
    expect(await screen.findByText('Campaign resumed past the failed gate')).toBeVisible()
    expect(screen.getByRole('button', { name: 'Pause' })).toBeVisible()
  })

  it('pauses a running campaign with an optional reason', async () => {
    const user = userEvent.setup()
    entry().campaign.status = 'running'
    entry().campaign.pause_kind = null
    entry().campaign.pause_reason = null
    entry().campaign.paused_at = null
    renderApp(`/campaigns/${gatePaused}`)
    await user.click(await screen.findByRole('button', { name: 'Pause' }))
    const dialog = await screen.findByRole('dialog', { name: `Pause ${gatePausedName}` })
    await user.type(within(dialog).getByRole('textbox', { name: 'Reason' }), 'Store opening hours')
    await user.click(within(dialog).getByRole('button', { name: 'Pause campaign' }))
    expect(await screen.findByText('Paused by an operator')).toBeVisible()
    expect(screen.getByText('Store opening hours')).toBeVisible()
    expect(entry().campaign).toMatchObject({
      pause_kind: 'operator',
      pause_reason: 'Store opening hours',
    })
    expect(screen.getByRole('button', { name: 'Resume' })).toBeVisible()
  })

  it('keeps the dialog open with the server message when the server refuses', async () => {
    const user = userEvent.setup()
    api.server.use(
      http.post(`/api/v1/campaigns/${gatePaused}/resume`, () =>
        HttpResponse.json(
          {
            error: {
              code: 'gate_override_required',
              message: 'The gate failed again 4 seconds ago; look at os_build=22631.4317 first.',
              details: {},
            },
          },
          { status: 409 },
        ),
      ),
    )
    renderApp(`/campaigns/${gatePaused}`)
    await user.click(await screen.findByRole('button', { name: 'Override and resume' }))
    const dialog = await screen.findByRole('dialog')
    await user.type(within(dialog).getByRole('textbox'), 'Trying anyway')
    await user.click(within(dialog).getByRole('button', { name: 'Override and resume' }))
    expect(await within(dialog).findByText('Override and resume failed')).toBeVisible()
    expect(
      within(dialog).getByText(
        'The gate failed again 4 seconds ago; look at os_build=22631.4317 first.',
      ),
    ).toBeVisible()
    expect(entry().campaign.status).toBe('paused')
  })

  it('aborts only with a reason and says how many nodes are cancelled', async () => {
    const user = userEvent.setup()
    const idle = entry().rows.filter((row) =>
      ['pending', 'backoff', 'excluded', 'conflict', 'verifying'].includes(row.state),
    )
    const unsent = idle.length
    renderApp(`/campaigns/${gatePaused}`)
    await user.click(await screen.findByRole('button', { name: 'Abort' }))
    const dialog = await screen.findByRole('dialog', { name: `Abort ${gatePausedName}` })
    expect(dialog).toHaveTextContent(
      unsent > 0
        ? `${unsent.toLocaleString('en-US')} ${unsent === 1 ? 'node that is' : 'nodes that are'} not in flight`
        : 'Nothing new is dispatched.',
    )
    const confirm = within(dialog).getByRole('button', { name: 'Abort campaign' })
    expect(confirm).toBeDisabled()
    await user.type(within(dialog).getByRole('textbox', { name: 'Reason' }), 'Vendor recall')
    await user.click(confirm)
    await waitFor(() => expect(entry().campaign.status).toBe('aborted'))
    expect(await screen.findByText('Aborted by an operator')).toBeVisible()
    expect(idle.every((row) => row.state === 'cancelled')).toBe(true)
  })

  it('discards a draft by aborting it with a reason', async () => {
    const user = userEvent.setup()
    const draft = '01929d00-6f7a-7b8c-9d0e-1f2a3b4c5d07'
    renderApp(`/campaigns/${draft}`)
    await user.click(await screen.findByRole('button', { name: 'More' }))
    await user.click(await screen.findByRole('menuitem', { name: 'Discard draft' }))
    const dialog = await screen.findByRole('dialog', {
      name: 'Discard Debug logging for EU stores',
    })
    const confirm = within(dialog).getByRole('button', { name: 'Discard draft' })
    expect(confirm).toBeDisabled()
    await user.type(within(dialog).getByRole('textbox', { name: 'Reason' }), 'Superseded')
    await user.click(confirm)
    const discarded = api.current().campaigns.find(({ campaign }) => campaign.id === draft)
    await waitFor(() => expect(discarded?.campaign.status).toBe('aborted'))
    expect(discarded?.campaign.abort_reason).toBe('Superseded')
    expect(await screen.findByText('Draft discarded')).toBeVisible()
  })

  it('will not start a draft whose nodes a running campaign of the same kind owns', async () => {
    const user = userEvent.setup()
    const draft = api
      .current()
      .campaigns.find(({ campaign }) => campaign.id === '01929d00-6f7a-7b8c-9d0e-1f2a3b4c5d07')
    if (draft === undefined) {
      throw new Error('the mock has no draft')
    }
    draft.campaign.kind = 'ensure_version'
    draft.campaign.selector = 'tenant == "northwind-logistics"'
    draft.campaign.action = {
      kind: 'ensure_version',
      version: '0.2.2',
      version_key: 'dusk.version',
      script: 'kvs set dusk.update.pending 0.2.2',
    }
    renderApp(`/campaigns/${draft.campaign.id}`)
    await user.click(await screen.findByRole('button', { name: 'Start' }))
    const dialog = await screen.findByRole('dialog', { name: `Start ${draft.campaign.name}` })
    expect(
      await within(dialog).findByText(
        /also targeted by 1 running or paused campaign of the same kind/,
      ),
    ).toBeVisible()
    expect(within(dialog).getByRole('button', { name: 'Start campaign' })).toBeDisabled()
  })
})

describe('campaign actions for a viewer', () => {
  it('shows the campaign but offers no action', async () => {
    api.current().role = 'viewer'
    renderApp(`/campaigns/${gatePaused}`)
    expect(await screen.findByText('View only')).toBeVisible()
    expect(screen.queryByRole('button', { name: 'Override and resume' })).toBeNull()
    expect(screen.queryByRole('button', { name: 'Abort' })).toBeNull()
  })
})
