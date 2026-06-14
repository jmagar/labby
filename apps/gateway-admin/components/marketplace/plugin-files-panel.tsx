'use client'

import { useEffect, useMemo, useState } from 'react'

import {
  deployPluginWorkspace,
  forkMarketplaceArtifact,
  getPluginWorkspace,
  previewPluginWorkspaceDeploy,
  savePluginWorkspaceFile,
} from '@/lib/api/marketplace-client'
import type {
  DeployPluginWorkspacePreviewEntry,
  DeployPluginWorkspacePreviewResult,
  EditorLanguage,
  MarketplaceWorkspaceFile,
} from '@/lib/editor/types'
import { detectEditorLanguage } from '@/lib/editor/language-registry'
import type { Artifact } from '@/lib/types/marketplace'
import { cn } from '@/lib/utils'
import { TextSurface } from '@/components/ui/text-surface'

interface PluginFilesPanelProps {
  pluginId: string
  artifacts: Artifact[]
}

type PanelStatusTone = 'info' | 'success' | 'warning' | 'error'

interface PanelStatus {
  tone: PanelStatusTone
  message: string
  detail?: string
}

export interface FileTreeNode {
  kind: 'dir' | 'file'
  name: string
  path: string
  dirty: boolean
  file?: MarketplaceWorkspaceFile
  children?: FileTreeNode[]
}

interface MutableFileTreeNode {
  kind: 'dir' | 'file'
  name: string
  path: string
  dirty: boolean
  file?: MarketplaceWorkspaceFile
  children?: Record<string, MutableFileTreeNode>
}

const FOLDER_ICON: Record<string, string> = {
  agents: '🤖',
  commands: '⌨️',
  skills: '✨',
  hooks: '🔗',
  monitors: '📊',
  bin: '⚙️',
  'output-styles': '🖋️',
  themes: '🎨',
  scripts: '📜',
}

function toWorkspaceFile(artifact: Artifact): MarketplaceWorkspaceFile {
  return {
    path: artifact.path,
    lang: detectEditorLanguage(artifact.path),
    content: artifact.content,
    savedContent: artifact.content,
    dirty: false,
  }
}

export function buildFileTree(files: MarketplaceWorkspaceFile[]): FileTreeNode[] {
  const root: Record<string, MutableFileTreeNode> = {}

  for (const file of files) {
    const parts = file.path.split('/')
    let level = root
    let currentPath = ''
    for (let index = 0; index < parts.length; index += 1) {
      const part = parts[index]
      currentPath = currentPath ? `${currentPath}/${part}` : part
      const isLeaf = index === parts.length - 1
      const existing = level[part]

      if (isLeaf) {
        level[part] = {
          kind: 'file',
          name: part,
          path: currentPath,
          dirty: Boolean(file.dirty),
          file,
        }
        continue
      }

      if (!existing || existing.kind !== 'dir') {
        level[part] = {
          kind: 'dir',
          name: part,
          path: currentPath,
          dirty: false,
          children: {},
        }
      }

      level = level[part].children ?? (level[part].children = {})
    }
  }

  function finalize(nodes: Record<string, MutableFileTreeNode>): FileTreeNode[] {
    const values: FileTreeNode[] = Object.values(nodes).map((node) => {
      if (node.kind === 'dir') {
        const children = finalize(node.children ?? {})
        return {
          kind: 'dir',
          name: node.name,
          path: node.path,
          dirty: children.some((child) => child.dirty),
          children,
        }
      }
      return {
        kind: 'file',
        name: node.name,
        path: node.path,
        dirty: node.dirty,
        file: node.file,
      }
    })

    return values.sort((left, right) => {
      if (left.kind !== right.kind) {
        return left.kind === 'dir' ? -1 : 1
      }
      return left.name.localeCompare(right.name)
    })
  }

  return finalize(root)
}

function FileTreeBranch({
  nodes,
  activePath,
  openFolders,
  onToggleFolder,
  onSelect,
  depth,
}: {
  nodes: FileTreeNode[]
  activePath: string | null
  openFolders: Set<string>
  onToggleFolder: (path: string) => void
  onSelect: (path: string) => void
  depth: number
}) {
  return (
    <>
      {nodes.map((node) => {
        if (node.kind === 'dir') {
          const isOpen = openFolders.has(node.path)
          const icon = FOLDER_ICON[node.name] ?? '📁'
          return (
            <div key={node.path}>
              <button
                type="button"
                onClick={() => onToggleFolder(node.path)}
                className="flex w-full items-center gap-[5px] py-[5px] pr-[10px] text-left text-[12px] font-semibold text-aurora-text-muted hover:bg-aurora-hover-bg hover:text-aurora-text-primary"
                style={{ paddingLeft: `${10 + depth * 16}px` }}
              >
                <span className={cn('transition-transform duration-150', isOpen && 'rotate-90')}>›</span>
                <span>{icon}</span>
                <span className="min-w-0 flex-1 truncate">{node.name}/</span>
                {node.dirty ? <span className="rounded-full bg-[color-mix(in_srgb,var(--aurora-warn)_12%,transparent)] px-1.5 py-0.5 text-[10px] font-bold uppercase tracking-[0.12em] text-aurora-warn">Dirty</span> : null}
              </button>
              {isOpen && node.children ? (
                <FileTreeBranch
                  nodes={node.children}
                  activePath={activePath}
                  openFolders={openFolders}
                  onToggleFolder={onToggleFolder}
                  onSelect={onSelect}
                  depth={depth + 1}
                />
              ) : null}
            </div>
          )
        }

        const isActive = activePath === node.path
        return (
          <button
            key={node.path}
            type="button"
            onClick={() => onSelect(node.path)}
            className={cn(
              'flex w-full items-center gap-2 py-1 pr-[10px] text-left text-[12px] font-medium text-aurora-text-muted transition-[background,color] duration-100 hover:bg-aurora-hover-bg hover:text-aurora-text-primary',
              isActive && 'border-l-2 border-aurora-accent-primary bg-[color-mix(in_srgb,var(--aurora-accent-primary)_10%,transparent)] text-aurora-accent-strong',
            )}
            style={{ paddingLeft: `${14 + depth * 16}px` }}
          >
            <span className="truncate">{node.name}</span>
            {node.dirty ? <span className="rounded-full bg-[color-mix(in_srgb,var(--aurora-warn)_12%,transparent)] px-1.5 py-0.5 text-[10px] font-bold uppercase tracking-[0.12em] text-aurora-warn">Dirty</span> : null}
          </button>
        )
      })}
    </>
  )
}

function FileTree({
  files,
  activePath,
  onSelect,
}: {
  files: MarketplaceWorkspaceFile[]
  activePath: string | null
  onSelect: (path: string) => void
}) {
  const tree = useMemo(() => buildFileTree(files), [files])
  const [openFolders, setOpenFolders] = useState<Set<string>>(() => {
    const folders = new Set<string>()
    const walk = (nodes: FileTreeNode[]) => {
      for (const node of nodes) {
        if (node.kind === 'dir') {
          folders.add(node.path)
          walk(node.children ?? [])
        }
      }
    }
    walk(tree)
    return folders
  })

  function toggleFolder(path: string) {
    setOpenFolders((previous) => {
      const next = new Set(previous)
      if (next.has(path)) next.delete(path)
      else next.add(path)
      return next
    })
  }

  return (
    <div className="h-full overflow-y-auto overflow-x-hidden border-r border-aurora-border-default bg-aurora-nav-bg pt-[6px] pb-3 aurora-scrollbar">
      <div className="px-[14px] pt-[10px] pb-[5px] text-[10px] font-bold uppercase tracking-[0.16em] text-aurora-text-muted">Files</div>
      <FileTreeBranch
        nodes={tree}
        activePath={activePath}
        openFolders={openFolders}
        onToggleFolder={toggleFolder}
        onSelect={onSelect}
        depth={0}
      />
    </div>
  )
}

function PreviewBucket({
  title,
  tone,
  entries,
  selectedPath,
  onSelect,
}: {
  title: string
  tone: 'changed' | 'skipped' | 'removed' | 'failed'
  entries: string[]
  selectedPath: string | null
  onSelect?: (path: string) => void
}) {
  if (entries.length === 0) return null

  const toneClass =
    tone === 'changed'
      ? 'border-aurora-accent-primary'
      : tone === 'removed'
        ? 'border-destructive'
        : tone === 'failed'
          ? 'border-aurora-error'
          : 'border-aurora-border-default'

  return (
    <details className={cn('rounded-aurora-1 border bg-aurora-nav-bg', toneClass)} open={tone !== 'skipped'}>
      <summary className="cursor-pointer px-3 py-2 text-xs font-semibold text-aurora-text-primary">
        {title} ({entries.length})
      </summary>
      <div className="border-t border-aurora-border-default px-2 py-2">
        {entries.map((entry) => (
          <button
            key={entry}
            type="button"
            onClick={() => onSelect?.(entry)}
            className={cn(
              'flex w-full rounded-aurora-1 px-2 py-1 text-left text-xs text-aurora-text-muted hover:bg-aurora-hover-bg hover:text-aurora-text-primary',
              selectedPath === entry && 'bg-[color-mix(in_srgb,var(--aurora-accent-primary)_10%,transparent)] text-aurora-text-primary',
            )}
          >
            <span className="truncate font-mono">{entry}</span>
          </button>
        ))}
      </div>
    </details>
  )
}

function DiffPreview({
  entry,
}: {
  entry: DeployPluginWorkspacePreviewEntry | null
}) {
  if (!entry) {
    return (
      <div className="rounded-aurora-1 border border-aurora-border-default bg-aurora-nav-bg px-3 py-4 text-xs text-aurora-text-muted">
        Select a changed or removed file to inspect the deploy preview.
      </div>
    )
  }

  const language: EditorLanguage = detectEditorLanguage(entry.path)
  const beforeValue = entry.beforeContent ?? ''
  const afterValue = entry.afterContent ?? ''

  return (
    <div className="grid gap-3 xl:grid-cols-2">
      <div className="min-h-[220px]">
        <div className="mb-2 text-xs font-semibold uppercase tracking-[0.14em] text-aurora-text-muted">Installed target</div>
        <TextSurface
          path={`${entry.path} (before)`}
          value={beforeValue}
          mode="view"
          language={language}
        />
      </div>
      <div className="min-h-[220px]">
        <div className="mb-2 text-xs font-semibold uppercase tracking-[0.14em] text-aurora-text-muted">Workspace mirror</div>
        <TextSurface
          path={`${entry.path} (after)`}
          value={afterValue}
          mode="view"
          language={language}
        />
      </div>
    </div>
  )
}

export function PluginFilesPanel({ pluginId, artifacts }: PluginFilesPanelProps) {
  const [files, setFiles] = useState<MarketplaceWorkspaceFile[]>(() => artifacts.map(toWorkspaceFile))
  const [activePath, setActivePath] = useState<string | null>(() => artifacts.find((artifact) => artifact.path === 'plugin.json')?.path ?? artifacts[0]?.path ?? null)
  const [status, setStatus] = useState<PanelStatus | null>(null)
  const [deployTarget, setDeployTarget] = useState<string | null>(null)
  const [preview, setPreview] = useState<DeployPluginWorkspacePreviewResult | null>(null)
  const [previewSelection, setPreviewSelection] = useState<string | null>(null)
  const [isPreviewing, setIsPreviewing] = useState(false)
  const [forkingPath, setForkingPath] = useState<string | null>(null)

  useEffect(() => {
    let cancelled = false
    getPluginWorkspace(pluginId)
      .then((workspace) => {
        if (cancelled) return
        setFiles(workspace.files.map((file) => ({ ...file, lang: detectEditorLanguage(file.path) })))
        setActivePath((current) => (
          current && workspace.files.some((file) => file.path === current)
            ? current
            : workspace.files[0]?.path ?? null
        ))
        setDeployTarget(workspace.deployTarget ?? null)
        setStatus(null)
      })
      .catch((error: unknown) => {
        if (cancelled) return
        setFiles(artifacts.map(toWorkspaceFile))
        setDeployTarget(null)
        setStatus({
          tone: 'warning',
          message: 'Workspace mirror unavailable. Showing source artifacts instead.',
          detail: error instanceof Error ? error.message : undefined,
        })
      })
    return () => {
      cancelled = true
    }
  }, [artifacts, pluginId])

  const activeFile = useMemo(
    () => files.find((file) => file.path === activePath) ?? null,
    [activePath, files],
  )
  const selectedPreviewEntry = useMemo(
    () => preview?.entries?.find((entry) => entry.path === previewSelection) ?? null,
    [preview, previewSelection],
  )
  const hasDirtyFiles = files.some((file) => file.dirty)

  async function saveActiveFile() {
    if (!activeFile) return
    try {
      const result = await savePluginWorkspaceFile({
        pluginId,
        path: activeFile.path,
        content: activeFile.content,
      })
      setFiles((current) => current.map((file) => (
        file.path === activeFile.path
          ? { ...file, savedContent: activeFile.content, dirty: false }
          : file
      )))
      setPreview(null)
      setPreviewSelection(null)
      setStatus({
        tone: 'success',
        message: `Saved ${activeFile.path}`,
        detail: `Workspace mirror updated at ${result.savedAt}`,
      })
    } catch (error) {
      setStatus({
        tone: 'error',
        message: `Failed to save ${activeFile.path}`,
        detail: error instanceof Error ? error.message : 'Unknown save error',
      })
    }
  }

  async function previewDeploy() {
    if (hasDirtyFiles) {
      setStatus({
        tone: 'warning',
        message: 'Save changes before previewing deploy output.',
      })
      return
    }
    setIsPreviewing(true)
    try {
      const result = await previewPluginWorkspaceDeploy(pluginId)
      setPreview(result)
      setPreviewSelection(result.entries?.[0]?.path ?? null)
      setDeployTarget(result.target ?? deployTarget)
      setStatus({
        tone: 'info',
        message: 'Deploy preview ready.',
        detail: `Changed: ${result.changed.length} · Skipped: ${result.skipped.length} · Removed: ${result.removed.length}`,
      })
    } catch (error) {
      setStatus({
        tone: 'error',
        message: 'Failed to preview deployment.',
        detail: error instanceof Error ? error.message : 'Unknown preview error',
      })
    } finally {
      setIsPreviewing(false)
    }
  }

  async function deployWorkspace() {
    if (hasDirtyFiles) {
      setStatus({
        tone: 'warning',
        message: 'Save changes before deploying the workspace.',
      })
      return
    }
    try {
      const result = await deployPluginWorkspace(pluginId)
      setDeployTarget(result.target ?? deployTarget)
      setPreview(null)
      setPreviewSelection(null)
      setStatus({
        tone: result.ok ? 'success' : 'error',
        message: result.ok
          ? `Deployed ${result.changed.length} file(s)`
          : 'Deployment finished with failures',
        detail: [
          result.target ? `Target: ${result.target}` : null,
          `Changed: ${result.changed.length}`,
          `Skipped: ${result.skipped.length}`,
          `Removed: ${result.removed.length}`,
          result.failed.length ? `Failed: ${result.failed.join(', ')}` : null,
        ].filter(Boolean).join(' · '),
      })
    } catch (error) {
      setStatus({
        tone: 'error',
        message: 'Failed to deploy the workspace.',
        detail: error instanceof Error ? error.message : 'Unknown deploy error',
      })
    }
  }

  async function handleForkSelectedFile() {
    if (!activeFile) return
    setForkingPath(activeFile.path)
    try {
      await forkMarketplaceArtifact({ pluginId, artifacts: [activeFile.path] })
      setStatus({
        tone: 'success',
        message: 'Forked to Stash',
        detail: activeFile.path,
      })
    } catch (error) {
      setStatus({
        tone: 'error',
        message: 'Fork failed',
        detail: error instanceof Error ? error.message : 'Unable to fork artifact into Stash.',
      })
    } finally {
      setForkingPath(null)
    }
  }

  if (!activeFile) {
    return (
      <div className="flex flex-1 flex-col items-center justify-center gap-2 text-sm text-aurora-text-muted">
        <div>No files available for this plugin.</div>
        {deployTarget ? <div className="text-xs">Deploy target: {deployTarget}</div> : null}
      </div>
    )
  }

  return (
    <div className="flex h-full flex-1 overflow-hidden">
      <div className="w-[clamp(220px,19vw,280px)] flex-shrink-0">
        <FileTree files={files} activePath={activePath} onSelect={setActivePath} />
      </div>
      <div className="flex min-w-0 flex-1 flex-col gap-3 p-4">
        {status ? (
          <div
            className={cn(
              'rounded-aurora-1 border px-3 py-2 text-xs',
              status.tone === 'success' && 'border-aurora-success bg-[color-mix(in_srgb,var(--aurora-success)_10%,transparent)] text-aurora-text-primary',
              status.tone === 'warning' && 'border-aurora-warn bg-[color-mix(in_srgb,var(--aurora-warn)_10%,transparent)] text-aurora-text-primary',
              status.tone === 'error' && 'border-destructive bg-[color-mix(in_srgb,var(--destructive)_10%,transparent)] text-aurora-text-primary',
              status.tone === 'info' && 'border-aurora-border-default bg-aurora-control-surface text-aurora-text-primary',
            )}
          >
            <div className="font-medium">{status.message}</div>
            {status.detail ? <div className="mt-1 text-aurora-text-muted">{status.detail}</div> : null}
          </div>
        ) : null}

        <div className="flex flex-wrap items-center gap-2 rounded-aurora-1 border border-aurora-border-default bg-aurora-nav-bg px-3 py-2 text-xs text-aurora-text-muted">
          {deployTarget ? (
            <span>
              Deploy target: <span className="font-mono text-aurora-text-primary">{deployTarget}</span>
            </span>
          ) : (
            <span>Deploy target unavailable until the plugin is installed.</span>
          )}
          <button
            type="button"
            onClick={() => {
              void previewDeploy()
            }}
            className="rounded-aurora-1 border border-aurora-border-default bg-aurora-control-surface px-3 py-1.5 font-semibold text-aurora-text-primary hover:bg-aurora-hover-bg"
          >
            {isPreviewing ? 'Previewing...' : 'Preview deploy'}
          </button>
          <button
            type="button"
            onClick={() => {
              void handleForkSelectedFile()
            }}
            disabled={!activeFile || forkingPath === activeFile.path}
            className="rounded-aurora-1 border border-aurora-border-strong bg-aurora-control-surface px-3 py-1.5 font-semibold text-aurora-text-primary hover:bg-aurora-hover-bg disabled:cursor-not-allowed disabled:opacity-50"
          >
            {forkingPath === activeFile.path ? 'Forking...' : 'Fork to Stash'}
          </button>
        </div>

        {preview ? (
          <div className="rounded-aurora-2 border border-aurora-border-strong bg-aurora-panel-strong p-3">
            <div className="mb-3 text-xs font-semibold uppercase tracking-[0.14em] text-aurora-text-muted">Deploy preview</div>
            <div className="grid gap-3 lg:grid-cols-[320px_minmax(0,1fr)]">
              <div className="space-y-2">
                <PreviewBucket
                  title="Changed"
                  tone="changed"
                  entries={preview.changed}
                  selectedPath={previewSelection}
                  onSelect={setPreviewSelection}
                />
                <PreviewBucket
                  title="Removed"
                  tone="removed"
                  entries={preview.removed}
                  selectedPath={previewSelection}
                  onSelect={setPreviewSelection}
                />
                <PreviewBucket
                  title="Skipped"
                  tone="skipped"
                  entries={preview.skipped}
                  selectedPath={previewSelection}
                />
              </div>
              <DiffPreview entry={selectedPreviewEntry} />
            </div>
          </div>
        ) : null}

        <TextSurface
          path={activeFile.path}
          value={activeFile.content}
          mode="edit"
          language={activeFile.lang}
          dirty={Boolean(activeFile.dirty)}
          onChange={(next) => {
            setFiles((current) => current.map((file) => (
              file.path === activeFile.path
                ? { ...file, content: next, dirty: next !== (file.savedContent ?? file.content) }
                : file
            )))
          }}
          onSave={() => {
            void saveActiveFile()
          }}
          onDeploy={() => {
            void deployWorkspace()
          }}
          onCopy={() => {
            void navigator.clipboard.writeText(activeFile.content)
          }}
        />
      </div>
    </div>
  )
}
