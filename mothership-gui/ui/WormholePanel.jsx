import { useCallback, useEffect, useMemo, useState } from 'react'
import { invoke } from '@tauri-apps/api/core'
import { open } from '@tauri-apps/plugin-dialog'
import { countChanges, diffLines, MAX_DIFF_LINES } from './lineDiff'

const errText = (e) => (typeof e === 'string' ? e : e?.message ?? String(e))

const WATCH_INTERVAL_MS = 3000

export default function WormholePanel({ project, rifts, onRiftsChanged, onBeamRift }) {
  const [wormholes, setWormholes] = useState([])
  const [activeId, setActiveId] = useState(null)
  const [detail, setDetail] = useState(null)
  const [name, setName] = useState('')
  const [picked, setPicked] = useState([])
  const [convergeName, setConvergeName] = useState('')
  const [watching, setWatching] = useState(false)
  const [lastUpdate, setLastUpdate] = useState(null)
  const [confirmDissolve, setConfirmDissolve] = useState(false)
  const [confirmDelete, setConfirmDelete] = useState(false)
  const [busy, setBusy] = useState(false)
  const [message, setMessage] = useState(null)
  const [changes, setChanges] = useState(null)
  const [showChanges, setShowChanges] = useState(false)

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
      setChanges(null)
      setShowChanges(false)
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
      await onRiftsChanged?.()
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
      const text = `Beamed ${res.written} file(s) to ${res.folder}${unsafe ? `. ${unsafe} unsafe path(s) skipped` : ''}`
      if (res.daemon_warning) {
        setMessage({
          kind: 'error',
          text: `${text}. The daemon will not clean this snapshot up automatically: ${res.daemon_warning}`,
        })
      } else {
        succeed(text)
      }
    })

  const beamConvertedRift = () => {
    if (!detail?.converted_rift_id) return
    onBeamRift?.(detail.converted_rift_id)
  }

  const dissolve = () =>
    run(async () => {
      await invoke('hub_dissolve_wormhole', { wormholeId: activeId })
      setActiveId(null)
      setDetail(null)
      setWatching(false)
      setConfirmDissolve(false)
      setConfirmDelete(false)
      setChanges(null)
      setShowChanges(false)
      await loadList()
      succeed('Wormhole dissolved')
    })

  const deleteWormhole = () =>
    run(async () => {
      await invoke('hub_delete_wormhole', { wormholeId: activeId })
      setActiveId(null)
      setDetail(null)
      setWatching(false)
      setConfirmDissolve(false)
      setConfirmDelete(false)
      setChanges(null)
      setShowChanges(false)
      await loadList()
      succeed('Wormhole permanently deleted')
    })

  const toggleSide = (side) =>
    run(async () => {
      const updated = await invoke('hub_toggle_wormhole_rift', {
        wormholeId: activeId,
        riftId: side.rift_id,
        enabled: !side.enabled,
      })
      setDetail(updated)
      await loadList()
      succeed(
        `Rift "${side.rift_name}" ${!side.enabled ? 'enabled' : 'disabled'} in wormhole`,
      )
    })

  const loadChanges = () =>
    run(async () => {
      const files = await invoke('hub_wormhole_rift_files', { wormholeId: activeId })
      setChanges(files)
      setShowChanges(true)
    })

  // One card per file per side pair: side 0 against each other side, skipping identical pairs.
  const changeCards = useMemo(() => {
    if (!changes || !detail) return []
    const sideName = (i) => detail.sides[i]?.rift_name ?? `side ${i}`
    const cards = []
    for (const file of changes) {
      for (let k = 1; k < file.contents.length; k++) {
        const oldText = file.contents[0]
        const newText = file.contents[k]
        if (oldText === newText) continue
        const lines = diffLines(oldText ?? '', newText ?? '')
        cards.push({
          key: `${file.path}|${k}`,
          path: file.path,
          title: `${sideName(0)} → ${sideName(k)}`,
          missing:
            oldText === null ? sideName(0) : newText === null ? sideName(k) : null,
          lines,
          counts: lines ? countChanges(lines) : null,
        })
      }
    }
    return cards
  }, [changes, detail])

  const riftName = (id) => rifts.find((r) => r.id === id)?.name ?? id
  const conflictCount = detail
    ? detail.files.reduce((n, f) => n + f.conflicts.filter((c) => !c.resolved).length, 0)
    : 0
  const isActive = detail?.status === 'active'

  if (!project) {
    return (
      <section className="card">
        <h3>Wormholes</h3>
        <p className="muted">Select a gateway to see its wormholes.</p>
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
        {rifts.length === 0 && <span className="muted">No rifts in this gateway.</span>}
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
            <span className={conflictCount ? 'bad' : 'ok'}>{conflictCount} unresolved conflict(s)</span>
          </p>
          <div className="form-row">
            <span>Modular rifts:</span>
            {detail.sides.map((s) => (
              <label key={s.rift_id} title={isActive ? 'Toggle this rift on or off in the wormhole' : ''}>
                <input
                  type="checkbox"
                  checked={s.enabled !== false}
                  disabled={busy || !isActive}
                  onChange={() => toggleSide(s)}
                />{' '}
                {s.rift_name} {s.enabled === false ? '(off)' : ''}
              </label>
            ))}
          </div>

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
            <button onClick={loadChanges} disabled={busy}>View changes</button>
            {detail.converted_rift_id ? (
              <button onClick={beamConvertedRift} disabled={busy} title="Joins the rift this wormhole converged into and writes it to the project folder">
                Beam converted rift
              </button>
            ) : (
              <button onClick={beam} disabled={busy || conflictCount > 0} title={conflictCount ? 'Resolve all conflicts first' : ''}>
                Beam to folder…
              </button>
            )}
            {isActive && !confirmDissolve && !confirmDelete && (
              <button className="danger" onClick={() => setConfirmDissolve(true)} disabled={busy}>Dissolve</button>
            )}
            {!confirmDissolve && !confirmDelete && (
              <button className="danger" onClick={() => setConfirmDelete(true)} disabled={busy}>Delete</button>
            )}
            {confirmDissolve && (
              <>
                <span>Dissolve this wormhole?</span>
                <button className="danger" onClick={dissolve} disabled={busy}>Yes</button>
                <button onClick={() => setConfirmDissolve(false)}>No</button>
              </>
            )}
            {confirmDelete && (
              <>
                <span>Permanently delete this wormhole on the server?</span>
                <button className="danger" onClick={deleteWormhole} disabled={busy}>Yes</button>
                <button onClick={() => setConfirmDelete(false)}>No</button>
              </>
            )}
          </div>
        </div>
      )}

      {showChanges && (
        <div className="diff-overlay" role="dialog" aria-label="Wormhole changes">
          <div className="diff-window">
            <div className="diff-header">
              <strong>Changes in {detail?.name}</strong>
              <span className="muted">
                {changeCards.length} file change(s)
              </span>
              <button onClick={loadChanges} disabled={busy}>Refresh</button>
              <button onClick={() => setShowChanges(false)} aria-label="Close">×</button>
            </div>
            <div className="diff-body">
              {changeCards.length === 0 && <p className="muted">No differences between the rifts.</p>}
              {changeCards.map((card) => (
                <div key={card.key} className="diff-card">
                  <div className="diff-card-header">
                    <code className="path">{card.path}</code>
                    <span className="muted">{card.title}</span>
                    {card.counts && (
                      <span>
                        <span className="ok">+{card.counts.added}</span>{' '}
                        <span className="bad">-{card.counts.removed}</span>
                      </span>
                    )}
                    {card.missing && <span className="muted">(missing in {card.missing})</span>}
                  </div>
                  {card.lines === null ? (
                    <p className="muted diff-note">
                      Too large to show inline (over {MAX_DIFF_LINES} lines on one side).
                    </p>
                  ) : (
                    <pre className="diff-lines">
                      {card.lines.map((line, i) => (
                        <div key={i} className={`diff-line ${line.type}`}>
                          <span className="diff-sign">
                            {line.type === 'add' ? '+' : line.type === 'del' ? '-' : ' '}
                          </span>
                          {line.text}
                        </div>
                      ))}
                    </pre>
                  )}
                </div>
              ))}
            </div>
          </div>
        </div>
      )}
    </section>
  )
}
