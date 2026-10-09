import { Button, Group, NativeSelect, Text } from '@mantine/core'
import { IconChevronLeft, IconChevronRight } from '@tabler/icons-react'
import { formatNumber } from '../format'
import type { CursorPages } from '../hooks/useCursorPages'

interface CursorPaginationProperties {
  pages: CursorPages
  shown: number
  total: number | null
  nextCursor: string | null
  noun: string
  pageSizes?: number[]
}

export function CursorPagination({
  pages,
  shown,
  total,
  nextCursor,
  noun,
  pageSizes = [25, 50, 100, 200],
}: CursorPaginationProperties) {
  const first = pages.pageIndex * pages.pageSize + (shown > 0 ? 1 : 0)
  const last = pages.pageIndex * pages.pageSize + shown
  const range =
    shown === 0
      ? `No ${noun}`
      : total === null
        ? `${formatNumber(first)}–${formatNumber(last)}`
        : `${formatNumber(first)}–${formatNumber(last)} of ${formatNumber(total)}`
  return (
    <Group
      justify="space-between"
      gap="sm"
      wrap="wrap"
      component="nav"
      aria-label={`${noun} pages`}
    >
      <Text size="sm" c="dimmed" className="tabular" aria-live="polite">
        {range}
      </Text>
      <Group gap="xs" wrap="nowrap">
        <NativeSelect
          size="xs"
          aria-label="Rows per page"
          value={String(pages.pageSize)}
          data={pageSizes.map((size) => ({ value: String(size), label: `${size} per page` }))}
          onChange={(event) => pages.setPageSize(Number(event.currentTarget.value))}
        />
        <Button
          size="xs"
          variant="default"
          leftSection={<IconChevronLeft size={14} aria-hidden="true" />}
          disabled={pages.pageIndex === 0}
          onClick={pages.previous}
        >
          Previous
        </Button>
        <Button
          size="xs"
          variant="default"
          rightSection={<IconChevronRight size={14} aria-hidden="true" />}
          disabled={nextCursor === null}
          onClick={() => {
            if (nextCursor !== null) {
              pages.next(nextCursor)
            }
          }}
        >
          Next
        </Button>
      </Group>
    </Group>
  )
}
