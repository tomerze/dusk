import { screen, within } from '@testing-library/react'
import { describe, expect, it } from 'vitest'
import type { CampaignNode } from '../api/types'
import { renderApp } from '../test/app'
import { serveMockApi } from '../test/server'

const api = serveMockApi()

const ensureVersion = '01929b3e-7c4a-7d1e-9f3a-5b8c2d4e6f01'
const runScript = '01929b41-02aa-7b6e-8d11-0c7f3e9a2b02'
const ensureConfig = '01929a77-5d10-7e44-a2c9-1f6b0e8d3c03'
const sampling = '01929c02-9e3b-7a55-b6d0-4c2e8f1a7d04'

describe("a node's processes", () => {
  it('lists each campaign process by its pid with the status dawn last reported', async () => {
    const node = api.current().nodes[0]
    if (node === undefined) {
      throw new Error('the mock has no nodes')
    }
    const minuteAgo = new Date(Date.now() - 60_000).toISOString()
    const row = (
      campaignId: string,
      pid: string | null,
      overrides: Partial<CampaignNode>,
    ): CampaignNode => ({
      campaign_id: campaignId,
      device_id: node.device_id,
      installation_id: node.installation_id,
      phase: 0,
      state: 'delivered',
      attempt: 1,
      failures: 0,
      unreached: 0,
      pid,
      epoch: 0,
      namespace_id: '',
      dispatched_at: pid === null ? null : minuteAgo,
      delivered_at: null,
      deadline_at: null,
      finished_at: null,
      next_attempt_at: null,
      last_error: '',
      last_status: '',
      event_at: null,
      back_at: null,
      silent: null,
      reaped_at: null,
      breakdown: {},
      ...overrides,
    })
    node.executions = [
      row(ensureVersion, '18446744073709551557', { state: 'succeeded', last_status: 'duplicate' }),
      row(runScript, '9223372036854775783', {
        state: 'unknown',
        attempt: 2,
        last_status: 'ended',
      }),
      row(ensureConfig, '4294967311', {
        state: 'succeeded',
        last_status: 'succeeded',
        reaped_at: minuteAgo,
      }),
      row(sampling, null, { state: 'pending' }),
    ]
    renderApp(`/nodes/${node.device_id}/${node.installation_id}?tab=executions`)
    const table = await screen.findByRole('table', { name: 'Campaign processes on this node' })
    const rowOf = (text: string) => {
      const found = within(table).getByText(text).closest('tr')
      if (found === null) {
        throw new Error(`${text} is not in a row`)
      }
      return within(found)
    }
    expect(
      await rowOf('18446744073709551557').findByText(/^Ensure version, attempt 1$/),
    ).toBeVisible()
    expect(rowOf('18446744073709551557').getByText('Duplicate')).toBeVisible()
    expect(rowOf('9223372036854775783').getByText('Run script, attempt 2')).toBeVisible()
    expect(rowOf('9223372036854775783').getByText('Ended')).toBeVisible()
    expect(rowOf('4294967311').getAllByText('Succeeded')).toHaveLength(2)
    expect(rowOf('4294967311').queryByText('not yet')).toBeNull()
    expect(rowOf('18446744073709551557').getByText('not yet')).toBeVisible()
    expect(rowOf('18446744073709551557').getByRole('button', { name: 'Copy pid' })).toBeVisible()
    expect(within(table).getAllByText('not sent').length).toBeGreaterThan(0)
  })
})
