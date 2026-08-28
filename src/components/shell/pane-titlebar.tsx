import { IconPencilPlus } from '@tabler/icons-react'
import { Link, useRouterState } from '@tanstack/react-router'
import * as React from 'react'

import { WindowControls } from '@/components/shell/window-controls'
import { Button } from '@/components/ui/button'
import { TITLEBAR_H } from '@/lib/chrome'

const TITLES: Record<string, string> = {
  '/': 'Queue',
  '/compose': 'Compose',
  '/calendar': 'Calendar',
  '/notes': 'Notes',
  '/stats': 'Stats',
  '/accounts': 'Accounts',
  '/settings': 'Settings',
}

/**
 * The content island's own titlebar. The island owning its chrome — rather than a shared band
 * across the top of the window — is what makes the content column read as one object instead of a
 * slab between two strips.
 */
export function PaneTitlebar() {
  const pathname = useRouterState({ select: (state) => state.location.pathname })
  const title = React.useMemo(() => TITLES[pathname] ?? 'Yapper', [pathname])

  return (
    <header
      data-tauri-drag-region
      className="drag-region flex shrink-0 items-center gap-2 border-b border-border/50 px-3"
      style={{ height: TITLEBAR_H }}
    >
      <h1 className="font-display truncate text-sm font-semibold tracking-tight">{title}</h1>
      <div className="ml-auto flex items-center gap-1.5">
        {pathname === '/compose' ? null : (
          <Button size="sm" render={<Link to="/compose" />}>
            <IconPencilPlus data-icon="inline-start" />
            New post
          </Button>
        )}
        <WindowControls className="-mr-1.5" />
      </div>
    </header>
  )
}
