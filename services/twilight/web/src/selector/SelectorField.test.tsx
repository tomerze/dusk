import { screen, waitFor } from '@testing-library/react'
import { http, HttpResponse } from 'msw'
import { describe, expect, it } from 'vitest'
import { selectableFields } from '../mocks/nodes'
import { evaluate, parseSelector } from '../mocks/selector'
import { renderWithProviders } from '../test/render'
import { serveMockApi } from '../test/server'
import { SelectorField } from './SelectorField'

const api = serveMockApi()

function expectedMatches(selector: string): number {
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

function editor(): HTMLElement {
  return screen.getByRole('textbox', { name: 'Selector' })
}

describe('SelectorField', () => {
  it('shows how many nodes match and a sample of them', async () => {
    const selector = 'os_name == "debian"'
    const count = expectedMatches(selector)
    renderWithProviders(<SelectorField value={selector} onChange={() => undefined} />)
    expect(await screen.findByText(`${count.toLocaleString('en-US')} nodes match`)).toBeVisible()
    const sample = screen.getByRole('region', { name: 'A sample of the matching nodes' })
    expect(sample).toHaveTextContent('debian 12')
    expect(editor()).not.toHaveAttribute('aria-invalid')
  })

  it('says an empty selector matches every node', async () => {
    renderWithProviders(<SelectorField value="" onChange={() => undefined} />)
    expect(await screen.findByText('600 nodes match')).toBeVisible()
    expect(screen.getByText('An empty selector matches every node.')).toBeVisible()
  })

  it('underlines the error where the server says it starts and reads it out', async () => {
    const selector = 'country == "US" and os_nme == "debian"'
    renderWithProviders(<SelectorField value={selector} onChange={() => undefined} />)
    expect(
      await screen.findByText('unknown field "os_nme"; use facts["os_nme"] for a fact'),
    ).toBeVisible()
    expect(screen.getByText('(at character 21)')).toBeVisible()
    await waitFor(() => expect(editor()).toHaveAttribute('aria-invalid', 'true'))
    const underline = document.querySelector('.cm-lintRange-error')
    expect(underline?.textContent).toBe('os_nme')
    expect(screen.queryByRole('region', { name: 'A sample of the matching nodes' })).toBeNull()
  })

  it('points at the last character when the selector ends too early', async () => {
    renderWithProviders(<SelectorField value={'country == "US" and'} onChange={() => undefined} />)
    expect(await screen.findByText(/^expected/)).toBeVisible()
    await waitFor(() => expect(document.querySelector('.cm-lintRange-error')).not.toBeNull())
    expect(document.querySelector('.cm-lintRange-error')?.textContent).toBe('d')
  })

  it('says when the selector could not be checked', async () => {
    api.server.use(
      http.post('/api/v1/selectors/validate', () =>
        HttpResponse.json(
          { error: { code: 'unavailable', message: 'the inventory is unreachable', details: {} } },
          { status: 503 },
        ),
      ),
    )
    renderWithProviders(<SelectorField value='country == "US"' onChange={() => undefined} />)
    expect(
      await screen.findByText('Could not check the selector: the inventory is unreachable'),
    ).toBeVisible()
  })
})
