import React, { useEffect, useState } from 'react'
import { api } from '../api.js'
import { IconTrash, IconCheck } from '../components/Icons.jsx'

const TYPE_LABELS = {
  checkpoints: '大模型', loras: 'LoRA', vae: 'VAE', controlnet: 'ControlNet',
  upscale_models: '放大模型', embeddings: 'Embedding', clip: 'CLIP',
  clip_vision: 'CLIP视觉', diffusion_models: '扩散模型', text_encoders: '文本编码器',
  unet: 'UNet', gligen: 'GLIGEN', style_models: '风格模型', hypernetworks: '超网络',
  photomaker: 'PhotoMaker', classifiers: '分类器', model_patches: '模型补丁',
  audio_encoders: '音频编码器', vae_approx: '近似VAE',
}

const PICKABLE_TYPES = [
  'checkpoints', 'diffusion_models', 'loras', 'vae', 'text_encoders',
  'clip', 'controlnet', 'upscale_models', 'embeddings',
]

export default function Models({ status, notify }) {
  const [entries, setEntries] = useState([])
  const [adding, setAdding] = useState(false)
  const [name, setName] = useState('')
  const [basePath, setBasePath] = useState('')
  const [scanned, setScanned] = useState(null) // { types: {}, looseFiles: n }
  const [pickedType, setPickedType] = useState('checkpoints')
  const [busy, setBusy] = useState(false)

  const load = async () => {
    if (!status?.installed) return
    try { setEntries(await api.invoke('models:get')) } catch (e) { notify(e.message, true) }
  }
  useEffect(() => { load() }, [status?.installed])

  const openAdd = () => { setAdding(true); setName(''); setBasePath(''); setScanned(null); setPickedType('checkpoints') }
  const closeAdd = () => { setAdding(false); setScanned(null) }

  const pick = async () => {
    const dir = await api.invoke('dialog:selectDir', '选择模型库文件夹')
    if (!dir) return
    setBasePath(dir)
    const found = await api.invoke('models:scan', dir)
    setScanned(found)
    if (!name) setName(dir.split(/[\\/]/).pop() || '我的模型库')
  }

  const add = async () => {
    if (!name || !basePath) { notify('请填写名称并选择文件夹', true); return }
    const known = scanned && Object.keys(scanned.types || {}).length > 0
    const types = known ? scanned.types : { [pickedType]: '.' }
    const next = [...entries.filter(e => e.name !== name), { name, basePath, types }]
    setBusy(true)
    try {
      await api.invoke('models:save', next)
      setEntries(next)
      closeAdd()
      notify('模型路径已添加，重启 ComfyUI 后生效')
    } catch (e) { notify(e.message, true) }
    setBusy(false)
  }

  const remove = async (n) => {
    const next = entries.filter(e => e.name !== n)
    try {
      await api.invoke('models:save', next)
      setEntries(next)
      notify('已移除，重启 ComfyUI 后生效')
    } catch (e) { notify(e.message, true) }
  }

  return (
    <div className="page">
      <div className="card" style={{ marginBottom: 12 }}>
        <div style={{ fontSize: 13, fontWeight: 700, marginBottom: 4 }}>外部模型文件夹</div>
        <div style={{ fontSize: 12, color: 'var(--muted)', lineHeight: 1.6 }}>
          把放在其他磁盘 / 文件夹里的模型共享给 ComfyUI 使用，不用复制文件。添加后重启 ComfyUI 生效。
        </div>
      </div>

      {!status?.installed && <div className="empty">请先在「启动」页设置 ComfyUI 目录</div>}

      {status?.installed && (
        <div className="section-title" style={{ marginTop: 4 }}>
          <span>已添加的模型文件夹（{entries.length}）</span>
          <button className="mini-btn accent" onClick={openAdd}>添加文件夹</button>
        </div>
      )}

      <div className="models-grid">
        {entries.map((e) => (
          <div className="model-item" key={e.name}>
            <div className="model-head">
              <div className="model-name">{e.name}</div>
              <button className="icon-btn" onClick={() => remove(e.name)} title="移除">
                <IconTrash size={15} />
              </button>
            </div>
            <div className="model-path" title={e.basePath}>{e.basePath}</div>
            <div className="model-tags">
              {Object.keys(e.types || {}).map((t) => (
                <span className="tag" key={t}>{TYPE_LABELS[t] || t}</span>
              ))}
              {Object.keys(e.types || {}).length === 0 && <span className="tag">未识别到模型子目录</span>}
            </div>
          </div>
        ))}
      </div>

      {adding && (
        <div className="modal-mask" onClick={closeAdd}>
          <div className="modal modal-sm" onClick={(e) => e.stopPropagation()}>
            <div className="modal-head">
              <h3>添加模型文件夹</h3>
              <div className="modal-sub">选择文件夹后会自动识别里面的模型类型</div>
            </div>
            <div className="modal-body">
              <div className="field">
                <label>起个好记的名字</label>
                <input className="text-input" style={{ background: '#f4f5fa' }} placeholder="例如：SD模型库" value={name} onChange={(e) => setName(e.target.value)} />
              </div>
              <div className="field">
                <label>模型文件夹位置</label>
                <div className="field-row">
                  <input className="text-input" style={{ background: '#f4f5fa' }} readOnly placeholder="点击右侧按钮选择…" value={basePath} />
                  <button className="mini-btn" onClick={pick}>浏览</button>
                </div>
              </div>
              {scanned && Object.keys(scanned.types || {}).length > 0 && (
                <div className="field">
                  <label>自动识别到以下模型类型（已自动匹配，无需手动填写）</label>
                  <div className="model-tags">
                    {Object.keys(scanned.types).map((t) => <span className="tag" key={t}><IconCheck size={10} style={{ marginRight: 4, verticalAlign: -1 }} />{TYPE_LABELS[t] || t}</span>)}
                  </div>
                </div>
              )}
              {scanned && Object.keys(scanned.types || {}).length === 0 && (
                <div className="field">
                  <label>
                    {scanned.looseFiles > 0
                      ? `文件夹里直接放着 ${scanned.looseFiles} 个模型文件，无法自动判断类型，请告诉我它是什么：`
                      : '未识别到标准子目录，请选择这个文件夹存放的模型类型：'}
                  </label>
                  <div className="model-tags">
                    {PICKABLE_TYPES.map((t) => (
                      <span
                        key={t}
                        className={`tag pick ${pickedType === t ? 'on' : ''}`}
                        onClick={() => setPickedType(t)}
                      >
                        {TYPE_LABELS[t] || t}
                      </span>
                    ))}
                  </div>
                </div>
              )}
            </div>
            <div className="modal-foot">
              <button className="big-action" style={{ flex: 1 }} onClick={add} disabled={busy}>{busy ? '保存中…' : '保存'}</button>
              <button className="mini-btn ghost" style={{ padding: '0 22px', borderRadius: 20 }} onClick={closeAdd}>取消</button>
            </div>
          </div>
        </div>
      )}
    </div>
  )
}
