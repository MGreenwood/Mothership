import { useEffect, useRef } from 'react'
import { Terminal } from '@xterm/xterm'
import { FitAddon } from '@xterm/addon-fit'
import '@xterm/xterm/css/xterm.css'
import { invoke } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'

function base64ToBytes(b64) {
  const bin = atob(b64)
  const bytes = new Uint8Array(bin.length)
  for (let i = 0; i < bin.length; i++) bytes[i] = bin.charCodeAt(i)
  return bytes
}

export default function TerminalTab({ id, kind = 'nvim', file, cwd, active, onExit, onFailed, onError }) {
  const hostRef = useRef(null)
  const termRef = useRef(null)
  const fitRef = useRef(null)
  const onExitRef = useRef(onExit)
  const onFailedRef = useRef(onFailed)
  const onErrorRef = useRef(onError)
  onExitRef.current = onExit
  onFailedRef.current = onFailed
  onErrorRef.current = onError

  useEffect(() => {
    const term = new Terminal({
      cursorBlink: true,
      fontFamily: 'Consolas, "Cascadia Mono", monospace',
      fontSize: 14,
      scrollback: 0,
    })
    const fit = new FitAddon()
    term.loadAddon(fit)
    term.open(hostRef.current)
    termRef.current = term
    fitRef.current = fit

    let disposed = false
    const unlisteners = []

    const syncSize = () => {
      fit.fit()
      invoke('nvim_resize', { id, cols: term.cols, rows: term.rows }).catch(() => {})
    }

    term.onData((data) => {
      invoke('nvim_write', { id, data }).catch((err) => {
        if (!disposed) onErrorRef.current?.(`Keystrokes were not sent to the editor: ${err}`)
      })
    })

    const resizeObserver = new ResizeObserver(() => {
      if (hostRef.current?.offsetParent) syncSize()
    })
    resizeObserver.observe(hostRef.current)

    // Listeners are registered before nvim starts so early output is not lost.
    Promise.all([
      listen('nvim-output', (event) => {
        if (event.payload.id === id) term.write(base64ToBytes(event.payload.data))
      }),
      listen('nvim-exit', (event) => {
        if (event.payload.id === id) onExitRef.current(id)
      }),
    ])
      .then((fns) => {
        if (disposed) fns.forEach((fn) => fn())
        else unlisteners.push(...fns)
      })
      .then(() => {
        fit.fit()
        if (kind === 'shell') {
          return invoke('terminal_open', {
            id,
            cwd: cwd ?? null,
            cols: term.cols,
            rows: term.rows,
          })
        }
        return invoke('nvim_open', {
          id,
          file: file ?? null,
          cwd: cwd ?? null,
          cols: term.cols,
          rows: term.rows,
        })
      })
      .catch((err) => {
        if (!disposed) onFailedRef.current(id, String(err))
      })

    return () => {
      disposed = true
      resizeObserver.disconnect()
      unlisteners.forEach((fn) => fn())
      term.dispose()
      termRef.current = null
      fitRef.current = null
    }
  }, [id])

  useEffect(() => {
    if (active && fitRef.current && termRef.current) {
      fitRef.current.fit()
      invoke('nvim_resize', {
        id,
        cols: termRef.current.cols,
        rows: termRef.current.rows,
      }).catch(() => {})
      termRef.current.focus()
    }
  }, [active, id])

  return (
    <div
      className="terminal-host"
      ref={hostRef}
      style={{ display: active ? 'block' : 'none' }}
    />
  )
}
