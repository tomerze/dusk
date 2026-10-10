import { screen, waitFor, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { describe, expect, it } from 'vitest'
import { selectableFields } from '../../mocks/nodes'
import { evaluate, parseSelector } from '../../mocks/selector'
import { renderApp, setEditorText } from '../../test/app'
import { serveMockApi } from '../../test/server'

const api = serveMockApi()

function matches(selector: string): number {
  const parsed = parseSelector(selector)
  if (!parsed.ok) {
    throw new Error(parsed.message)
  }
  return api
    .current()
    .nodes.filter((node) =>
      evaluate(parsed.expression, { fields: selectableFields(node), facts: node.facts }),
    ).length
}

async function fillEnsureVersion(user: ReturnType<typeof userEvent.setup>, selector: string) {
  await user.type(await screen.findByRole('textbox', { name: 'Name' }), 'Dusk 0.2.2 to Contoso')
  setEditorText(screen.getByRole('textbox', { name: 'Selector' }), selector)
  await screen.findByText(`${matches(selector).toLocaleString('en-US')} nodes match`)
  await user.click(screen.getByRole('radio', { name: 'Ensure version' }))
  await user.type(screen.getByRole('textbox', { name: 'Version' }), '0.2.2')
  setEditorText(
    screen.getByRole('textbox', { name: 'Script' }),
    'cp :/mnt/updates/dusk-node-0.2.2 :/var/lib/dusk/dusk-node.next',
  )
}

describe('new campaign', () => {
  it('reviews the campaign in plain English, creates it and starts it', async () => {
    const user = userEvent.setup()
    const selector = 'tenant == "contoso-health"'
    const count = matches(selector)
    renderApp('/campaigns/new')
    await fillEnsureVersion(user, selector)
    await user.click(screen.getByRole('button', { name: 'Review' }))
    expect(await screen.findByRole('heading', { name: 'Review' })).toBeVisible()
    expect(
      screen.getByText(
        `Bring ${count.toLocaleString('en-US')} nodes to dusk.version 0.2.2 in 4 phases (1% → 10% → 50% → 100%), at most 50 nodes/s, pausing if more than 5% fail in any OS build, hardware class, dusk version or country.`,
      ),
    ).toBeVisible()
    expect(screen.queryByText(/overlaps a running one/)).toBeNull()
    await user.click(screen.getByRole('button', { name: 'Create and start' }))
    expect(
      await screen.findByRole('heading', { level: 1, name: 'Dusk 0.2.2 to Contoso' }),
    ).toBeVisible()
    const created = api.current().campaigns[0]?.campaign
    expect(created?.name).toBe('Dusk 0.2.2 to Contoso')
    expect(created?.status).toBe('running')
    expect(created?.action).toEqual({
      kind: 'ensure_version',
      version: '0.2.2',
      version_key: 'dusk.version',
      script: 'cp :/mnt/updates/dusk-node-0.2.2 :/var/lib/dusk/dusk-node.next',
    })
    expect(created?.policy.retry.max_attempts).toBe(5)
    expect(await screen.findAllByText('Running')).not.toHaveLength(0)
  })

  it('saves a draft that overlaps a running campaign but will not start it', async () => {
    const user = userEvent.setup()
    renderApp('/campaigns/new')
    await fillEnsureVersion(user, 'tenant == "northwind-logistics"')
    const warning = await screen.findByText('Overlaps a running campaign')
    expect(warning.closest('[role="alert"]')).toHaveTextContent('Northwind fleet dusk 0.2.1')
    await user.click(screen.getByRole('button', { name: 'Review' }))
    expect(
      await screen.findByText('This campaign overlaps a running one and cannot start yet'),
    ).toBeVisible()
    expect(screen.getByRole('button', { name: 'Create and start' })).toBeDisabled()
    await user.click(screen.getByRole('button', { name: 'Save as draft' }))
    await screen.findByRole('heading', { level: 1, name: 'Dusk 0.2.2 to Contoso' })
    expect(api.current().campaigns[0]?.campaign.status).toBe('draft')
  })

  it('keeps the operator on the form and points at what is missing', async () => {
    const user = userEvent.setup()
    renderApp('/campaigns/new')
    await user.click(await screen.findByRole('button', { name: 'Review' }))
    expect(screen.queryByRole('heading', { name: 'Review' })).toBeNull()
    const name = screen.getByRole('textbox', { name: 'Name' })
    expect(name).toHaveAttribute('aria-invalid', 'true')
    const summary = screen.getByRole('complementary', { name: 'Summary' })
    expect(within(summary).getByText('3 things left before review')).toBeVisible()
    expect(within(summary).getByText('Name the campaign.')).toBeVisible()
    expect(
      within(summary).getByText('Say which nodes it targets. For every node, use has(device_id).'),
    ).toBeVisible()
    expect(within(summary).getByText('Write the script the nodes run.')).toBeVisible()
  })

  it('shows the server message when it refuses the campaign', async () => {
    const user = userEvent.setup()
    const { http, HttpResponse } = await import('msw')
    api.server.use(
      http.post('/api/v1/campaigns', () =>
        HttpResponse.json(
          {
            error: {
              code: 'invalid_argument',
              message: 'policy.rate.per_second: must be above 0 and at most 100000',
              details: { field: 'policy.rate.per_second' },
            },
          },
          { status: 400 },
        ),
      ),
    )
    renderApp('/campaigns/new')
    await fillEnsureVersion(user, 'tenant == "contoso-health"')
    await user.click(screen.getByRole('button', { name: 'Review' }))
    await user.click(await screen.findByRole('button', { name: 'Save as draft' }))
    const failure = (await screen.findByText('twilight did not save it')).closest('[role="alert"]')
    expect(failure).toHaveTextContent('policy.rate.per_second: must be above 0 and at most 100000')
    expect(screen.getByRole('heading', { name: 'Review' })).toBeVisible()
  })

  it('asks before leaving with unsaved changes', async () => {
    const user = userEvent.setup()
    const { router } = renderApp('/campaigns/new')
    await user.type(await screen.findByRole('textbox', { name: 'Name' }), 'Draft')
    const navigation = screen.getByRole('navigation', { name: 'Main' })
    await user.click(within(navigation).getByRole('link', { name: 'Nodes' }))
    const dialog = await screen.findByRole('dialog', { name: 'Leave without saving?' })
    await user.click(within(dialog).getByRole('button', { name: 'Keep editing' }))
    expect(router.state.location.pathname).toBe('/campaigns/new')
    await user.click(within(navigation).getByRole('link', { name: 'Nodes' }))
    await user.click(
      within(await screen.findByRole('dialog')).getByRole('button', { name: 'Discard and leave' }),
    )
    await waitFor(() => expect(router.state.location.pathname).toBe('/nodes'))
  })

  it('saves changes to a draft against the version it was read at', async () => {
    const user = userEvent.setup()
    const draft = '01929d00-6f7a-7b8c-9d0e-1f2a3b4c5d07'
    const before = api.current().campaigns.find(({ campaign }) => campaign.id === draft)
    const version = before?.campaign.version
    renderApp(`/campaigns/${draft}/edit`)
    const name = await screen.findByRole('textbox', { name: 'Name' })
    await user.clear(name)
    await user.type(name, 'Debug logging for EU stores, round two')
    await screen.findByText(/match now\./)
    await user.click(screen.getByRole('button', { name: 'Review' }))
    await user.click(await screen.findByRole('button', { name: 'Save changes' }))
    expect(
      await screen.findByRole('heading', {
        level: 1,
        name: 'Debug logging for EU stores, round two',
      }),
    ).toBeVisible()
    expect(before?.campaign.version).toBe((version ?? 0) + 1)
  })

  it('starts from a copy of another campaign', async () => {
    renderApp('/campaigns/new?from=01929b3e-7c4a-7d1e-9f3a-5b8c2d4e6f01')
    expect(await screen.findByRole('textbox', { name: 'Name' })).toHaveValue(
      'Dusk 0.2.1 to retail stores (copy)',
    )
    expect(screen.getByRole('radio', { name: 'Ensure version' })).toHaveAttribute(
      'aria-checked',
      'true',
    )
  })
})
