import { useEffect, useState } from 'react'
import { invoke } from '@tauri-apps/api/core'

export default function SecretsPanel({ project }) {
  const [holds, setHolds] = useState([])
  const [status, setStatus] = useState(null)
  const [busyPath, setBusyPath] = useState(null)

  const load = async () => {
    if (!project) return
    try {
      setHolds(await invoke('hub_secret_holds', { projectId: project.id }))
      setStatus(null)
    } catch (err) {
      setStatus(String(err))
    }
  }

  useEffect(() => {
    setHolds([])
    load()
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [project?.id])

  const allow = async (path) => {
    setBusyPath(path)
    setStatus(null)
    try {
      await invoke('hub_allow_secret', { projectId: project.id, path })
      await load()
    } catch (err) {
      setStatus(String(err))
    } finally {
      setBusyPath(null)
    }
  }

  if (!project) {
    return <section className="card"><h3>Secrets</h3><p className="muted">Pick a gateway first.</p></section>
  }

  return (
    <section className="card">
      <h3>Secrets</h3>
      <p className="muted">
        Files that contain a secret are kept on this machine until you allow them. The secret itself is never shown here.
      </p>
      <div className="form-row">
        <button onClick={load}>Refresh</button>
      </div>

      {status && <p className="bad">{status}</p>}
      {holds.length === 0 && !status && <p className="muted">No files are held back.</p>}

      <ul className="list">
        {holds.map((hold) => (
          <li key={hold.path}>
            <span className="path">{hold.path}</span>
            <span className="muted">
              {hold.findings.map((f) => `line ${f.line}: ${f.kind}`).join('; ')}
            </span>
            <button onClick={() => allow(hold.path)} disabled={busyPath === hold.path}>
              Allow syncing
            </button>
          </li>
        ))}
      </ul>
    </section>
  )
}
