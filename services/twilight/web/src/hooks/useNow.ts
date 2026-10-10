import { useSyncExternalStore } from 'react'

const listeners = new Set<() => void>()
let current = Date.now()
let timer: ReturnType<typeof setInterval> | null = null

function subscribe(listener: () => void) {
  listeners.add(listener)
  if (timer === null) {
    current = Date.now()
    timer = setInterval(() => {
      current = Date.now()
      for (const notify of listeners) {
        notify()
      }
    }, 15_000)
  }
  return () => {
    listeners.delete(listener)
    if (listeners.size === 0 && timer !== null) {
      clearInterval(timer)
      timer = null
    }
  }
}

export function useNow(): number {
  return useSyncExternalStore(subscribe, () => current)
}
