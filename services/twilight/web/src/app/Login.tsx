import { Alert, Button, Divider, Paper, PasswordInput, Stack, Text, Title } from '@mantine/core'
import { IconKey, IconLogin2 } from '@tabler/icons-react'
import { useQueryClient } from '@tanstack/react-query'
import { useEffect, useState } from 'react'
import { ApiError, apiRequest, errorMessage, storeToken } from '../api/client'
import { queryKeys } from '../api/queries'
import type { Me } from '../api/types'
import { Brand } from './Brand'
import classes from './Login.module.css'

export function Login({ error }: { error: unknown }) {
  const client = useQueryClient()
  const [token, setToken] = useState('')
  const [pending, setPending] = useState(false)
  const [failure, setFailure] = useState<string | null>(null)
  const loginPath = error instanceof ApiError ? error.loginPath : null
  const oidc = loginPath !== null
  const forbidden = error instanceof ApiError && error.status === 403

  useEffect(() => {
    document.title = 'Sign in · twilight'
  }, [])

  const signInWithToken = async () => {
    setPending(true)
    setFailure(null)
    storeToken(token.trim())
    try {
      const me = await apiRequest<Me>('GET', '/api/v1/me')
      client.setQueryData(queryKeys.me, me)
    } catch (signInError) {
      storeToken(null)
      setFailure(
        signInError instanceof ApiError && signInError.status === 401
          ? 'twilight did not accept that token. Check that it was copied whole and has not been revoked.'
          : errorMessage(signInError),
      )
    } finally {
      setPending(false)
    }
  }

  const returnTo = encodeURIComponent(window.location.pathname + window.location.search)

  return (
    <div className={classes.page}>
      <Paper className={classes.card} p="xl" withBorder>
        <Stack gap="lg">
          <Brand />
          <div>
            <Title order={1} size="h2">
              Sign in to twilight
            </Title>
            <Text size="sm" c="dimmed" mt={4}>
              Campaigns, inventory and alerts for the nodes in your fleet.
            </Text>
          </div>
          {forbidden && (
            <Alert color="red" variant="light" title="twilight refused your client certificate">
              {errorMessage(error)}
            </Alert>
          )}
          {oidc && (
            <>
              <Button
                component="a"
                href={`${loginPath}?return_to=${returnTo}`}
                onClick={() => storeToken(null)}
                size="md"
                leftSection={<IconLogin2 size={18} aria-hidden="true" />}
              >
                Sign in
              </Button>
              <Divider label="or use an API token" labelPosition="center" />
            </>
          )}
          <form
            onSubmit={(event) => {
              event.preventDefault()
              if (token.trim().length > 0) {
                void signInWithToken()
              }
            }}
          >
            <Stack gap="sm">
              <PasswordInput
                label="API token"
                description="Created with twilight token create. Kept in this browser tab only."
                value={token}
                onChange={(event) => setToken(event.currentTarget.value)}
                autoComplete="off"
                leftSection={<IconKey size={16} aria-hidden="true" />}
              />
              {failure !== null && (
                <Alert color="red" variant="light" role="alert">
                  {failure}
                </Alert>
              )}
              <Button
                type="submit"
                variant={oidc ? 'default' : 'filled'}
                loading={pending}
                disabled={token.trim().length === 0}
              >
                Sign in with token
              </Button>
            </Stack>
          </form>
        </Stack>
      </Paper>
    </div>
  )
}
