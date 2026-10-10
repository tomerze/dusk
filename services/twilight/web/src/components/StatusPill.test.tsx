import { screen } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { describe, expect, it } from 'vitest'
import { renderWithProviders } from '../test/render'
import {
  campaignStatusStyles,
  gateVerdictStyles,
  lifecycleStyles,
  nodeStateStyles,
  severityStyles,
} from './status'
import { StatusPill } from './StatusPill'

describe('StatusPill', () => {
  it('names the status in words next to an icon, never colour alone', () => {
    renderWithProviders(<StatusPill status={campaignStatusStyles.paused} />)
    const pill = screen.getByText('Paused')
    expect(pill.closest('.mantine-Badge-root')?.querySelector('svg')).not.toBeNull()
  })

  it('explains the status on hover', async () => {
    const user = userEvent.setup()
    renderWithProviders(<StatusPill status={nodeStateStyles.unknown} />)
    await user.hover(screen.getByText('Unknown'))
    expect(
      await screen.findByText('Delivered but no result arrived; resolve by hand or retry.'),
    ).toBeInTheDocument()
  })

  it('prefers the detail it is given over the generic description', async () => {
    const user = userEvent.setup()
    renderWithProviders(
      <StatusPill
        status={campaignStatusStyles.paused}
        detail="failure rate 0.62 in os_build=22631.4317, 41 of 66"
      />,
    )
    await user.hover(screen.getByText('Paused'))
    expect(
      await screen.findByText('failure rate 0.62 in os_build=22631.4317, 41 of 66'),
    ).toBeInTheDocument()
  })

  it('has a distinct label for every status it can show', () => {
    for (const styles of [
      campaignStatusStyles,
      nodeStateStyles,
      lifecycleStyles,
      severityStyles,
      gateVerdictStyles,
    ]) {
      const labels = Object.values(styles).map((style) => style.label)
      expect(new Set(labels).size).toBe(labels.length)
      for (const style of Object.values(styles)) {
        expect(style.label.length).toBeGreaterThan(0)
        expect(style.icon).toBeDefined()
      }
    }
  })
})
