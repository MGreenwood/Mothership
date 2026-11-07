import { useEffect, useState } from 'react'
import { invoke } from '@tauri-apps/api/core'

const STATUS_MARK = { added: 'A', removed: 'D', modified: 'M' }

export default function DiffPanel({ project, rifts, checkpoints }) {
  const [currentRift, setCurrentRift] = useState(null)
  const [target, setTarget] = useState('')
  const [diffs, setDiffs] = useState(null)
  const [selectedPath, setSelectedPath] = useState(null)
  const [fileText, setFileText] = useState('')
  const [allText, setAllText] = useState(null)
  const [status, setStatus] = useState(null)

  useEffect(() => {
    setDiffs(null)
    setSelectedPath(null)
    setAllText(null)
    setTarget('')
    if (!project) {
      setCurrentRift(null)
      return
    }
    let cancelled = false
    invoke('hub_active_rift', { projectId: project.id })
      .then((res) => {
        if (!cancelled) setCurrentRift(res?.rift_name ?? null)
      })
      .catch(() => {
        if (!cancelled) setCurrentRift(null)
      })
    return () => {
      cancelled = true
    }
  }, [project?.id])

  // Each target is compared against the current rift: what changed from it to here
  const sideValue = target
  const otherRifts = rifts.filter((r) => r.name !== currentRift)

  const compare = async (full) => {
    if (!sideValue || !currentRift) return
    setStatus(null)
    try {
      const result = await invoke('hub_diff', { from: sideValue, to: currentRift, full })
      if (full) {
        setAllText(result)
      } else {
        setDiffs(result)
        setAllText(null)
      }
      setSelectedPath(null)
      setFileText('')
    } catch (err) {
      setStatus(String(err))
    }
  }

  const showFile = async (path) => {
    setSelectedPath(path)
    setStatus(null)
    try {
      const file = await invoke('hub_file_diff', { from: sideValue, to: currentRift, path })
      setFileText(file.unified ?? '')
    } catch (err) {
      setStatus(String(err))
    }
  }

  if (!project) {
    return <section className="card"><h3>Compare</h3><p className="muted">Pick a gateway first.</p></section>
  }
  if (!currentRift) {
    return (
      <section className="card">
        <h3>Compare</h3>
        <p className="muted">Beam this project into a rift to compare it with other rifts and checkpoints.</p>
      </section>
    )
  }

  return (
    <section className="card">
      <h3>Compare</h3>
      <p className="muted">Changes in <strong>{currentRift}</strong> since the chosen rift or checkpoint.</p>
      <div className="form-row">
        <select value={target} onChange={(e) => setTarget(e.target.value)}>
          <option value="">Choose what to compare against</option>
          {otherRifts.length > 0 && (
            <optgroup label="Rifts">
              {otherRifts.map((r) => (
                <option key={r.id} value={r.name}>{r.name}</option>
              ))}
            </optgroup>
          )}
          {checkpoints.length > 0 && (
            <optgroup label="Checkpoints">
              {checkpoints.map((c) => (
                <option key={c.id} value={`checkpoint:${c.id}`}>
                  {(c.message || '(no message)')} {c.partial ? '(partial)' : ''}
                </option>
              ))}
            </optgroup>
          )}
        </select>
        <button onClick={() => compare(false)} disabled={!sideValue}>Compare</button>
        <button onClick={() => compare(true)} disabled={!sideValue}>Show all changes</button>
      </div>

      {status && <p className="bad">{status}</p>}

      {diffs && diffs.length === 0 && <p className="muted">No differences.</p>}
      {diffs && diffs.length > 0 && (
        <ul className="list diff-list">
          {diffs.map((d) => (
            <li key={d.path} className={selectedPath === d.path ? 'active' : ''} onClick={() => showFile(d.path)}>
              <span className="diff-mark">{STATUS_MARK[d.status]}</span>
              <span className="path">{d.path}</span>
              <span className="ok">+{d.added}</span>
              <span className="bad">-{d.removed}</span>
            </li>
          ))}
        </ul>
      )}

      {selectedPath && fileText && <pre className="diff-text">{fileText}</pre>}

      {allText && allText.length === 0 && <p className="muted">No differences.</p>}
      {allText && allText.map((d) => (
        <div key={d.path} className="diff-block">
          <div className="muted">{STATUS_MARK[d.status]} {d.path} (+{d.added} -{d.removed})</div>
          <pre className="diff-text">{d.unified ?? ''}</pre>
        </div>
      ))}
    </section>
  )
}
