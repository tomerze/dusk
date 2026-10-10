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
            index: true,
            lazy: async () => ({
              Component: (await import('../overview/OverviewPage')).OverviewPage,
            }),
          },
          {
            path: 'campaigns',
            lazy: async () => ({
              Component: (await import('../campaigns/CampaignsPage')).CampaignsPage,
            }),
          },
          {
            path: 'campaigns/new',
            lazy: async () => ({
              Component: (await import('../campaigns/editor/CampaignEditorPage')).NewCampaignPage,
            }),
          },
          {
            path: 'campaigns/:campaignId/edit',
            lazy: async () => ({
              Component: (await import('../campaigns/editor/CampaignEditorPage')).EditCampaignPage,
            }),
          },
          {
            path: 'campaigns/:campaignId',
            lazy: async () => ({
              Component: (await import('../campaigns/detail/CampaignPage')).CampaignPage,
            }),
          },
          {
            path: 'nodes',
            lazy: async () => ({
              Component: (await import('../nodes/NodesPage')).NodesPage,
            }),
          },
          {
            path: 'nodes/:deviceId/:installationId',
            lazy: async () => ({
              Component: (await import('../nodes/NodePage')).NodePage,
            }),
          },
          {
            path: 'alerts',
            lazy: async () => ({
              Component: (await import('../alerts/AlertsPage')).AlertsPage,
            }),
          },
          { path: '*', element: <NotFound /> },
        ],
      },
    ],
  },
]
