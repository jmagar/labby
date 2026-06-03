'use client'

import React from 'react'
import { EditorState, type Extension } from '@codemirror/state'
import { EditorView } from '@codemirror/view'

import { collectEditorAutocomplete, collectEditorDiagnostics } from '@/lib/editor/diagnostics-registry'
import type { EditorDiagnostic, EditorLanguage } from '@/lib/editor/types'
import { loadLanguageExtension } from '@/lib/editor/language-registry'
import {
  baseTextSurfaceExtensions,
  diagnosticsCompartment,
  diagnosticsExtension,
  editableCompartment,
  languageCompartment,
} from './text-surface-theme'
import { TextSurfaceToolbar } from './text-surface-toolbar'

export interface TextSurfaceProps {
  path: string
  value: string
  mode: 'view' | 'edit'
  language: EditorLanguage
  dirty?: boolean
  diagnostics?: EditorDiagnostic[]
  onChange?: (next: string) => void
  onSave?: () => void
  onDeploy?: () => void
  onCopy?: () => void
}

function createState(doc: string, editable: boolean, diagnostics: EditorDiagnostic[]): EditorState {
  return EditorState.create({
    doc,
    extensions: baseTextSurfaceExtensions({ editable, diagnostics }),
  })
}

export function TextSurface({ path, value, mode, language, dirty = false, diagnostics, onChange, onSave, onDeploy, onCopy }: TextSurfaceProps) {
  const hostRef = React.useRef<HTMLDivElement | null>(null)
  const viewRef = React.useRef<EditorView | null>(null)
  const onChangeRef = React.useRef(onChange)
  const initialValueRef = React.useRef(value)
  const initialEditableRef = React.useRef(mode === 'edit')
  const initialDiagnosticsRef = React.useRef(diagnostics ?? [])
  const [resolvedDiagnostics, setResolvedDiagnostics] = React.useState<EditorDiagnostic[]>(diagnostics ?? [])

  React.useEffect(() => {
    onChangeRef.current = onChange
  }, [onChange])

  React.useEffect(() => {
    let cancelled = false
    if (diagnostics) {
      setResolvedDiagnostics(diagnostics)
      return
    }
    collectEditorDiagnostics(path, value).then((next) => {
      if (!cancelled) {
        setResolvedDiagnostics(next)
      }
    })
    return () => {
      cancelled = true
    }
  }, [diagnostics, path, value])

  React.useEffect(() => {
    if (!hostRef.current || viewRef.current) return

    const view = new EditorView({
      state: createState(initialValueRef.current, initialEditableRef.current, initialDiagnosticsRef.current),
      parent: hostRef.current,
      dispatch(transaction) {
        view.update([transaction])
        if (transaction.docChanged) {
          onChangeRef.current?.(transaction.state.doc.toString())
        }
      },
    })
    viewRef.current = view

    return () => {
      view.destroy()
      viewRef.current = null
    }
  }, [])

  React.useEffect(() => {
    const view = viewRef.current
    if (!view) return
    const current = view.state.doc.toString()
    if (current !== value) {
      view.dispatch({ changes: { from: 0, to: current.length, insert: value } })
    }
  }, [value])

  React.useEffect(() => {
    const view = viewRef.current
    if (!view) return
    view.dispatch({
      effects: [
        editableCompartment.reconfigure(EditorView.editable.of(mode === 'edit')),
        diagnosticsCompartment.reconfigure(diagnosticsExtension(resolvedDiagnostics)),
      ],
    })
  }, [mode, resolvedDiagnostics])

  React.useEffect(() => {
    let cancelled = false
    void Promise.all([loadLanguageExtension(language), collectEditorAutocomplete(path, value)]).then(([extensions, completions]) => {
      const view = viewRef.current
      if (!view || cancelled) return
      view.dispatch({ effects: [languageCompartment.reconfigure(extensions as Extension)] })
      view.dom.dataset.autocompleteCount = String(completions.length)
    })
    return () => {
      cancelled = true
    }
  }, [language, path, value])

  return (
    <div className="aurora-text-surface flex h-full min-h-0 flex-col overflow-hidden rounded-aurora-2 border border-aurora-border-strong bg-aurora-panel-strong shadow-[var(--aurora-shadow-strong),var(--aurora-highlight-strong)]">
      <TextSurfaceToolbar
        path={path}
        language={language}
        dirty={dirty}
        diagnostics={resolvedDiagnostics}
        canEdit={mode === 'edit'}
        onSave={onSave}
        onDeploy={onDeploy}
        onCopy={onCopy}
      />
      <div className="min-h-0 flex-1 overflow-hidden bg-aurora-page-bg">
        <div ref={hostRef} className="cm-editor h-full" aria-label="Code editor" />
      </div>
    </div>
  )
}
