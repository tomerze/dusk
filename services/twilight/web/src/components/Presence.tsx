import { Group, Text } from '@mantine/core'
import { RelativeTime } from './RelativeTime'
import classes from './Presence.module.css'

interface PresenceProperties {
  online: boolean
  lastSeen: string | null
  size?: 'xs' | 'sm'
}

export function Presence({ online, lastSeen, size = 'sm' }: PresenceProperties) {
  return (
    <Group gap={6} wrap="nowrap" className={classes.presence}>
      <span className={classes.dot} data-online={online} aria-hidden="true" />
      {online ? (
        <Text component="span" size={size} c="green">
          Online
        </Text>
      ) : (
        <Text component="span" size={size} c="dimmed">
          Offline, <RelativeTime value={lastSeen} size={size} c="dimmed" fallback="never seen" />
        </Text>
      )}
    </Group>
  )
}
