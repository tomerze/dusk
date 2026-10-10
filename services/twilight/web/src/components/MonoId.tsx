import { ActionIcon, CopyButton, Group, Text, Tooltip, VisuallyHidden } from '@mantine/core'
import { IconCheck, IconCopy } from '@tabler/icons-react'
import { Link } from 'react-router'
import { middleEllipsis } from '../format'

interface MonoIdProperties {
  value: string
  label: string
  maximum?: number
  to?: string
  copy?: boolean
  size?: 'xs' | 'sm' | 'md'
}

export function MonoId({
  value,
  label,
  maximum = 14,
  to,
  copy = true,
  size = 'sm',
}: MonoIdProperties) {
  const shown = middleEllipsis(value, maximum)
  const text = (
    <Text
      component="span"
      ff="monospace"
      size={size}
      className="mono"
      title={shown === value ? undefined : value}
    >
      {shown === value ? (
        shown
      ) : (
        <>
          <span aria-hidden="true">{shown}</span>
          <VisuallyHidden>{`${label} ${value}`}</VisuallyHidden>
        </>
      )}
    </Text>
  )
  return (
    <Group gap={4} wrap="nowrap" className="mono-id">
      {to === undefined ? (
        text
      ) : (
        <Link to={to} className="mono-link" onClick={(event) => event.stopPropagation()}>
          {text}
        </Link>
      )}
      {copy && (
        <CopyButton value={value} timeout={1500}>
          {({ copied, copy: copyValue }) => (
            <Tooltip label={copied ? 'Copied' : `Copy ${label}`} withArrow>
              <ActionIcon
                variant="subtle"
                color={copied ? 'green' : 'gray'}
                size="xs"
                aria-label={`Copy ${label}`}
                onClick={(event) => {
                  event.stopPropagation()
                  copyValue()
                }}
              >
                {copied ? (
                  <IconCheck size={12} aria-hidden="true" />
                ) : (
                  <IconCopy size={12} aria-hidden="true" />
                )}
              </ActionIcon>
            </Tooltip>
          )}
        </CopyButton>
      )}
    </Group>
  )
}
