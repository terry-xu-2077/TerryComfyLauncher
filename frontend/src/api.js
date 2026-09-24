// Bridge to the Tauri (Rust) backend, with an in-browser mock so the UI
// can be previewed in a plain browser (vite dev) without the desktop shell.
import { invoke } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'

const isTauri = typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window

// channel -> [tauri command, args mapper]
const ROUTES = {
  'settings:get': ['get_settings', () => ({})],
  'settings:set': ['set_settings', (a) => ({ settings: a[0] })],
  'dialog:selectDir': ['pick_directory', (a) => ({ title: a[0] })],
  'comfy:status': ['comfy_status', () => ({})],
  'comfy:launch': ['comfy_launch', () => ({})],
  'comfy:stop': ['comfy_stop', () => ({})],
  'comfy:openBrowser': ['comfy_open_browser', () => ({})],
  'comfy:checkUpdate': ['comfy_check_update', () => ({})],
  'comfy:update': ['comfy_update', () => ({})],
  'nodes:list': ['nodes_list', () => ({})],
  'nodes:checkUpdates': ['nodes_check_updates', () => ({})],
  'nodes:install': ['nodes_install', (a) => ({ url: a[0] })],
  'nodes:update': ['nodes_update', (a) => ({ name: a[0] })],
  'nodes:updateAll': ['nodes_update_all', (a) => ({ names: a[0] || null })],
  'nodes:remove': ['nodes_remove', (a) => ({ name: a[0] })],
  'nodes:search': ['nodes_search', (a) => ({ query: a[0], refresh: !!a[1] })],
  'models:get': ['models_get', () => ({})],
  'models:scan': ['models_scan', (a) => ({ basePath: a[0] })],
  'models:save': ['models_save', (a) => ({ entries: a[0] })],
}

const demoNodes = [
  { name: 'ComfyUI-Manager', isGit: true, version: 'a3f9c21' },
  { name: 'ComfyUI-Impact-Pack', isGit: true, version: '7be01d4' },
  { name: 'rgthree-comfy', isGit: true, version: 'c55e8aa' },
]

let mockState = 'stopped'

const mock = {
  invoke: async (channel, ...args) => {
    await new Promise(r => setTimeout(r, 300))
    switch (channel) {
      case 'settings:get': return { comfyPath: 'G:\\AIGC\\ComfyUI_windows_portable\\ComfyUI', port: 8188, lanAccess: false, extraArgs: '' }
      case 'settings:set': return args[0]
      case 'dialog:selectDir': return 'G:\\AIGC\\models'
      case 'comfy:status':
        return { installed: true, version: 'v0.3.12', date: '2026-09-20', nodeCount: 3, state: mockState, pythonFound: true, path: 'G:\\AIGC\\ComfyUI_windows_portable\\ComfyUI', port: 8188 }
      case 'comfy:launch': mockState = 'running'; return { ok: true }
      case 'comfy:stop': mockState = 'stopped'; return { ok: true }
      case 'comfy:openBrowser': window.open('http://127.0.0.1:8188'); return { ok: true }
      case 'comfy:checkUpdate':
        return { local: 'a1b2c3d', localDate: '2026-09-18', remote: 'e4f5g6h', remoteDate: '2026-09-23', remoteMsg: 'feat: support new sampler', behind: 5, hasUpdate: true, links: [] }
      case 'comfy:update': return { ok: true }
      case 'nodes:list': return demoNodes
      case 'nodes:checkUpdates':
        return {
          outdated: ['ComfyUI-Manager'],
          list: [
            { name: 'ComfyUI-Manager', local: 'a3f9c21', remote: 'e77aa10', behind: 4, fetched: true, error: null },
            { name: 'ComfyUI-Impact-Pack', local: '7be01d4', remote: '7be01d4', behind: 0, fetched: true, error: null },
            { name: 'rgthree-comfy', local: 'c55e8aa', remote: 'c55e8aa', behind: 0, fetched: false, error: '无法连接远端，结果可能不是最新' },
          ],
        }
      case 'nodes:install': case 'nodes:update': case 'nodes:updateAll': case 'nodes:remove': return { ok: true, done: 3, failed: 0 }
      case 'nodes:search':
        return {
          fromCache: true,
          results: [
            { title: 'ComfyUI-Manager', author: 'ltdrdata', description: '节点管理器：安装、更新、禁用扩展节点', url: 'https://github.com/ltdrdata/ComfyUI-Manager', installed: true },
            { title: 'ComfyUI-Impact-Pack', author: 'ltdrdata', description: '检测、分割、高清修复等实用节点合集', url: 'https://github.com/ltdrdata/ComfyUI-Impact-Pack', installed: false },
            { title: 'rgthree-comfy', author: 'rgthree', description: '让工作流更干净高效的一批节点', url: 'https://github.com/rgthree/rgthree-comfy', installed: false },
          ].filter(n => !args[0] || (n.title + n.author + n.description).toLowerCase().includes(String(args[0]).toLowerCase())),
        }
      case 'models:get':
        return [
          { name: 'SD模型库', basePath: 'G:\\AIGC\\models\\sd', types: { checkpoints: 'checkpoints', loras: 'loras', vae: 'vae' } },
          { name: 'Flux模型库', basePath: 'G:\\AIGC\\models\\flux', types: { diffusion_models: 'diffusion_models', text_encoders: 'text_encoders' } },
        ]
      case 'models:scan':
        return { types: {}, looseFiles: 3 }
      case 'models:save': return { ok: true }
      default: return { ok: true }
    }
  },
  onLog: () => () => {},
  onState: () => () => {},
  onError: () => () => {},
}

const tauri = {
  invoke: (channel, ...args) => {
    const route = ROUTES[channel]
    if (!route) return Promise.reject(new Error(`未知命令: ${channel}`))
    // Rust commands reject with a plain string; normalize so `e.message`
    // always carries the real reason instead of a generic fallback text.
    return invoke(route[0], route[1](args)).catch((e) => {
      throw new Error(typeof e === 'string' && e ? e : (e?.message || String(e)))
    })
  },
  onLog: (cb) => {
    let unlisten = () => {}
    listen('log', (e) => cb(e.payload)).then((fn) => { unlisten = fn })
    return () => unlisten()
  },
  onState: (cb) => {
    let unlisten = () => {}
    listen('comfy-state', (e) => cb(e.payload)).then((fn) => { unlisten = fn })
    return () => unlisten()
  },
  // backend-side failures that happen while nobody is awaiting a command
  // (e.g. ComfyUI dying right after launch)
  onError: (cb) => {
    let unlisten = () => {}
    listen('comfy-error', (e) => cb(e.payload)).then((fn) => { unlisten = fn })
    return () => unlisten()
  },
}

export const api = isTauri ? tauri : mock
export const isMock = !isTauri
