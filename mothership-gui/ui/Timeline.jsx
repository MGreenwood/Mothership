import { useEffect, useRef, useState } from 'react'
import { invoke } from '@tauri-apps/api/core'

const PAGE = 200
const KEEP = 2000
const WAIT_SECS = 25
const RETRY_MS = 5000
const NEAR_BOTTOM_PX = 40

const TYPE_LABELS = {
  file_saved: 'File saved',
  checkpoint: 'Checkpoint',
  chat: 'Chat',
  join: 'Joined',
  leave: 'Left',
  restore: 'Restore',
  wormhole: 'Wormhole',
  access_change: 'Access',
}

function summarize(event) {
  const p = event.payload ?? {}
  switch (event.event_type) {
    case 'file_saved':
      return p.path ?? ''
    case 'checkpoint':
      return `${p.message || 'Checkpoint'} (${p.file_count ?? 0} files)`
    case 'chat':
      return p.content ?? ''
    case 'restore':
      return `Restored checkpoint (${p.file_count ?? 0} files)`
    case 'wormhole':
      return `${p.action ?? 'changed'} ${p.wormhole_name ?? ''}`.trim()
    case 'access_change':
      return `${p.action === 'add' ? 'Added' : 'Removed'} ${p.target_username ?? ''}`.trim()
    default:
      return ''
  }
}

// Event paths are relative to the project folder; the editor needs an absolute path
function joinLocal(root, relative) {
  return `${root.replace(/[\\/]+$/, '')}\\${relative.replace(/[\\/]+/g, '\\')}`
}

function TimelineRow({ event, onOpen }) {
  return (
    <li
      className={`timeline-row${onOpen ? ' clickable' : ''}`}
      onClick={onOpen}
    >
      <span className="timeline-time">{new Date(event.created_at).toLocaleTimeString()}</span>
      <span className="timeline-actor">{event.actor_username ?? 'system'}</span>
      <span className="timeline-type">{TYPE_LABELS[event.event_type] ?? event.event_type}</span>
      <span className="timeline-summary">{summarize(event)}</span>
    </li>
  )
}

export default function Timeline({ gateway, active, explorerRoot, openFile, onError }) {
  const [rifts, setRifts] = useState([])
  const [riftId, setRiftId] = useState(null)
  const [events, setEvents] = useState([])
  const [follow, setFollow] = useState(true)
  const [reachedStart, setReachedStart] = useState(false)
  const [loadingOlder, setLoadingOlder] = useState(false)
  const [hiddenTypes, setHiddenTypes] = useState([])
  const [status, setStatus] = useState(null)
  const listRef = useRef(null)

  // Rifts for the selected gateway, defaulting to the rift the local folder is beamed into
  useEffect(() => {
    if (!gateway) {
      setRifts([])
      setRiftId(null)
      return
    }
    let cancelled = false
    Promise.all([
      invoke('hub_list_rifts', { projectId: gateway.id }),
      invoke('hub_active_rift', { projectId: gateway.id }),
    ])
      .then(([list, current]) => {
        if (cancelled) return
        setRifts(list)
        setRiftId((prev) => {
          if (prev && list.some((r) => r.id === prev)) return prev
          if (current?.rift_id && list.some((r) => r.id === current.rift_id)) return current.rift_id
          return list[0]?.id ?? null
        })
      })
      .catch((err) => {
        if (!cancelled) setStatus(String(err))
      })
    return () => {
      cancelled = true
    }
  }, [gateway?.id])

  // Load the newest events, then follow with long-polls. Runs only while this view is open,
  // so a hidden timeline makes no requests.
  useEffect(() => {
    if (!active || !riftId) return
    let cancelled = false
    setEvents([])
    setReachedStart(false)
    setStatus(null)

    const run = async () => {
      let cursor = 0
      let initial = true
      while (!cancelled) {
        try {
          const batch = await invoke(
            'hub_rift_events',
            initial
              ? { riftId, after: 0, before: null, latest: true, waitSecs: 0 }
              : { riftId, after: cursor, before: null, latest: false, waitSecs: WAIT_SECS },
          )
          if (cancelled) return
          if (initial) setReachedStart(batch.length < PAGE)
          initial = false
          if (batch.length > 0) {
            cursor = batch[batch.length - 1].id
            setEvents((prev) => [...prev, ...batch].slice(-KEEP))
          }
          setStatus(null)
        } catch (err) {
          if (cancelled) return
          setStatus(`Live updates paused (${String(err)}). Retrying…`)
          await new Promise((resolve) => setTimeout(resolve, RETRY_MS))
        }
      }
    }
    run()
    return () => {
      cancelled = true
    }
  }, [riftId, active])

  useEffect(() => {
    if (follow && listRef.current) {
      listRef.current.scrollTop = listRef.current.scrollHeight
    }
  }, [events, follow])

  const onScroll = (e) => {
    const el = e.currentTarget
    const nearBottom = el.scrollHeight - el.scrollTop - el.clientHeight < NEAR_BOTTOM_PX
    if (nearBottom !== follow) setFollow(nearBottom)
  }

  const loadOlder = async () => {
    if (!events.length || loadingOlder || reachedStart) return
    setLoadingOlder(true)
    try {
      const batch = await invoke('hub_rift_events', {
        riftId,
        after: 0,
        before: events[0].id,
        latest: false,
        waitSecs: 0,
      })
      setReachedStart(batch.length < PAGE)
      if (batch.length > 0) setEvents((prev) => [...batch, ...prev])
    } catch (err) {
      onError(String(err))
    } finally {
      setLoadingOlder(false)
    }
  }

  const toggleType = (type) => {
    setHiddenTypes((prev) =>
      prev.includes(type) ? prev.filter((t) => t !== type) : [...prev, type],
    )
  }

  if (!gateway) {
    return (
      <section className="timeline">
        <p className="empty">Pick a gateway in the Hub to see its timeline.</p>
      </section>
    )
  }

  const visible = events.filter((event) => !hiddenTypes.includes(event.event_type))

  return (
    <section className="timeline">
      <div className="timeline-bar">
        <label>
          Rift{' '}
          <select value={riftId ?? ''} onChange={(e) => setRiftId(e.target.value)}>
            {rifts.map((r) => (
              <option key={r.id} value={r.id}>{r.name}</option>
            ))}
          </select>
        </label>
        <label>
          <input type="checkbox" checked={follow} onChange={(e) => setFollow(e.target.checked)} /> Follow live
        </label>
        {!reachedStart && events.length > 0 && (
          <button onClick={loadOlder} disabled={loadingOlder}>
            {loadingOlder ? 'Loading…' : 'Load older'}
          </button>
        )}
        <span className="timeline-filters">
          {Object.entries(TYPE_LABELS).map(([type, label]) => (
            <label key={type}>
              <input
                type="checkbox"
                checked={!hiddenTypes.includes(type)}
                onChange={() => toggleType(type)}
              />
              {label}
            </label>
          ))}
        </span>
      </div>

      {status && <p className="bad">{status}</p>}

      <ol className="timeline-list" ref={listRef} onScroll={onScroll}>
        {visible.map((event) => (
          <TimelineRow
            key={event.id}
            event={event}
            onOpen={
              event.event_type === 'file_saved' && explorerRoot && event.payload?.path
                ? () => openFile(joinLocal(explorerRoot, event.payload.path))
                : null
            }
          />
        ))}
      </ol>

      {visible.length === 0 && (
        <p className="empty">{riftId ? 'No events yet.' : 'This gateway has no rifts.'}</p>
      )}
    </section>
  )
}
