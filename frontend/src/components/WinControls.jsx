import React, { useEffect, useState } from 'react'
import { isMock } from '../api.js'

// minimal inline icons for window controls (not part of the Iconly-style set)
const IconMin = () => (
  <svg width="11" height="11" viewBox="0 0 11 11" fill="none">
    <path d="M1 5.5h9" stroke="currentColor" strokeWidth="1.2" strokeLinecap="round" />
  </svg>
)
const IconMax = () => (
  <svg width="10" height="10" viewBox="0 0 10 10" fill="none">
    <rect x="1" y="1" width="8" height="8" rx="2" stroke="currentColor" strokeWidth="1.2" />
  </svg>
)
const IconRestore = () => (
  <svg width="10" height="10" viewBox="0 0 10 10" fill="none">
    <rect x="2.6" y="1" width="6.4" height="6.4" rx="1.8" stroke="currentColor" strokeWidth="1.1" />
    <path d="M7.4 7.4v.4a1.8 1.8 0 0 1-1.8 1.8H2.8A1.8 1.8 0 0 1 1 7.8V4.4a1.8 1.8 0 0 1 1.8-1.8h.4"
      stroke="currentColor" strokeWidth="1.1" fill="none" />
  </svg>
)
const IconClose = () => (
  <svg width="11" height="11" viewBox="0 0 11 11" fill="none">
    <path d="M1.5 1.5l8 8m0-8l-8 8" stroke="currentColor" strokeWidth="1.2" strokeLinecap="round" />
  </svg>
)

// 仅窗口控制按钮（最小化/最大化/关闭），嵌入顶栏右侧；品牌区由侧边栏承担
export default function WinControls() {
  const [maximized, setMaximized] = useState(false)

  useEffect(() => {
    if (isMock) return
    let unlisten
    let alive = true
    ;(async () => {
      const { getCurrentWindow } = await import('@tauri-apps/api/window')
      const win = getCurrentWindow()
      if (!alive) return
      setMaximized(await win.isMaximized())
      unlisten = await win.onResized(async () => {
        const m = await win.isMaximized()
        setMaximized(m)
        document.documentElement.classList.toggle('maximized', m)
      })
    })()
    return () => { alive = false; unlisten && unlisten() }
  }, [])

  if (isMock) return null

  const call = async (fn) => {
    const { getCurrentWindow } = await import('@tauri-apps/api/window')
    const w = getCurrentWindow()
    await w[fn]()
  }

  return (
    <div className="win-controls">
      <button className="win-btn" onClick={() => call('minimize')} title="最小化"><IconMin /></button>
      <button className="win-btn" onClick={() => call('toggleMaximize')} title={maximized ? '还原' : '最大化'}>
        {maximized ? <IconRestore /> : <IconMax />}
      </button>
      <button className="win-btn close" onClick={() => call('close')} title="关闭"><IconClose /></button>
    </div>
  )
}
