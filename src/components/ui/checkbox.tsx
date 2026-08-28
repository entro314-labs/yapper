'use client'

import { Checkbox as CheckboxPrimitive } from '@base-ui/react/checkbox'
import { IconCheck, IconMinus } from '@tabler/icons-react'

import { cn } from '@/lib/utils'

/**
 * Checkbox built on Base UI (same primitive family as the dropdown-menu / switch). Supports the
 * tristate `indeterminate` prop for "some selected" bulk-select headers.
 */
function Checkbox({ className, ...props }: CheckboxPrimitive.Root.Props) {
  return (
    <CheckboxPrimitive.Root
      data-slot="checkbox"
      className={cn(
        'peer size-4 shrink-0 rounded-[5px] border border-input bg-input/20 shadow-xs outline-none',
        'transition-[background-color,border-color,box-shadow] duration-150',
        'focus-visible:ring-3 focus-visible:ring-ring/40',
        'data-checked:border-primary data-checked:bg-primary data-checked:text-primary-foreground',
        'data-indeterminate:border-primary data-indeterminate:bg-primary data-indeterminate:text-primary-foreground',
        'data-disabled:cursor-not-allowed data-disabled:opacity-50',
        className,
      )}
      {...props}
    >
      <CheckboxPrimitive.Indicator
        data-slot="checkbox-indicator"
        className="flex items-center justify-center text-current"
        render={(indicatorProps, state) => (
          <span {...indicatorProps}>
            {state.indeterminate ? (
              <IconMinus className="size-3.5" stroke={3} />
            ) : (
              <IconCheck className="size-3.5" stroke={3} />
            )}
          </span>
        )}
      />
    </CheckboxPrimitive.Root>
  )
}

export { Checkbox }
