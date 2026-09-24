import React, { useState } from 'react'
import { api } from '../api.js'

export default function SettingsModal({ settings, onClose, onSaved, notify }) {
  const [comfyPath, setComfyPath] = useState(settings.comfyPath || '')
  const [port, setPort] = useState(settings.port || 8188)
  const [lanAccess, setLanAccess] = useState(!!settings.lanAccess)
  const [extraArgs, setExtraArgs] = useState(settings.extraArgs || '')
  const [saving, setSaving] = useState(false)

  const pick = async () => {
    const dir = await api.invoke('dialog:selectDir', '选择 ComfyUI 安装目录（包含 main.py 的文件夹）')
    if (dir) setComfyPath(dir)
  }

  const save = async () => {
    if (!comfyPath) { notify('请先选择 ComfyUI 目录', true); return }
    setSaving(true)
    try {
      const s = await api.invoke('settings:set', {
        ...settings,
        comfyPath,
        port: Number(port) || 8188,
        lanAccess,
        extraArgs: extraArgs.trim(),
      })
      onSaved(s)
      onClose()
      notify('设置已保存，重启 ComfyUI 后生效')
    } catch (e) { notify(e.message, true) }
    setSaving(false)
  }

  return (
    <div className="modal-mask" onClick={onClose}>
      <div className="modal" onClick={(e) => e.stopPropagation()}>
        <div className="modal-head">
          <h3>设置</h3>
          <div className="modal-sub">启动器放在 ComfyUI 目录内（或其上级目录）会自动识别，无需手动选择</div>
        </div>

        <div className="modal-body">
        <div className="field">
          <label>ComfyUI 安装目录（包含 main.py 的文件夹）</label>
          <div className="field-row">
            <input className="text-input" value={comfyPath} readOnly placeholder="点击右侧按钮选择…" />
            <button className="mini-btn" onClick={pick}>浏览</button>
          </div>
        </div>

        <div className="field">
          <label>端口（默认 8188，一般不用改）</label>
          <input className="text-input" value={port} onChange={(e) => setPort(e.target.value.replace(/\D/g, ''))} />
        </div>

        <div className="field">
          <label>局域网访问</label>
          <label className="check-row">
            <input type="checkbox" checked={lanAccess} onChange={(e) => setLanAccess(e.target.checked)} />
            <span className="check-label">允许局域网内其他设备访问</span>
          </label>
          <div className="field-hint">开启后启动时附加 --listen 0.0.0.0，其他设备可用「本机IP:端口」打开 ComfyUI</div>
        </div>

        <div className="field">
          <label>附加启动参数（进阶，不懂可留空）</label>
          <input
            className="text-input mono"
            value={extraArgs}
            onChange={(e) => setExtraArgs(e.target.value)}
            placeholder="例如：--use-sage-attention"
          />
          <div className="field-hint">以空格分隔，原样附加到启动命令末尾</div>
        </div>
        </div>

        <div className="modal-foot">
          <button className="big-action" onClick={save} disabled={saving}>
            {saving ? '保存中…' : '保存设置'}
          </button>
        </div>
      </div>
    </div>
  )
}
