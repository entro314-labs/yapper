import { Input as InputPrimitive } from '@base-ui/react/input'
import * as React from 'react'

import { cn } from '@/lib/utils'

function Input({ className, type, ...props }: React.ComponentProps<'input'>) {
  return (
    <InputPrimitive
      type={type}
      data-slot="input"
      className={cn(
        // Native form-field treatment: subtle inset highlight along
        // the top edge gives the field a sense of "depth" without
        // adding a heavy border. Focus state swaps to a sharper ring
        // + slightly elevated card surface — feels closer to a macOS
        // text field than a web input.
        'h-8 w-full min-w-0 rounded-md border border-input bg-background/40 px-2.5 py-1 text-base outline-none',
        'shadow-[inset_0_1px_0_0_oklch(0_0_0_/_0.04)]',
        'transition-[background-color,border-color,box-shadow] duration-150',
        'hover:border-input/80 hover:bg-background/60',
        'focus-visible:border-ring/60 focus-visible:bg-background focus-visible:ring-3 focus-visible:ring-ring/30',
        'file:inline-flex file:h-6 file:border-0 file:bg-transparent file:text-sm file:font-medium file:text-foreground',
        'placeholder:text-muted-foreground',
        'disabled:pointer-events-none disabled:cursor-not-allowed disabled:bg-input/50 disabled:opacity-50',
        'aria-invalid:border-destructive aria-invalid:ring-3 aria-invalid:ring-destructive/20',
        'md:text-sm',
        // Dark: black inset highlight is invisible on the translucent
        // input surface, so flip to a faint white top-edge highlight
        // that gives the field the same recessed-but-lit feel.
        'dark:bg-input/20 dark:hover:bg-input/30 dark:focus-visible:bg-background/30',
        'dark:shadow-[inset_0_1px_0_0_oklch(1_0_0_/_0.04)]',
        'dark:disabled:bg-input/80 dark:aria-invalid:border-destructive/50 dark:aria-invalid:ring-destructive/40',
        className,
      )}
      {...props}
    />
  )
}

export { Input }
