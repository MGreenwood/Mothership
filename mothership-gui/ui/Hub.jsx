import { useCallback, useEffect, useState } from 'react'
import { invoke } from '@tauri-apps/api/core'
import { ask, open } from '@tauri-apps/plugin-dialog'
import DaemonPanel from './DaemonPanel.jsx'
import WormholePanel from './WormholePanel.jsx'

const errText = (e) => (typeof e === 'string' ? e : e?.message ?? String(e))

export default function Hub({ onOpenInEditor }) {
  const [health, setHealth] = useState(null)
  const [projects, setProjects] = useState([])
  const [selected, setSelected] = useState(null)
  const [rifts, setRifts] = useState([])
  const [checkpoints, setCheckpoints] = useState([])
  const [message, setMessage] = useState(null)
  const [busy, setBusy] = useState(false)
  const [newProject, setNewProject] = useState({ name: '', description: '', folder: '' })
  const [newRiftName, setNewRiftName] = useState('')
  const [checkpointMsg, setCheckpointMsg] = useState('')
  const [restoreId, setRestoreId] = useState(null)
  const [auth, setAuth] = useState(null)
  const [authProvider, setAuthProvider] = useState(null)
  const [authToken, setAuthToken] = useState('')

  const run = async (label, fn) => {
    setBusy(true)
    setMessage(null)
    try {
      const result = await fn()
      if (label) setMessage({ kind: 'ok', text: label })
      return result
    } catch (e) {
      setMessage({ kind: 'error', text: errText(e) })
      return undefined
    } finally {
      setBusy(false)
    }
  }

  const loadProjects = useCallback(async () => {
    const list = await invoke('hub_list_projects')
    setProjects(list)
    return list
  }, [])

  const loadHealth = useCallback(async () => {
    setHealth(await invoke('hub_server_info'))
  }, [])

  const loadProjectDetails = useCallback(async (project) => {
    setSelected(project)
    const id = project.id
    const [r, c] = await Promise.all([
      invoke('hub_list_rifts', { projectId: id }),
      invoke('hub_checkpoint_history', { projectId: id }),
    ])
    setRifts(r)
    setCheckpoints(c)
  }, [])

  const loadAuth = useCallback(async () => {
    setAuth(await invoke('hub_auth_status'))
  }, [])

  useEffect(() => {
    run(null, async () => {
      await loadAuth()
      await loadHealth()
      await loadProjects()
    })
    invoke('hub_ensure_daemon').catch((e) =>
      setMessage({ kind: 'error', text: `Daemon: ${errText(e)}` }),
    )
  }, [loadAuth, loadHealth, loadProjects])

  const signIn = (provider) =>
    run(`Opened ${provider} sign-in in your browser. Paste the code it shows below.`, async () => {
      setAuthProvider(provider)
      await invoke('hub_auth_start', { provider })
    })

  const completeSignIn = () =>
    run('Signed in', async () => {
      await invoke('hub_auth_complete', { token: authToken })
      setAuthToken('')
      setAuthProvider(null)
      await loadAuth()
      await loadHealth()
      await loadProjects()
    })

  const [confirmSignOut, setConfirmSignOut] = useState(false)

  const signOut = () =>
    run('Signed out', async () => {
      await invoke('hub_auth_logout')
      setConfirmSignOut(false)
      await loadAuth()
    })

  const refreshAll = () =>
    run('Refreshed', async () => {
      await loadHealth()
      await loadProjects()
      if (selected) await loadProjectDetails(selected)
    })

  const pickFolder = async () => {
    const path = await open({ directory: true, multiple: false })
    if (typeof path === 'string') setNewProject((p) => ({ ...p, folder: path }))
  }

  const createProject = () =>
    run(null, async () => {
      const { name, description, folder } = newProject
      if (!name.trim() || !folder) throw new Error('Name and folder are required')
      const res = await invoke('hub_create_project', {
        name: name.trim(),
        description: description.trim(),
        folder,
      })
      setNewProject({ name: '', description: '', folder: '' })
      await loadProjects()
      setMessage({
        kind: 'ok',
        text: `Created "${res.project.name}": uploaded ${res.uploaded_files} file(s), skipped ${res.skipped_too_large} too large, ${res.skipped_binary} binary, ${res.skipped_unreadable} unreadable.`,
      })
    })

  const beamProject = async (project) => {
    let localDir = null
    if (!project.local_path) {
      const parent = await open({ directory: true, multiple: false })
      if (typeof parent !== 'string') return
      localDir = parent
    }
    setBusy(true)
    setMessage(null)
    try {
      const res = await invoke('hub_beam', { projectId: project.id, localDir })
      await loadProjects()
      const parts = [
        `Beamed "${res.project_name}" to ${res.local_path}: ${res.downloaded} file(s) downloaded`,
      ]
      if (res.skipped_unsafe.length) parts.push(`${res.skipped_unsafe.length} unsafe path(s) skipped`)
      setMessage({
        kind: res.daemon_warning ? 'error' : 'ok',
        text: `${parts.join('. ')}.${res.daemon_warning ? ` Daemon: ${res.daemon_warning}` : ''}`,
      })
      onOpenInEditor?.(res.local_path)
    } catch (e) {
      setMessage({ kind: 'error', text: errText(e) })
    } finally {
      setBusy(false)
    }
  }

  const deleteProject = async (project) => {
    const yes = await ask(
      `Delete "${project.name}" from the server? This removes the server copy only. Local files are not touched.`,
      { title: 'Delete project', kind: 'warning' },
    )
    if (!yes) return
    await run(`Deleted "${project.name}" from the server`, async () => {
      await invoke('hub_delete_project', { projectId: project.id })
      if (selected?.id === project.id) {
        setSelected(null)
        setRifts([])
        setCheckpoints([])
      }
      await loadProjects()
    })
  }

  const createRift = () =>
    run(`Created rift "${newRiftName.trim()}"`, async () => {
      if (!newRiftName.trim()) throw new Error('Rift name is required')
      await invoke('hub_create_rift', {
        projectId: selected.id,
        name: newRiftName.trim(),
        description: null,
      })
      setNewRiftName('')
      setRifts(await invoke('hub_list_rifts', { projectId: selected.id }))
    })

  const switchRift = (rift) =>
    run(`Switched server to rift "${rift.name}". Daemon reconnect is not wired yet.`, async () => {
      await invoke('hub_switch_rift', { projectId: selected.id, riftName: rift.name })
    })

  const createCheckpoint = () =>
    run('Checkpoint created', async () => {
      await invoke('hub_create_checkpoint', {
        projectId: selected.id,
        message: checkpointMsg.trim() || null,
      })
      setCheckpointMsg('')
      setCheckpoints(await invoke('hub_checkpoint_history', { projectId: selected.id }))
    })

  const restoreCheckpoint = (checkpointId) =>
    run('', async () => {
      const result = await invoke('hub_restore_checkpoint', {
        projectId: selected.id,
        checkpointId,
      })
      setRestoreId(null)
      const skipped = result.skipped_unsafe.length
      setMessage({
        kind: 'ok',
        text: `Restored ${result.written} file(s) into ${result.local_path}` +
          (skipped ? `. Skipped ${skipped} unsafe path(s).` : '.') +
          ' The daemon will sync these as new changes to your rift.',
      })
    })

  return (
    <div className="hub">
      <header className="hub-header">
        <h2>Mothership hub</h2>
        <button onClick={refreshAll} disabled={busy}>Refresh</button>
      </header>

      {message && (
        <div className={`notice ${message.kind}`} role="status">
          <span>{message.text}</span>
          <button onClick={() => setMessage(null)} aria-label="Dismiss">×</button>
        </div>
      )}

      <section className="card">
        <h3>Sign in</h3>
        {auth?.signed_in ? (
          <>
            <p><span className="ok">Signed in</span> to <code>{auth.server_url}</code></p>
            {confirmSignOut ? (
              <div className="row">
                <span>Sign out? The stored token will be removed.</span>
                <button onClick={signOut} disabled={busy}>Yes</button>
                <button onClick={() => setConfirmSignOut(false)}>No</button>
              </div>
            ) : (
              <button onClick={() => setConfirmSignOut(true)}>Sign out</button>
            )}
          </>
        ) : (
          <>
            <p className="muted">
              Pick a provider. Your browser opens, and after you approve, it shows a code. Paste that code here.
            </p>
            <div className="row">
              <button onClick={() => signIn('google')} disabled={busy}>Sign in with Google</button>
              <button onClick={() => signIn('github')} disabled={busy}>Sign in with GitHub</button>
            </div>
            {authProvider && (
              <div className="row">
                <input
                  type="password"
                  placeholder="Paste code from browser"
                  value={authToken}
                  onChange={(e) => setAuthToken(e.target.value)}
                />
                <button onClick={completeSignIn} disabled={busy || !authToken.trim()}>Save token</button>
              </div>
            )}
          </>
        )}
      </section>

      <section className="card">
        <h3>Server</h3>
        {health ? (
          <>
            <p>
              <code>{health.server_url}</code>{' '}
              {health.reachable ? <span className="ok">reachable</span> : <span className="bad">unreachable: {health.error}</span>}
            </p>
            {health.reachable && (
              <p className="muted">
                {health.projects} project(s) · {health.rifts} rift(s) · {health.active_wormholes} active wormhole(s)
              </p>
            )}
          </>
        ) : (
          <p>Checking…</p>
        )}
      </section>

      <DaemonPanel />

      <section className="card">
        <h3>Projects</h3>
        {projects.length === 0 && <p className="muted">No projects on the server.</p>}
        <ul className="list">
          {projects.map((p) => (
            <li key={p.id} className={selected?.id === p.id ? 'selected' : ''}>
              <button className="link" onClick={() => run(null, () => loadProjectDetails(p))}>
                {p.name}
              </button>
              {p.local_path ? (
                <span className="ok" title={p.local_path}>local</span>
              ) : (
                <span className="bad">remote</span>
              )}
              <button onClick={() => beamProject(p)} disabled={busy}>Beam</button>
              <button className="danger" onClick={() => deleteProject(p)} disabled={busy}>
                Delete
              </button>
            </li>
          ))}
        </ul>

        <h4>New project from folder</h4>
        <div className="form-row">
          <input
            placeholder="Name"
            value={newProject.name}
            onChange={(e) => setNewProject({ ...newProject, name: e.target.value })}
          />
          <input
            placeholder="Description"
            value={newProject.description}
            onChange={(e) => setNewProject({ ...newProject, description: e.target.value })}
          />
          <button onClick={pickFolder}>Choose folder…</button>
          <span className="muted path">{newProject.folder || 'No folder selected'}</span>
          <button onClick={createProject} disabled={busy}>Create project</button>
        </div>
      </section>

      {selected && (
        <>
          <section className="card">
            <h3>Rifts in {selected.name}</h3>
            <p className="muted path">
              {projects.find((p) => p.id === selected.id)?.local_path ?? 'Remote only: not beamed to this machine'}
            </p>
            <ul className="list">
              {rifts.map((r) => (
                <li key={r.id}>
                  <span>{r.name}</span>
                  <span className="muted">{r.file_count} file(s){r.is_conflict_rift ? ' · conflict rift' : ''}</span>
                  <button onClick={() => switchRift(r)} disabled={busy}>Switch</button>
                </li>
              ))}
            </ul>
            <div className="form-row">
              <input
                placeholder="New rift name"
                value={newRiftName}
                onChange={(e) => setNewRiftName(e.target.value)}
              />
              <button onClick={createRift} disabled={busy}>New rift</button>
            </div>
          </section>

          <section className="card">
            <h3>Checkpoints</h3>
            <div className="form-row">
              <input
                placeholder="Checkpoint message (optional)"
                value={checkpointMsg}
                onChange={(e) => setCheckpointMsg(e.target.value)}
              />
              <button onClick={createCheckpoint} disabled={busy}>Create checkpoint</button>
            </div>
            <ul className="list">
              {checkpoints.map((c) => (
                <li key={c.id}>
                  <span>{c.message || '(no message)'}</span>
                  <span className="muted">{c.timestamp}</span>
                  {restoreId === c.id ? (
                    <span className="row">
                      <span className="bad">Overwrite local files with this checkpoint?</span>
                      <button className="danger" onClick={() => restoreCheckpoint(c.id)} disabled={busy}>Yes</button>
                      <button onClick={() => setRestoreId(null)}>No</button>
                    </span>
                  ) : (
                    <button onClick={() => setRestoreId(c.id)} disabled={busy}>Restore</button>
                  )}
                </li>
              ))}
            </ul>
          </section>

          <WormholePanel project={selected} rifts={rifts} />
        </>
      )}
    </div>
  )
}
