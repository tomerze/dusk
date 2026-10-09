import { Alert, Button, Group, Text } from '@mantine/core'
import { IconAlertTriangle, IconRefresh } from '@tabler/icons-react'
import { ApiError, errorMessage } from '../api/client'

interface QueryErrorProperties {
  error: unknown
  what: string
  onRetry?: () => void
}

export function QueryError({ error, what, onRetry }: QueryErrorProperties) {
  const status = error instanceof ApiError && error.status > 0 ? ` (HTTP ${error.status})` : ''
  return (
    <Alert
      color="red"
      variant="light"
      title={`Could not load ${what}${status}`}
      icon={<IconAlertTriangle size={18} aria-hidden="true" />}
      role="alert"
    >
      <Group justify="space-between" align="center" gap="sm" wrap="wrap">
        <Text size="sm" style={{ overflowWrap: 'anywhere' }}>
          {errorMessage(error)}
        </Text>
        {onRetry !== undefined && (
          <Button
            size="xs"
            variant="default"
            leftSection={<IconRefresh size={14} aria-hidden="true" />}
            onClick={onRetry}
          >
            Try again
          </Button>
        )}
      </Group>
    </Alert>
  )
}
