import * as React from 'react'

import { cn } from '@/lib/utils'

/**
 * The composer's writing surface. A plain textarea on purpose: every destination here takes plain
 * text, so a rich editor would render formatting that none of them would carry — the post would
 * look one way here and another everywhere else.
 */
function Textarea({ className, ...props }: React.ComponentProps<'textarea'>) {
  return (
    <textarea
      data-slot="textarea"
      className={cn(
        'w-full min-w-0 resize-none rounded-md border border-input bg-background/40 px-3 py-2.5',
        'text-sm leading-relaxed outline-none',
        'shadow-[inset_0_1px_0_0_oklch(0_0_0_/_0.04)]',
        'transition-[background-color,border-color,box-shadow] duration-150',
        'hover:border-input/80',
        'focus-visible:border-ring/60 focus-visible:bg-background focus-visible:ring-3 focus-visible:ring-ring/30',
        'placeholder:text-muted-foreground',
        'disabled:pointer-events-none disabled:opacity-50',
        'dark:bg-input/20 dark:shadow-[inset_0_1px_0_0_oklch(1_0_0_/_0.04)]',
        className,
      )}
      {...props}
    />
  )
}

export { Textarea }
