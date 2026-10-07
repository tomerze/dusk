import { Center, Group, Loader, Text } from '@mantine/core'
import type { RouteObject } from 'react-router'
import { AuthenticatedApp } from './AuthenticatedApp'
import { NotFound, RouteError } from './RouteError'

export const routes: RouteObject[] = [
  {
    path: '/',
    element: <AuthenticatedApp />,
    errorElement: <RouteError />,
    hydrateFallbackElement: (
      <Center h="100dvh">
        <Group gap="xs">
          <Loader size="sm" aria-hidden="true" />
          <Text size="sm" c="dimmed">
            Loading twilight
          </Text>
        </Group>
      </Center>
    ),
    children: [
      {
        errorElement: <RouteError />,
        children: [
          {
            path: 'campaigns',
            lazy: async () => ({
              Component: (await import('../campaigns/CampaignsPage')).CampaignsPage,
            }),
          },
          { path: '*', element: <NotFound /> },
        ],
      },
    ],
  },
]
