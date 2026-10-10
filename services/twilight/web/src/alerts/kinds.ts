import type { Alert, JsonValue } from '../api/types'

export interface AlertKind {
  label: string
  meaning: string
}

export const alertKinds: Record<string, AlertKind> = {
  process_without_intent: {
    label: 'Process nobody intended',
    meaning:
      'A process was created on a node at a pid twilight never recorded as intended for that node. Treat it as a stolen client certificate, someone using the SDK directly, or a compromised dawn until shown otherwise.',
  },
  default_shell_without_intent: {
    label: 'Default shell used without intent',
    meaning:
      "A command ran in a node's default shell while no process twilight intended for that node was open. dawn only reads, kills and reaps there on behalf of intended work, so treat it as a stolen client certificate or a compromised dawn until shown otherwise.",
  },
  target_mismatch: {
    label: 'Process on the wrong node',
    meaning:
      'A process was created at an intended pid on a node other than the one twilight recorded for it. A dawn is misrouting, or the pid was replayed.',
  },
  process_after_deadline: {
    label: 'Calls after the deadline',
    meaning:
      'Calls kept arriving under a pid after the time twilight recorded for it, other than killing it or waiting for it.',
  },
  pid_reused: {
    label: 'Pid reused',
    meaning: 'A process at one pid was created from more than one client session.',
  },
  process_after_result: {
    label: 'Calls after the result',
    meaning:
      'Calls kept arriving under a pid more than a minute after dawn reported its final result, other than the reap.',
  },
  process_shape: {
    label: 'Unexpected process shape',
    meaning:
      'More scripts ran under a pid than twilight intended for it. dawn runs a fixed number per dispatch.',
  },
  result_without_ledger: {
    label: 'Result without a ledger entry',
    meaning:
      'dawn reported a process it delivered, but nightfall ledgered no process at that pid within 10 minutes. Either the ledger lost entries or the result is forged.',
  },
  ledger_chain_broken: {
    label: 'Ledger chain broken',
    meaning:
      'An entry in the nightfall ledger is missing, changed or out of order, or a checkpoint signature does not verify. Compare the evidence copy in object storage.',
  },
  quarantine_override: {
    label: 'Quarantine override',
    meaning: 'A principal used its override to call a quarantined node.',
  },
  revocation_not_enforced: {
    label: 'Revocation not enforced',
    meaning:
      'A revoked or retired node still appeared in a nightfall census, or never got its revoked disconnect.',
  },
  enrollment_rate: {
    label: 'Enrollment spike',
    meaning:
      'More nodes enrolled in a minute than the threshold. A leaked fleet token looks like this; so does a large rollout.',
  },
  campaign_conflict: {
    label: 'Campaign conflict',
    meaning:
      'Two running campaigns set the same key on the same nodes. Only the earlier one acts there; the other shows those rows as conflicts.',
  },
  campaign_paused_by_gate: {
    label: 'Campaign paused by its gate',
    meaning:
      "A campaign's health gate failed and the campaign paused. Resuming it needs a gate override; the alert resolves when the campaign is resumed, aborted or completed.",
  },
  campaign_failed_by_policy: {
    label: 'Campaign failed by its policy',
    meaning:
      "A campaign's health gate failed and its policy aborts on a gate failure, so the campaign failed. Nothing more is dispatched for it.",
  },
}

export function alertKindLabel(kind: string): string {
  return alertKinds[kind]?.label ?? kind.replaceAll('_', ' ')
}

function detailText(detail: Record<string, JsonValue>, key: string): string | null {
  const value = detail[key]
  return typeof value === 'string' && value.length > 0 ? value : null
}

export function alertMessage(alert: Pick<Alert, 'kind' | 'detail'>): string {
  const message = detailText(alert.detail, 'message')
  if (message !== null) {
    return message.charAt(0).toUpperCase() + message.slice(1)
  }
  if (alert.kind === 'revocation_not_enforced') {
    const lifecycle = detailText(alert.detail, 'lifecycle') ?? 'revoked'
    const instance = detailText(alert.detail, 'instance')
    return `A ${lifecycle} node is still connected${instance === null ? '' : ` to ${instance}`}`
  }
  const meaning = alertKinds[alert.kind]?.meaning
  if (meaning !== undefined) {
    const end = meaning.indexOf('. ')
    return end < 0 ? meaning.replace(/\.$/, '') : meaning.slice(0, end)
  }
  return alertKindLabel(alert.kind)
}

export function alertSubject(alert: Pick<Alert, 'detail'>): string | null {
  const action = detailText(alert.detail, 'action')
  const principal = detailText(alert.detail, 'principal')
  if (action === null && principal === null) {
    return null
  }
  return [action, principal === null ? null : `by ${principal}`]
    .filter((part) => part !== null)
    .join(' ')
}
