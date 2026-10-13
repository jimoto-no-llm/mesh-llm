import { describe, expect, it } from 'vitest'
import { isLoopbackConsoleHost, resolveLiveTopNavData } from './shell-adapter'
import type { StatusPayload } from '@/lib/api/types'

const statusWithoutToken = { api_port: 9337, node_state: 'serving' } as StatusPayload

function hints(hostname: string) {
  return resolveLiveTopNavData(statusWithoutToken, hostname).topNavJoinCommands.map(
    (command) => command.hint ?? ''
  )
}

describe('isLoopbackConsoleHost', () => {
  it('treats loopback hosts as local', () => {
    for (const host of ['localhost', '127.0.0.1', '127.1.2.3', '::1', '[::1]', 'LOCALHOST']) {
      expect(isLoopbackConsoleHost(host)).toBe(true)
    }
  })

  it('treats a remote host as remote', () => {
    for (const host of ['192.168.0.10', 'meshllm.cloud', '10.0.0.1', '127.0.0.1.example.com']) {
      expect(isLoopbackConsoleHost(host)).toBe(false)
    }
  })

  it('keeps the neutral wording when no window is available', () => {
    expect(isLoopbackConsoleHost(undefined)).toBe(true)
    expect(isLoopbackConsoleHost('')).toBe(true)
  })
})

describe('unavailable invite token hints', () => {
  it('points a remote console at the node machine', () => {
    const [tokenHint, autoHint, clientHint] = hints('192.168.0.10')
    expect(tokenHint).toContain('trusted local session')
    expect(tokenHint).not.toContain('has not reported')
    expect(autoHint).toContain('machine running the node')
    expect(clientHint).toContain('machine running the node')
  })

  it('keeps the not-issued wording for a local console', () => {
    const [tokenHint, autoHint, clientHint] = hints('localhost')
    expect(tokenHint).toBe('Live /api/status has not reported an invite token yet.')
    expect(autoHint).toBe(
      'This command becomes available after the backend issues a live invite token.'
    )
    expect(clientHint).toBe(
      'This command becomes available after the backend issues a live invite token.'
    )
  })

  it('still shows the token when the backend discloses one', () => {
    const commands = resolveLiveTopNavData(
      { api_port: 9337, node_state: 'serving', token: 'invite-abc' } as StatusPayload,
      '192.168.0.10'
    ).topNavJoinCommands
    expect(commands.some((command) => command.value === 'invite-abc')).toBe(true)
  })
})
