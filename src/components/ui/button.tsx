import { Button as ButtonPrimitive } from '@base-ui/react/button'
import { cva } from 'class-variance-authority'
import type { VariantProps } from 'class-variance-authority'

import { cn } from '@/lib/utils'

const buttonVariants = cva(
  // Base feels like an OS control: inner highlight on the top edge so
  // even bare colors read as a slightly raised surface, hairline
  // ring for definition against busy backgrounds, snappy active press
  // (translate-y instead of a scale so neighboring layout stays put).
  "group/button inline-flex shrink-0 items-center justify-center rounded-md border border-transparent bg-clip-padding text-sm font-medium whitespace-nowrap outline-none select-none transition-[background-color,box-shadow,transform,border-color,color] duration-150 ease-out focus-visible:ring-3 focus-visible:ring-ring/40 active:not-aria-[haspopup]:translate-y-px disabled:pointer-events-none disabled:opacity-50 aria-invalid:border-destructive aria-invalid:ring-3 aria-invalid:ring-destructive/20 dark:aria-invalid:border-destructive/50 dark:aria-invalid:ring-destructive/40 [&_svg]:pointer-events-none [&_svg]:shrink-0 [&_svg:not([class*='size-'])]:size-4",
  {
    variants: {
      variant: {
        // Default: primary fill + an inset top-edge highlight so it
        // looks like a real button cap rather than a tinted div, and
        // a faint subtle outer shadow that sells the lift.
        default:
          'bg-primary text-primary-foreground shadow-[0_1px_0_0_oklch(0_0_0_/_0.06),inset_0_1px_0_0_oklch(from_var(--primary-foreground)_l_c_h_/_0.18)] hover:bg-primary/95 active:shadow-[inset_0_1px_2px_0_oklch(0_0_0_/_0.12)]',
        // Outline: subtle inset highlight on hover so it reads as a
        // depressible surface instead of a flat box.
        outline:
          'border-border/70 bg-background/40 hover:bg-muted/70 hover:text-foreground hover:shadow-[inset_0_1px_0_0_oklch(from_var(--foreground)_l_c_h_/_0.04)] aria-expanded:bg-muted aria-expanded:text-foreground dark:border-input/80 dark:bg-input/20 dark:hover:bg-input/40',
        secondary:
          'bg-secondary text-secondary-foreground shadow-[inset_0_1px_0_0_oklch(from_var(--foreground)_l_c_h_/_0.04)] hover:bg-secondary/80 aria-expanded:bg-secondary aria-expanded:text-secondary-foreground',
        ghost:
          'hover:bg-muted/70 hover:text-foreground aria-expanded:bg-muted aria-expanded:text-foreground dark:hover:bg-muted/50',
        destructive:
          'bg-destructive/10 text-destructive shadow-[inset_0_1px_0_0_oklch(from_var(--destructive)_l_c_h_/_0.08)] hover:bg-destructive/20 focus-visible:ring-destructive/20 dark:bg-destructive/20 dark:hover:bg-destructive/30 dark:focus-visible:ring-destructive/40',
        link: 'text-primary underline-offset-4 hover:underline',
      },
      size: {
        default:
          'h-8 gap-1.5 px-2.5 has-data-[icon=inline-end]:pr-2 has-data-[icon=inline-start]:pl-2',
        xs: "h-6 gap-1 rounded-[min(var(--radius-md),10px)] px-2 text-xs in-data-[slot=button-group]:rounded-lg has-data-[icon=inline-end]:pr-1.5 has-data-[icon=inline-start]:pl-1.5 [&_svg:not([class*='size-'])]:size-3",
        sm: "h-7 gap-1 rounded-[min(var(--radius-md),12px)] px-2.5 text-[0.8rem] in-data-[slot=button-group]:rounded-lg has-data-[icon=inline-end]:pr-1.5 has-data-[icon=inline-start]:pl-1.5 [&_svg:not([class*='size-'])]:size-3.5",
        lg: 'h-9 gap-1.5 px-2.5 has-data-[icon=inline-end]:pr-2 has-data-[icon=inline-start]:pl-2',
        icon: 'size-8',
        'icon-xs':
          "size-6 rounded-[min(var(--radius-md),10px)] in-data-[slot=button-group]:rounded-lg [&_svg:not([class*='size-'])]:size-3",
        'icon-sm':
          'size-7 rounded-[min(var(--radius-md),12px)] in-data-[slot=button-group]:rounded-lg',
        'icon-lg': 'size-9',
      },
    },
    defaultVariants: {
      variant: 'default',
      size: 'default',
    },
  },
)

function Button({
  className,
  variant = 'default',
  size = 'default',
  ...props
}: ButtonPrimitive.Props & VariantProps<typeof buttonVariants>) {
  return (
    <ButtonPrimitive
      data-slot="button"
      className={cn(buttonVariants({ variant, size, className }))}
      {...props}
    />
  )
}

export { Button, buttonVariants }
