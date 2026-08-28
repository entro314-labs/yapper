import { IconAlertTriangle } from '@tabler/icons-react'
import { Link } from '@tanstack/react-router'

import { Button } from '@/components/ui/button'
import { humanMessage } from '@/lib/tauri/client'

/** The router's error boundary. Shows what broke — a blank screen tells nobody anything. */
export function RouteErrorScreen({ error }: { error: Error }) {
  return (
    <div className="grid h-full place-items-center px-6 text-center">
      <div className="max-w-md">
        <div className="mx-auto mb-4 grid size-11 place-items-center rounded-xl border border-destructive/30 bg-destructive/10">
          <IconAlertTriangle className="size-5 text-destructive" />
        </div>
        <h2 className="font-display text-base font-semibold tracking-tight">
          This screen could not load
        </h2>
        <p className="mt-1.5 text-sm leading-relaxed text-muted-foreground">
          {humanMessage(error)}
        </p>
        <Button
          className="mt-5"
          variant="outline"
          onClick={() => {
            window.location.reload()
          }}
        >
          Reload
        </Button>
      </div>
    </div>
  )
}

export function NotFoundScreen() {
  return (
    <div className="grid h-full place-items-center px-6 text-center">
      <div className="max-w-sm">
        <h2 className="font-display text-base font-semibold tracking-tight">Nothing here</h2>
        <p className="mt-1.5 text-sm text-muted-foreground">That route does not exist in Yapper.</p>
        <Button className="mt-5" variant="outline" render={<Link to="/" />}>
          Back to the queue
        </Button>
      </div>
    </div>
  )
}
