import { screen, waitFor, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { describe, expect, it } from 'vitest'
import type { NodeState } from '../../api/types'
import { plural } from '../../format'
import { renderApp } from '../../test/app'
import { serveMockApi } from '../../test/server'

const gatePaused = '01929b41-02aa-7b6e-8d11-0c7f3e9a2b02'

const api = serveMockApi({ nodeCount: 4000 })

const retryable: NodeState[] = ['failed', 'unknown']

function entry() {
  const found = api.current().campaigns.find(({ campaign }) => campaign.id === gatePaused)
  if (found === undefined) {
    throw new Error('the mock has no gate-paused campaign')
  }
  return found
}

describe("retrying a campaign's nodes", () => {
  it('sends the failed nodes of a paused campaign back to pending', async () => {
    const user = userEvent.setup()
    const failedRows = entry().rows.filter((row) => row.state === 'failed')
    const failed = failedRows.length
    expect(failed).toBeGreaterThan(0)
    expect(failedRows.every((row) => row.pid !== null)).toBe(true)
    renderApp(`/campaigns/${gatePaused}?tab=nodes&state=failed`)
    await user.click(await screen.findByRole('button', { name: 'Retry failed' }))
    const dialog = await screen.findByRole('dialog')
    await waitFor(() =>
      expect(dialog).toHaveTextContent(
        `The ${plural(failed, 'failed node')} get a new attempt at a new pid`,
      ),
    )
    expect(dialog).toHaveTextContent('once the campaign resumes')
    await user.type(within(dialog).getByRole('textbox', { name: 'Reason' }), 'Driver hotfix')
    await user.click(within(dialog).getByRole('button', { name: 'Retry' }))
    await waitFor(() => expect(entry().rows.some((row) => row.state === 'failed')).toBe(false))
    expect(failedRows.every((row) => row.state === 'pending' && row.pid === null)).toBe(true)
    expect(entry().events.at(-1)).toMatchObject({
      kind: 'nodes_retried',
      detail: { count: failed, reason: 'Driver hotfix' },
    })
  })

  it('retries only the nodes of the phase it is filtered to', async () => {
    const user = userEvent.setup()
    const rows = entry().rows
    const inPhase = rows.filter((row) => row.phase === 1 && retryable.includes(row.state))
    const elsewhere = rows.filter((row) => row.phase !== 1 && retryable.includes(row.state))
    expect(inPhase.length).toBeGreaterThan(0)
    expect(elsewhere.length).toBeGreaterThan(0)
    renderApp(`/campaigns/${gatePaused}?tab=nodes&phase=1`)
    await user.click(await screen.findByRole('button', { name: 'Retry failed and unknown' }))
    const dialog = await screen.findByRole('dialog')
    await waitFor(() =>
      expect(dialog).toHaveTextContent(
        `The ${plural(inPhase.length, 'failed and unknown node')} in phase 2`,
      ),
    )
    expect(dialog).toHaveTextContent(`Retry ${plural(inPhase.length, 'node')}`)
    await user.type(within(dialog).getByRole('textbox', { name: 'Reason' }), 'Canary only')
    await user.click(within(dialog).getByRole('button', { name: 'Retry' }))
    await waitFor(() => expect(inPhase.every((row) => row.state === 'pending')).toBe(true))
    expect(elsewhere.every((row) => retryable.includes(row.state))).toBe(true)
    expect(entry().events.at(-1)).toMatchObject({
      kind: 'nodes_retried',
      detail: { count: inPhase.length, reason: 'Canary only' },
    })
  })

  it('offers no retry once the campaign has stopped', async () => {
    entry().campaign.status = 'aborted'
    entry().campaign.abort_reason = 'Vendor recall'
    renderApp(`/campaigns/${gatePaused}?tab=nodes&state=failed`)
    expect(await screen.findByRole('button', { name: 'Retry failed' })).toBeDisabled()
  })
})
