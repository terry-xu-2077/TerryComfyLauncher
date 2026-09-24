import React, { useEffect, useRef, useState } from 'react'
import { api } from '../api.js'
import { IconRefresh, IconTrash } from '../components/Icons.jsx'

export default function Nodes({ status, notify }) {
  const [nodes, setNodes] = useState([])
  const [loaded, setLoaded] = useState(false)
  const [loading, setLoading] = useState(false)
  const [url, setUrl] = useState('')
  const [busy, setBusy] = useState(false)
  const [removing, setRemoving] = useState(null)

  // 更新检查：联网操作，必须手动触发
  const [checking, setChecking] = useState(false)
  const [checked, setChecked] = useState(false) // 是否已检查过（本次会话）
  const [updates, setUpdates] = useState({})     // name -> { behind, remote, error }
  const [outdated, setOutdated] = useState([])   // 落后于远端的节点名

  // registry search
  const [query, setQuery] = useState('')
  const [results, setResults] = useState(null) // null = 未搜索，显示已安装列表
  const [searching, setSearching] = useState(false)
  const [fromCache, setFromCache] = useState(false)
  const [installingUrl, setInstallingUrl] = useState(null)
  const debounceRef = useRef(null)

  // 读取已安装列表：纯本地（读目录 + git rev-parse），不联网，
  // 所以切到本页就直接显示；「是否有更新」才是需要手动触发的联网操作。
  const load = async () => {
    if (!status?.installed) return
    setLoading(true)
    try { setNodes(await api.invoke('nodes:list')); setLoaded(true) }
    catch (e) { notify(e.message, true) }
    setLoading(false)
  }
  useEffect(() => { load() }, [status?.installed]) // eslint-disable-line react-hooks/exhaustive-deps

  const checkUpdates = async () => {
    if (!status?.installed) return
    setChecking(true)
    try {
      const r = await api.invoke('nodes:checkUpdates')
      const map = {}
      for (const it of r.list || []) map[it.name] = it
      setUpdates(map)
      setOutdated(r.outdated || [])
      setChecked(true)
      const failed = (r.list || []).filter((i) => i.error).length
      const parts = []
      parts.push(r.outdated?.length ? `${r.outdated.length} 个节点有新提交` : '所有节点都已是最新')
      if (failed) parts.push(`${failed} 个无法连接远端`)
      notify(parts.join('，'), failed > 0)
    } catch (e) { notify(e.message, true) }
    setChecking(false)
  }

  const clearUpdates = () => { setUpdates({}); setOutdated([]); setChecked(false) }

  const search = async (q, refresh = false) => {
    setSearching(true)
    try {
      const r = await api.invoke('nodes:search', q, refresh)
      setResults(r.results)
      setFromCache(r.fromCache)
    } catch (e) { notify(e.message, true) }
    setSearching(false)
  }

  // 输入防抖搜索；清空则回到已安装列表
  useEffect(() => {
    const q = query.trim()
    if (!q) { setResults(null); return }
    clearTimeout(debounceRef.current)
    debounceRef.current = setTimeout(() => search(q), 450)
    return () => clearTimeout(debounceRef.current)
  }, [query])

  const refreshRegistry = () => {
    const q = query.trim()
    if (!q) { notify('先输入关键词，再刷新在线列表'); return }
    search(q, true)
  }

  const install = async () => {
    if (!url.trim()) return
    setBusy(true)
    try {
      await api.invoke('nodes:install', url.trim())
      notify('节点安装完成，重启 ComfyUI 后生效')
      setUrl('')
      clearUpdates()
      await load()
    } catch (e) { notify(e.message, true) }
    setBusy(false)
  }

  const installFromSearch = async (item) => {
    setInstallingUrl(item.url)
    try {
      await api.invoke('nodes:install', item.url)
      notify(`${item.title} 安装完成，重启 ComfyUI 后生效`)
      setResults((prev) => prev && prev.map((r) => (r.url === item.url ? { ...r, installed: true } : r)))
      clearUpdates()
      await load()
    } catch (e) { notify(e.message, true) }
    setInstallingUrl(null)
  }

  const updateOne = async (name) => {
    setBusy(true)
    try {
      await api.invoke('nodes:update', name)
      notify(`${name} 已更新`)
      setUpdates((prev) => { const n = { ...prev }; delete n[name]; return n })
      setOutdated((prev) => prev.filter((x) => x !== name))
      await load()
    } catch (e) { notify(e.message, true) }
    setBusy(false)
  }

  // sel 为空 = 更新全部 Git 节点；传了名单就只更新落后的那几个
  const updateAll = async (sel) => {
    setBusy(true)
    try {
      const r = await api.invoke('nodes:updateAll', sel && sel.length ? sel : null)
      notify(`更新完成：成功 ${r.done} 个，失败 ${r.failed} 个`)
      clearUpdates()
      await load()
    } catch (e) { notify(e.message, true) }
    setBusy(false)
  }

  const remove = async (name) => {
    if (removing !== name) { setRemoving(name); setTimeout(() => setRemoving(null), 3000); return }
    setRemoving(null)
    setBusy(true)
    try {
      await api.invoke('nodes:remove', name)
      notify(`${name} 已删除`)
      setUpdates((prev) => { const n = { ...prev }; delete n[name]; return n })
      setOutdated((prev) => prev.filter((x) => x !== name))
      await load()
    } catch (e) { notify(e.message, true) }
    setBusy(false)
  }

  return (
    <div className="page">
      <div className="add-row">
        <input
          className="text-input"
          placeholder="输入节点名或关键词搜索（与 ComfyUI Manager 同源）"
          value={query}
          onChange={(e) => setQuery(e.target.value)}
        />
        <button
          className="mini-btn"
          onClick={refreshRegistry}
          disabled={searching}
          title="重新从在线仓库拉取最新节点列表"
        >
          {searching ? '搜索中…' : '刷新'}
        </button>
      </div>

      {results !== null ? (
        <>
          <div className="section-title">
            <span>搜索结果（{results.length}）{fromCache ? ' · 本地缓存' : ' · 在线'}</span>
            <button className="mini-btn" onClick={() => { setQuery(''); setResults(null) }}>返回已安装</button>
          </div>
          {results.length === 0 && <div className="empty">没有匹配的节点包，换个关键词试试</div>}
          {results.map((r) => (
            <div className="node-item" key={r.url}>
              <div className="node-icon">{(r.title || 'N')[0].toUpperCase()}</div>
              <div className="node-info">
                <div className="node-name">{r.title} <span className="node-author">by {r.author}</span></div>
                <div className="node-ver node-desc">{r.description || r.url}</div>
              </div>
              <div className="node-actions">
                {r.installed ? (
                  <span className="tag installed-tag">已安装</span>
                ) : (
                  <button
                    className="mini-btn accent"
                    onClick={() => installFromSearch(r)}
                    disabled={installingUrl !== null || !status?.installed}
                  >
                    {installingUrl === r.url ? '安装中…' : '安装'}
                  </button>
                )}
              </div>
            </div>
          ))}
        </>
      ) : (
        <>
          <div className="add-row">
            <input
              className="text-input"
              placeholder="粘贴节点的 GitHub 地址，一键安装"
              value={url}
              onChange={(e) => setUrl(e.target.value)}
              onKeyDown={(e) => e.key === 'Enter' && install()}
            />
            <button className="mini-btn" onClick={install} disabled={busy || !url.trim()}>安装</button>
          </div>

          <div className="section-title">
            <span>已安装节点（{nodes.length}）</span>
            <span>
              <button
                className="mini-btn"
                onClick={load}
                disabled={loading || busy || !status?.installed}
                title="重新读取已安装列表（纯本地，不联网）"
              >
                <IconRefresh size={14} />
                {loading ? '读取中…' : '刷新列表'}
              </button>
              <button
                className="mini-btn"
                onClick={checkUpdates}
                disabled={checking || busy || !status?.installed}
                title="联网检查哪些节点有新提交（切到本页不会自动检查，需要手动点这里）"
              >
                <IconRefresh size={14} />
                {checking ? '检查中…' : '检查更新'}
              </button>
              {checked && outdated.length > 0 ? (
                <button className="mini-btn accent" onClick={() => updateAll(outdated)} disabled={busy}>
                  更新这 {outdated.length} 个
                </button>
              ) : (
                nodes.some(n => n.isGit) && (
                  <button className="mini-btn accent" onClick={() => updateAll(null)} disabled={busy || checking}>
                    全部更新
                  </button>
                )
              )}
            </span>
          </div>

          {checked && !checking && (
            <div className="hint-row">
              {outdated.length > 0
                ? `${outdated.length} 个节点有新提交，可点「更新这 ${outdated.length} 个」`
                : '已检查：所有节点都已是最新'}
            </div>
          )}
          {checking && <div className="hint-row">正在连接各节点的远端仓库…（节点较多时需要一两分钟）</div>}

          {!status?.installed && <div className="empty">请先在「启动」页设置 ComfyUI 目录</div>}
          {status?.installed && loaded && !loading && nodes.length === 0 && (
            <div className="empty">还没有安装任何节点包</div>
          )}

          {nodes.map((n) => {
            const u = updates[n.name]
            return (
            <div className="node-item" key={n.name}>
              <div className="node-icon">{n.name.replace(/^ComfyUI-?/i, '')[0]?.toUpperCase() || 'N'}</div>
              <div className="node-info">
                <div className="node-name">
                  {n.name}
                  {u && u.behind > 0 && <span className="tag update-tag">有新版本</span>}
                  {u && u.error && <span className="tag warn-tag" title={u.error}>未检查到</span>}
                </div>
                <div className="node-ver">
                  {n.isGit
                    ? `版本 ${n.version || '—'}${u && u.behind > 0 ? ` → ${u.remote}（落后 ${u.behind} 个提交）` : ''}`
                    : '手动安装（非 Git）'}
                </div>
              </div>
              <div className="node-actions">
                {n.isGit && (
                  <button
                    className={`icon-btn${u && u.behind > 0 ? ' accent-ring' : ''}`}
                    style={{ width: 36, height: 36 }}
                    onClick={() => updateOne(n.name)}
                    disabled={busy}
                    title="更新"
                  >
                    <IconRefresh size={16} />
                  </button>
                )}
                <button
                  className="icon-btn" style={{ width: 36, height: 36, color: removing === n.name ? 'var(--red)' : undefined }}
                  onClick={() => remove(n.name)} disabled={busy}
                  title={removing === n.name ? '再点一次确认删除' : '删除'}
                >
                  <IconTrash size={16} />
                </button>
              </div>
            </div>
            )
          })}
        </>
      )}
    </div>
  )
}
