import { Badge, Tooltip, VisuallyHidden } from '@mantine/core'
import type { StatusStyle } from './status'
import classes from './StatusPill.module.css'

interface StatusPillProperties {
  status: StatusStyle
  size?: 'xs' | 'sm' | 'md' | 'lg'
  detail?: string
}

export function StatusPill({ status, size = 'sm', detail }: StatusPillProperties) {
  const StatusIcon = status.icon
  const iconSize = size === 'lg' ? 16 : size === 'md' ? 14 : 12
  const description = detail ?? status.description
  const badge = (
    <Badge
      variant="light"
      color={status.color}
      size={size}
      className={classes.pill}
      leftSection={<StatusIcon size={iconSize} stroke={2.2} aria-hidden="true" />}
    >
      {status.label}
      {description.length > 0 && <VisuallyHidden>: {description}</VisuallyHidden>}
    </Badge>
  )
  if (description.length === 0) {
    return badge
  }
  return <Tooltip label={description}>{badge}</Tooltip>
}
