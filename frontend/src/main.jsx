import React from 'react'
import { createRoot } from 'react-dom/client'
import App from './App.jsx'
import { applyWindowFrameStyle } from './api.js'
import './styles.css'

// 样式表已就位，这里把窗口描边的颜色/粗细交给原生描边窗
applyWindowFrameStyle()

createRoot(document.getElementById('root')).render(<App />)
