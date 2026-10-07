import { screen, waitFor, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { describe, expect, it } from 'vitest'
import type { MockNode } from '../mocks/nodes'
import { renderApp } from '../test/app'
import { serveMockApi } from '../test/server'

const api = serveMockApi()

function pick(predicate: (node: MockNode) => boolean): MockNode {
  const node = api.current().nodes.find(predicate)
  if (node === undefined) {
    throw new Error('the mock fleet has no such node')
  }
  return node
}

function onlineActive() {
  return pick((node) => node.online && node.lifecycle === 'active' && node.hostname !== null)
}

async function openLifecycleItem(user: ReturnType<typeof userEvent.setup>, item: string) {
  await user.click(await screen.findByRole('button', { name: 'Lifecycle' }))
  await user.click(await screen.findByRole('menuitem', { name: item }))
}

describe('node actions', () => {
  it('revokes only once the reason is given and the node name is typed exactly', async () => {
    const user = userEvent.setup()
    const node = onlineActive()
    const name = node.hostname ?? ''
    renderApp(`/nodes/${node.device_id}/${node.installation_id}`)
    await openLifecycleItem(user, 'Revoke')
    const dialog = await screen.findByRole('dialog', { name: `Revoke ${name}` })
    expect(within(dialog).getByText(/Treat this as final/)).toBeVisible()
    const confirm = within(dialog).getByRole('button', { name: 'Revoke' })
    expect(confirm).toBeDisabled()
    await user.type(within(dialog).getByRole('textbox', { name: 'Reason' }), 'Reported stolen')
    expect(confirm).toBeDisabled()
    const typed = within(dialog).getByRole('textbox', { name: `Type ${name} to confirm` })
    await user.type(typed, name.slice(0, -1))
    expect(confirm).toBeDisabled()
    await user.type(typed, name.slice(-1))
    expect(confirm).toBeEnabled()
    await user.click(confirm)
    await waitFor(() => expect(node.lifecycle).toBe('revoked'))
    expect(node.lifecycle_reason).toBe('Reported stolen')
    expect(await screen.findByText('Revoked: refused at every handshake')).toBeVisible()
    expect(screen.getByText('Reported stolen')).toBeVisible()
  })

  it('quarantines with a reason and offers the release afterwards', async () => {
    const user = userEvent.setup()
    const node = onlineActive()
    renderApp(`/nodes/${node.device_id}/${node.installation_id}`)
    await openLifecycleItem(user, 'Quarantine')
    const dialog = await screen.findByRole('dialog')
    expect(within(dialog).queryByRole('textbox', { name: /to confirm/ })).toBeNull()
    await user.type(within(dialog).getByRole('textbox', { name: 'Reason' }), 'Suspected tampering')
    await user.click(within(dialog).getByRole('button', { name: 'Quarantine' }))
    expect(
      await screen.findByText('Quarantined: callers may only do what the quarantine policy allows'),
    ).toBeVisible()
    expect(node.lifecycle).toBe('quarantined')
    await user.click(screen.getByRole('button', { name: 'Lifecycle' }))
    expect(await screen.findByRole('menuitem', { name: 'Release from quarantine' })).toBeVisible()
  })

  it('lets an operator quarantine but leaves retiring and revoking to admins', async () => {
    const user = userEvent.setup()
    api.current().role = 'operator'
    const node = onlineActive()
    renderApp(`/nodes/${node.device_id}/${node.installation_id}`)
    await user.click(await screen.findByRole('button', { name: 'Lifecycle' }))
    expect(await screen.findByRole('menuitem', { name: 'Quarantine' })).toBeEnabled()
    expect(screen.getByRole('menuitem', { name: 'Retire' })).toBeDisabled()
    expect(screen.getByRole('menuitem', { name: 'Revoke' })).toBeDisabled()
    expect(screen.getByRole('menuitem', { name: 'Retire the device' })).toBeDisabled()
    expect(screen.getByRole('menuitem', { name: 'Revoke the device' })).toBeDisabled()
    expect(screen.getByText('Retiring and revoking need the admin role')).toBeVisible()
  })

  it('revokes the whole device once the reason is given and the node name is typed', async () => {
    const user = userEvent.setup()
    const node = onlineActive()
    const name = node.hostname ?? ''
    renderApp(`/nodes/${node.device_id}/${node.installation_id}`)
    await openLifecycleItem(user, 'Revoke the device')
    const dialog = await screen.findByRole('dialog', { name: `Revoke the device ${name} runs on` })
    expect(within(dialog).getByText(/including installations made later/)).toBeVisible()
    const confirm = within(dialog).getByRole('button', { name: 'Revoke the device' })
    await user.type(within(dialog).getByRole('textbox', { name: 'Reason' }), 'Board compromised')
    expect(confirm).toBeDisabled()
    await user.type(within(dialog).getByRole('textbox', { name: `Type ${name} to confirm` }), name)
    await user.click(confirm)
    expect(await screen.findByText(`The device ${name} runs on is revoked`)).toBeVisible()
    expect(node.device).toMatchObject({ lifecycle: 'revoked', reason: 'Board compromised' })
    expect(
      await screen.findByText(
        'Device revoked: every installation of this machine is refused at every handshake',
      ),
    ).toBeVisible()
    expect(
      api
        .current()
        .nodes.filter((candidate) => candidate.device_id === node.device_id)
        .every((candidate) => !candidate.online),
    ).toBe(true)
  })

  it('lifts a device block with a reason and leaves the installation its own lifecycle', async () => {
    const user = userEvent.setup()
    const node = onlineActive()
    const name = node.hostname ?? ''
    node.device = {
      device_id: node.device_id,
      lifecycle: 'retired',
      reason: 'Sent back to the vendor',
      changed_at: new Date(Date.now() - 3600_000).toISOString(),
      actor: 'fleet-admin',
    }
    renderApp(`/nodes/${node.device_id}/${node.installation_id}`)
    expect(
      await screen.findByText(
        'Device retired: every installation of this machine is refused at every handshake',
      ),
    ).toBeVisible()
    expect(screen.getByText('Sent back to the vendor')).toBeVisible()
    expect(screen.getByText('Changed by fleet-admin')).toBeVisible()
    await user.click(screen.getByRole('button', { name: 'Lifecycle' }))
    expect(screen.queryByRole('menuitem', { name: 'Revoke the device' })).toBeNull()
    await user.click(await screen.findByRole('menuitem', { name: 'Lift the device block' }))
    const dialog = await screen.findByRole('dialog', {
      name: `Lift the block on the device ${name} runs on`,
    })
    expect(within(dialog).getByText(/Each installation keeps its own lifecycle/)).toBeVisible()
    expect(within(dialog).queryByRole('textbox', { name: /to confirm/ })).toBeNull()
    await user.type(within(dialog).getByRole('textbox', { name: 'Reason' }), 'Back from repair')
    await user.click(within(dialog).getByRole('button', { name: 'Lift the block' }))
    expect(await screen.findByText(`The device ${name} runs on is no longer blocked`)).toBeVisible()
    expect(node.device).toMatchObject({ lifecycle: 'active', reason: 'Back from repair' })
    expect(node.lifecycle).toBe('active')
    await waitFor(() => expect(screen.queryByText(/^Device retired/)).toBeNull())
  })

  it('opens an interactive session and hands back the request for dawn', async () => {
    const user = userEvent.setup()
    const node = onlineActive()
    renderApp(`/nodes/${node.device_id}/${node.installation_id}`)
    await user.click(await screen.findByRole('button', { name: 'Open session' }))
    const dialog = await screen.findByRole('dialog')
    await user.type(
      within(dialog).getByRole('textbox', { name: 'Why do you need a shell on this node?' }),
      'Printer queue stuck, ticket 4471',
    )
    await user.click(within(dialog).getByRole('button', { name: 'Open session' }))
    const ready = await screen.findByRole('dialog', { name: 'Session ready' })
    const body = JSON.parse(within(ready).getByText(/"pid"/).textContent ?? '{}') as {
      node: Record<string, string | null>
      pid: string
    }
    expect(Object.keys(body).sort()).toEqual(['node', 'pid'])
    expect(body.node).toEqual({
      device_id: node.device_id,
      installation_id: node.installation_id,
      namespace_id: node.sessions[0]?.namespace_id,
      nightfall: node.sessions[0]?.inner_address,
    })
    expect(body.pid).toMatch(/^[1-9][0-9]{0,19}$/)
    expect(BigInt(body.pid) < 2n ** 64n).toBe(true)
  })

  it('does not offer node actions on an offline node', async () => {
    const node = pick((candidate) => !candidate.online && candidate.lifecycle === 'active')
    renderApp(`/nodes/${node.device_id}/${node.installation_id}`)
    expect(await screen.findByRole('button', { name: 'Open session' })).toBeDisabled()
    expect(screen.getByRole('button', { name: 'Stream logs' })).toBeDisabled()
    expect(screen.getByRole('button', { name: 'Collect file' })).toBeDisabled()
  })

  it('collects a file only from an absolute path', async () => {
    const user = userEvent.setup()
    const node = onlineActive()
    renderApp(`/nodes/${node.device_id}/${node.installation_id}`)
    await user.click(await screen.findByRole('button', { name: 'Collect file' }))
    const dialog = await screen.findByRole('dialog')
    const confirm = within(dialog).getByRole('button', { name: 'Collect file' })
    const path = within(dialog).getByRole('textbox', { name: 'Path on the node' })
    await user.type(path, 'var/log/syslog')
    expect(confirm).toBeDisabled()
    expect(within(dialog).getByText('Use an absolute path.')).toBeVisible()
    await user.clear(path)
    await user.type(path, '/var/log/syslog')
    await user.click(confirm)
    expect(await screen.findByText('Collecting the file')).toBeVisible()
  })
})
