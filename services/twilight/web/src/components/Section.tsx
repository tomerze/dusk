import { Group, Paper, Text, Title } from '@mantine/core'
import type { ReactNode } from 'react'
import classes from './Section.module.css'

interface SectionProperties {
  title: ReactNode
  description?: ReactNode
  actions?: ReactNode
  children: ReactNode
  padded?: boolean
  id?: string
}

export function Section({
  title,
  description,
  actions,
  children,
  padded = true,
  id,
}: SectionProperties) {
  return (
    <Paper component="section" className={classes.section} withBorder aria-labelledby={id}>
      <Group
        justify="space-between"
        align="flex-start"
        className={classes.header}
        wrap="wrap"
        gap="xs"
      >
        <div style={{ minWidth: 0 }}>
          <Title order={2} id={id} className={classes.title}>
            {title}
          </Title>
          {description !== undefined && (
            <Text size="xs" c="dimmed" mt={2}>
              {description}
            </Text>
          )}
        </div>
        {actions}
      </Group>
      <div className={padded ? classes.body : undefined}>{children}</div>
    </Paper>
  )
}
