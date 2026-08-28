import * as React from 'react'

import { cn } from '@/lib/utils'

/**
 * Inline keycap. Use to render keyboard shortcuts adjacent to menu items, help text, and the
 * command palette. EmuHub-specific — there is no shadcn registry `kbd` (as of the May 2026 registry
 * snapshot), so this lives next to the registry components as a project addition.
 */
function Kbd({ className, ...props }: React.ComponentProps<'kbd'>) {
  return (
    <kbd
      data-slot="kbd"
      className={cn(
        'inline-flex items-center justify-center min-w-5 h-5 rounded-[5px]',
        'border border-border/60 bg-muted/70 px-1.5 text-[10px] font-medium',
        'text-muted-foreground shadow-[inset_0_-1px_0_0_oklch(0_0_0_/_0.08)]',
        'font-mono leading-none tabular-nums',
        className,
      )}
      {...props}
    />
  )
}

export { Kbd }
