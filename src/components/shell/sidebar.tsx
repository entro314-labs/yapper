import {
  IconCalendarClock,
  IconLayoutSidebarLeftCollapse,
  IconLayoutSidebarLeftExpand,
  IconPencilPlus,
  IconSettings,
  IconStack2,
  IconUsers,
} from '@tabler/icons-react'
import { Link, useRouterState } from '@tanstack/react-router'
import * as React from 'react'

import { Tooltip, TooltipContent, TooltipTrigger } from '@/components/ui/tooltip'
import {
  APP_NAME,
  SIDEBAR_MAX_W,
  SIDEBAR_MIN_W,
  SIDEBAR_RAIL_W,
  TITLEBAR_H,
  TITLEBAR_INSET_LEFT,
  IS_MACOS,
} from '@/lib/chrome'
import { usePrefs } from '@/lib/prefs'
import { cn } from '@/lib/utils'

interface NavItem {
  to: string
  label: string
  icon: React.ElementType
}

const NAV: NavItem[] = [
  { to: '/', label: 'Queue', icon: IconStack2 },
  { to: '/compose', label: 'Compose', icon: IconPencilPlus },
  { to: '/calendar', label: 'Calendar', icon: IconCalendarClock },
  { to: '/accounts', label: 'Accounts', icon: IconUsers },
]

const FOOTER: NavItem[] = [{ to: '/settings', label: 'Settings', icon: IconSettings }]

/**
 * The navigation column — and, with the system frame gone, the window's leading chrome. It runs the
 * full window height and hosts the macOS traffic lights in its own header band, so there is no
 * shared horizontal titlebar; the content island carries its own (see PaneTitlebar).
 *
 * Two states. The outer shell animates its width while the inner wrapper keeps a fixed one, so
 * content slides out of the clip instead of squashing mid-animation.
 */
export function Sidebar() {
  const { sidebarMode } = usePrefs()

  return (
    <aside
      data-mode={sidebarMode}
      className={cn(
        'relative flex h-full shrink-0 flex-col overflow-hidden',
        // Deliberately no background: the sidebar sits directly on the desk,
        // which is the one translucent surface and therefore the one the OS
        // effect can show through. Painting it here would put an opaque slab in
        // front of the very effect it is meant to carry.
        'text-sidebar-foreground',
        'transition-[width] duration-300 ease-[cubic-bezier(0.22,1,0.36,1)]',
      )}
      style={{
        width:
          sidebarMode === 'full'
            ? 'var(--pane-sidebar-w, var(--pane-sidebar-default))'
            : SIDEBAR_RAIL_W,
      }}
      aria-label="Primary navigation"
    >
      {/* Ambient carrier glow at the top of the column, tying this chrome
          surface to the accent the rest of the app uses. Pointer-events-none so
          it never intercepts a click. */}
      <div
        aria-hidden
        className="pointer-events-none absolute inset-x-0 top-0 h-40 bg-gradient-to-b from-primary/[0.07] via-primary/[0.02] to-transparent"
      />

      {sidebarMode === 'rail' ? <RailContent /> : <FullContent />}
      {sidebarMode === 'full' ? <ResizeSeam /> : null}
    </aside>
  )
}

function FullContent() {
  return (
    <div
      className="flex h-full flex-col"
      style={{ width: 'var(--pane-sidebar-w, var(--pane-sidebar-default))' }}
    >
      <header
        data-tauri-drag-region
        className="drag-region flex shrink-0 items-center"
        style={{
          height: TITLEBAR_H,
          paddingLeft: IS_MACOS ? TITLEBAR_INSET_LEFT : 14,
        }}
      >
        <span className="font-display pointer-events-none truncate text-sm font-semibold tracking-tight">
          {APP_NAME}
        </span>
        <LayoutButton />
      </header>

      <nav className="flex min-h-0 flex-1 flex-col gap-0.5 overflow-y-auto px-2 py-1">
        {NAV.map((item) => (
          <NavLink key={item.to} item={item} />
        ))}
      </nav>

      <div className="flex flex-col gap-0.5 px-2 pb-2">
        {FOOTER.map((item) => (
          <NavLink key={item.to} item={item} />
        ))}
      </div>
    </div>
  )
}

function RailContent() {
  return (
    <div className="flex h-full flex-col" style={{ width: SIDEBAR_RAIL_W }}>
      <header
        data-tauri-drag-region
        className="drag-region flex shrink-0 items-end justify-center pb-1"
        // On macOS the traffic lights own this band, so the rail's toggle drops
        // below them instead of colliding with the close button.
        style={{ height: IS_MACOS ? TITLEBAR_H + 24 : TITLEBAR_H }}
      >
        <LayoutButton />
      </header>
      <nav className="flex min-h-0 flex-1 flex-col items-center gap-1 overflow-y-auto py-1">
        {NAV.map((item) => (
          <NavLink key={item.to} item={item} rail />
        ))}
      </nav>
      <div className="flex flex-col items-center gap-1 pb-2">
        {FOOTER.map((item) => (
          <NavLink key={item.to} item={item} rail />
        ))}
      </div>
    </div>
  )
}

function NavLink({ item, rail = false }: { item: NavItem; rail?: boolean }) {
  const pathname = useRouterState({ select: (state) => state.location.pathname })
  // `/` would prefix-match everything, so the root is compared exactly.
  const active = item.to === '/' ? pathname === '/' : pathname.startsWith(item.to)
  const Icon = item.icon

  const link = (
    <Link
      to={item.to}
      aria-current={active ? 'page' : undefined}
      className={cn(
        'group relative flex items-center rounded-md text-sm transition-colors duration-[var(--motion-micro)]',
        rail ? 'size-9 justify-center' : 'h-8 gap-2.5 px-2.5',
        active
          ? 'bg-sidebar-accent text-sidebar-accent-foreground font-medium'
          : 'text-muted-foreground hover:bg-sidebar-accent/60 hover:text-foreground',
      )}
    >
      {/* The active marker: a short bar on the leading edge rather than a full
          border, so a selected row reads as attached to the column. */}
      <span
        aria-hidden
        className={cn(
          'absolute inset-y-1.5 left-0 w-0.5 rounded-full bg-primary transition-opacity',
          active ? 'opacity-100' : 'opacity-0',
        )}
      />
      <Icon className="size-4 shrink-0" />
      {rail ? null : <span className="truncate">{item.label}</span>}
    </Link>
  )

  if (!rail) return link
  return (
    <Tooltip>
      <TooltipTrigger render={link} />
      <TooltipContent side="right">{item.label}</TooltipContent>
    </Tooltip>
  )
}

function LayoutButton() {
  const { sidebarMode, setSidebarMode } = usePrefs()
  const Icon = sidebarMode === 'full' ? IconLayoutSidebarLeftCollapse : IconLayoutSidebarLeftExpand
  return (
    <button
      type="button"
      aria-label={sidebarMode === 'full' ? 'Collapse the sidebar' : 'Expand the sidebar'}
      onClick={() => {
        setSidebarMode(sidebarMode === 'full' ? 'rail' : 'full')
      }}
      className="ml-auto mr-2 grid size-7 shrink-0 place-items-center rounded-md text-muted-foreground transition-colors hover:bg-sidebar-accent hover:text-foreground"
    >
      <Icon className="size-4" />
    </button>
  )
}

/**
 * Drag-to-resize at the sidebar's trailing edge.
 *
 * The width is written straight to the custom property during the drag and only committed to state
 * on release: re-rendering the whole shell on every pointermove is what makes a resize handle feel
 * like it is lagging behind the cursor.
 */
function ResizeSeam() {
  const { sidebarWidth, setSidebarWidth } = usePrefs()

  const onPointerDown = React.useCallback(
    (event: React.PointerEvent<HTMLDivElement>) => {
      event.preventDefault()
      const startX = event.clientX
      const startWidth = sidebarWidth
      let latest = startWidth

      const onMove = (move: PointerEvent) => {
        latest = Math.min(
          SIDEBAR_MAX_W,
          Math.max(SIDEBAR_MIN_W, startWidth + move.clientX - startX),
        )
        document.documentElement.style.setProperty('--pane-sidebar-w', `${latest}px`)
      }
      const onUp = () => {
        window.removeEventListener('pointermove', onMove)
        window.removeEventListener('pointerup', onUp)
        document.body.style.cursor = ''
        setSidebarWidth(latest)
      }

      document.body.style.cursor = 'col-resize'
      window.addEventListener('pointermove', onMove)
      window.addEventListener('pointerup', onUp)
    },
    [sidebarWidth, setSidebarWidth],
  )

  return (
    <div
      role="separator"
      aria-orientation="vertical"
      aria-label="Resize the sidebar"
      onPointerDown={onPointerDown}
      // Four pixels wide but reaching further with a pseudo-element: a 1px seam
      // is honest visually and miserable to hit.
      className="absolute inset-y-0 right-0 z-10 w-1 cursor-col-resize after:absolute after:inset-y-0 after:-left-1.5 after:w-4 hover:bg-primary/30"
    />
  )
}
