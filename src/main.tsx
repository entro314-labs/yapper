import './index.css'

import { QueryClientProvider } from '@tanstack/react-query'
import { RouterProvider, createRouter } from '@tanstack/react-router'
import React from 'react'
import ReactDOM from 'react-dom/client'
import { toast } from 'sonner'

import { NotFoundScreen, RouteErrorScreen } from '@/components/shell/error-screen'
import { attachEventBridge, queryClient } from '@/lib/query'

import { routeTree } from './routeTree.gen'

// Kept in module scope so Vite's HMR can detach between hot reloads instead of
// stacking a new listener on every save.
let detachEvents: (() => void) | null = null

const router = createRouter({
  routeTree,
  context: { queryClient },
  defaultPreload: 'intent',
  defaultPreloadStaleTime: 0,
  defaultErrorComponent: RouteErrorScreen,
  defaultNotFoundComponent: NotFoundScreen,
})

declare module '@tanstack/react-router' {
  interface Register {
    router: typeof router
  }
}

const root = document.getElementById('root')
if (!root) throw new Error('index.html is missing #root')

// Rendered immediately: first paint must not wait on IPC. The event bridge
// attaches in the background — queries fetch on their own regardless, and the
// bridge only adds push-based invalidation on top.
ReactDOM.createRoot(root).render(
  <React.StrictMode>
    <QueryClientProvider client={queryClient}>
      <RouterProvider router={router} />
    </QueryClientProvider>
  </React.StrictMode>,
)

async function connectEventBridge() {
  try {
    detachEvents = await attachEventBridge(queryClient)
  } catch (err) {
    const detail = err instanceof Error ? err.message : String(err)
    toast.error('Live updates unavailable', {
      description: `Yapper could not subscribe to the scheduler (${detail}). The queue still loads, but posts going out will not appear until you navigate. Restart to retry.`,
      duration: Number.POSITIVE_INFINITY,
    })
  }
}
void connectEventBridge()

if (import.meta.hot) {
  import.meta.hot.dispose(() => {
    detachEvents?.()
    detachEvents = null
  })
}
