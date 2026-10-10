import { Button, Code, EmptyState, Group, Stack } from '@mantine/core'
import { IconBug, IconMapPinOff } from '@tabler/icons-react'
import { useEffect } from 'react'
import { isRouteErrorResponse, Link, useRouteError } from 'react-router'

export function NotFound() {
  useEffect(() => {
    document.title = 'Not found · twilight'
  }, [])
  return (
    <EmptyState
      mt="xl"
      size="lg"
      icon={<IconMapPinOff size={28} aria-hidden="true" />}
      title="Nothing lives at this address"
      description="The page may have moved, or the link was cut short. Campaigns and nodes are reachable from the navigation."
    >
      <EmptyState.Actions>
        <Button component={Link} to="/">
          Go to the overview
        </Button>
        <Button component={Link} to="/campaigns" variant="default">
          Campaigns
        </Button>
      </EmptyState.Actions>
    </EmptyState>
  )
}

export function RouteError() {
  const error = useRouteError()
  if (isRouteErrorResponse(error) && error.status === 404) {
    return <NotFound />
  }
  const message = error instanceof Error ? `${error.name}: ${error.message}` : String(error)
  console.error('twilight: a page failed to render', error)
  return (
    <Stack p="xl" maw={720}>
      <EmptyState
        align="left"
        size="lg"
        color="red"
        icon={<IconBug size={28} aria-hidden="true" />}
        title="This page stopped working"
        description="Something in the interface failed while showing this page. Your fleet is not affected. Reload to try again; if it keeps happening, report the message below."
      >
        <Code block mt="sm">
          {message}
        </Code>
        <Group mt="md">
          <Button onClick={() => window.location.reload()}>Reload</Button>
          <Button component={Link} to="/" variant="default">
            Go to the overview
          </Button>
        </Group>
      </EmptyState>
    </Stack>
  )
}
