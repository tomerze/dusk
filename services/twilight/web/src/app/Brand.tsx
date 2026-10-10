import { Group, Text } from '@mantine/core'

export function Brand() {
  return (
    <Group gap={10} wrap="nowrap">
      <svg width="26" height="26" viewBox="0 0 26 26" aria-hidden="true">
        <defs>
          <clipPath id="twilight-horizon">
            <rect x="0" y="0" width="26" height="15" />
          </clipPath>
        </defs>
        <circle
          cx="13"
          cy="15"
          r="8"
          fill="var(--mantine-primary-color-filled)"
          clipPath="url(#twilight-horizon)"
        />
        <rect x="2" y="16.5" width="22" height="2" rx="1" fill="currentColor" opacity="0.85" />
        <rect x="6" y="20.5" width="14" height="2" rx="1" fill="currentColor" opacity="0.45" />
      </svg>
      <Text fw={700} size="lg" style={{ letterSpacing: '0.01em' }}>
        twilight
      </Text>
    </Group>
  )
}
