import React from 'react'
import { IconHome, IconRocket, IconNodes, IconFolder, IconGear } from './Icons.jsx'
import brandMark from '../assets/brand-mark.svg'

const TABS = [
  { id: 'home', label: '一键启动', Icon: IconHome },
  { id: 'update', label: '更新核心', Icon: IconRocket },
  { id: 'nodes', label: '节点包', Icon: IconNodes },
  { id: 'models', label: '模型路径', Icon: IconFolder },
]

export default function Sidebar({ tab, onChange, onSettings }) {
  return (
    <aside className="sidebar">
      <div className="side-brand" data-tauri-drag-region>
        <div className="brand-chip" data-tauri-drag-region>
          <img src={brandMark} alt="" data-tauri-drag-region />
        </div>
        <div className="side-brand-text" data-tauri-drag-region>
          <div className="side-brand-name" data-tauri-drag-region>TerryComfy</div>
          <div className="side-brand-sub" data-tauri-drag-region>启动器</div>
        </div>
      </div>

      <nav className="side-nav">
        {TABS.map(({ id, label, Icon }) => (
          <button
            key={id}
            className={`side-item ${tab === id ? 'active' : ''}`}
            onClick={() => onChange(id)}
          >
            <Icon size={17} />
            <span>{label}</span>
          </button>
        ))}
      </nav>

      <div className="side-divider" />
      <button className="side-item" onClick={onSettings}>
        <IconGear size={17} />
        <span>设置</span>
      </button>
    </aside>
  )
}
