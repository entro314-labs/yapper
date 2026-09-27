import type { PostStatus, TargetStatus } from '@/lib/tauri/types'
import { cn } from '@/lib/utils'

/**
 * The queue's whole vocabulary in six pixels.
 *
 * A dot rather than a label because the queue is scanned, not read: colour and motion carry the
 * state, and the word next to it is the confirmation. The `publishing` dot is the only animated one
 * — it pulses in opacity rather than scale, since a resizing dot in a dense list reads as jitter.
 */
const TONE: Record<PostStatus | TargetStatus, string> = {
  draft: 'bg-muted-foreground/50',
  scheduled: 'bg-primary',
  pending: 'bg-primary',
  publishing: 'bg-primary animate-[var(--animate-carrier)]',
  published: 'bg-success',
  partial: 'bg-warning',
  failed: 'bg-destructive',
  missed: 'bg-warning',
}

export const STATUS_LABEL: Record<PostStatus, string> = {
  draft: 'Draft',
  scheduled: 'Scheduled',
  publishing: 'Sending',
  published: 'Published',
  partial: 'Partly sent',
  failed: 'Failed',
  missed: 'Missed',
}

/**
 * Whether a post can still be changed. A published post is history and a publishing one is in
 * flight; editing or moving either would describe something that did not happen.
 */
export function isEditable(status: PostStatus): boolean {
  return status !== 'published' && status !== 'publishing'
}

export function StatusDot({
  status,
  className,
}: {
  status: PostStatus | TargetStatus
  className?: string
}) {
  return (
    <span
      aria-hidden
      data-status={status}
      className={cn('size-1.5 shrink-0 rounded-full', TONE[status], className)}
    />
  )
}
