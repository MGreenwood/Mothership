import { useCallback, useEffect, useState } from 'react'
import { invoke } from '@tauri-apps/api/core'

const errText = (e) => (typeof e === 'string' ? e : e?.message ?? String(e))

export default function DaemonPanel() {
  const [panel, setPanel] = useState(null)
  const [busy, setBusy] = useState(false)
  const [message, setMessage] = useState(null)

  const load = useCallback(async () => {
    setPanel(await invoke('hub_daemon_status'))
  }, [])

  useEffect(() => {
    load().catch((e) => setMessage({ kind: 'error', text: errText(e) }))
  }, [load])

  const act = async (label, command) => {
    setBusy(true)
    setMessage(null)
    try {
      await invoke(command)
      setMessage({ kind: 'ok', text: label })
    } catch (e) {
      setMessage({ kind: 'error', text: errText(e) })
    } finally {
      await load().catch(() => {})
      setBusy(false)
    }
  }

  const running = panel?.running
  const status = panel?.status

  return (
    <section className="card">
      <h3>Daemon</h3>
      {message && (
        <div className={`notice ${message.kind}`} role="status">
          <span>{message.text}</span>
          <button onClick={() => setMessage(null)} aria-label="Dismiss">×</button>
        </div>
      )}
      {!panel ? (
        <p>Checking…</p>
      ) : (
        <>
          <p>
            {running ? <span className="ok">running</span> : <span className="bad">stopped</span>}
            {status && (
              <>
                {' · '}
                server: {status.server_connected ? <span className="ok">connected</span> : <span className="bad">disconnected</span>}
                {' · '}
                {status.projects_tracked} gateway(s) tracked, {status.files_syncing} file(s) syncing
                {' · '}
                last sync: {status.last_sync ? new Date(status.last_sync).toLocaleString() : 'never'}
              </>
            )}
          </p>
          {panel.error && <p className="bad">{panel.error}</p>}

          <div className="row">
            <button onClick={() => act('Daemon started', 'hub_daemon_start')} disabled={busy || running}>Start</button>
            <button onClick={() => act('Daemon stopped', 'hub_daemon_stop')} disabled={busy || !running}>Stop</button>
            <button onClick={() => act('Daemon restarted', 'hub_daemon_restart')} disabled={busy}>Restart</button>
            <button onClick={load} disabled={busy}>Refresh</button>
          </div>

          {panel.tracked.length > 0 && (
            <ul className="list">
              {panel.tracked.map((t) => (
                <li key={t.project_id}>
                  <span>{t.project_name}</span>
                  <span className="muted path" title={t.project_path}>{t.project_path}</span>
                </li>
              ))}
            </ul>
          )}
        </>
      )}
    </section>
  )
}
