import { ActionIcon, Button, Group, NumberInput, Text, TextInput, Tooltip } from '@mantine/core'
import { IconPlus, IconTrash } from '@tabler/icons-react'
import type { Phase } from '../../api/types'
import { formatDuration, plural } from '../../format'
import { issueAt, validatePhases } from '../validation'
import { DurationInput } from './DurationInput'
import classes from './PhaseEditor.module.css'

const phasePresets: { label: string; percents: number[] }[] = [
  { label: '1% → 10% → 50% → 100%', percents: [1, 10, 50, 100] },
  { label: '5% → 25% → 100%', percents: [5, 25, 100] },
  { label: '10% → 100%', percents: [10, 100] },
  { label: 'All at once', percents: [100] },
]

const presetNames: Record<number, string[]> = {
  1: ['all'],
  2: ['canary', 'all'],
  3: ['canary', 'early', 'all'],
  4: ['canary', 'early', 'half', 'all'],
}

function finitePercent(phase: Phase): number {
  return Number.isFinite(phase.percent) ? phase.percent : 0
}

function startOf(phases: Phase[], index: number): number {
  return phases
    .slice(0, index)
    .reduce((highest, phase) => Math.max(highest, finitePercent(phase)), 0)
}

interface PhaseEditorProperties {
  phases: Phase[]
  onChange: (phases: Phase[]) => void
  silentWindowSeconds: number
  nodeTimeoutSeconds: number
  matched: number | null
}

export function PhaseEditor({
  phases,
  onChange,
  silentWindowSeconds,
  nodeTimeoutSeconds,
  matched,
}: PhaseEditorProperties) {
  const issues = validatePhases(phases, silentWindowSeconds, nodeTimeoutSeconds)
  const minimumBake = Math.max(silentWindowSeconds, nodeTimeoutSeconds)
  const defaultBake = Math.max(minimumBake, 3600)
  const update = (index: number, patch: Partial<Phase>) =>
    onChange(phases.map((phase, position) => (position === index ? { ...phase, ...patch } : phase)))
  const add = () => {
    const last = phases[phases.length - 1]
    if (last === undefined) {
      onChange([{ name: 'all', percent: 100, bake_seconds: defaultBake }])
      return
    }
    const previous = startOf(phases, phases.length - 1)
    const middle = Math.min(99, Math.max(previous + 1, Math.round((previous + 100) / 2)))
    onChange([
      ...phases.slice(0, -1),
      { name: `phase ${phases.length}`, percent: middle, bake_seconds: defaultBake },
      last,
    ])
  }
  const applyPreset = (percents: number[]) => {
    const names = presetNames[percents.length] ?? []
    onChange(
      percents.map((percent, index) => ({
        name: names[index] ?? `phase ${index + 1}`,
        percent,
        bake_seconds: phases[index]?.bake_seconds ?? defaultBake,
      })),
    )
  }
  const bands = phases.map((phase, index) => {
    const start = startOf(phases, index)
    return {
      name: phase.name,
      start,
      width: Math.max(0, Math.min(100, finitePercent(phase)) - start),
    }
  })
  return (
    <div className={classes.editor}>
      <Group gap="xs" wrap="wrap" className={classes.presets}>
        <Text size="xs" c="dimmed" component="span">
          Start from
        </Text>
        {phasePresets.map((preset) => (
          <Button
            key={preset.label}
            size="compact-xs"
            variant="default"
            onClick={() => applyPreset(preset.percents)}
          >
            {preset.label}
          </Button>
        ))}
      </Group>
      <div className={classes.coverage} aria-hidden="true">
        {bands.map((band, index) => (
          <span
            key={index}
            className={classes.band}
            data-index={index % 4}
            style={{ left: `${band.start}%`, width: `${band.width}%` }}
          />
        ))}
      </div>
      <ol className={classes.rows} aria-label="Phases">
        {phases.map((phase, index) => {
          const start = bands[index]?.start ?? 0
          const share = Math.max(0, finitePercent(phase) - start)
          const expected =
            matched === null || !Number.isFinite(phase.percent)
              ? null
              : Math.round((matched * share) / 100)
          return (
            <li key={index} className={classes.row}>
              <span className={classes.number} data-index={index % 4} aria-hidden="true">
                {index + 1}
              </span>
              <TextInput
                label="Phase"
                aria-label={`Phase ${index + 1} name`}
                value={phase.name}
                onChange={(event) => update(index, { name: event.currentTarget.value })}
                error={issueAt(issues, `phases.${index}.name`)}
                className={classes.name}
                classNames={{ label: classes.label }}
              />
              <NumberInput
                label="Reaches"
                aria-label={`Phase ${index + 1} reaches, percent of matched nodes`}
                description={
                  share > 0
                    ? `+${share}%${expected === null ? '' : `, about ${plural(expected, 'node')}`}`
                    : undefined
                }
                suffix="%"
                value={Number.isFinite(phase.percent) ? phase.percent : ''}
                min={0}
                max={100}
                decimalScale={2}
                allowNegative={false}
                onChange={(value) =>
                  update(index, {
                    percent: typeof value === 'number' ? value : Number.parseFloat(value),
                  })
                }
                error={issueAt(issues, `phases.${index}.percent`)}
                className={classes.percent}
                classNames={{ label: classes.label }}
                inputWrapperOrder={['label', 'input', 'description', 'error']}
              />
              <div className={classes.bake}>
                <DurationInput
                  label="Bake"
                  value={phase.bake_seconds}
                  onChange={(seconds) => update(index, { bake_seconds: seconds })}
                  error={issueAt(issues, `phases.${index}.bake_seconds`)}
                  labelClassName={classes.label}
                />
              </div>
              <Tooltip
                label={
                  phases.length === 1
                    ? 'A campaign needs at least one phase'
                    : `Remove ${phase.name.trim() || `phase ${index + 1}`}`
                }
              >
                <ActionIcon
                  variant="subtle"
                  color="gray"
                  size="lg"
                  aria-label={`Remove phase ${index + 1}`}
                  disabled={phases.length === 1}
                  onClick={() => onChange(phases.toSpliced(index, 1))}
                  className={classes.remove}
                >
                  <IconTrash size={16} aria-hidden="true" />
                </ActionIcon>
              </Tooltip>
            </li>
          )
        })}
      </ol>
      {issueAt(issues, 'phases') !== undefined && (
        <Text size="sm" c="red" role="alert">
          {issueAt(issues, 'phases')}
        </Text>
      )}
      <Group justify="space-between" gap="sm" wrap="wrap" className={classes.footer}>
        <Button
          variant="subtle"
          size="xs"
          leftSection={<IconPlus size={14} aria-hidden="true" />}
          onClick={add}
        >
          Add a phase
        </Button>
        <Text size="xs" c="dimmed">
          Shares are cumulative. Each bake is at least {formatDuration(minimumBake)}, the longer of
          the silent window and the node timeout.
        </Text>
      </Group>
    </div>
  )
}
