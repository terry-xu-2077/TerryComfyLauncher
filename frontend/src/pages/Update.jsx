import React, { useState, useCallback } from 'react'
import { api } from '../api.js'
import { IconCheck, IconRefresh } from '../components/Icons.jsx'
import LogPanel from '../components/LogPanel.jsx'

const BASE_STEPS = [
  { name: '更新 ComfyUI 本体', desc: '拉取最新代码（失败自动重试 / 代理兜底）' },
  { name: '更新运行依赖', desc: '多软件源自动兜底安装' },
]

export default function Update({ status, logs, notify, refresh }) {
  const [phase, setPhase] = useState(-1) // -1 idle, 0..n-1 doing, n done
  const [info, setInfo] = useState(null) // { local, localDate, remote, remoteDate, remoteMsg, behind, hasUpdate, links }
  const [checking, setChecking] = useState(false)
  const [checkError, setCheckError] = useState('')
  const steps = [
    ...BASE_STEPS,
    ...(info?.links?.length
      ? [{ name: '检查目录链接', desc: `确认 ${info.links.join('、')} 链接有效` }]
      : []),
  ]
  const busy = phase >= 0 && phase < steps.length
  const canOperate = status?.installed && status.state === 'stopped'

  const check = useCallback(async (silent) => {
    if (!status?.installed) return
    setChecking(true)
    setCheckError('')
    try {
      const r = await api.invoke('comfy:checkUpdate')
      setInfo(r)
      if (!silent) {
        notify(r.hasUpdate ? `发现新版本 ${r.remote}，落后 ${r.behind} 个提交` : '已是最新版本')
      }
    } catch (e) {
      setCheckError(e.message || '检查更新失败')
      setInfo(null)
    }
    setChecking(false)
  }, [status?.installed, notify])

  // 不自动检查：本页只在用户点「检查更新」时才联网。
  // 切走再切回来会重置状态，所以默认显示占位提示而不是直接拉远端。

  const run = async () => {
    setPhase(0)
    const off = api.onLog((l) => {
      if (l.source !== 'update') return
      const m = l.text.match(/第\s*(\d+)\s*步/)
      if (m) setPhase(Number(m[1]) - 1)
      if (l.text.includes('更新完成')) setPhase(steps.length)
    })
    try {
      await api.invoke('comfy:update')
      setPhase(steps.length)
      notify('ComfyUI 已更新到最新版本')
      refresh()
      check(true)
    } catch (e) {
      notify('更新失败：' + e.message, true)
      setPhase(-1)
    }
    off()
  }

  return (
    <div className="page">
      <div className="card">
        {/* version panel */}
        {!info && !checking && !checkError && (
          <div className="version-panel">
            <div className="version-checking">
              <IconRefresh size={15} />
              尚未检查。点击下方「检查更新」才会联网查询最新版本
            </div>
          </div>
        )}
        {checking && !info && (
          <div className="version-panel">
            <div className="version-checking">
              <span className="spin"><IconRefresh size={15} /></span>
              正在连接远程仓库检查更新…
            </div>
          </div>
        )}
        {checkError && !checking && (
          <div className="version-panel">
            <div className="version-checking">检查失败：{checkError}</div>
          </div>
        )}
        {info && (
          <div className="version-panel">
            <div className="version-row">
              <div className="version-cell">
                <div className="version-tag">当前版本</div>
                <div className="version-hash mono">{info.local || '未知'}</div>
                <div className="version-date">{info.localDate}</div>
              </div>
              <div className={`version-arrow ${info.hasUpdate ? 'hot' : ''}`}>
                {info.hasUpdate ? `落后 ${info.behind} 个提交` : '已是最新'}
              </div>
              <div className="version-cell">
                <div className="version-tag">最新版本</div>
                <div className="version-hash mono">{info.remote || '未知'}</div>
                <div className="version-date">{info.remoteDate}</div>
              </div>
            </div>
            {info.hasUpdate && info.remoteMsg && (
              <div className="version-msg" title={info.remoteMsg}>最新提交：{info.remoteMsg}</div>
            )}
            {!info.hasUpdate && (
              <div className="version-latest">
                <IconCheck size={14} /> 当前已是最新版本，无需更新
              </div>
            )}
          </div>
        )}

        <div className="steps-row">
          {steps.map((s, i) => (
            <div key={i} className={`step ${phase === i ? 'doing' : ''} ${phase > i || phase === steps.length ? 'done' : ''}`}>
              <div className="step-icon">
                {phase > i || phase === steps.length ? <IconCheck size={18} /> : <IconRefresh size={18} />}
              </div>
              <div className="step-text">
                <div className="step-name">{s.name}</div>
                <div className="step-desc">{s.desc}</div>
              </div>
            </div>
          ))}
        </div>

        {info?.hasUpdate || busy ? (
          <button className="big-action" onClick={run} disabled={busy || !canOperate}>
            {busy ? '更新中，请稍候…' : `一键更新到 ${info?.remote || '最新版'}`}
          </button>
        ) : (
          <button className="big-action" onClick={() => check(false)} disabled={checking || !canOperate}>
            {checking ? '检查中…' : '检查更新'}
          </button>
        )}
        {status?.state !== 'stopped' && (
          <div style={{ fontSize: 12, color: 'var(--muted)', textAlign: 'center', marginTop: 10 }}>
            更新前请先停止运行中的 ComfyUI
          </div>
        )}
      </div>

      <div className="section-title">更新日志</div>
      <LogPanel logs={logs.filter(l => l.source === 'update')} />
    </div>
  )
}
