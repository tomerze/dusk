import { screen, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { useState } from 'react'
import { describe, expect, it } from 'vitest'
import type { Phase } from '../../api/types'
import { renderWithProviders } from '../../test/render'
import { PhaseEditor } from './PhaseEditor'

const fourPhases: Phase[] = [
  { name: 'canary', percent: 1, bake_seconds: 3600 },
  { name: 'early', percent: 10, bake_seconds: 7200 },
  { name: 'half', percent: 50, bake_seconds: 14400 },
  { name: 'all', percent: 100, bake_seconds: 14400 },
]

function Harness({ initial, matched = 1000 }: { initial: Phase[]; matched?: number | null }) {
  const [phases, setPhases] = useState(initial)
  return (
    <>
      <PhaseEditor
        phases={phases}
        onChange={setPhases}
        silentWindowSeconds={1800}
        nodeTimeoutSeconds={900}
        matched={matched}
      />
      <output data-testid="phases">{JSON.stringify(phases)}</output>
    </>
  )
}

function current(): Phase[] {
  return JSON.parse(screen.getByTestId('phases').textContent ?? '[]') as Phase[]
}

describe('PhaseEditor', () => {
  it('shows what each phase adds over the one before it', () => {
    renderWithProviders(<Harness initial={fourPhases} />)
    expect(screen.getByText('+9%, about 90 nodes')).toBeInTheDocument()
    expect(screen.getByText('+50%, about 500 nodes')).toBeInTheDocument()
  })

  it('leaves out the node estimate while the match count is unknown', () => {
    renderWithProviders(<Harness initial={fourPhases} matched={null} />)
    expect(screen.getByText('+40%')).toBeInTheDocument()
  })

  it('flags a phase that does not cover more than the one before it', async () => {
    const user = userEvent.setup()
    renderWithProviders(<Harness initial={fourPhases} />)
    const third = screen.getByRole('textbox', { name: /Phase 3 reaches/ })
    await user.clear(third)
    await user.type(third, '5')
    expect(
      screen.getByText('Phases are cumulative: cover more than the 10% before this one.'),
    ).toBeInTheDocument()
    expect(third).toHaveAttribute('aria-invalid', 'true')
    expect(current()[2]?.percent).toBe(5)
  })

  it('requires the last phase to reach every matched node', async () => {
    const user = userEvent.setup()
    renderWithProviders(<Harness initial={fourPhases} />)
    const last = screen.getByRole('textbox', { name: /Phase 4 reaches/ })
    await user.clear(last)
    await user.type(last, '90')
    expect(
      screen.getByText('The last phase must reach 100% of the matched nodes.'),
    ).toBeInTheDocument()
  })

  it('requires each bake to cover the silent window', async () => {
    const user = userEvent.setup()
    renderWithProviders(<Harness initial={fourPhases} />)
    const bake = screen.getAllByRole('textbox', { name: 'Bake' })[0]
    if (bake === undefined) {
      throw new Error('no bake input')
    }
    await user.clear(bake)
    await user.type(bake, '0.25')
    expect(screen.getByText('Bake at least 30m, the silent window.')).toBeInTheDocument()
    expect(current()[0]?.bake_seconds).toBe(900)
  })

  it('applies a preset and keeps the bakes it already had', async () => {
    const user = userEvent.setup()
    renderWithProviders(<Harness initial={fourPhases} />)
    await user.click(screen.getByRole('button', { name: '10% → 100%' }))
    expect(current()).toEqual([
      { name: 'canary', percent: 10, bake_seconds: 3600 },
      { name: 'all', percent: 100, bake_seconds: 7200 },
    ])
  })

  it('adds a phase before the last one, halfway to 100%', async () => {
    const user = userEvent.setup()
    renderWithProviders(
      <Harness
        initial={[
          { name: 'canary', percent: 10, bake_seconds: 3600 },
          { name: 'all', percent: 100, bake_seconds: 3600 },
        ]}
      />,
    )
    await user.click(screen.getByRole('button', { name: 'Add a phase' }))
    expect(current().map((phase) => phase.percent)).toEqual([10, 55, 100])
  })

  it('removes a phase but never the only one', async () => {
    const user = userEvent.setup()
    renderWithProviders(<Harness initial={fourPhases} />)
    await user.click(screen.getByRole('button', { name: 'Remove phase 2' }))
    expect(current().map((phase) => phase.name)).toEqual(['canary', 'half', 'all'])
    const list = screen.getByRole('list', { name: 'Phases' })
    expect(within(list).getAllByRole('listitem')).toHaveLength(3)
  })

  it('does not offer to remove the last remaining phase', () => {
    renderWithProviders(<Harness initial={[{ name: 'all', percent: 100, bake_seconds: 3600 }]} />)
    expect(screen.getByRole('button', { name: 'Remove phase 1' })).toBeDisabled()
  })
})
