import { IconAlertTriangle } from '@tabler/icons-react'
import { Link } from '@tanstack/react-router'
import type { ErrorComponentProps } from '@tanstack/react-router'

import { Button } from '@/components/ui/button'
import { humanMessage } from '@/lib/tauri/client'
import { cn } from '@/lib/utils'

/**
 * The router's error boundary. Shows what broke — a blank screen tells nobody anything.
 *
 * `error` is `unknown` rather than `Error` because that is what it actually is: a throw site can
 * throw anything, and the router types it honestly. `humanMessage` already narrows it.
 */
export function RouteErrorScreen({ error }: ErrorComponentProps) {
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

/** The slice of a `useQuery` result this needs — structural, so any query's result fits. */
interface FailableQuery {
  isError: boolean
  error: Error | null
  refetch: () => Promise<unknown>
}

/**
 * What a screen, or a section of one, shows when a read FAILED. Kept apart from the empty state on
 * purpose: "Nothing queued" over a store that could not be read tells someone their posts are
 * gone.
 *
 * Every failed query's reason is shown, and Retry refetches all of them — the reads a screen
 * depends on usually fail together, and retrying only the first would leave the rest broken.
 * `compact` is the one-line form for a row inside a screen that otherwise loaded.
 */
export function QueryErrorState({
  what,
  queries,
  compact = false,
  className,
}: {
  what: string
  queries: FailableQuery[]
  compact?: boolean
  className?: string
}) {
  const failed = queries.filter((query) => query.isError)
  const reasons = [...new Set(failed.map((query) => humanMessage(query.error)))]
  const retry = () => {
    for (const query of failed) void query.refetch()
  }

  if (compact) {
    return (
      <div role="alert" className={cn('flex items-start gap-2 text-xs', className)}>
        <IconAlertTriangle className="mt-px size-3.5 shrink-0 text-destructive" />
        <span className="min-w-0 flex-1 leading-relaxed">
          <span className="font-medium">Could not load {what}.</span>{' '}
          <span className="text-muted-foreground">{reasons.join(' ')}</span>
        </span>
        <Button size="xs" variant="outline" onClick={retry}>
          Retry
        </Button>
      </div>
    )
  }

  return (
    <div role="alert" className={cn('grid place-items-center px-6 py-20 text-center', className)}>
      <div className="max-w-md">
        <div className="mx-auto mb-4 grid size-11 place-items-center rounded-xl border border-destructive/30 bg-destructive/10">
          <IconAlertTriangle className="size-5 text-destructive" />
        </div>
        <h2 className="font-display text-base font-semibold tracking-tight">
          Could not load {what}
        </h2>
        {reasons.map((reason) => (
          <p key={reason} className="mt-1.5 text-sm leading-relaxed text-muted-foreground">
            {reason}
          </p>
        ))}
        <Button className="mt-5" variant="outline" onClick={retry}>
          Retry
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
        <p className="mt-1.5 text-sm text-muted-foreground">
          That route does not exist in Windbag.
        </p>
        <Button className="mt-5" variant="outline" render={<Link to="/" />}>
          Back to the queue
        </Button>
      </div>
    </div>
  )
}
