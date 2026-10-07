import { useDebouncedValue } from '@mantine/hooks'
import { keepPreviousData, useQuery } from '@tanstack/react-query'
import { validateSelector } from '../api/queries'

export function useSelectorValidation(selector: string, delay = 300) {
  const [debounced] = useDebouncedValue(selector, delay)
  const query = useQuery({
    queryKey: ['selector-validation', debounced],
    queryFn: ({ signal }) => validateSelector(debounced, signal),
    placeholderData: keepPreviousData,
    staleTime: 30_000,
    retry: false,
  })
  const current = query.data !== undefined && !query.isPlaceholderData
  return {
    validation: query.data,
    validatedSelector: debounced,
    checking: debounced !== selector || query.isFetching,
    stale: !current || debounced !== selector,
    failure: query.error,
  }
}
