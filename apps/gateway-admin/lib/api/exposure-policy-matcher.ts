/**
 * Shared, pure exposure-policy wildcard matcher.
 *
 * This module is the single canonical implementation of the exposure-policy
 * pattern-matching semantics used by both the mock preview path (client) and
 * the real preview path (server adapter). Keeping the logic here ensures that
 * local previews always produce results identical to production behavior.
 *
 * The algorithm is a TypeScript port of the Rust `wildcard_matches` function
 * in `crates/lab/src/dispatch/upstream/types.rs`. When updating either
 * implementation, update both.
 *
 * Supported pattern syntax:
 * - `*`            — match any tool
 * - `prefix_*`     — match tools whose name starts with `prefix_`
 * - `*_suffix`     — match tools whose name ends with `_suffix`
 * - `a_*_b`        — match tools containing `a_` anywhere before `_b`
 * - exact strings  — literal name match
 */

import type { ExposurePolicyPreview } from '../types/gateway.ts'

/**
 * Return true when `toolName` matches `pattern`.
 *
 * The pattern may contain one or more `*` wildcards. `*` alone matches
 * everything. Anchoring rules:
 * - If the pattern does not start with `*`, the first non-wildcard segment
 *   must appear at the start of the tool name.
 * - If the pattern does not end with `*`, the last non-wildcard segment must
 *   appear at the end of the tool name.
 */
export function matchPattern(toolName: string, pattern: string): boolean {
  if (pattern === '*') {
    return true
  }

  const parts = pattern.split('*')
  if (parts.length === 1) {
    // No wildcard — exact match.
    return pattern === toolName
  }

  const anchoredStart = !pattern.startsWith('*')
  const anchoredEnd = !pattern.endsWith('*')
  const nonEmptyParts = parts.filter((part) => part.length > 0)

  if (nonEmptyParts.length === 0) {
    // Pattern is all wildcards (e.g. "**") — matches everything.
    return true
  }

  let cursor = 0
  for (const [index, part] of nonEmptyParts.entries()) {
    if (index === 0 && anchoredStart) {
      if (!toolName.slice(cursor).startsWith(part)) {
        return false
      }
      cursor += part.length
      continue
    }

    const found = toolName.slice(cursor).indexOf(part)
    if (found === -1) {
      return false
    }
    cursor += found + part.length
  }

  if (anchoredEnd) {
    const last = nonEmptyParts[nonEmptyParts.length - 1]!
    return toolName.endsWith(last)
  }

  return true
}

/**
 * Preview which tools would be exposed under a given allowlist of patterns.
 *
 * An empty `patterns` array means "expose all" (equivalent to `*`).
 *
 * @param toolNames - All discovered tool names for the upstream.
 * @param patterns  - The allowlist patterns to evaluate.
 * @returns         A preview describing matched, filtered, and unmatched items.
 */
export function previewExposurePolicy(
  toolNames: string[],
  patterns: string[],
): ExposurePolicyPreview {
  if (patterns.length === 0) {
    return {
      matched_tools: toolNames.map((name) => ({ name, matched_by: '*' })),
      unmatched_patterns: [],
      filtered_tools: [],
      exposed_count: toolNames.length,
      filtered_count: 0,
    }
  }

  const matched_tools: ExposurePolicyPreview['matched_tools'] = []
  const filtered_tools: string[] = []
  const usedPatterns = new Set<string>()

  for (const toolName of toolNames) {
    let matchedBy: string | null = null
    for (const pattern of patterns) {
      if (matchPattern(toolName, pattern)) {
        matchedBy = pattern
        usedPatterns.add(pattern)
        break
      }
    }

    if (matchedBy) {
      matched_tools.push({ name: toolName, matched_by: matchedBy })
    } else {
      filtered_tools.push(toolName)
    }
  }

  return {
    matched_tools,
    unmatched_patterns: patterns.filter((pattern) => !usedPatterns.has(pattern)),
    filtered_tools,
    exposed_count: matched_tools.length,
    filtered_count: filtered_tools.length,
  }
}
