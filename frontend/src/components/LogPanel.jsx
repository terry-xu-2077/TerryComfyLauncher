import React, { useEffect, useRef } from 'react'

export default function LogPanel({ logs, className }) {
  const ref = useRef(null)
  useEffect(() => {
    if (ref.current) ref.current.scrollTop = ref.current.scrollHeight
  }, [logs])
  return (
    <div className={`log-panel ${className || ''}`} ref={ref}>
      {logs.length === 0 && <div className="log-empty">暂无日志输出…</div>}
      {logs.map((l, i) => (
        <div key={i}>
          <span className="log-src">[{l.source}]</span>
          <span>{l.text.replace(/\n+$/, '')}</span>
        </div>
      ))}
    </div>
  )
}
