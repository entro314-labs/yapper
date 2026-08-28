'use client'

import { Switch as SwitchPrimitive } from '@base-ui/react/switch'

import { cn } from '@/lib/utils'

function Switch({
  className,
  size = 'default',
  ...props
}: SwitchPrimitive.Root.Props & {
  size?: 'sm' | 'default'
}) {
  return (
    <SwitchPrimitive.Root
      data-slot="switch"
      data-size={size}
      className={cn(
        // Larger touch target (mac/iOS-style proportions) + a subtle
        // inner shadow that gives the off-state a "well" feel rather
        // than a flat slab. The accent fill in the on-state ships
        // its own inner highlight so it reads as a depressed switch.
        'peer group/switch relative inline-flex shrink-0 items-center rounded-full border border-transparent',
        'transition-[background-color,box-shadow] duration-200 ease-out outline-none',
        'after:absolute after:-inset-x-3 after:-inset-y-2',
        'focus-visible:ring-3 focus-visible:ring-ring/40',
        'aria-invalid:ring-3 aria-invalid:ring-destructive/20',
        'data-[size=default]:h-[22px] data-[size=default]:w-[38px]',
        'data-[size=sm]:h-[18px] data-[size=sm]:w-[30px]',
        // Off state: pressed-in well. Light mode uses a black inset to
        // suggest a darkened groove; dark mode flips to a subtle white
        // inset against a slightly lifted track so the well doesn't
        // vanish into the panel.
        'data-unchecked:bg-input data-unchecked:shadow-[inset_0_1px_2px_0_oklch(0_0_0_/_0.10)]',
        'dark:data-unchecked:bg-[oklch(1_0_0_/_0.06)] dark:data-unchecked:shadow-[inset_0_1px_2px_0_oklch(0_0_0_/_0.4)]',
        // On state: tinted with a primary fill + an inner highlight
        // that gives the thumb something to "sit" on.
        'data-checked:bg-primary data-checked:shadow-[inset_0_1px_0_0_oklch(from_var(--primary-foreground)_l_c_h_/_0.18)]',
        'data-disabled:cursor-not-allowed data-disabled:opacity-50',
        className,
      )}
      {...props}
    >
      <SwitchPrimitive.Thumb
        data-slot="switch-thumb"
        className={cn(
          // Thumb sits on a 2px inset gutter; the shadow gives it
          // visual lift against either the well (off) or the accent
          // fill (on). In light mode the thumb is white; in dark mode
          // it stays near-white so it stands out on both the dark well
          // and the primary fill (background would be ~panel color and
          // disappear).
          'pointer-events-none block rounded-full bg-white dark:bg-[oklch(0.97_0.005_260)]',
          'shadow-[0_1px_2px_0_oklch(0_0_0_/_0.18),0_0_0_0.5px_oklch(0_0_0_/_0.04)]',
          'dark:shadow-[0_1px_2px_0_oklch(0_0_0_/_0.5),0_0_0_0.5px_oklch(0_0_0_/_0.2)]',
          'transition-transform duration-200 ease-out',
          'group-data-[size=default]/switch:size-[18px]',
          'group-data-[size=sm]/switch:size-[14px]',
          'group-data-[size=default]/switch:data-checked:translate-x-[16px]',
          'group-data-[size=sm]/switch:data-checked:translate-x-[12px]',
          'group-data-[size=default]/switch:data-unchecked:translate-x-[2px]',
          'group-data-[size=sm]/switch:data-unchecked:translate-x-[2px]',
        )}
      />
    </SwitchPrimitive.Root>
  )
}

export { Switch }
