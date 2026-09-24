// Minimal stroke icons (Iconly-style)
const S = ({ children, size = 20, ...p }) => (
  <svg width={size} height={size} viewBox="0 0 24 24" fill="none" stroke="currentColor"
    strokeWidth="1.8" strokeLinecap="round" strokeLinejoin="round" {...p}>{children}</svg>
)

export const IconHome = (p) => <S {...p}><path d="M3 10.5 12 3l9 7.5"/><path d="M5 9.5V21h14V9.5"/><path d="M10 21v-6h4v6"/></S>
export const IconRocket = (p) => <S {...p}><path d="M12 15c5-3 7-8 7-12-4 0-9 2-12 7l-3 1 4 4 1-3Z"/><path d="M9 15c-2 1-3 4-3 6 2 0 5-1 6-3"/><circle cx="14" cy="8" r="1.6"/></S>
export const IconNodes = (p) => <S {...p}><rect x="3" y="3" width="7" height="7" rx="2.2"/><rect x="14" y="3" width="7" height="7" rx="2.2"/><rect x="8.5" y="14" width="7" height="7" rx="2.2"/><path d="M6.5 10v2.5a2 2 0 0 0 2 2M17.5 10v2.5a2 2 0 0 1-2 2"/></S>
export const IconFolder = (p) => <S {...p}><path d="M3 7a2 2 0 0 1 2-2h4l2 3h8a2 2 0 0 1 2 2v9a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2Z"/></S>
export const IconGear = (p) => <S {...p}><circle cx="12" cy="12" r="3"/><path d="M12 2v3M12 19v3M2 12h3M19 12h3M4.9 4.9l2.1 2.1M17 17l2.1 2.1M19.1 4.9 17 7M7 17l-2.1 2.1"/></S>
export const IconPlay = (p) => <svg width={p.size || 34} height={p.size || 34} viewBox="0 0 24 24" fill="currentColor"><path d="M8.5 5.8v12.4c0 .9 1 1.5 1.8 1l9.6-6.2c.7-.5.7-1.5 0-2L10.3 4.8c-.8-.5-1.8.1-1.8 1Z"/></svg>
export const IconStop = (p) => <svg width={p.size || 30} height={p.size || 30} viewBox="0 0 24 24" fill="currentColor"><rect x="6.5" y="6.5" width="11" height="11" rx="2.5"/></svg>
export const IconRefresh = (p) => <S {...p}><path d="M20 12a8 8 0 1 1-2.3-5.6"/><path d="M20 3v4h-4"/></S>
export const IconTrash = (p) => <S {...p}><path d="M4 7h16M9 7V5a1.5 1.5 0 0 1 1.5-1.5h3A1.5 1.5 0 0 1 15 5v2"/><path d="M6.5 7 7.3 20a1.5 1.5 0 0 0 1.5 1.4h6.4a1.5 1.5 0 0 0 1.5-1.4L17.5 7"/></S>
export const IconPlus = (p) => <S {...p}><path d="M12 5v14M5 12h14"/></S>
export const IconCheck = (p) => <S {...p}><path d="m5 12.5 4.5 4.5L19 7.5"/></S>
export const IconExternal = (p) => <S {...p}><path d="M14 4h6v6"/><path d="M20 4 10 14"/><path d="M20 14v5a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V6a2 2 0 0 1 2-2h5"/></S>
