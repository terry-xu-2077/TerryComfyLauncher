import React from 'react'
import { IconPlay, IconStop } from '../components/Icons.jsx'
import LogPanel from '../components/LogPanel.jsx'

export default function Home({ status, logs, busy, onLaunch, onStop, onOpenBrowser, onPickPath }) {
  const state = status?.state || 'stopped'
  const running = state === 'running'
  const starting = state === 'starting'
  // 启动中也能点：万一卡住或启动错了，可以马上取消
  const cancellable = running || starting

  const heroText = !status?.installed
    ? '请先选择 ComfyUI 安装目录'
    : starting
      ? '正在启动 ComfyUI，请稍候…'
      : running
        ? `ComfyUI 运行中 · 端口 ${status.port}`
        : '已就绪，点击左侧按钮一键启动'

  return (
    <div className="page home-page">
      <div className="hero-row">
        <div className={`launch-wrap ${running ? 'running' : ''}`}>
          <div className="launch-ring" />
          <button
            className={`launch-btn ${cancellable ? 'stopping' : ''}`}
            disabled={busy || !status?.installed}
            onClick={cancellable ? onStop : onLaunch}
            title={starting ? '点击取消启动' : running ? '点击停止 ComfyUI' : '启动 ComfyUI'}
          >
            {cancellable ? <IconStop size={80} /> : <IconPlay size={80} />}
            <span className="launch-label">
              {starting ? '取消启动' : running ? '停止' : '启动'}
            </span>
          </button>
        </div>

        <div className="hero-info">
          <div className="hero-status">
            <span className={`dot ${status?.installed ? state : ''}`} />
            {heroText}
          </div>

          {status?.installed ? (
            <>
              <div className="chips">
                <div className="chip">
                  <div className="chip-num">{status.version || '—'}</div>
                  <div className="chip-label">当前版本</div>
                </div>
                <div className="chip">
                  <div className="chip-num">{status.nodeCount}</div>
                  <div className="chip-label">已装节点</div>
                </div>
                <div className="chip">
                  <div className="chip-num">{status.date || '—'}</div>
                  <div className="chip-label">更新日期</div>
                </div>
              </div>
              {running && (
                <button className="open-btn" onClick={onOpenBrowser}>打开 ComfyUI 界面</button>
              )}
            </>
          ) : (
            <div className="welcome">
              <div className="w-title">欢迎使用 TerryComfy启动器</div>
              <div className="w-desc">先告诉我 ComfyUI 安装在哪里，剩下的都交给我</div>
              <button className="big-action" onClick={onPickPath}>选择 ComfyUI 目录</button>
            </div>
          )}
        </div>
      </div>

      <div className="section-title">运行日志</div>
      <LogPanel logs={logs} className="fill" />
    </div>
  )
}
