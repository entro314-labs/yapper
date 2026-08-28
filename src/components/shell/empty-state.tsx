import type * as React from 'react'

import { cn } from '@/lib/utils'

/**
 * What a screen shows before it has anything to show. Always names the next action: an empty queue
 * with no way forward is a dead end, not a state.
 */
export function EmptyState({
  icon: Icon,
  title,
  description,
  action,
  className,
}: {
  icon: React.ElementType
  title: string
  description: string
  action?: React.ReactNode
  className?: string
}) {
  return (
    <div className={cn('grid place-items-center px-6 py-20 text-center', className)}>
      <div className="max-w-sm">
        <div className="mx-auto mb-4 grid size-11 place-items-center rounded-xl border border-border/60 bg-muted/40">
          <Icon className="size-5 text-muted-foreground" />
        </div>
        <h2 className="font-display text-base font-semibold tracking-tight">{title}</h2>
        <p className="mt-1.5 text-sm leading-relaxed text-muted-foreground">{description}</p>
        {action ? <div className="mt-5 flex justify-center">{action}</div> : null}
      </div>
    </div>
  )
}
