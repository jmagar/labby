export interface ParsedStdioCommandLine {
  command: string
  args: string[]
}

function isEnvAssignment(token: string): boolean {
  return /^[A-Za-z_][A-Za-z0-9_]*=/.test(token)
}

export function parseStdioCommandLine(input: string): ParsedStdioCommandLine {
  const tokens: string[] = []
  let token = ''
  let quote: "'" | '"' | null = null
  let escaped = false
  let tokenStarted = false

  for (const char of input.trim()) {
    if (escaped) {
      token += char
      tokenStarted = true
      escaped = false
      continue
    }

    if (char === '\\' && quote !== "'") {
      escaped = true
      tokenStarted = true
      continue
    }

    if ((char === "'" || char === '"') && quote === null) {
      quote = char
      tokenStarted = true
      continue
    }

    if (char === quote) {
      quote = null
      tokenStarted = true
      continue
    }

    if (/\s/.test(char) && quote === null) {
      if (tokenStarted) {
        tokens.push(token)
        token = ''
        tokenStarted = false
      }
      continue
    }

    token += char
    tokenStarted = true
  }

  if (escaped) {
    throw new Error('Command cannot end with a trailing escape')
  }
  if (quote) {
    throw new Error('Command has an unterminated quote')
  }
  if (tokenStarted) {
    tokens.push(token)
  }
  if (!tokens[0]) {
    throw new Error('Command is required')
  }

  if (isEnvAssignment(tokens[0]) && tokens.length > 1) {
    return {
      command: 'env',
      args: tokens,
    }
  }

  return {
    command: tokens[0],
    args: tokens.slice(1),
  }
}

function quoteToken(token: string): string {
  if (/^[^\s'"\\]+$/.test(token)) {
    return token
  }
  return `'${token.replaceAll("'", "'\\''")}'`
}

export function formatStdioCommandLine(command?: string | null, args?: string[]): string {
  const parts = [command, ...(args ?? [])].filter((part): part is string => Boolean(part))
  return parts.map(quoteToken).join(' ')
}
