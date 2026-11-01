import { useState } from 'react'
import { invoke } from '@tauri-apps/api/core'
import { ask, open } from '@tauri-apps/plugin-dialog'
import TerminalTab from './TerminalTab.jsx'
import Hub from './Hub.jsx'
import Explorer from './Explorer.jsx'
import ActiveRiftBar from './ActiveRiftBar.jsx'

let tabCounter = 0
const newSessionId = () => `nvim-${Date.now()}-${++tabCounter}`
const EXPLORER_ROOT_KEY = 'mothership-explorer-root'

export default function App() {
  const [tabs, setTabs] = useState([])
  const [activeKey, setActiveKey] = useState(null)
  const [notice, setNotice] = useState(null)
  const [view, setView] = useState('hub')
  const [explorerRoot, setExplorerRoot] = useState(() => localStorage.getItem(EXPLORER_ROOT_KEY))
  const activeFile = tabs.find((t) => t.key === activeKey)?.file ?? null
  const [gateway, setGateway] = useState(null)
  const [riftVersion, setRiftVersion] = useState(0)
  const bumpRift = () => setRiftVersion((v) => v + 1)

  const changeExplorerRoot = (path) => {
    localStorage.setItem(EXPLORER_ROOT_KEY, path)
    setExplorerRoot(path)
  }

  const openProjectFolder = (path) => {
    changeExplorerRoot(path)
    setView('editor')
  }

  const removeTab = (key) => {
    setTabs((prev) => {
      const next = prev.filter((t) => t.key !== key)
      setActiveKey((current) =>
        current === key ? (next.length ? next[next.length - 1].key : null) : current,
      )
      return next
    })
  }

  const addTab = ({ kind = 'nvim', file = null, cwd = null, title }) => {
    const key = newSessionId()
    setTabs((prev) => [...prev, { key, kind, file, cwd, title }])
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

  const openTerminal = () => {
    const folder = explorerRoot ? explorerRoot.split(/[\\/]/).filter(Boolean).pop() : null
    addTab({ kind: 'shell', cwd: explorerRoot ?? null, title: folder ? `terminal: ${folder}` : 'terminal' })
  }

  const closeTab = async (tab) => {
    if (tab.kind === 'shell') {
      try {
        await invoke('terminal_close', { id: tab.key })
        removeTab(tab.key)
      } catch (err) {
        setNotice(String(err))
      }
      return
    }
    try {
      const ended = await invoke('nvim_close', { id: tab.key, force: false })
      if (ended) {
        removeTab(tab.key)
        return
      }
      const discard = await ask(
        `"${tab.title}" has buffers with unsaved changes. Quit Neovim and discard them?`,
        { title: 'Unsaved changes', kind: 'warning', okLabel: 'OK', cancelLabel: 'Cancel' },
      )
      if (!discard) return
      const forced = await invoke('nvim_close', { id: tab.key, force: true })
      if (forced) removeTab(tab.key)
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
        <button onClick={openTerminal}>New Terminal</button>
        <span className="toolbar-sep" />
        <ActiveRiftBar gateway={gateway} version={riftVersion} />
      </header>

      {notice && (
        <div className="notice" role="alert">
          <span>{notice}</span>
          <button onClick={() => setNotice(null)} aria-label="Dismiss">×</button>
        </div>
      )}

      {view === 'hub' && (
        <Hub
          onOpenInEditor={openProjectFolder}
          onGatewayChange={setGateway}
          onRiftChanged={bumpRift}
        />
      )}

      <div className="editor-view" style={{ display: view === 'editor' ? 'flex' : 'none' }}>
      <Explorer
        root={explorerRoot}
        onRootChange={changeExplorerRoot}
        openFile={openFile}
        activeFile={activeFile}
      />
      <div className="editor-main">
      <nav className="tabs">
        {tabs.map((tab) => (
          <div
            key={tab.key}
            className={`tab${tab.key === activeKey ? ' active' : ''}`}
            onClick={() => setActiveKey(tab.key)}
          >
            <span className="tab-title" title={tab.file ?? tab.cwd ?? 'nvim'}>{tab.title}</span>
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
          <p className="empty">Open a file, or start a new Neovim or terminal session.</p>
        )}
        {tabs.map((tab) => (
          <TerminalTab
            key={tab.key}
            id={tab.key}
            kind={tab.kind}
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
    </div>
  )
}
