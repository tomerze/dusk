import { useState } from 'react'

interface CursorPagesState {
  key: string
  previous: (string | null)[]
  cursor: string | null
  pageSize: number
}

export interface CursorPages {
  cursor: string | null
  pageIndex: number
  pageSize: number
  next: (cursor: string) => void
  previous: () => void
  setPageSize: (pageSize: number) => void
}

export function useCursorPages(resetKey: string, initialPageSize = 50): CursorPages {
  const [state, setState] = useState<CursorPagesState>({
    key: resetKey,
    previous: [],
    cursor: null,
    pageSize: initialPageSize,
  })
  let current = state
  if (state.key !== resetKey) {
    current = { key: resetKey, previous: [], cursor: null, pageSize: state.pageSize }
    setState(current)
  }
  return {
    cursor: current.cursor,
    pageIndex: current.previous.length,
    pageSize: current.pageSize,
    next: (cursor) =>
      setState((value) => ({ ...value, previous: [...value.previous, value.cursor], cursor })),
    previous: () =>
      setState((value) =>
        value.previous.length === 0
          ? value
          : {
              ...value,
              previous: value.previous.slice(0, -1),
              cursor: value.previous[value.previous.length - 1] ?? null,
            },
      ),
    setPageSize: (pageSize) =>
      setState((value) => ({ ...value, pageSize, previous: [], cursor: null })),
  }
}
