import { useCallback, useEffect, useState } from 'react'
import { invoke } from '@tauri-apps/api/core'
import { open } from '@tauri-apps/plugin-dialog'

const errText = (e) => (typeof e === 'string' ? e : e?.message ?? String(e))

const WATCH_INTERVAL_MS = 3000

export default function WormholePanel({ project, rifts }) {
  const [wormholes, setWormholes] = useState([])
  const [activeId, setActiveId] = useState(null)
  const [detail, setDetail] = useState(null)
  const [name, setName] = useState('')
  const [picked, setPicked] = useState([])
  const [convergeName, setConvergeName] = useState('')
  const [watching, setWatching] = useState(false)
  const [lastUpdate, setLastUpdate] = useState(null)
  const [confirmDissolve, setConfirmDissolve] = useState(false)
  const [busy, setBusy] = useState(false)
  const [message, setMessage] = useState(null)

  const fail = (e) => setMessage({ kind: 'error', text: errText(e) })
  const succeed = (text) => setMessage({ kind: 'ok', text })

  const loadList = useCallback(async () => {
    if (!project) return []
    const list = await invoke('hub_list_wormholes', { projectId: project.id })
    setWormholes(list)
    return list
  }, [project])

  const loadDetail = useCallback(async (id) => {
    const d = await invoke('hub_show_wormhole', { wormholeId: id })
    setDetail(d)
    setLastUpdate(new Date())
    return d
  }, [])

  // A different project starts with a clean slate.
  useEffect(() => {
    setWormholes([])
    setActiveId(null)
    setDetail(null)
    setWatching(false)
    setPicked([])
    setConfirmDissolve(false)
    setMessage(null)
    loadList().catch(fail)
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [project?.id])

  // Live watch: refresh the open wormhole on an interval while watching.
  useEffect(() => {
    if (!watching || !activeId) return undefined
    const timer = setInterval(() => {
      loadDetail(activeId).catch(() => setWatching(false))
      loadList().catch(() => {})
    }, WATCH_INTERVAL_MS)
    return () => clearInterval(timer)
  }, [watching, activeId, loadDetail, loadList])

  const run = async (fn) => {
    setBusy(true)
    setMessage(null)
    try {
      return await fn()
    } catch (e) {
      fail(e)
      return undefined
    } finally {
      setBusy(false)
    }
  }

  const togglePicked = (riftId) =>
    setPicked((p) => (p.includes(riftId) ? p.filter((id) => id !== riftId) : [...p, riftId]))

  const createWormhole = () =>
    run(async () => {
      const trimmed = name.trim()
      if (!trimmed) throw new Error('Wormhole name is required')
      if (picked.length < 2) throw new Error('Pick at least two rifts')
      const created = await invoke('hub_create_wormhole', {
        projectId: project.id,
        name: trimmed,
        riftIds: picked,
      })
      setName('')
      setPicked([])
      await loadList()
      setActiveId(created.id)
      setConfirmDissolve(false)
      await loadDetail(created.id)
      succeed(`Created wormhole "${created.name}"`)
    })

  const openWormhole = (wormhole) =>
    run(async () => {
      setActiveId(wormhole.id)
      setWatching(false)
      setConfirmDissolve(false)
      setConvergeName('')
      await loadDetail(wormhole.id)
    })

  const resolve = (file, conflict, sideIndex) =>
    run(async () => {
      await invoke('hub_resolve_conflict', {
        wormholeId: activeId,
        path: file.path,
        conflictId: conflict.id,
        choice: { Side: sideIndex },
      })
      await loadDetail(activeId)
      succeed(`Resolved a conflict in ${file.path}`)
    })

  const converge = () =>
    run(async () => {
      const res = await invoke('hub_converge_wormhole', {
        wormholeId: activeId,
        name: convergeName.trim() || null,
      })
      setConvergeName('')
      await loadList()
      await loadDetail(activeId)
      succeed(
        `Converged into rift "${res.rift_name}": ${res.files_written} file(s) written, ${res.files_removed} removed`,
      )
    })

  const beam = () =>
    run(async () => {
      const parent = await open({ directory: true, multiple: false })
      if (typeof parent !== 'string') return
      const res = await invoke('hub_beam_wormhole', {
        wormholeId: activeId,
        parentDir: parent,
      })
      const unsafe = res.skipped_unsafe.length
      succeed(
        `Beamed ${res.written} file(s) to ${res.folder}${unsafe ? `. ${unsafe} unsafe path(s) skipped` : ''}`,
      )
    })

  const dissolve = () =>
    run(async () => {
      await invoke('hub_dissolve_wormhole', { wormholeId: activeId })
      setActiveId(null)
      setDetail(null)
      setWatching(false)
      setConfirmDissolve(false)
      await loadList()
      succeed('Wormhole dissolved')
    })

  const riftName = (id) => rifts.find((r) => r.id === id)?.name ?? id
  const conflictCount = detail
    ? detail.files.reduce((n, f) => n + f.conflicts.filter((c) => !c.resolved).length, 0)
    : 0
  const isActive = detail?.status === 'active'

  if (!project) {
    return (
      <section className="card">
        <h3>Wormholes</h3>
        <p className="muted">Select a project to see its wormholes.</p>
      </section>
    )
  }

  return (
    <section className="card">
      <h3>Wormholes in {project.name}</h3>
      {message && (
        <div className={`notice ${message.kind}`} role="status">
          <span>{message.text}</span>
          <button onClick={() => setMessage(null)} aria-label="Dismiss">×</button>
        </div>
      )}

      {wormholes.length === 0 && <p className="muted">No wormholes yet.</p>}
      <ul className="list">
        {wormholes.map((w) => (
          <li key={w.id} className={activeId === w.id ? 'selected' : ''}>
            <button className="link" onClick={() => openWormhole(w)} disabled={busy}>{w.name}</button>
            <span className={w.status === 'active' ? 'ok' : 'muted'}>{w.status}</span>
            {w.status === 'active' && (
              <span className={w.unresolved_conflicts ? 'bad' : 'muted'}>
                {w.unresolved_conflicts} conflict(s)
              </span>
            )}
          </li>
        ))}
      </ul>

      <h4>New wormhole</h4>
      <div className="form-row">
        <input placeholder="Name" value={name} onChange={(e) => setName(e.target.value)} />
        <button onClick={createWormhole} disabled={busy || picked.length < 2}>Create wormhole</button>
      </div>
      <div className="form-row">
        {rifts.length === 0 && <span className="muted">No rifts in this project.</span>}
        {rifts.map((r) => (
          <label key={r.id}>
            <input
              type="checkbox"
              checked={picked.includes(r.id)}
              onChange={() => togglePicked(r.id)}
            />{' '}
            {r.name}
          </label>
        ))}
      </div>

      {detail && (
        <div className="wormhole-detail">
          <h4>{detail.name}</h4>
          <p>
            <span className={isActive ? 'ok' : 'muted'}>{detail.status}</span>
            {' · '}
            Sides: {detail.sides.map((s) => s.rift_name).join(', ')}
            {' · '}
            <span className={conflictCount ? 'bad' : 'ok'}>{conflictCount} unresolved conflict(s)</span>
          </p>

          {isActive && (
            <label className="row">
              <input type="checkbox" checked={watching} onChange={(e) => setWatching(e.target.checked)} />
              Watch for changes (refreshes every 3 s)
              {watching && lastUpdate && <span className="muted">updated {lastUpdate.toLocaleTimeString()}</span>}
            </label>
          )}

          {detail.files.length === 0 && <p className="muted">No conflicts. The merge is clean.</p>}
          {detail.files.map((file) => (
            <div key={file.path} className="conflict-file">
              <code className="path">{file.path}</code>
              {file.conflicts.map((conflict) => (
                <div key={conflict.id} className="conflict">
                  <span className={conflict.resolved ? 'ok' : 'bad'}>
                    {conflict.resolved ? 'resolved' : 'unresolved'}
                  </span>
                  {isActive && !conflict.resolved && (
                    <div className="row">
                      {conflict.sides.map((_, i) => (
                        <button key={i} onClick={() => resolve(file, conflict, i)} disabled={busy}>
                          Use {detail.sides[i]?.rift_name ?? `side ${i}`}
                        </button>
                      ))}
                    </div>
                  )}
                  <pre className="conflict-preview">
                    {conflict.sides.map((lines, i) => (
                      <span key={i}>
                        {`--- ${detail.sides[i]?.rift_name ?? `side ${i}`}\n${lines.join('\n')}\n`}
                      </span>
                    ))}
                  </pre>
                </div>
              ))}
            </div>
          ))}

          <div className="form-row">
            {isActive && (
              <>
                <input
                  placeholder="New rift name (optional)"
                  value={convergeName}
                  onChange={(e) => setConvergeName(e.target.value)}
                />
                <button onClick={converge} disabled={busy || conflictCount > 0} title={conflictCount ? 'Resolve all conflicts first' : ''}>
                  Converge
                </button>
              </>
            )}
            {detail.converted_rift_id && <span className="muted">Converted to a rift</span>}
          </div>

          <div className="row">
            <button onClick={beam} disabled={busy || conflictCount > 0} title={conflictCount ? 'Resolve all conflicts first' : ''}>
              Beam to folder…
            </button>
            {isActive && !confirmDissolve && (
              <button className="danger" onClick={() => setConfirmDissolve(true)} disabled={busy}>Dissolve</button>
            )}
            {confirmDissolve && (
              <>
                <span>Dissolve this wormhole?</span>
                <button className="danger" onClick={dissolve} disabled={busy}>Yes</button>
                <button onClick={() => setConfirmDissolve(false)}>No</button>
              </>
            )}
          </div>
        </div>
      )}
    </section>
  )
}
