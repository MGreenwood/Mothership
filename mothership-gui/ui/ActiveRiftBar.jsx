import { useEffect, useState } from 'react'
import { invoke } from '@tauri-apps/api/core'

export default function ActiveRiftBar({ gateway, version }) {
  const [active, setActive] = useState(null)

  useEffect(() => {
    if (!gateway) {
      setActive(null)
      return
    }
    let cancelled = false
    invoke('hub_active_rift', { projectId: gateway.id })
      .then((res) => {
        if (!cancelled) setActive(res)
      })
      .catch(() => {
        if (!cancelled) setActive(null)
      })
    return () => {
      cancelled = true
    }
  }, [gateway?.id, version])

  if (!gateway) {
    return <span className="active-rift muted">No gateway selected</span>
  }

  let state = 'ok'
  let text
  if (!active) {
    state = 'muted'
    text = 'Checking…'
  } else if (!active.local_path) {
    state = 'bad'
    text = 'Not local'
  } else if (!active.rift_id) {
    state = 'muted'
    text = 'Not beamed into a rift'
  } else {
    text = active.rift_name || active.rift_id
  }

  return (
    <span className="active-rift" title={active?.local_path ?? undefined}>
      <span className="muted">{gateway.name}</span>
      <span className="active-rift-sep">›</span>
      <span className={state}>{text}</span>
    </span>
  )
}
