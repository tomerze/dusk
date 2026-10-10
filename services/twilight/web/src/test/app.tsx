import { render } from '@testing-library/react'
import { EditorView } from '@codemirror/view'
import { act } from 'react'
import { createMemoryRouter, RouterProvider } from 'react-router'
import { Providers } from '../app/Providers'
import { routes } from '../app/routes'
import { testQueryClient } from './render'

export function renderApp(path: string) {
  const router = createMemoryRouter(routes, { initialEntries: [path] })
  const result = render(
    <Providers client={testQueryClient()} test>
      <RouterProvider router={router} />
    </Providers>,
  )
  return { ...result, router }
}

export function setEditorText(element: HTMLElement, text: string) {
  const view = EditorView.findFromDOM(element)
  if (view === null) {
    throw new Error('the element is not inside a CodeMirror editor')
  }
  act(() => {
    view.dispatch({ changes: { from: 0, to: view.state.doc.length, insert: text } })
  })
}
