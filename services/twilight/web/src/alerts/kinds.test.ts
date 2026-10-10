import { describe, expect, it } from 'vitest'
import { alertKindLabel, alertMessage, alertSubject } from './kinds'

describe('alertMessage', () => {
  it('uses the message twilight wrote, as a sentence', () => {
    expect(
      alertMessage({
        kind: 'process_without_intent',
        detail: { message: 'a process was created at a pid twilight never intended' },
      }),
    ).toBe('A process was created at a pid twilight never intended')
  })

  it('composes one for a revocation that was not enforced', () => {
    expect(
      alertMessage({
        kind: 'revocation_not_enforced',
        detail: { lifecycle: 'retired', instance: 'nightfall-1', device_id: 'a'.repeat(32) },
      }),
    ).toBe('A retired node is still connected to nightfall-1')
  })

  it('falls back to what the kind means, then to its name', () => {
    expect(alertMessage({ kind: 'pid_reused', detail: {} })).toBe(
      'A process at one pid was created from more than one client session',
    )
    expect(alertMessage({ kind: 'disk_full', detail: {} })).toBe('disk full')
  })
})

describe('alertKindLabel', () => {
  it('labels known kinds and spells out unknown ones', () => {
    expect(alertKindLabel('ledger_chain_broken')).toBe('Ledger chain broken')
    expect(alertKindLabel('disk_full')).toBe('disk full')
  })
})

describe('alertSubject', () => {
  it('names the call and who made it when the alert carries them', () => {
    expect(alertSubject({ detail: { action: 'ShPortal.sh', principal: 'dawn-2' } })).toBe(
      'ShPortal.sh by dawn-2',
    )
    expect(alertSubject({ detail: { principal: 'ops-breakglass' } })).toBe('by ops-breakglass')
    expect(alertSubject({ detail: { minute: '2026-10-09T09:34:00Z' } })).toBeNull()
  })
})
