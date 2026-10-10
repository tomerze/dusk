import { Center, Loader, Stack, Text } from '@mantine/core'
import { ApiError } from '../api/client'
import { useMe } from '../api/queries'
import { QueryError } from '../components/QueryError'
import { Layout } from './Layout'
import { Login } from './Login'

export function AuthenticatedApp() {
  const me = useMe()
  if (me.isPending) {
    return (
      <Center h="100dvh">
        <Stack align="center" gap="xs">
          <Loader size="sm" />
          <Text size="sm" c="dimmed">
            Connecting to twilight
          </Text>
        </Stack>
      </Center>
    )
  }
  if (me.isError) {
    if (me.error instanceof ApiError && (me.error.status === 401 || me.error.status === 403)) {
      return <Login error={me.error} />
    }
    return (
      <Center h="100dvh" p="md">
        <Stack maw={560} w="100%">
          <QueryError error={me.error} what="your session" onRetry={() => void me.refetch()} />
        </Stack>
      </Center>
    )
  }
  return <Layout me={me.data} />
}
