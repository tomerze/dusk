import '@testing-library/jest-dom/vitest'
import { cleanup, configure } from '@testing-library/react'
import { afterEach, vi } from 'vitest'

configure({ asyncUtilTimeout: 5000 })

Object.defineProperty(window, 'matchMedia', {
  writable: true,
  value: vi.fn().mockImplementation((query: string) => ({
    matches: false,
    media: query,
    onchange: null,
    addListener: vi.fn(),
    removeListener: vi.fn(),
    addEventListener: vi.fn(),
    removeEventListener: vi.fn(),
    dispatchEvent: vi.fn(),
  })),
})

class ResizeObserverStub {
  observe() {}
  unobserve() {}
  disconnect() {}
}

window.ResizeObserver = ResizeObserverStub as unknown as typeof ResizeObserver
window.HTMLElement.prototype.scrollIntoView = function scrollIntoView() {}
window.scrollTo = (() => undefined) as typeof window.scrollTo
Object.defineProperty(document, 'fonts', {
  configurable: true,
  value: {
    ready: Promise.resolve(),
    addEventListener: () => undefined,
    removeEventListener: () => undefined,
  },
})
document.createRange = () => {
  const range = new Range()
  range.getBoundingClientRect = () => new DOMRect()
  range.getClientRects = () =>
    ({
      item: () => null,
      length: 0,
      [Symbol.iterator]: [][Symbol.iterator],
    }) as unknown as DOMRectList
  return range
}

afterEach(() => {
  cleanup()
  window.sessionStorage.clear()
})
