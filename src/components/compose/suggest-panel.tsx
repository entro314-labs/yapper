import { IconX } from '@tabler/icons-react'
import { Link } from '@tanstack/react-router'
import * as React from 'react'
import { toast } from 'sonner'

import { RefreshCwIcon } from '@/components/icons/refresh-cw'
import { SparklesIcon } from '@/components/icons/sparkles'
import { Button } from '@/components/ui/button'
import { Textarea } from '@/components/ui/textarea'
import { useAnimatedIcon } from '@/lib/animated-icon'
import { useNotes, useSettings, useSuggestPosts } from '@/lib/query'
import { humanMessage } from '@/lib/tauri/client'
import type { Suggestion } from '@/lib/tauri/types'
import { cn } from '@/lib/utils'

/**
 * The assistant, inside the composer.
 *
 * It produces DRAFTS and nothing else. A suggestion is applied by clicking it, which fills the
 * composer's own fields — so the counters, the validation and the human press of "Schedule" all
 * still stand between a generated sentence and a published post. Nothing here can reach the queue.
 *
 * Context is a note (or several) plus whatever free text you add; the notes are read server-side
 * from their ids, so the assistant sees what is actually saved rather than a copy that may have
 * drifted.
 */
export function SuggestPanel({
  accountIds,
  initialNoteId,
  onApply,
  onClose,
}: {
  accountIds: number[]
  initialNoteId?: number
  onApply: (suggestion: Suggestion) => void
  onClose: () => void
}) {
  const settings = useSettings()
  const notes = useNotes()
  const suggest = useSuggestPosts()
  const [draftRef, draftHover] = useAnimatedIcon()

  const [selectedNotes, setSelectedNotes] = React.useState<number[]>(
    initialNoteId ? [initialNoteId] : [],
  )
  const [context, setContext] = React.useState('')
  const [instructions, setInstructions] = React.useState('')
  const [drafts, setDrafts] = React.useState<Suggestion[]>([])

  const backend = settings.data?.aiBackend ?? 'off'
  const hasMaterial = selectedNotes.length > 0 || context.trim().length > 0

  const run = React.useCallback(async () => {
    try {
      const result = await suggest.mutateAsync({
        context,
        instructions,
        noteIds: selectedNotes,
        accountIds,
        count: 3,
      })
      setDrafts(result)
    } catch (err) {
      toast.error(humanMessage(err), { duration: 10_000 })
    }
  }, [suggest, context, instructions, selectedNotes, accountIds])

  if (backend === 'off') {
    return (
      <aside className="rounded-lg border border-border/60 bg-card/50 p-3.5">
        <p className="text-sm">No assistant is switched on.</p>
        <p className="mt-1 text-xs leading-relaxed text-muted-foreground">
          Windbag can borrow one you already have — Apple&apos;s on-device model, or the Claude Code
          or Codex CLI. It never ships a key of its own.
        </p>
        <Button className="mt-3" size="sm" variant="outline" render={<Link to="/settings" />}>
          Pick one in Settings
        </Button>
      </aside>
    )
  }

  return (
    <aside className="flex flex-col gap-3 rounded-lg border border-primary/30 bg-primary/[0.04] p-3.5">
      <header className="flex items-center gap-2">
        <SparklesIcon size={16} className="text-primary" />
        <h2 className="font-display text-xs font-semibold tracking-wide uppercase">
          Draft with {settings.data?.aiBackend === 'apple' ? 'Apple Intelligence' : backend}
        </h2>
        <Button
          size="icon-xs"
          variant="ghost"
          className="ml-auto"
          aria-label="Close"
          onClick={onClose}
        >
          <IconX />
        </Button>
      </header>

      {(notes.data ?? []).length > 0 ? (
        <div className="flex flex-col gap-1.5">
          <span className="text-xs text-muted-foreground">Notes to work from</span>
          <div className="flex flex-wrap gap-1.5">
            {(notes.data ?? []).slice(0, 12).map((note) => {
              const on = selectedNotes.includes(note.id)
              return (
                <button
                  key={note.id}
                  type="button"
                  aria-pressed={on}
                  onClick={() => {
                    setSelectedNotes((current) =>
                      current.includes(note.id)
                        ? current.filter((id) => id !== note.id)
                        : [...current, note.id],
                    )
                  }}
                  className={cn(
                    'max-w-48 truncate rounded-4xl border px-2.5 py-1 text-xs transition-colors',
                    on
                      ? 'border-primary/50 bg-primary/10 text-foreground'
                      : 'border-border/60 text-muted-foreground hover:border-border hover:text-foreground',
                  )}
                >
                  {note.title || note.body.slice(0, 40)}
                </button>
              )
            })}
          </div>
        </div>
      ) : null}

      <Textarea
        value={context}
        onChange={(event) => {
          setContext(event.target.value)
        }}
        rows={3}
        placeholder="Or paste the material here — a link, a changelog, an argument."
        aria-label="Context for the assistant"
        className="text-sm"
      />

      <Textarea
        value={instructions}
        onChange={(event) => {
          setInstructions(event.target.value)
        }}
        rows={2}
        placeholder="Anything about the angle or tone. Optional."
        aria-label="Instructions for the assistant"
        className="text-sm"
      />

      <div className="flex items-center gap-2">
        {accountIds.length > 0 ? (
          <span className="text-xs text-muted-foreground">
            Written to fit the {accountIds.length} destination
            {accountIds.length === 1 ? '' : 's'} you picked
          </span>
        ) : (
          <span className="text-xs text-muted-foreground">
            Pick destinations first and drafts will be written to their limits
          </span>
        )}
        <Button
          size="sm"
          className="ml-auto"
          disabled={!hasMaterial || suggest.isPending}
          onClick={() => {
            void run()
          }}
          {...draftHover}
        >
          {suggest.isPending ? (
            <>
              <RefreshCwIcon data-icon="inline-start" className="animate-spin" />
              Drafting…
            </>
          ) : (
            <>
              <SparklesIcon ref={draftRef} data-icon="inline-start" />
              {drafts.length > 0 ? 'Again' : 'Draft'}
            </>
          )}
        </Button>
      </div>

      {suggest.isPending ? (
        <p className="text-xs text-muted-foreground">
          This runs on your machine and can take up to three minutes on a cold start.
        </p>
      ) : null}

      {drafts.map((draft, index) => (
        <button
          // Drafts have no ids and can repeat; the index IS the identity here,
          // and the list is replaced wholesale on every run.
          key={`draft-${index}`}
          type="button"
          onClick={() => {
            onApply(draft)
            toast.success('Draft applied — edit it before it goes out.')
          }}
          className="flex flex-col gap-1.5 rounded-md border border-border/60 bg-card/70 p-2.5 text-left transition-colors hover:border-primary/50"
        >
          <span className="text-sm leading-relaxed whitespace-pre-wrap">{draft.body}</span>
          <span className="flex items-center gap-2 text-xs text-muted-foreground">
            <span className="tabular-nums">{draft.body.length} chars</span>
            {draft.rationale ? <span className="truncate italic">{draft.rationale}</span> : null}
          </span>
        </button>
      ))}
    </aside>
  )
}
