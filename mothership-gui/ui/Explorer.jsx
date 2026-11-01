import { useCallback, useEffect, useState } from 'react'
import { invoke } from '@tauri-apps/api/core'
import { open } from '@tauri-apps/plugin-dialog'

function TreeNode({ entry, depth, openFile, activeFile }) {
  const [expanded, setExpanded] = useState(false)
  const [children, setChildren] = useState(null)
  const [error, setError] = useState(null)

  const toggle = async () => {
    if (!expanded && children === null) {
      try {
        setChildren(await invoke('explorer_list_dir', { path: entry.path }))
        setError(null)
      } catch (err) {
        setError(String(err))
      }
    }
    setExpanded(!expanded)
  }

  const indent = { paddingLeft: 8 + depth * 14 }

  if (!entry.is_dir) {
    return (
      <div
        className={`tree-row file${activeFile === entry.path ? ' selected' : ''}`}
        style={indent}
        title={entry.path}
        onClick={() => openFile(entry.path)}
      >
        <span className="tree-icon"> </span>
        <span className="tree-name">{entry.name}</span>
      </div>
    )
  }

  return (
    <>
      <div className="tree-row dir" style={indent} title={entry.path} onClick={toggle}>
        <span className="tree-icon">{expanded ? '▾' : '▸'}</span>
        <span className="tree-name">{entry.name}</span>
      </div>
      {expanded && error && <div className="tree-error" style={indent}>{error}</div>}
      {expanded &&
        children &&
        children.map((child) => (
          <TreeNode
            key={child.path}
            entry={child}
            depth={depth + 1}
            openFile={openFile}
            activeFile={activeFile}
          />
        ))}
    </>
  )
}

export default function Explorer({ root, onRootChange, openFile, activeFile }) {
  const [entries, setEntries] = useState([])
  const [error, setError] = useState(null)

  const loadRoot = useCallback(async (path) => {
    try {
      setEntries(await invoke('explorer_list_dir', { path }))
      setError(null)
    } catch (err) {
      setEntries([])
      setError(String(err))
    }
  }, [])

  useEffect(() => {
    if (root) loadRoot(root)
  }, [root, loadRoot])

  const chooseFolder = async () => {
    const path = await open({ multiple: false, directory: true })
    if (typeof path === 'string') onRootChange(path)
  }

  return (
    <aside className="explorer">
      <div className="explorer-header">
        <span className="explorer-title" title={root ?? ''}>
          {root ? root.split(/[\\/]/).filter(Boolean).pop() : 'Explorer'}
        </span>
        <button onClick={chooseFolder}>Open folder…</button>
      </div>
      <div className="explorer-tree">
        {!root && <p className="explorer-hint">Open a folder to browse its files.</p>}
        {error && <p className="explorer-hint error">{error}</p>}
        {entries.map((entry) => (
          <TreeNode
            key={entry.path}
            entry={entry}
            depth={0}
            openFile={openFile}
            activeFile={activeFile}
          />
        ))}
      </div>
    </aside>
  )
}
