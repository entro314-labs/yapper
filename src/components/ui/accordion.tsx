'use client'

import { Accordion as AccordionPrimitive } from '@base-ui/react/accordion'
import { IconChevronRight } from '@tabler/icons-react'

import { cn } from '@/lib/utils'

/**
 * Accordion built on Base UI. `openMultiple` defaults to true so several title trees can be
 * expanded at once. Used for the title-first library view: each title is an item whose panel holds
 * its package tree.
 */
function Accordion({ className, ...props }: AccordionPrimitive.Root.Props) {
  return (
    <AccordionPrimitive.Root
      data-slot="accordion"
      className={cn('flex flex-col gap-2', className)}
      {...props}
    />
  )
}

function AccordionItem({ className, ...props }: AccordionPrimitive.Item.Props) {
  return (
    <AccordionPrimitive.Item
      data-slot="accordion-item"
      className={cn('overflow-hidden rounded-lg border border-border bg-card', className)}
      {...props}
    />
  )
}

/**
 * The clickable header row. `children` is the custom header content (title, badges, summary); a
 * chevron that rotates on open is prepended.
 */
function AccordionTrigger({ className, children, ...props }: AccordionPrimitive.Trigger.Props) {
  return (
    <AccordionPrimitive.Header data-slot="accordion-header" className="flex">
      <AccordionPrimitive.Trigger
        data-slot="accordion-trigger"
        className={cn(
          'group/accordion-trigger flex flex-1 items-center gap-2 px-3 py-2.5 text-left outline-none',
          'transition-colors hover:bg-muted/40 focus-visible:bg-muted/40',
          'data-panel-open:bg-muted/20',
          className,
        )}
        {...props}
      >
        <IconChevronRight
          className="size-4 shrink-0 text-muted-foreground transition-transform duration-200 group-data-panel-open/accordion-trigger:rotate-90"
          aria-hidden
        />
        {children}
      </AccordionPrimitive.Trigger>
    </AccordionPrimitive.Header>
  )
}

function AccordionPanel({ className, children, ...props }: AccordionPrimitive.Panel.Props) {
  return (
    <AccordionPrimitive.Panel
      data-slot="accordion-panel"
      className={cn(
        'h-[var(--accordion-panel-height)] overflow-hidden border-t border-border',
        'transition-[height] duration-200 ease-out',
        'data-starting-style:h-0 data-ending-style:h-0',
        className,
      )}
      {...props}
    >
      {children}
    </AccordionPrimitive.Panel>
  )
}

export { Accordion, AccordionItem, AccordionTrigger, AccordionPanel }
