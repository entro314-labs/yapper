import * as React from 'react'

import { cn } from '@/lib/utils'

/**
 * A native `<select>`, styled to match the rest of the chrome.
 *
 * Deliberately not the Base UI listbox: every select in Windbag is a short, closed enumeration (a
 * visibility, a policy, a theme) where the OS's own popup is faster to use and gets keyboard and
 * accessibility behaviour right for free.
 */
function Select({ className, children, ...props }: React.ComponentProps<'select'>) {
  return (
    <select
      data-slot="select"
      className={cn(
        'h-8 w-full min-w-0 appearance-none rounded-md border border-input bg-background/40',
        'px-2.5 pr-7 text-sm outline-none',
        'shadow-[inset_0_1px_0_0_oklch(0_0_0_/_0.04)]',
        'transition-[background-color,border-color,box-shadow] duration-150',
        'hover:border-input/80',
        'focus-visible:border-ring/60 focus-visible:ring-3 focus-visible:ring-ring/30',
        'disabled:pointer-events-none disabled:opacity-50',
        'dark:bg-input/20 dark:shadow-[inset_0_1px_0_0_oklch(1_0_0_/_0.04)]',
        // The chevron, drawn rather than imported so it inherits currentColor
        // and follows the theme without a second element in every select.
        "bg-[url(\"data:image/svg+xml;charset=utf-8,%3Csvg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 16 16' fill='none' stroke='%23888' stroke-width='1.5' stroke-linecap='round'%3E%3Cpath d='m4 6 4 4 4-4'/%3E%3C/svg%3E\")]",
        'bg-[length:14px] bg-[position:right_0.5rem_center] bg-no-repeat',
        className,
      )}
      {...props}
    >
      {children}
    </select>
  )
}

export { Select }
