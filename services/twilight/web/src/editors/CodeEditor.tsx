import { closeBrackets, closeBracketsKeymap, completionKeymap } from '@codemirror/autocomplete'
import { defaultKeymap, history, historyKeymap } from '@codemirror/commands'
import { HighlightStyle, syntaxHighlighting } from '@codemirror/language'
import type { Diagnostic } from '@codemirror/lint'
import { lintKeymap, setDiagnostics } from '@codemirror/lint'
import type { Extension } from '@codemirror/state'
import { Compartment, EditorState, Prec } from '@codemirror/state'
import { EditorView, keymap, placeholder as placeholderExtension } from '@codemirror/view'
import { tags } from '@lezer/highlight'
import { useEffect, useRef } from 'react'
import classes from './CodeEditor.module.css'

const highlight = HighlightStyle.define([
  { tag: tags.keyword, color: 'var(--twilight-syntax-keyword)', fontWeight: '600' },
  { tag: tags.operator, color: 'var(--twilight-syntax-operator)' },
  { tag: tags.string, color: 'var(--twilight-syntax-string)' },
  { tag: tags.special(tags.string), color: 'var(--twilight-syntax-path)' },
  { tag: tags.number, color: 'var(--twilight-syntax-number)' },
  { tag: tags.bool, color: 'var(--twilight-syntax-number)' },
  { tag: tags.propertyName, color: 'var(--twilight-syntax-property)' },
  { tag: tags.variableName, color: 'var(--twilight-syntax-variable)' },
  {
    tag: tags.function(tags.variableName),
    color: 'var(--twilight-syntax-command)',
    fontWeight: '600',
  },
  { tag: tags.attributeName, color: 'var(--twilight-syntax-flag)' },
  { tag: tags.comment, color: 'var(--twilight-syntax-comment)', fontStyle: 'italic' },
  { tag: tags.invalid, color: 'var(--mantine-color-red-text)' },
  { tag: tags.bracket, color: 'var(--twilight-syntax-operator)' },
])

const theme = EditorView.theme({
  '&': {
    color: 'var(--mantine-color-text)',
    backgroundColor: 'transparent',
    fontSize: 'var(--mantine-font-size-sm)',
  },
  '.cm-content': {
    fontFamily: 'var(--mantine-font-family-monospace)',
    caretColor: 'var(--mantine-primary-color-filled)',
    padding: '8px 0',
  },
  '.cm-line': { padding: '0 12px' },
  '.cm-cursor': { borderLeftColor: 'var(--mantine-primary-color-filled)' },
  '&.cm-focused .cm-selectionBackground, .cm-selectionBackground, ::selection': {
    backgroundColor: 'color-mix(in srgb, var(--mantine-primary-color-filled) 30%, transparent)',
  },
  '.cm-placeholder': { color: 'var(--mantine-color-dimmed)' },
  '.cm-tooltip': {
    backgroundColor: 'var(--twilight-surface-raised)',
    border: '1px solid var(--mantine-color-default-border)',
    borderRadius: 'var(--mantine-radius-sm)',
    color: 'var(--mantine-color-text)',
  },
  '.cm-tooltip-autocomplete > ul': { fontFamily: 'var(--mantine-font-family-monospace)' },
  '.cm-tooltip-autocomplete > ul > li[aria-selected]': {
    backgroundColor: 'var(--twilight-accent-soft)',
    color: 'var(--twilight-text-strong)',
  },
  '.cm-completionDetail': { color: 'var(--mantine-color-dimmed)', fontStyle: 'normal' },
  '.cm-completionInfo': { maxWidth: '320px' },
  '.cm-lintRange-error': {
    backgroundImage: 'none',
    textDecoration: 'underline wavy var(--mantine-color-red-filled)',
    textDecorationSkipInk: 'none',
    textUnderlineOffset: '4px',
    backgroundColor: 'color-mix(in srgb, var(--mantine-color-red-filled) 14%, transparent)',
  },
  '.cm-diagnostic-error': { borderLeftColor: 'var(--mantine-color-red-filled)' },
})

export interface CodeEditorProperties {
  value: string
  onChange?: (value: string) => void
  label: string
  language: Extension
  placeholder?: string
  readOnly?: boolean
  singleLine?: boolean
  onSubmit?: () => void
  diagnostics?: Diagnostic[]
  invalid?: boolean
  minimumRows?: number
  describedBy?: string
}

export function CodeEditor({
  value,
  onChange,
  label,
  language,
  placeholder,
  readOnly = false,
  singleLine = false,
  onSubmit,
  diagnostics,
  invalid = false,
  minimumRows = 1,
  describedBy,
}: CodeEditorProperties) {
  const host = useRef<HTMLDivElement>(null)
  const view = useRef<EditorView | null>(null)
  const latest = useRef({ onChange, onSubmit, value, label })
  const attributes = useRef(new Compartment())
  useEffect(() => {
    latest.current = { onChange, onSubmit, value, label }
  })

  useEffect(() => {
    if (host.current === null) {
      return
    }
    const extensions: Extension[] = [
      theme,
      syntaxHighlighting(highlight),
      language,
      EditorState.readOnly.of(readOnly),
      EditorView.editable.of(!readOnly),
      attributes.current.of(
        EditorView.contentAttributes.of({ 'aria-label': latest.current.label }),
      ),
      EditorView.updateListener.of((update) => {
        if (update.docChanged) {
          latest.current.onChange?.(update.state.doc.toString())
        }
      }),
    ]
    if (!readOnly) {
      extensions.push(
        history(),
        closeBrackets(),
        keymap.of([
          ...closeBracketsKeymap,
          ...defaultKeymap,
          ...historyKeymap,
          ...completionKeymap,
          ...lintKeymap,
        ]),
      )
    }
    if (placeholder !== undefined) {
      extensions.push(placeholderExtension(placeholder))
    }
    if (singleLine) {
      extensions.push(
        EditorState.transactionFilter.of((transaction) =>
          transaction.newDoc.lines > 1
            ? [
                transaction,
                {
                  changes: {
                    from: 0,
                    to: transaction.newDoc.length,
                    insert: transaction.newDoc.sliceString(0).replace(/[\r\n]+/g, ' '),
                  },
                  sequential: true,
                },
              ]
            : transaction,
        ),
        Prec.highest(
          keymap.of([
            {
              key: 'Enter',
              run: () => {
                latest.current.onSubmit?.()
                return true
              },
            },
          ]),
        ),
      )
    } else {
      extensions.push(EditorView.lineWrapping)
    }
    const created = new EditorView({
      parent: host.current,
      state: EditorState.create({ doc: latest.current.value, extensions }),
    })
    view.current = created
    return () => {
      created.destroy()
      view.current = null
    }
  }, [language, readOnly, singleLine, placeholder])

  useEffect(() => {
    const current = view.current
    if (current === null) {
      return
    }
    const attributesValue: Record<string, string> = { 'aria-label': label }
    if (describedBy !== undefined) {
      attributesValue['aria-describedby'] = describedBy
    }
    if (invalid) {
      attributesValue['aria-invalid'] = 'true'
    }
    if (!singleLine) {
      attributesValue['aria-multiline'] = 'true'
    }
    current.dispatch({
      effects: attributes.current.reconfigure(EditorView.contentAttributes.of(attributesValue)),
    })
  }, [label, describedBy, invalid, singleLine])

  useEffect(() => {
    const current = view.current
    if (current === null) {
      return
    }
    const document = current.state.doc.toString()
    if (document !== value) {
      current.dispatch({ changes: { from: 0, to: document.length, insert: value } })
    }
  }, [value])

  useEffect(() => {
    const current = view.current
    if (current === null || diagnostics === undefined) {
      return
    }
    const length = current.state.doc.length
    const bounded = diagnostics.map((diagnostic) => ({
      ...diagnostic,
      from: Math.min(diagnostic.from, length),
      to: Math.min(Math.max(diagnostic.to, diagnostic.from), length),
    }))
    current.dispatch(setDiagnostics(current.state, bounded))
  }, [diagnostics, value])

  return (
    <div
      ref={host}
      className={classes.editor}
      data-invalid={invalid || undefined}
      data-readonly={readOnly || undefined}
      data-single-line={singleLine || undefined}
      style={{ minHeight: singleLine ? undefined : `calc(${minimumRows} * 1.55em + 18px)` }}
    />
  )
}
