import type { ColumnDef, RowData } from '@tanstack/react-table'
import { columnVisibilityFeature, createColumnHelper, tableFeatures } from '@tanstack/react-table'

export interface ColumnDescription {
  label: string
  align?: 'left' | 'right'
  width?: number
  mobile?: 'title' | 'hidden' | 'block' | 'normal'
  hideable?: boolean
  sort?: 'ascending' | 'descending' | 'none'
}

export const tableFeatureSet = tableFeatures({
  columnVisibilityFeature,
  columnMeta: {} as ColumnDescription,
})

export type TableFeatureSet = typeof tableFeatureSet

export type TableColumn<Row extends RowData> = ColumnDef<TableFeatureSet, Row, unknown>

export function columnHelper<Row extends RowData>() {
  return createColumnHelper<TableFeatureSet, Row>()
}
