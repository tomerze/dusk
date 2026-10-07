import {
  Anchor,
  Button,
  Checkbox,
  EmptyState,
  Group,
  Menu,
  SegmentedControl,
  Stack,
  Text,
} from '@mantine/core'
import { IconColumns3, IconFilterOff, IconServer2 } from '@tabler/icons-react'
import type { ColumnVisibilityState } from '@tanstack/react-table'
import { useState } from 'react'
import { Link, useNavigate, useSearchParams } from 'react-router'
import type { NodeListParameters } from '../api/queries'
import { useNodes } from '../api/queries'
import type { NodeSummary, SelectorValidation } from '../api/types'
import { CursorPagination } from '../components/CursorPagination'
import { DataTable, SortHeader } from '../components/DataTable'
import { MonoId } from '../components/MonoId'
import { PageHeader } from '../components/PageHeader'
import { Presence } from '../components/Presence'
import { QueryError } from '../components/QueryError'
import { RelativeTime } from '../components/RelativeTime'
import { StatusPill } from '../components/StatusPill'
import { lifecycleStyles } from '../components/status'
import type { TableColumn } from '../components/table'
import { columnHelper } from '../components/table'
import { middleEllipsis } from '../format'
import { useCursorPages } from '../hooks/useCursorPages'
import { SelectorField } from '../selector/SelectorField'

const columnStorageKey = 'twilight.nodes.columns'

const sortableFields = [
  'hostname',
  'lifecycle',
  'os_name',
  'os_build',
  'dusk_version',
  'hardware_class',
  'country',
  'tenant',
  'locale',
  'impl',
  'target_arch',
  'reported_version',
] as const
type SortField = (typeof sortableFields)[number]

interface Sort {
  field: SortField
  descending: boolean
}

function parseSort(text: string | null): Sort {
  const descending = text?.startsWith('-') ?? false
  const name = descending ? text?.slice(1) : text
  const field = sortableFields.find((candidate) => candidate === name) ?? 'hostname'
  return { field, descending: name === field && descending }
}

function sortText(sort: Sort): string {
  return `${sort.descending ? '-' : ''}${sort.field}`
}

const defaultHidden: ColumnVisibilityState = {
  os_build: false,
  locale: false,
  impl: false,
  target_arch: false,
  reported_version: false,
  installation_id: false,
  first_seen_at: false,
  enrolled_at: false,
}

function loadVisibility(): ColumnVisibilityState {
  try {
    const stored = window.localStorage.getItem(columnStorageKey)
    if (stored !== null) {
      const parsed: unknown = JSON.parse(stored)
      if (typeof parsed === 'object' && parsed !== null) {
        return { ...defaultHidden, ...(parsed as ColumnVisibilityState) }
      }
    }
  } catch {
    return defaultHidden
  }
  return defaultHidden
}

function storeVisibility(visibility: ColumnVisibilityState) {
  try {
    window.localStorage.setItem(columnStorageKey, JSON.stringify(visibility))
  } catch {
    return
  }
}

const helper = columnHelper<NodeSummary>()

function text(value: string | null, mono = false) {
  return value === null ? (
    <Text size="sm" c="dimmed">
      unknown
    </Text>
  ) : (
    <Text size="sm" className={mono ? 'mono' : undefined} style={{ whiteSpace: 'nowrap' }}>
      {value}
    </Text>
  )
}

function buildColumns(sort: Sort, onSort: (field: SortField) => void): TableColumn<NodeSummary>[] {
  const header = (label: string, field: SortField) => () => (
    <SortHeader
      label={label}
      direction={sort.field === field ? (sort.descending ? 'descending' : 'ascending') : 'none'}
      onSort={() => onSort(field)}
    />
  )
  const sortState = (field: SortField) =>
    sort.field === field ? (sort.descending ? 'descending' : 'ascending') : 'none'
  return [
    helper.display({
      id: 'hostname',
      header: header('Node', 'hostname'),
      meta: { label: 'Node', mobile: 'title', sort: sortState('hostname') },
      cell: ({ row }) => (
        <Stack gap={0} style={{ minWidth: 150 }}>
          <Anchor
            component={Link}
            to={`/nodes/${row.original.device_id}/${row.original.installation_id}`}
            size="sm"
            fw={600}
            c="var(--twilight-text-strong)"
            onClick={(event) => event.stopPropagation()}
            className="mono"
          >
            {row.original.hostname ?? middleEllipsis(row.original.device_id, 16)}
          </Anchor>
          <Text size="xs" c="dimmed" className="mono">
            {middleEllipsis(row.original.device_id, 16)}
          </Text>
        </Stack>
      ),
    }),
    helper.display({
      id: 'lifecycle',
      header: header('Lifecycle', 'lifecycle'),
      meta: { label: 'Lifecycle', width: 130, hideable: true, sort: sortState('lifecycle') },
      cell: ({ row }) => <StatusPill status={lifecycleStyles[row.original.lifecycle]} />,
    }),
    helper.display({
      id: 'last_seen_at',
      header: 'Presence',
      meta: { label: 'Presence', width: 150, hideable: true },
      cell: ({ row }) => (
        <Presence online={row.original.online} lastSeen={row.original.last_seen_at} />
      ),
    }),
    helper.display({
      id: 'os_name',
      header: header('OS', 'os_name'),
      meta: { label: 'OS', hideable: true, sort: sortState('os_name') },
      cell: ({ row }) =>
        text(
          row.original.os_name === null
            ? null
            : `${row.original.os_name} ${row.original.os_version ?? ''}`.trim(),
        ),
    }),
    helper.display({
      id: 'os_build',
      header: header('OS build', 'os_build'),
      meta: { label: 'OS build', hideable: true, sort: sortState('os_build') },
      cell: ({ row }) => text(row.original.os_build, true),
    }),
    helper.display({
      id: 'dusk_version',
      header: header('Dusk', 'dusk_version'),
      meta: { label: 'Dusk version', hideable: true, sort: sortState('dusk_version') },
      cell: ({ row }) => text(row.original.dusk_version, true),
    }),
    helper.display({
      id: 'hardware_class',
      header: header('Hardware', 'hardware_class'),
      meta: {
        label: 'Hardware class',
        hideable: true,
        mobile: 'hidden',
        sort: sortState('hardware_class'),
      },
      cell: ({ row }) => text(row.original.hardware_class, true),
    }),
    helper.display({
      id: 'country',
      header: header('Country', 'country'),
      meta: { label: 'Country', width: 90, hideable: true, sort: sortState('country') },
      cell: ({ row }) => text(row.original.country),
    }),
    helper.display({
      id: 'tenant',
      header: header('Tenant', 'tenant'),
      meta: { label: 'Tenant', hideable: true, mobile: 'hidden', sort: sortState('tenant') },
      cell: ({ row }) => text(row.original.tenant),
    }),
    helper.display({
      id: 'locale',
      header: header('Locale', 'locale'),
      meta: { label: 'Locale', hideable: true, mobile: 'hidden', sort: sortState('locale') },
      cell: ({ row }) => text(row.original.locale, true),
    }),
    helper.display({
      id: 'impl',
      header: header('Impl', 'impl'),
      meta: { label: 'Impl', hideable: true, mobile: 'hidden', sort: sortState('impl') },
      cell: ({ row }) => text(row.original.impl, true),
    }),
    helper.display({
      id: 'target_arch',
      header: header('Arch', 'target_arch'),
      meta: {
        label: 'Architecture',
        hideable: true,
        mobile: 'hidden',
        sort: sortState('target_arch'),
      },
      cell: ({ row }) => text(row.original.target_arch, true),
    }),
    helper.display({
      id: 'reported_version',
      header: header('Reported version', 'reported_version'),
      meta: {
        label: 'Reported version',
        hideable: true,
        mobile: 'hidden',
        sort: sortState('reported_version'),
      },
      cell: ({ row }) => text(row.original.reported_version, true),
    }),
    helper.display({
      id: 'installation_id',
      header: 'Installation',
      meta: { label: 'Installation id', hideable: true, mobile: 'hidden' },
      cell: ({ row }) => (
        <MonoId value={row.original.installation_id} label="installation id" maximum={16} />
      ),
    }),
    helper.display({
      id: 'first_seen_at',
      header: 'First seen',
      meta: { label: 'First seen', hideable: true, mobile: 'hidden' },
      cell: ({ row }) => <RelativeTime value={row.original.first_seen_at} />,
    }),
    helper.display({
      id: 'enrolled_at',
      header: 'Enrolled',
      meta: { label: 'Enrolled', hideable: true, mobile: 'hidden' },
      cell: ({ row }) => <RelativeTime value={row.original.enrolled_at} fallback="not recorded" />,
    }),
  ]
}

function ColumnChooser({
  columns,
  visibility,
  onChange,
}: {
  columns: TableColumn<NodeSummary>[]
  visibility: ColumnVisibilityState
  onChange: (visibility: ColumnVisibilityState) => void
}) {
  return (
    <Menu position="bottom-end" closeOnItemClick={false} withinPortal>
      <Menu.Target>
        <Button variant="default" leftSection={<IconColumns3 size={16} aria-hidden="true" />}>
          Columns
        </Button>
      </Menu.Target>
      <Menu.Dropdown>
        <Menu.Label>Show columns</Menu.Label>
        {columns
          .filter((column) => column.meta?.hideable === true && column.id !== undefined)
          .map((column) => {
            const columnId = column.id ?? ''
            const visible = visibility[columnId] !== false
            return (
              <Menu.Item
                key={columnId}
                role="menuitemcheckbox"
                aria-checked={visible}
                leftSection={
                  <Checkbox checked={visible} readOnly size="xs" tabIndex={-1} aria-hidden />
                }
                onClick={() => onChange({ ...visibility, [columnId]: !visible })}
              >
                {column.meta?.label}
              </Menu.Item>
            )
          })}
        <Menu.Divider />
        <Menu.Item onClick={() => onChange(defaultHidden)}>Reset to the defaults</Menu.Item>
      </Menu.Dropdown>
    </Menu>
  )
}

type OnlineFilter = NodeListParameters['online']

export function NodesPage() {
  const navigate = useNavigate()
  const [searchParameters, setSearchParameters] = useSearchParams()
  const applied = searchParameters.get('selector') ?? ''
  const onlineText = searchParameters.get('online')
  const online: OnlineFilter =
    onlineText === 'online' || onlineText === 'offline' ? onlineText : 'any'
  const sort = parseSort(searchParameters.get('sort'))
  const [draft, setDraft] = useState(applied)
  const [draftFor, setDraftFor] = useState(applied)
  if (draftFor !== applied) {
    setDraftFor(applied)
    setDraft(applied)
  }
  const [validation, setValidation] = useState<SelectorValidation | undefined>(undefined)
  const [visibility, setVisibility] = useState(loadVisibility)
  const pages = useCursorPages(`${applied}/${online}/${sortText(sort)}`, 50)
  const nodes = useNodes({
    selector: applied,
    online,
    sort: sortText(sort),
    limit: pages.pageSize,
    cursor: pages.cursor,
  })

  const setParameter = (key: string, value: string | null) =>
    setSearchParameters(
      (previous) => {
        const next = new URLSearchParams(previous)
        if (value === null || value.length === 0) {
          next.delete(key)
        } else {
          next.set(key, value)
        }
        return next
      },
      { replace: true },
    )
  const onSort = (field: SortField) =>
    setParameter(
      'sort',
      sortText({ field, descending: sort.field === field ? !sort.descending : false }),
    )
  const columns = buildColumns(sort, onSort)
  const apply = () => {
    if (validation?.ok === false) {
      return
    }
    setParameter('selector', draft.trim())
  }
  const filtered = applied.length > 0 || online !== 'any'
  const pending = draft.trim() !== applied
  const clear = () => {
    setDraft('')
    setSearchParameters(
      new URLSearchParams(
        sort.field === 'hostname' && !sort.descending ? {} : { sort: sortText(sort) },
      ),
      {
        replace: true,
      },
    )
  }

  return (
    <>
      <PageHeader
        title="Nodes"
        documentTitle="Nodes"
        description="Every node in the inventory, one row each, as twilight knows it now."
      />
      <Stack gap="md">
        <div role="search" aria-label="Filter nodes">
          <SelectorField
            value={draft}
            onChange={setDraft}
            label="Selector"
            singleLine
            showSample={false}
            onSubmit={apply}
            onValidation={setValidation}
          />
          <Group justify="space-between" gap="sm" mt="sm" wrap="wrap">
            <Group gap="sm" wrap="wrap">
              <Button onClick={apply} disabled={!pending || validation?.ok === false}>
                Apply selector
              </Button>
              <SegmentedControl
                aria-label="Presence"
                value={online}
                onChange={(value) => setParameter('online', value === 'any' ? null : value)}
                data={[
                  { value: 'any', label: 'Any' },
                  { value: 'online', label: 'Online' },
                  { value: 'offline', label: 'Offline' },
                ]}
              />
              {filtered && (
                <Button
                  variant="subtle"
                  color="gray"
                  leftSection={<IconFilterOff size={16} aria-hidden="true" />}
                  onClick={clear}
                >
                  Clear filters
                </Button>
              )}
            </Group>
            <ColumnChooser
              columns={columns}
              visibility={visibility}
              onChange={(next) => {
                setVisibility(next)
                storeVisibility(next)
              }}
            />
          </Group>
          {pending && validation?.ok !== false && (
            <Text size="xs" c="dimmed" mt={6}>
              Press Enter or Apply selector to filter the table.
            </Text>
          )}
        </div>
        {nodes.isError ? (
          <QueryError error={nodes.error} what="nodes" onRetry={() => void nodes.refetch()} />
        ) : (
          <>
            <DataTable
              label="Nodes"
              columns={columns}
              data={nodes.data?.items ?? []}
              getRowId={(node) => `${node.device_id}/${node.installation_id}`}
              loading={nodes.isPending}
              columnVisibility={visibility}
              maxHeight="max(420px, calc(100dvh - 300px))"
              onOpen={(node) => void navigate(`/nodes/${node.device_id}/${node.installation_id}`)}
              empty={
                filtered ? (
                  <EmptyState
                    size="sm"
                    icon={<IconFilterOff size={22} aria-hidden="true" />}
                    title="No node matches these filters"
                    description="Change the selector or the presence filter, or clear them to see every node."
                  >
                    <EmptyState.Actions>
                      <Button size="xs" variant="light" onClick={clear}>
                        Clear filters
                      </Button>
                    </EmptyState.Actions>
                  </EmptyState>
                ) : (
                  <EmptyState
                    size="md"
                    icon={<IconServer2 size={24} aria-hidden="true" />}
                    title="No nodes yet"
                    description="A node appears here once it enrolls with nightfall: build it with an init script that runs nightfall -c, and start it."
                  />
                )
              }
            />
            {nodes.data !== undefined && (nodes.data.items.length > 0 || pages.pageIndex > 0) && (
              <CursorPagination
                pages={pages}
                shown={nodes.data.items.length}
                total={null}
                nextCursor={nodes.data.next_cursor}
                noun="nodes"
              />
            )}
          </>
        )}
      </Stack>
    </>
  )
}
