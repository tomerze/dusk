import {
  Checkbox,
  Group,
  NumberInput,
  SegmentedControl,
  SimpleGrid,
  Stack,
  Switch,
  Text,
  TextInput,
} from '@mantine/core'
import { useId } from 'react'
import type { ActionKind, BreakdownField, Policy } from '../../api/types'
import { breakdownFields } from '../../api/types'
import { breakdownLabels } from '../summary'
import type { Issue } from '../validation'
import { issueAt } from '../validation'
import { DurationInput } from './DurationInput'
import classes from './CampaignEditor.module.css'

function numberFrom(value: string | number): number {
  return typeof value === 'number' ? value : Number.parseFloat(value)
}

function shown(value: number): number | string {
  return Number.isFinite(value) ? value : ''
}

function percentOf(rate: number): number | string {
  return Number.isFinite(rate) ? Math.round(rate * 10000) / 100 : ''
}

function localInputValue(iso: string | null): string {
  if (iso === null) {
    return ''
  }
  const date = new Date(iso)
  if (Number.isNaN(date.getTime())) {
    return ''
  }
  const pad = (value: number) => String(value).padStart(2, '0')
  return `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())}T${pad(date.getHours())}:${pad(date.getMinutes())}`
}

interface SectionProperties {
  policy: Policy
  onChange: (policy: Policy) => void
  issues: Issue[]
}

export function RateEditor({ policy, onChange, issues }: SectionProperties) {
  return (
    <SimpleGrid cols={{ base: 1, sm: 2 }} spacing="md">
      <NumberInput
        label="Nodes per second"
        description="The most dispatches a second, shared by every twilight instance."
        value={shown(policy.rate.per_second)}
        min={0}
        decimalScale={2}
        allowNegative={false}
        onChange={(value) =>
          onChange({ ...policy, rate: { ...policy.rate, per_second: numberFrom(value) } })
        }
        error={issueAt(issues, 'policy.rate.per_second')}
      />
      <NumberInput
        label="Burst"
        description="How many may go at once after a quiet spell."
        value={shown(policy.rate.burst)}
        min={1}
        allowDecimal={false}
        allowNegative={false}
        onChange={(value) =>
          onChange({ ...policy, rate: { ...policy.rate, burst: numberFrom(value) } })
        }
        error={issueAt(issues, 'policy.rate.burst')}
      />
    </SimpleGrid>
  )
}

export function GatesEditor({ policy, onChange, issues }: SectionProperties) {
  const gates = policy.gates
  const failureLabelId = useId()
  const setGates = (patch: Partial<Policy['gates']>) =>
    onChange({ ...policy, gates: { ...gates, ...patch } })
  return (
    <Stack gap="md">
      <SimpleGrid cols={{ base: 1, sm: 2 }} spacing="md">
        <NumberInput
          label="Minimum sample"
          description="Results needed before a phase can pass or a group can fail. A phase holds until it has them; a phase with fewer nodes needs a result from every one of them, and a phase with no node passes once it has baked."
          value={shown(gates.min_sample)}
          min={1}
          allowDecimal={false}
          allowNegative={false}
          onChange={(value) => setGates({ min_sample: numberFrom(value) })}
          error={issueAt(issues, 'policy.gates.min_sample')}
        />
        <NumberInput
          label="Failure limit"
          description="Failed share of reported results."
          suffix="%"
          value={percentOf(gates.max_failure_rate)}
          min={0}
          max={100}
          decimalScale={2}
          allowNegative={false}
          onChange={(value) => setGates({ max_failure_rate: numberFrom(value) / 100 })}
          error={issueAt(issues, 'policy.gates.max_failure_rate')}
        />
        <NumberInput
          label="Silent limit"
          description="Share that reported success, then never came back."
          suffix="%"
          value={percentOf(gates.max_silent_rate)}
          min={0}
          max={100}
          decimalScale={2}
          allowNegative={false}
          onChange={(value) => setGates({ max_silent_rate: numberFrom(value) / 100 })}
          error={issueAt(issues, 'policy.gates.max_silent_rate')}
        />
        <DurationInput
          label="Silent window"
          description="How long a node may be gone after success."
          value={gates.silent_window_seconds}
          onChange={(seconds) => setGates({ silent_window_seconds: seconds })}
          error={issueAt(issues, 'policy.gates.silent_window_seconds')}
        />
      </SimpleGrid>
      <Checkbox.Group
        label="Also judge each group of"
        description="A group with enough results that crosses a limit fails the gate on its own, so a problem in one OS build shows up before it moves the overall rate."
        value={gates.breakdown}
        onChange={(values) =>
          setGates({
            breakdown: breakdownFields.filter((field) => values.includes(field)),
          })
        }
      >
        <Group gap="lg" mt={8}>
          {breakdownFields.map((field: BreakdownField) => (
            <Checkbox key={field} value={field} label={breakdownLabels[field]} />
          ))}
        </Group>
      </Checkbox.Group>
      <SimpleGrid cols={{ base: 1, sm: 2 }} spacing="md">
        <div>
          <Text size="sm" fw={500} id={failureLabelId}>
            When a gate fails
          </Text>
          <Text size="xs" c="dimmed" mb={6}>
            Pausing lets you look and resume with an override; aborting cancels every node not yet
            sent.
          </Text>
          <SegmentedControl
            aria-labelledby={failureLabelId}
            value={policy.abort.on_gate_failure}
            onChange={(value) =>
              onChange({
                ...policy,
                abort: { ...policy.abort, on_gate_failure: value as 'pause' | 'abort' },
              })
            }
            data={[
              { value: 'pause', label: 'Pause' },
              { value: 'abort', label: 'Abort' },
            ]}
          />
        </div>
        <NumberInput
          label="Most failures allowed"
          description="The gate fails once more nodes than this have failed. Leave empty for no cap."
          placeholder="No cap"
          value={policy.abort.max_total_failures ?? ''}
          min={1}
          allowDecimal={false}
          allowNegative={false}
          onChange={(value) =>
            onChange({
              ...policy,
              abort: {
                ...policy.abort,
                max_total_failures: value === '' ? null : numberFrom(value),
              },
            })
          }
          error={issueAt(issues, 'policy.abort.max_total_failures')}
        />
      </SimpleGrid>
    </Stack>
  )
}

export function LimitsEditor({
  policy,
  onChange,
  issues,
  kind,
}: SectionProperties & { kind: ActionKind }) {
  const oneShot = kind === 'run_script' || kind === 'quarantine'
  const retry = policy.retry
  const setRetry = (patch: Partial<Policy['retry']>) =>
    onChange({ ...policy, retry: { ...retry, ...patch } })
  return (
    <Stack gap="md">
      <SimpleGrid cols={{ base: 1, sm: 2 }} spacing="md">
        <DurationInput
          label="Node timeout"
          description={
            oneShot
              ? 'No result this long after delivery: the node is marked unknown.'
              : 'No result this long after delivery: the node is asked again under the same pid.'
          }
          value={policy.node_timeout_seconds}
          onChange={(seconds) => onChange({ ...policy, node_timeout_seconds: seconds })}
          error={issueAt(issues, 'policy.node_timeout_seconds')}
        />
        <NumberInput
          label="Attempts"
          description={
            oneShot
              ? 'For nodes that report a failure. 1 means no retry.'
              : 'For nodes that report a failure. Silent nodes are resent with no limit.'
          }
          value={shown(retry.max_attempts)}
          min={1}
          max={100}
          allowDecimal={false}
          allowNegative={false}
          onChange={(value) => setRetry({ max_attempts: numberFrom(value) })}
          error={issueAt(issues, 'policy.retry.max_attempts')}
        />
      </SimpleGrid>
      {retry.max_attempts > 1 && (
        <SimpleGrid cols={{ base: 1, sm: 3 }} spacing="md" className={classes.indented}>
          <DurationInput
            label="First backoff"
            value={retry.initial_backoff_seconds}
            onChange={(seconds) => setRetry({ initial_backoff_seconds: seconds })}
            error={issueAt(issues, 'policy.retry.initial_backoff_seconds')}
          />
          <DurationInput
            label="Longest backoff"
            value={retry.max_backoff_seconds}
            onChange={(seconds) => setRetry({ max_backoff_seconds: seconds })}
            error={issueAt(issues, 'policy.retry.max_backoff_seconds')}
          />
          <NumberInput
            label="Multiplier"
            value={shown(retry.multiplier)}
            min={1}
            max={10}
            decimalScale={2}
            allowNegative={false}
            onChange={(value) => setRetry({ multiplier: numberFrom(value) })}
            error={issueAt(issues, 'policy.retry.multiplier')}
          />
        </SimpleGrid>
      )}
      <SimpleGrid cols={{ base: 1, sm: 2 }} spacing="md">
        <TextInput
          type="datetime-local"
          label="Deadline"
          description="Optional, in your time zone. The campaign completes then, and nodes that are not in flight are cancelled."
          value={localInputValue(policy.deadline)}
          onChange={(event) => {
            const text = event.currentTarget.value
            const time = new Date(text)
            onChange({
              ...policy,
              deadline:
                text.length === 0 || Number.isNaN(time.getTime()) ? null : time.toISOString(),
            })
          }}
          error={issueAt(issues, 'policy.deadline')}
        />
        {!oneShot && (
          <Switch
            mt={{ base: 0, sm: 28 }}
            label="Allow overlap"
            description="Start even if another running campaign sets the same key on some of these nodes. Only the earlier one acts there."
            checked={policy.allow_overlap}
            onChange={(event) =>
              onChange({ ...policy, allow_overlap: event.currentTarget.checked })
            }
          />
        )}
      </SimpleGrid>
    </Stack>
  )
}
