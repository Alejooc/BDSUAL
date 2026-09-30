import { useEffect, useRef, useState, type KeyboardEvent as ReactKeyboardEvent, type MouseEvent } from 'react'

export type ContextMenuItem = { label: string; action: () => void; disabled?: boolean; separator?: boolean }
type ContextMenuRequest = { x: number; y: number; items: ContextMenuItem[] }

export function openContextMenu(event: MouseEvent, items: ContextMenuItem[]) {
  event.preventDefault()
  event.stopPropagation()
  if (!items.length) return
  window.dispatchEvent(new CustomEvent<ContextMenuRequest>('dbsual:context-menu', { detail: { x: event.clientX, y: event.clientY, items } }))
}

export function ContextMenuHost() {
  const [menu, setMenu] = useState<ContextMenuRequest | null>(null)
  const menuRef = useRef<HTMLDivElement>(null)
  useEffect(() => {
    const open = (event: Event) => setMenu((event as CustomEvent<ContextMenuRequest>).detail)
    const close = () => setMenu(null)
    const key = (event: globalThis.KeyboardEvent) => { if (event.key === 'Escape') close() }
    window.addEventListener('dbsual:context-menu', open)
    window.addEventListener('pointerdown', close)
    window.addEventListener('resize', close)
    window.addEventListener('keydown', key)
    return () => {
      window.removeEventListener('dbsual:context-menu', open)
      window.removeEventListener('pointerdown', close)
      window.removeEventListener('resize', close)
      window.removeEventListener('keydown', key)
    }
  }, [])
  useEffect(() => { if (menu) menuRef.current?.querySelector<HTMLButtonElement>('button:not(:disabled)')?.focus() }, [menu])
  if (!menu) return null
  const width = 220
  const height = menu.items.length * 36 + 12
  const left = Math.max(8, Math.min(menu.x, window.innerWidth - width - 8))
  const top = Math.max(8, Math.min(menu.y, window.innerHeight - height - 8))
  const navigate = (event: ReactKeyboardEvent<HTMLDivElement>) => {
    if (event.key !== 'ArrowDown' && event.key !== 'ArrowUp') return
    event.preventDefault()
    const buttons = [...(menuRef.current?.querySelectorAll<HTMLButtonElement>('button:not(:disabled)') ?? [])]
    if (!buttons.length) return
    const current = buttons.indexOf(document.activeElement as HTMLButtonElement)
    const direction = event.key === 'ArrowDown' ? 1 : -1
    buttons[(current + direction + buttons.length) % buttons.length]?.focus()
  }
  return <div ref={menuRef} className="dbsual-context-menu" role="menu" aria-label="Opciones de DBSUAL" style={{ left, top }} onKeyDown={navigate} onPointerDown={(event) => event.stopPropagation()}>
    {menu.items.map((item, index) => <div className={item.separator ? 'context-menu-item separated' : 'context-menu-item'} key={`${item.label}-${index}`}>
      <button type="button" role="menuitem" disabled={item.disabled} onClick={() => { setMenu(null); item.action() }}>{item.label}</button>
    </div>)}
  </div>
}
