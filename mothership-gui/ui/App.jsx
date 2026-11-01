import { useState } from 'react'
import { invoke } from '@tauri-apps/api/core'
import { ask, open } from '@tauri-apps/plugin-dialog'
import TerminalTab from './TerminalTab.jsx'
import Hub from './Hub.jsx'

let tabCounter = 0
const newSessionId = () => `nvim-${Date.now()}-${++tabCounter}`

export default function App() {
  const [tabs, setTabs] = useState([])
  const [activeKey, setActiveKey] = useState(null)
  const [notice, setNotice] = useState(null)
  const [view, setView] = useState('hub')

  const removeTab = (key) => {
    setTabs((prev) => {
      const next = prev.filter((t) => t.key !== key)
      setActiveKey((current) =>
        current === key ? (next.length ? next[next.length - 1].key : null) : current,
      )
      return next
    })
  }

  const addTab = ({ file = null, cwd = null, title }) => {
    const key = newSessionId()
    setTabs((prev) => [...prev, { key, file, cwd, title }])
    setActiveKey(key)
  }

  const openFile = async (path) => {
    const existing = tabs.find((t) => t.file === path)
    if (existing) {
      setActiveKey(existing.key)
      return
    }
    try {
      await invoke('nvim_check_file', { path })
    } catch (err) {
      setNotice(String(err))
      return
    }
    const name = path.split(/[\\/]/).pop()
    addTab({ file: path, cwd: null, title: name })
  }

  const pickFile = async () => {
    const path = await open({ multiple: false, directory: false })
    if (typeof path === 'string') await openFile(path)
  }

  const closeTab = async (tab) => {
    let unsaved = 0
    try {
      unsaved = await invoke('nvim_unsaved_buffers', { id: tab.key })
    } catch {
      removeTab(tab.key)
      return
    }
    if (unsaved > 0) {
      const discard = await ask(
        `"${tab.title}" has ${unsaved} buffer(s) with unsaved changes. Quit Neovim and discard them?`,
        { title: 'Unsaved changes', kind: 'warning' },
      )
      if (!discard) return
    }
    try {
      const ended = await invoke('nvim_close', { id: tab.key, force: unsaved > 0 })
      if (ended) removeTab(tab.key)
      else setNotice(`Neovim in "${tab.title}" is still running. Finish or quit it in the terminal.`)
    } catch (err) {
      setNotice(String(err))
    }
  }

  const failTab = (key, message) => {
    removeTab(key)
    setNotice(`Could not start Neovim: ${message}`)
  }

  return (
    <div className="app">
      <header className="toolbar">
        <button
          className={view === 'hub' ? 'active' : ''}
          onClick={() => setView('hub')}
        >
          Hub
        </button>
        <button
          className={view === 'editor' ? 'active' : ''}
          onClick={() => setView('editor')}
        >
          Editor
        </button>
        <span className="toolbar-sep" />
        <button onClick={pickFile}>Open file…</button>
        <button onClick={() => addTab({ title: 'nvim' })}>New Neovim</button>
      </header>

      {notice && (
        <div className="notice" role="alert">
          <span>{notice}</span>
          <button onClick={() => setNotice(null)} aria-label="Dismiss">×</button>
        </div>
      )}

      {view === 'hub' && <Hub />}

      <div className="editor-view" style={{ display: view === 'editor' ? 'flex' : 'none' }}>
      <nav className="tabs">
        {tabs.map((tab) => (
          <div
            key={tab.key}
            className={`tab${tab.key === activeKey ? ' active' : ''}`}
            onClick={() => setActiveKey(tab.key)}
          >
            <span className="tab-title" title={tab.file ?? 'nvim'}>{tab.title}</span>
            <button
              className="tab-close"
              aria-label={`Close ${tab.title}`}
              onClick={(e) => {
                e.stopPropagation()
                closeTab(tab)
              }}
            >
              ×
            </button>
          </div>
        ))}
      </nav>

      <main className="terminals">
        {tabs.length === 0 && (
          <p className="empty">Open a file or start a new Neovim session.</p>
        )}
        {tabs.map((tab) => (
          <TerminalTab
            key={tab.key}
            id={tab.key}
            file={tab.file}
            cwd={tab.cwd}
            active={tab.key === activeKey}
            onExit={removeTab}
            onFailed={failTab}
          />
        ))}
      </main>
      </div>
    </div>
  )
}
