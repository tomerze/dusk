import type { ApiErrorBody, JsonValue } from './types'

const tokenStorageKey = 'twilight.token'
const csrfCookieName = 'twilight_csrf'
const csrfHeaderName = 'X-CSRF-Token'

export class ApiError extends Error {
  readonly status: number
  readonly code: string
  readonly details: Record<string, JsonValue>

  constructor(
    status: number,
    code: string,
    message: string,
    details: Record<string, JsonValue> = {},
  ) {
    super(message)
    this.name = 'ApiError'
    this.status = status
    this.code = code
    this.details = details
  }

  get loginPath(): string | null {
    const login = this.details.login
    return typeof login === 'string' && login.startsWith('/') ? login : null
  }

  get retryAfterSeconds(): number | null {
    const seconds = this.details.retry_after_seconds
    return typeof seconds === 'number' && Number.isFinite(seconds) && seconds > 0 ? seconds : null
  }
}

export function storedToken(): string | null {
  try {
    return window.sessionStorage.getItem(tokenStorageKey)
  } catch {
    return null
  }
}

export function storeToken(token: string | null): void {
  try {
    if (token === null) {
      window.sessionStorage.removeItem(tokenStorageKey)
    } else {
      window.sessionStorage.setItem(tokenStorageKey, token)
    }
  } catch {
    return
  }
}

function csrfToken(): string | null {
  const prefix = `${csrfCookieName}=`
  for (const part of document.cookie.split(';')) {
    const trimmed = part.trim()
    if (trimmed.startsWith(prefix)) {
      return decodeURIComponent(trimmed.slice(prefix.length))
    }
  }
  return null
}

function isErrorBody(value: unknown): value is ApiErrorBody {
  if (typeof value !== 'object' || value === null || !('error' in value)) {
    return false
  }
  const error = (value as { error: unknown }).error
  return (
    typeof error === 'object' &&
    error !== null &&
    typeof (error as { message?: unknown }).message === 'string'
  )
}

function exactInteger(_key: string, value: unknown, context?: { source?: string }): unknown {
  if (
    typeof value === 'number' &&
    !Number.isSafeInteger(value) &&
    context?.source !== undefined &&
    /^-?\d+$/.test(context.source) &&
    'rawJSON' in JSON &&
    typeof JSON.rawJSON === 'function'
  ) {
    return JSON.rawJSON(context.source)
  }
  return value
}

export const requestTimeoutMilliseconds = 30_000

export async function apiRequest<Result>(
  method: 'GET' | 'POST' | 'PUT' | 'DELETE',
  path: string,
  body?: unknown,
  signal?: AbortSignal,
): Promise<Result> {
  const headers: Record<string, string> = { Accept: 'application/json' }
  const token = storedToken()
  if (token !== null) {
    headers.Authorization = `Bearer ${token}`
  }
  if (body !== undefined) {
    headers['Content-Type'] = 'application/json'
  }
  if (method !== 'GET') {
    const csrf = csrfToken()
    if (csrf !== null) {
      headers[csrfHeaderName] = csrf
    }
  }
  const deadline = AbortSignal.timeout(requestTimeoutMilliseconds)
  let response: Response
  let text: string
  try {
    response = await fetch(path, {
      method,
      headers,
      credentials: 'same-origin',
      body: body === undefined ? undefined : JSON.stringify(body),
      signal: signal === undefined ? deadline : AbortSignal.any([signal, deadline]),
    })
    text = response.status === 204 ? '' : await response.text()
  } catch (failure) {
    if (signal?.aborted === true) {
      throw failure
    }
    if (deadline.aborted) {
      throw new ApiError(
        0,
        'timeout',
        `twilight did not answer within ${requestTimeoutMilliseconds / 1000} seconds`,
      )
    }
    const detail = failure instanceof Error ? failure.message : String(failure)
    throw new ApiError(0, 'network', `Could not reach twilight: ${detail}`)
  }
  if (response.status === 204) {
    return undefined as Result
  }
  let parsed: unknown = null
  if (text.length > 0) {
    try {
      parsed = JSON.parse(text, exactInteger)
    } catch {
      parsed = null
    }
  }
  if (!response.ok) {
    if (isErrorBody(parsed)) {
      const details = parsed.error.details
      throw new ApiError(
        response.status,
        parsed.error.code,
        parsed.error.message,
        typeof details === 'object' && details !== null && !Array.isArray(details) ? details : {},
      )
    }
    throw new ApiError(
      response.status,
      'http',
      `twilight answered ${response.status} ${response.statusText}`.trim(),
    )
  }
  return parsed as Result
}

export function queryString(
  parameters: Record<string, string | number | boolean | null | undefined>,
) {
  const search = new URLSearchParams()
  for (const [key, value] of Object.entries(parameters)) {
    if (value !== null && value !== undefined && value !== '') {
      search.set(key, String(value))
    }
  }
  const text = search.toString()
  return text.length > 0 ? `?${text}` : ''
}

export function errorMessage(error: unknown): string {
  if (error instanceof Error) {
    return error.message
  }
  return String(error)
}
