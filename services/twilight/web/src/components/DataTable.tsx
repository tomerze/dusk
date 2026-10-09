import { Skeleton } from '@mantine/core'
import type { ColumnVisibilityState, RowData } from '@tanstack/react-table'
import { useTable } from '@tanstack/react-table'
import type { KeyboardEvent, ReactNode } from 'react'
import { useRef, useState } from 'react'
import type { TableColumn } from './table'
import { tableFeatureSet } from './table'
import classes from './DataTable.module.css'

interface DataTableProperties<Row extends RowData> {
  label: string
  columns: TableColumn<Row>[]
  data: Row[]
  getRowId: (row: Row) => string
  onOpen?: (row: Row) => void
  loading?: boolean
  skeletonRows?: number
  empty?: ReactNode
  columnVisibility?: ColumnVisibilityState
  rowTone?: (row: Row) => 'danger' | 'warning' | 'accent' | undefined
  maxHeight?: string
  dense?: boolean
  framed?: boolean
}

const noVisibility: ColumnVisibilityState = {}

export function DataTable<Row extends RowData>({
  label,
  columns,
  data,
  getRowId,
  onOpen,
  loading = false,
  skeletonRows = 8,
  empty,
  columnVisibility = noVisibility,
  rowTone,
  maxHeight,
  dense = false,
  framed = true,
}: DataTableProperties<Row>) {
  const table = useTable({
    features: tableFeatureSet,
    columns,
    data,
    getRowId: (row: Row) => getRowId(row),
    state: { columnVisibility },
  })
  const rowElements = useRef<(HTMLTableRowElement | null)[]>([])
  const [activeIndex, setActiveIndex] = useState(0)
  const rows = table.getRowModel().rows
  const visibleColumns = table.getVisibleLeafColumns()

  const focusRow = (index: number) => {
    const bounded = Math.max(0, Math.min(rows.length - 1, index))
    setActiveIndex(bounded)
    rowElements.current[bounded]?.focus()
  }

  const handleKeyDown = (event: KeyboardEvent<HTMLTableRowElement>, index: number, row: Row) => {
    if (event.target !== event.currentTarget) {
      return
    }
    switch (event.key) {
      case 'ArrowDown':
      case 'j':
        event.preventDefault()
        focusRow(index + 1)
        break
      case 'ArrowUp':
      case 'k':
        event.preventDefault()
        focusRow(index - 1)
        break
      case 'Home':
        event.preventDefault()
        focusRow(0)
        break
      case 'End':
        event.preventDefault()
        focusRow(rows.length - 1)
        break
      case 'Enter':
      case ' ':
        if (onOpen !== undefined) {
          event.preventDefault()
          onOpen(row)
        }
        break
    }
  }

  const showEmpty = !loading && rows.length === 0
  return (
    <div
      className={classes.frame}
      data-framed={framed}
      style={maxHeight === undefined ? undefined : { maxHeight }}
    >
      <table
        className={classes.table}
        aria-label={label}
        data-dense={dense || undefined}
        aria-busy={loading || undefined}
      >
        <thead>
          {table.getHeaderGroups().map((group) => (
            <tr key={group.id}>
              {group.headers.map((header) => {
                const meta = header.column.columnDef.meta
                return (
                  <th
                    key={header.id}
                    scope="col"
                    data-align={meta?.align}
                    style={meta?.width === undefined ? undefined : { width: meta.width }}
                  >
                    {header.isPlaceholder ? null : <table.FlexRender header={header} />}
                  </th>
                )
              })}
            </tr>
          ))}
        </thead>
        <tbody>
          {loading &&
            Array.from(Array(skeletonRows).keys(), (rowIndex) => (
              <tr key={`skeleton-${rowIndex}`} className={classes.skeletonRow} aria-hidden="true">
                {visibleColumns.map((column) => (
                  <td key={column.id} data-label={column.columnDef.meta?.label ?? ''}>
                    <Skeleton
                      height={12}
                      radius="sm"
                      width={`${50 + ((rowIndex * 17 + column.id.length * 11) % 45)}%`}
                    />
                  </td>
                ))}
              </tr>
            ))}
          {!loading &&
            rows.map((row, index) => {
              const tone = rowTone?.(row.original)
              return (
                <tr
                  key={row.id}
                  ref={(element) => {
                    rowElements.current[index] = element
                  }}
                  tabIndex={index === Math.min(activeIndex, rows.length - 1) ? 0 : -1}
                  data-openable={onOpen === undefined ? undefined : true}
                  data-tone={tone}
                  onFocus={() => setActiveIndex(index)}
                  onKeyDown={(event) => handleKeyDown(event, index, row.original)}
                  onClick={(event) => {
                    const target = event.target as HTMLElement
                    if (
                      onOpen !== undefined &&
                      target.closest('a, button, input, [role="checkbox"], [data-no-open]') === null
                    ) {
                      onOpen(row.original)
                    }
                  }}
                >
                  {row.getVisibleCells().map((cell) => {
                    const meta = cell.column.columnDef.meta
                    return (
                      <td
                        key={cell.id}
                        data-label={meta?.label ?? ''}
                        data-align={meta?.align}
                        data-mobile={meta?.mobile}
                      >
                        <table.FlexRender cell={cell} />
                      </td>
                    )
                  })}
                </tr>
              )
            })}
        </tbody>
      </table>
      {showEmpty && <div className={classes.empty}>{empty}</div>}
    </div>
  )
}
