import { Anchor, Button, Group, Text } from '@mantine/core'
import { notifications } from '@mantine/notifications'
import { IconAlertOctagon } from '@tabler/icons-react'
import { Link } from 'react-router'
import { errorMessage } from '../api/client'
import { usePermissions } from '../api/permissions'
import { alertMessage } from '../alerts/kinds'
import { useAlertCommand, useCriticalAlerts, waitsForAcknowledgement } from '../api/queries'
import { formatNumber, plural } from '../format'
import classes from './CriticalBanner.module.css'

export function CriticalBanner({ waiting }: { waiting: number }) {
  const command = useAlertCommand()
  const { canOperate } = usePermissions()
  const open = useCriticalAlerts(waiting > 0)
  if (waiting === 0 || open.data === undefined) {
    return null
  }
  const critical = open.data.items.filter(waitsForAcknowledgement)
  const first = critical[0]
  const more = open.data.truncated
  if (first === undefined && !more) {
    return null
  }
  const acknowledge = (alertId: number) =>
    command.mutate(
      { alertId, command: 'acknowledge' },
      {
        onSuccess: () => notifications.show({ color: 'green', message: 'Alert acknowledged' }),
        onError: (error) =>
          notifications.show({
            color: 'red',
            title: 'Could not acknowledge the alert',
            message: errorMessage(error),
          }),
      },
    )
  const title =
    waiting === 1
      ? 'Critical alert'
      : `${plural(waiting, 'critical alert')} are waiting for acknowledgement`
  return (
    <div className={classes.banner} role="alert">
      <Group gap="sm" wrap="nowrap" align="flex-start" className={classes.inner}>
        <IconAlertOctagon size={20} aria-hidden="true" className={classes.icon} />
        <div className={classes.text}>
          <Text size="sm" fw={700}>
            {title}
          </Text>
          <Text size="sm" className={classes.message}>
            {first === undefined
              ? `None of the ${formatNumber(open.data.items.length)} newest open alerts waits for acknowledgement; the rest are on the alerts page.`
              : alertMessage(first)}
          </Text>
        </div>
        <Group gap="xs" wrap="nowrap" className={classes.actions}>
          <Anchor component={Link} to="/alerts" size="sm" className={classes.link}>
            View alerts
          </Anchor>
          {canOperate && first !== undefined && (
            <Button
              size="xs"
              variant="white"
              className={classes.acknowledge}
              loading={command.isPending}
              onClick={() => acknowledge(first.id)}
            >
              Acknowledge
            </Button>
          )}
        </Group>
      </Group>
    </div>
  )
}
