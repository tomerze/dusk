import { Group, Input, NativeSelect, NumberInput } from '@mantine/core'
import { useId, useState } from 'react'

const units = [
  { value: '1', label: 'seconds' },
  { value: '60', label: 'minutes' },
  { value: '3600', label: 'hours' },
  { value: '86400', label: 'days' },
]

function bestUnit(seconds: number): string {
  for (const unit of [86400, 3600, 60]) {
    if (seconds >= unit && seconds % unit === 0) {
      return String(unit)
    }
  }
  return '1'
}

interface DurationInputProperties {
  label: string
  value: number
  onChange: (seconds: number) => void
  description?: string
  error?: string
  labelClassName?: string
}

export function DurationInput({
  label,
  value,
  onChange,
  description,
  error,
  labelClassName,
}: DurationInputProperties) {
  const [unit, setUnit] = useState(() => bestUnit(value))
  const inputId = useId()
  const factor = Number(unit)
  return (
    <Input.Wrapper
      label={label}
      description={description}
      error={error}
      id={inputId}
      classNames={labelClassName === undefined ? undefined : { label: labelClassName }}
    >
      <Group gap={6} wrap="nowrap">
        <NumberInput
          id={inputId}
          value={Number.isFinite(value) ? value / factor : ''}
          min={0}
          decimalScale={factor === 1 ? 0 : 2}
          allowNegative={false}
          onChange={(next) => {
            const amount = typeof next === 'number' ? next : Number.parseFloat(next)
            onChange(Number.isFinite(amount) ? Math.round(amount * factor) : Number.NaN)
          }}
          error={error !== undefined}
          aria-describedby={
            [
              description === undefined ? null : `${inputId}-description`,
              error === undefined ? null : `${inputId}-error`,
            ]
              .filter((part) => part !== null)
              .join(' ') || undefined
          }
          style={{ flex: 1, minWidth: 64 }}
        />
        <NativeSelect
          aria-label={`${label}, unit`}
          value={unit}
          data={units}
          onChange={(event) => setUnit(event.currentTarget.value)}
          w={104}
        />
      </Group>
    </Input.Wrapper>
  )
}
