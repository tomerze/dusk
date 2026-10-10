import { screen } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { useState } from 'react'
import { describe, expect, it } from 'vitest'
import { useCursorPages } from '../hooks/useCursorPages'
import { renderWithProviders } from '../test/render'
import { CursorPagination } from './CursorPagination'

const items = Array.from(Array(120).keys(), (index) => `node-${index + 1}`)

function Harness({ resetKey, total }: { resetKey: string; total: number | null }) {
  const pages = useCursorPages(resetKey, 50)
  const offset = pages.cursor === null ? 0 : Number(pages.cursor)
  const shown = items.slice(offset, offset + pages.pageSize)
  const next = offset + pages.pageSize < items.length ? String(offset + pages.pageSize) : null
  return (
    <>
      <output data-testid="first">{shown[0]}</output>
      <CursorPagination
        pages={pages}
        shown={shown.length}
        total={total}
        nextCursor={next}
        noun="nodes"
      />
    </>
  )
}

function FilterHarness() {
  const [filter, setFilter] = useState('all')
  return (
    <>
      <button type="button" onClick={() => setFilter('online')}>
        Only online
      </button>
      <Harness resetKey={filter} total={120} />
    </>
  )
}

describe('CursorPagination', () => {
  it('walks forward on the server cursor and back through the cursors it saw', async () => {
    const user = userEvent.setup()
    renderWithProviders(<Harness resetKey="all" total={120} />)
    const navigation = screen.getByRole('navigation', { name: 'nodes pages' })
    expect(navigation).toHaveTextContent('1–50 of 120')
    expect(screen.getByRole('button', { name: 'Previous' })).toBeDisabled()
    await user.click(screen.getByRole('button', { name: 'Next' }))
    expect(screen.getByTestId('first')).toHaveTextContent('node-51')
    expect(navigation).toHaveTextContent('51–100 of 120')
    await user.click(screen.getByRole('button', { name: 'Next' }))
    expect(navigation).toHaveTextContent('101–120 of 120')
    expect(screen.getByRole('button', { name: 'Next' })).toBeDisabled()
    await user.click(screen.getByRole('button', { name: 'Previous' }))
    expect(screen.getByTestId('first')).toHaveTextContent('node-51')
  })

  it('leaves out the total when the server does not count', () => {
    renderWithProviders(<Harness resetKey="all" total={null} />)
    expect(screen.getByRole('navigation', { name: 'nodes pages' })).toHaveTextContent(/^1–50/)
    expect(screen.getByRole('navigation', { name: 'nodes pages' })).not.toHaveTextContent('of')
  })

  it('starts over from the first page when the page size changes', async () => {
    const user = userEvent.setup()
    renderWithProviders(<Harness resetKey="all" total={120} />)
    await user.click(screen.getByRole('button', { name: 'Next' }))
    await user.selectOptions(screen.getByRole('combobox', { name: 'Rows per page' }), '100')
    expect(screen.getByTestId('first')).toHaveTextContent('node-1')
    expect(screen.getByRole('navigation', { name: 'nodes pages' })).toHaveTextContent(
      '1–100 of 120',
    )
  })

  it('starts over from the first page when the filters change', async () => {
    const user = userEvent.setup()
    renderWithProviders(<FilterHarness />)
    await user.click(screen.getByRole('button', { name: 'Next' }))
    expect(screen.getByTestId('first')).toHaveTextContent('node-51')
    await user.click(screen.getByRole('button', { name: 'Only online' }))
    expect(screen.getByTestId('first')).toHaveTextContent('node-1')
  })
})
