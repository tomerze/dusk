import { Text, Tooltip } from '@mantine/core'
import type { TextProps } from '@mantine/core'
import { formatAbsolute, formatRelative } from '../format'
import { useNow } from '../hooks/useNow'

interface RelativeTimeProperties extends TextProps {
  value: string | null
  fallback?: string
}

export function RelativeTime({
  value,
  fallback = 'never',
  ...textProperties
}: RelativeTimeProperties) {
  const now = useNow()
  if (value === null) {
    return (
      <Text
        component="span"
        c="dimmed"
        size="sm"
        style={{ whiteSpace: 'nowrap' }}
        {...textProperties}
      >
        {fallback}
      </Text>
    )
  }
  return (
    <Tooltip label={formatAbsolute(value)}>
      <Text
        component="time"
        dateTime={value}
        size="sm"
        className="tabular"
        style={{ whiteSpace: 'nowrap' }}
        {...textProperties}
      >
        {formatRelative(value, now)}
      </Text>
    </Tooltip>
  )
}
