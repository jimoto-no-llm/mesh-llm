import { SHELL_HARNESS } from '@/features/app-tabs/data'
import type { LinkItem, ShellHarnessData, TopNavJoinCommand } from '@/features/app-tabs/types'
import type { StatusPayload } from '@/lib/api/types'
import { env } from '@/lib/env'
import { isPublicMesh } from '@/lib/api/mesh-visibility'

export type TopNavShellData = {
  apiUrl: string
  topNavApiAccessLinks: LinkItem[]
  topNavJoinCommands: TopNavJoinCommand[]
  topNavJoinLinks: LinkItem[]
}

const LOCAL_API_HOSTS = new Set(['localhost', '127.0.0.1', '0.0.0.0'])
const LOOPBACK_CONSOLE_HOSTS = new Set(['localhost', '127.0.0.1', '::1', '[::1]'])

/**
 * Mirrors the server's trusted-local disclosure closely enough for console
 * hints: the live invite token only reaches loopback sessions. An unknown
 * host (no `window`, e.g. SSR or a test) is treated as local so the neutral
 * "not issued yet" wording is preserved.
 */
export function isLoopbackConsoleHost(hostname: string | undefined): boolean {
  if (hostname === undefined) return true
  const normalized = hostname.trim().toLowerCase()
  if (normalized === '') return true
  if (LOOPBACK_CONSOLE_HOSTS.has(normalized)) return true
  return /^127\.\d{1,3}\.\d{1,3}\.\d{1,3}$/.test(normalized)
}

function currentConsoleHostname(): string | undefined {
  if (typeof window === 'undefined') return undefined
  return window.location.hostname
}

function normalizeOpenAIBaseUrl(value: string) {
  const trimmed = value.trim().replace(/\/+$/, '')
  return trimmed.endsWith('/v1') ? trimmed : `${trimmed}/v1`
}

function isLocalApiUrl(value: string) {
  try {
    return LOCAL_API_HOSTS.has(new URL(value).hostname)
  } catch {
    return false
  }
}

function browserOrigin() {
  if (typeof window === 'undefined') return null
  return window.location.origin
}

export function resolveOpenAIBaseUrl(status?: StatusPayload) {
  if (typeof window !== 'undefined' && LOCAL_API_HOSTS.has(window.location.hostname)) {
    return normalizeOpenAIBaseUrl(`http://127.0.0.1:${status?.api_port ?? 9337}`)
  }

  if (env.isDevelopment) {
    return normalizeOpenAIBaseUrl(env.apiUrl)
  }

  const origin = browserOrigin()
  if (origin && isLocalApiUrl(env.apiUrl)) {
    return normalizeOpenAIBaseUrl(origin)
  }

  return normalizeOpenAIBaseUrl(env.apiUrl)
}

function resolveInviteToken(status?: StatusPayload) {
  return status?.token?.trim() || null
}

function buildAvailableJoinCommands(inviteToken: string): TopNavJoinCommand[] {
  return [
    {
      label: 'Invite token',
      value: inviteToken,
      hint: 'Use the issued live token from /api/status.',
      noWrapValue: true
    },
    {
      label: 'Auto join and serve command',
      value: `mesh-llm --auto --join ${inviteToken}`,
      prefix: '$',
      hint: 'Matches the Connect panel flow: join, select a model, and serve the API.'
    },
    { label: 'Client-only join command', value: `mesh-llm client --join ${inviteToken}`, prefix: '$' }
  ]
}

function buildPublicJoinCommands(): TopNavJoinCommand[] {
  return [
    {
      label: 'Public mesh command',
      value: 'mesh-llm --auto',
      prefix: '$',
      hint: 'Join public discovery, auto-select a model, and serve the local API.'
    }
  ]
}

function buildUnavailableJoinCommands(remoteSession: boolean): TopNavJoinCommand[] {
  // A remote console never receives the token, so "not issued yet" would read
  // as "wait here" and never resolve. Say what actually unblocks the operator.
  const tokenHint = remoteSession
    ? 'The live invite token is only disclosed to a trusted local session. Open this console on the machine running the node.'
    : 'Live /api/status has not reported an invite token yet.'
  const commandHint = remoteSession
    ? 'Open the console on the machine running the node to read the live invite token.'
    : 'This command becomes available after the backend issues a live invite token.'
  return [
    {
      label: 'Invite token',
      value: 'Invite token unavailable',
      hint: tokenHint,
      noWrapValue: true,
      disabled: true
    },
    {
      label: 'Auto join and serve command',
      value: 'Auto join command unavailable',
      prefix: '$',
      hint: commandHint,
      disabled: true
    },
    {
      label: 'Client-only join command',
      value: 'Client-only join command unavailable',
      prefix: '$',
      hint: commandHint,
      disabled: true
    }
  ]
}

export function resolveHarnessTopNavData(data: ShellHarnessData): TopNavShellData {
  return {
    apiUrl: env.apiUrl,
    topNavApiAccessLinks: data.topNavApiAccessLinks,
    topNavJoinCommands: data.topNavJoinCommands,
    topNavJoinLinks: data.topNavJoinLinks
  }
}

export function resolveLiveTopNavData(
  status?: StatusPayload,
  hostname: string | undefined = currentConsoleHostname()
): TopNavShellData {
  const inviteToken = resolveInviteToken(status)
  const topNavJoinCommands =
    status && isPublicMesh(status)
      ? buildPublicJoinCommands()
      : inviteToken
        ? buildAvailableJoinCommands(inviteToken)
        : buildUnavailableJoinCommands(!isLoopbackConsoleHost(hostname))

  return {
    apiUrl: resolveOpenAIBaseUrl(status),
    topNavApiAccessLinks: SHELL_HARNESS.topNavApiAccessLinks,
    topNavJoinCommands,
    topNavJoinLinks: SHELL_HARNESS.topNavJoinLinks
  }
}
