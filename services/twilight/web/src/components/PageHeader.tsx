import { Anchor, Breadcrumbs, Group, Stack, Text, Title } from '@mantine/core'
import type { ReactNode } from 'react'
import { useEffect } from 'react'
import { Link } from 'react-router'

interface PageHeaderProperties {
  title: ReactNode
  documentTitle: string
  description?: ReactNode
  trail?: { label: string; to: string }[]
  actions?: ReactNode
  status?: ReactNode
}

export function PageHeader({
  title,
  documentTitle,
  description,
  trail,
  actions,
  status,
}: PageHeaderProperties) {
  useEffect(() => {
    document.title = `${documentTitle} · twilight`
  }, [documentTitle])
  return (
    <Stack gap={6} mb="lg">
      {trail !== undefined && trail.length > 0 && (
        <Breadcrumbs separator="/" separatorMargin={6}>
          {trail.map((crumb) => (
            <Anchor key={crumb.to} component={Link} to={crumb.to} size="sm" c="dimmed">
              {crumb.label}
            </Anchor>
          ))}
        </Breadcrumbs>
      )}
      <Group justify="space-between" align="flex-start" gap="md" wrap="wrap">
        <Stack gap={4} style={{ minWidth: 0, flex: '1 1 320px' }}>
          <Group gap="sm" wrap="wrap" align="center">
            <Title order={1} style={{ overflowWrap: 'anywhere' }}>
              {title}
            </Title>
            {status}
          </Group>
          {description !== undefined && (
            <Text c="dimmed" size="sm" maw={760}>
              {description}
            </Text>
          )}
        </Stack>
        {actions !== undefined && (
          <Group gap="xs" wrap="wrap">
            {actions}
          </Group>
        )}
      </Group>
    </Stack>
  )
}
