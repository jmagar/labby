'use client'

import * as React from 'react'
import {
  Streamdown,
  defaultUrlTransform,
  type AllowElement,
  type UrlTransform,
} from 'streamdown'
import { cn } from '@/lib/utils'

const SAFE_MARKDOWN_IMAGE_ELEMENTS = ['img'] as const
const NO_REHYPE_PLUGINS: never[] = []
const DISABLED_LINK_SAFETY = { enabled: false } as const

function isAllowedMarkdownUrl(url: string) {
  const trimmed = url.trim()
  if (trimmed.startsWith('//')) return false

  const scheme = trimmed.match(/^([a-z][a-z0-9+.-]*):/i)?.[1]?.toLowerCase()
  return !scheme || scheme === 'http' || scheme === 'https' || scheme === 'mailto'
}

const safeMarkdownUrlTransform: UrlTransform = (url, key, node) => {
  const transformed = defaultUrlTransform(url, key, node)
  if (!transformed) return transformed

  return isAllowedMarkdownUrl(transformed) ? transformed : null
}

const allowSafeMarkdownElement: AllowElement = (element) => {
  if (element.tagName === 'img') return false

  if (element.tagName === 'a') {
    const href = element.properties?.href
    return typeof href === 'string' && isAllowedMarkdownUrl(href)
  }

  return true
}

export type SafeMarkdownProps = {
  /**
   * Renders untrusted markdown with raw HTML skipped, images disallowed,
   * links limited to relative/http/https/mailto URLs, and Streamdown controls disabled.
   */
  text: string
  isStreaming?: boolean
  className?: string
}

function StreamingCursor() {
  return (
    <span
      aria-hidden="true"
      className="ml-0.5 inline-block h-3.5 w-0.5 animate-pulse rounded-sm bg-aurora-accent-primary align-middle"
    />
  )
}

export function SafeMarkdown({
  text,
  isStreaming = false,
  className,
}: SafeMarkdownProps) {
  return (
    <div
      className={cn(
        'min-w-0 max-w-full text-[13px] leading-[1.55] text-aurora-text-primary [overflow-wrap:anywhere] [&_a]:break-words [&_code]:break-words [&_li]:min-w-0 [&_ol]:min-w-0 [&_pre]:max-w-full [&_pre]:overflow-x-auto [&_pre]:[overflow-wrap:normal] [&_pre]:whitespace-pre [&_table]:block [&_table]:max-w-full [&_table]:overflow-x-auto [&_ul]:min-w-0',
        className,
      )}
    >
      <Streamdown
        mode={isStreaming ? 'streaming' : 'static'}
        skipHtml
        rehypePlugins={NO_REHYPE_PLUGINS}
        disallowedElements={SAFE_MARKDOWN_IMAGE_ELEMENTS}
        allowElement={allowSafeMarkdownElement}
        urlTransform={safeMarkdownUrlTransform}
        controls={false}
        linkSafety={DISABLED_LINK_SAFETY}
        lineNumbers={false}
      >
        {text}
      </Streamdown>
      {isStreaming ? <StreamingCursor /> : null}
    </div>
  )
}
