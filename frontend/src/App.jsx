import React, { useCallback, useEffect, useRef, useState } from 'react'
import { api, applyWindowFrameStyle, isMock } from './api.js'
import Sidebar from './components/Sidebar.jsx'
import WinControls from './components/WinControls.jsx'
import SettingsModal from './components/SettingsModal.jsx'
import Home from './pages/Home.jsx'
import Update from './pages/Update.jsx'
import Nodes from './pages/Nodes.jsx'
import Models from './pages/Models.jsx'

const TITLES = {
  home: ['', ''],
  update: ['更新', '保持 ComfyUI 始终最新'],
  nodes: ['节点包', '安装、更新、管理扩展节点'],
  models: ['模型路径', '共享外部模型文件夹'],
}

const TABS = ['home', 'update', 'nodes', 'models']

export default function App() {
  // hash keeps the tab in the URL, which makes browser preview / screenshots easy
  const initial = TABS.includes(window.location.hash.slice(1)) ? window.location.hash.slice(1) : 'home'
  const [tab, setTab] = useState(initial)
  const [status, setStatus] = useState(null)
  const [settings, setSettings] = useState({})
  const [logs, setLogs] = useState([])
  const [busy, setBusy] = useState(false)
  const [showSettings, setShowSettings] = useState(false)
  const [toast, setToast] = useState(null)

  const notify = useCallback((text, isError = false) => {
    setToast({ text, isError, key: Date.now() })
    setTimeout(() => setToast(null), 3200)
  }, [])

  const refresh = useCallback(async () => {
    try { setStatus(await api.invoke('comfy:status')) } catch { }
  }, [])

  const notifyRef = useRef(notify)
  notifyRef.current = notify

  useEffect(() => {
    // 挂载时样式表一定已经生效，这里再送一次描边样式（main.jsx 里那次可能太早）
    applyWindowFrameStyle()
    ; (async () => {
      setSettings(await api.invoke('settings:get'))
      await refresh()
    })()
    const offLog = api.onLog((l) => setLogs((prev) => [...prev.slice(-400), l]))
    const offState = api.onState((s) => setStatus((prev) => (prev ? { ...prev, state: s } : prev)))
    // ComfyUI 启动失败这类"没人等着命令返回"的错误，用气泡提示出来
    const offError = api.onError((msg) => notifyRef.current(msg || '启动失败', true))
    const timer = setInterval(refresh, 3000)
    return () => { offLog(); offState(); offError(); clearInterval(timer) }
  }, [refresh])

  const launch = async () => {
    setBusy(true)
    try { await api.invoke('comfy:launch'); refresh() }
    catch (e) { notify(e.message, true) }
    setBusy(false)
  }
  const stop = async () => { await api.invoke('comfy:stop'); refresh() }
  const openBrowser = () => api.invoke('comfy:openBrowser')

  const pickPathFirstRun = async () => {
    const dir = await api.invoke('dialog:selectDir', '选择 ComfyUI 安装目录（包含 main.py 的文件夹）')
    if (dir) {
      const s = await api.invoke('settings:set', { ...settings, comfyPath: dir, port: 8188 })
      setSettings(s)
      await refresh()
      notify('设置完成，点击启动按钮即可开始')
    }
  }

  const [title, sub] = TITLES[tab]
  const changeTab = useCallback((id) => {
    setTab(id)
    window.location.hash = id
  }, [])
  const toggleMax = useCallback(async () => {
    if (isMock) return
    const { getCurrentWindow } = await import('@tauri-apps/api/window')
    await getCurrentWindow().toggleMaximize()
  }, [])
  const state = status?.state || 'stopped'
  const stateLabel = !status?.installed
    ? '未设置目录'
    : state === 'running'
      ? `运行中 · ${status.port}`
      : state === 'starting'
        ? '启动中…'
        : '已就绪'

  return (
    <div className={`app ${isMock ? '' : 'frameless'}`}>
      <div className="app-body">
        <Sidebar tab={tab} onChange={changeTab} onSettings={() => setShowSettings(true)} />

        <main className="main">
          <div className="topbar" data-tauri-drag-region onDoubleClick={toggleMax}>
            <div className="topbar-text" data-tauri-drag-region>
              {title && <div className="title" data-tauri-drag-region>{title}</div>}
              {sub && <div className="sub" data-tauri-drag-region>{sub}{isMock ? ' · 浏览器预览模式' : ''}</div>}
            </div>
            <div className="topbar-right">
              <div className="state-pill" data-tauri-drag-region>
                <span className={`dot ${status?.installed ? state : ''}`} />
                {stateLabel}
              </div>
              <WinControls />
            </div>
          </div>

          <div className="content">
            {tab === 'home' && (
              <Home status={status} logs={logs} busy={busy}
                onLaunch={launch} onStop={stop} onOpenBrowser={openBrowser}
                onPickPath={pickPathFirstRun} />
            )}
            {tab === 'update' && <Update status={status} logs={logs} notify={notify} refresh={refresh} />}
            {tab === 'nodes' && <Nodes status={status} notify={notify} />}
            {tab === 'models' && <Models status={status} notify={notify} />}
          </div>
        </main>
      </div>

      {showSettings && (
        <SettingsModal
          settings={settings}
          onClose={() => setShowSettings(false)}
          onSaved={(s) => { setSettings(s); refresh() }}
          notify={notify}
        />
      )}

      {toast && <div key={toast.key} className={`toast ${toast.isError ? 'error' : ''}`}>{toast.text}</div>}
    </div>
  )
}
