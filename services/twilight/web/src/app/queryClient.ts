import { QueryCache, QueryClient } from '@tanstack/react-query'
import { ApiError, storedToken, storeToken } from '../api/client'

export function createQueryClient(): QueryClient {
  const client: QueryClient = new QueryClient({
    queryCache: new QueryCache({
      onError: (error, query) => {
        if (!(error instanceof ApiError) || error.status !== 401) {
          return
        }
        if (storedToken() !== null) {
          storeToken(null)
          void client.invalidateQueries({ queryKey: ['me'] })
        } else if (query.queryKey[0] !== 'me') {
          void client.invalidateQueries({ queryKey: ['me'] })
        }
      },
    }),
    defaultOptions: {
      queries: {
        staleTime: 10_000,
        refetchOnWindowFocus: true,
        retry: (count, error) =>
          count < 2 && (!(error instanceof ApiError) || error.status === 0 || error.status >= 500),
        retryDelay: (attempt, error) =>
          Math.max(
            error instanceof ApiError ? (error.retryAfterSeconds ?? 0) * 1000 : 0,
            Math.random() * Math.min(30_000, 1000 * 2 ** attempt),
          ),
      },
      mutations: { retry: false },
    },
  })
  return client
}
