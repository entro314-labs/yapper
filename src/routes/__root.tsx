import type { QueryClient } from '@tanstack/react-query'
import { Outlet, createRootRouteWithContext, useRouterState } from '@tanstack/react-router'
import { AnimatePresence, MotionConfig, motion } from 'motion/react'
import * as React from 'react'
import { Toaster, toast } from 'sonner'

import { PaneTitlebar } from '@/components/shell/pane-titlebar'
import { Sidebar } from '@/components/shell/sidebar'
import { StatusBar } from '@/components/shell/status-bar'
import { TooltipProvider } from '@/components/ui/tooltip'
import { pageVariants } from '@/lib/motion'
import { PrefsProvider } from '@/lib/prefs'
import { useSettings } from '@/lib/query'
import { humanMessage, subscribeEvent } from '@/lib/tauri/client'
import { IPC_EVENTS } from '@/lib/tauri/ipc'
import type { AuthOutcome } from '@/lib/tauri/types'
import { ThemeProvider, useTheme } from '@/lib/theme'
import { cn } from '@/lib/utils'

interface RouterContext {
  queryClient: QueryClient
}

export const Route = createRootRouteWithContext<RouterContext>()({
  component: RootShell,
})

function RootShell() {
  const settings = useSettings()

  return (
    <ThemeProvider settings={settings.data}>
      <PrefsProvider>
        <TooltipProvider delay={250} closeDelay={100}>
          {/* The CSS reduced-motion rule only reaches CSS animations. Motion drives its own, so
              it has to be told about the setting separately. */}
          <MotionConfig reducedMotion="user">
            <AuthListener />
            <ThemedToaster />
            <ShellLayout />
          </MotionConfig>
        </TooltipProvider>
      </PrefsProvider>
    </ThemeProvider>
  )
}

/**
 * A connect flow outlives the command that started it — the user is in a browser — so its result
 * arrives as an event rather than a promise. This is the one place that turns it into something
 * visible.
 */
function AuthListener() {
  React.useEffect(() => {
    let detach: (() => void) | null = null
    void (async () => {
      detach = await subscribeEvent<AuthOutcome>(IPC_EVENTS.auth, (outcome) => {
        if (outcome.ok) {
          toast.success(outcome.message)
        } else {
          toast.error(`Could not connect ${outcome.platform}`, {
            description: humanMessage(outcome.message),
            // Long, because the message is usually a setup instruction — a
            // missing scope, an unregistered redirect URI — not a status.
            duration: 10_000,
          })
        }
      })
    })()
    return () => {
      detach?.()
    }
  }, [])
  return null
}

function ShellLayout() {
  const pathname = useRouterState({ select: (state) => state.location.pathname })

  return (
    // The desk, with a sidebar on it and one content island floating above.
    //
    // The DESK is the only translucent surface — it is what an OS effect shows
    // through. The sidebar has no background of its own and sits directly on it,
    // so the shell has three strata rather than two flat greys. The desk is
    // painted rather than left bare: a webview region with nothing drawn in it
    // does not show the effect behind a transparent window, it renders black.
    <div className="app-desk grid h-screen grid-cols-[auto_1fr] overflow-hidden">
      <Sidebar />
      <div
        className={cn(
          'relative flex min-h-0 min-w-0 flex-col overflow-hidden',
          'bg-[var(--pane-surface)]',
        )}
      >
        <PaneTitlebar />
        <main className="relative min-h-0 flex-1 overflow-hidden">
          <AnimatePresence mode="wait" initial={false}>
            <motion.div
              // Keyed on the route so the transition plays per destination
              // rather than once, ever.
              key={pathname}
              variants={pageVariants}
              initial="initial"
              animate="animate"
              exit="exit"
              className="absolute inset-0 overflow-y-auto"
            >
              <Outlet />
            </motion.div>
          </AnimatePresence>
        </main>
        <StatusBar />
      </div>
    </div>
  )
}

function ThemedToaster() {
  const { resolvedTheme } = useTheme()
  return (
    <Toaster
      position="bottom-right"
      theme={resolvedTheme}
      closeButton
      // 30px clears the 24px status bar plus a gutter.
      offset={30}
      gap={10}
      toastOptions={{
        duration: 4500,
        classNames: {
          // The same chrome as every other raised surface, so a toast reads as
          // part of Windbag rather than as sonner's stock floating bubble.
          toast: cn(
            '!bg-popover/95 backdrop-blur-md text-popover-foreground',
            '!border-border/60 ring-1 ring-foreground/5 !shadow-elevated rounded-lg',
          ),
          title: 'text-[13px] font-semibold tracking-tight',
          description: 'text-[12px] text-muted-foreground leading-snug',
          actionButton:
            '!bg-primary !text-primary-foreground !text-[11px] !font-medium !rounded-md !px-2 !h-6',
          success: '!text-success [&_[data-icon]]:!text-success',
          error: '!text-destructive [&_[data-icon]]:!text-destructive',
          warning: '!text-warning [&_[data-icon]]:!text-warning',
        },
      }}
    />
  )
}
