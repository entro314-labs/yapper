import { IconNotebook, IconPin, IconPinFilled, IconTrash } from '@tabler/icons-react'
import { createFileRoute, useNavigate } from '@tanstack/react-router'
import * as React from 'react'
import { toast } from 'sonner'

import { SparklesIcon } from '@/components/icons/sparkles'
import { EmptyState } from '@/components/shell/empty-state'
import { QueryErrorState } from '@/components/shell/error-screen'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Kbd } from '@/components/ui/kbd'
import { Textarea } from '@/components/ui/textarea'
import { useAnimatedIcon } from '@/lib/animated-icon'
import { IS_MACOS } from '@/lib/chrome'
import { useDeleteNote, useNotes, useSaveNote, useSettings } from '@/lib/query'
import { humanMessage } from '@/lib/tauri/client'
import type { Note } from '@/lib/tauri/types'
import { useUnsavedGuard } from '@/lib/unsaved'
import { cn, formatRelative } from '@/lib/utils'

export const Route = createFileRoute('/notes')({ component: NotesScreen })

/**
 * The scratch surface: where a thought lands before it is a post.
 *
 * Two jobs, and the second is why this is not just a text file. A note is the assistant's CONTEXT —
 * "Draft from this" hands it straight to the composer's suggest panel — and it is the one place in
 * Windbag where writing something carries no obligation to publish it.
 *
 * A list beside an editor rather than a modal per note: notes get re-read while writing the next
 * one, and a dialog would make that a round trip each time.
 */
function NotesScreen() {
  const notes = useNotes()
  const saveNote = useSaveNote()
  const deleteNote = useDeleteNote()
  const navigate = useNavigate()
  const settings = useSettings()
  const [draftRef, draftHover] = useAnimatedIcon()

  const [selected, setSelected] = React.useState<number | null>(null)
  const [title, setTitle] = React.useState('')
  const [body, setBody] = React.useState('')
  const [loaded, setLoaded] = React.useState<number | null>(null)
  const [confirmingDelete, setConfirmingDelete] = React.useState(false)

  const current = React.useMemo(
    () => notes.data?.find((note) => note.id === selected),
    [notes.data, selected],
  )

  // Seeds the editor once per selected note. Keyed on the id rather than on the
  // note object so a background refetch cannot overwrite what is being typed.
  React.useEffect(() => {
    if (!current || loaded === current.id) return
    setTitle(current.title)
    setBody(current.body)
    setLoaded(current.id)
  }, [current, loaded])

  // Compared trimmed, the way Rust stores a note, so a trailing newline left
  // after saving does not read as an unsaved edit.
  const dirty = current
    ? title.trim() !== current.title || body.trim() !== current.body
    : selected === null && (title.trim() !== '' || body.trim() !== '')
  const { confirmDiscard, allowNextNavigation } = useUnsavedGuard(dirty, 'this note')

  const resetEditor = React.useCallback(() => {
    setConfirmingDelete(false)
    setSelected(null)
    setLoaded(null)
    setTitle('')
    setBody('')
  }, [])

  const startNew = React.useCallback(async () => {
    if (await confirmDiscard()) resetEditor()
  }, [confirmDiscard, resetEditor])

  const openNote = React.useCallback(
    async (id: number) => {
      if (id === selected || !(await confirmDiscard())) return
      // An armed delete belongs to the note it was armed on.
      setConfirmingDelete(false)
      setSelected(id)
    },
    [selected, confirmDiscard],
  )

  /** Resolves to whether the note was saved, so a caller can go on only after it was. */
  const save = React.useCallback(async (): Promise<boolean> => {
    if (!body.trim()) {
      toast.error('A note needs some text.')
      return false
    }
    try {
      const id = await saveNote.mutateAsync({
        id: selected,
        title,
        body,
        pinned: current?.pinned ?? false,
      })
      setSelected(id)
      setLoaded(id)
      return true
    } catch (err) {
      toast.error(humanMessage(err))
      return false
    }
  }, [body, title, selected, current, saveNote])

  // Pinning sends the STORED title and body: it is a flag on the note, and
  // using it must not also save whatever half-edit is in the editor.
  const togglePin = React.useCallback(
    async (note: Note) => {
      try {
        await saveNote.mutateAsync({
          id: note.id,
          title: note.title,
          body: note.body,
          pinned: !note.pinned,
        })
      } catch (err) {
        toast.error(humanMessage(err))
      }
    },
    [saveNote],
  )

  // Cmd/Ctrl+S saves, behind the same guard as the Save button. Default
  // prevented even when there is nothing to save, so the keystroke never falls
  // through to the webview.
  const canSave = !saveNote.isPending && body.trim() !== ''
  React.useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (event.key.toLowerCase() !== 's' || !(event.metaKey || event.ctrlKey)) return
      event.preventDefault()
      if (canSave) void save()
    }
    window.addEventListener('keydown', onKey)
    return () => {
      window.removeEventListener('keydown', onKey)
    }
  }, [canSave, save])

  const aiOn = settings.data ? settings.data.aiBackend !== 'off' : false

  if (notes.isError) {
    return <QueryErrorState what="your notes" queries={[notes]} />
  }

  return (
    <div className="grid h-full grid-cols-[minmax(200px,280px)_1fr] overflow-hidden">
      <aside className="flex min-h-0 flex-col border-r border-border/50">
        <div className="flex items-center gap-2 px-3 py-2.5">
          <h2 className="font-display text-xs font-semibold tracking-wide text-muted-foreground uppercase">
            Notes
          </h2>
          <Button
            size="xs"
            variant="outline"
            className="ml-auto"
            onClick={() => {
              void startNew()
            }}
          >
            New
          </Button>
        </div>

        <ul className="min-h-0 flex-1 overflow-y-auto px-2 pb-2">
          {(notes.data ?? []).map((note) => (
            <li key={note.id}>
              <button
                type="button"
                onClick={() => {
                  void openNote(note.id)
                }}
                className={cn(
                  'flex w-full flex-col gap-0.5 rounded-md px-2.5 py-2 text-left transition-colors',
                  note.id === selected
                    ? 'bg-sidebar-accent text-sidebar-accent-foreground'
                    : 'hover:bg-sidebar-accent/60',
                )}
              >
                <span className="flex items-center gap-1.5 text-sm font-medium">
                  {note.pinned ? <IconPinFilled className="size-3 shrink-0 text-primary" /> : null}
                  <span className="truncate">{note.title || firstLine(note)}</span>
                </span>
                <span className="truncate text-xs text-muted-foreground">
                  {formatRelative(note.updatedAt)}
                </span>
              </button>
            </li>
          ))}
        </ul>
      </aside>

      <section className="flex min-h-0 flex-col overflow-hidden">
        {notes.data?.length === 0 && selected === null && !body ? (
          <EmptyState
            icon={IconNotebook}
            title="No notes yet"
            description="Park a thought, a link, a half-formed argument. Notes are also what the assistant reads when it drafts a post for you."
            action={<Button onClick={resetEditor}>Write one</Button>}
          />
        ) : (
          <div className="flex min-h-0 flex-1 flex-col gap-3 p-4">
            <div className="flex items-center gap-2">
              <Input
                value={title}
                onChange={(event) => {
                  setTitle(event.target.value)
                }}
                placeholder="Title (optional)"
                aria-label="Note title"
                className="h-9 flex-1 text-sm"
              />
              {current ? (
                <>
                  <Button
                    size="icon-sm"
                    variant="ghost"
                    aria-label={current.pinned ? 'Unpin' : 'Pin'}
                    onClick={() => {
                      void togglePin(current)
                    }}
                  >
                    {current.pinned ? <IconPinFilled className="text-primary" /> : <IconPin />}
                  </Button>
                  <Button
                    size="icon-sm"
                    variant={confirmingDelete ? 'destructive' : 'ghost'}
                    aria-label={confirmingDelete ? 'Confirm delete' : 'Delete note'}
                    // Two-step, the same as deleting a post from the queue.
                    onClick={() => {
                      if (!confirmingDelete) {
                        setConfirmingDelete(true)
                        window.setTimeout(() => {
                          setConfirmingDelete(false)
                        }, 3000)
                        return
                      }
                      setConfirmingDelete(false)
                      void (async () => {
                        try {
                          await deleteNote.mutateAsync(current.id)
                          resetEditor()
                          toast.success('Deleted')
                        } catch (err) {
                          toast.error(humanMessage(err))
                        }
                      })()
                    }}
                  >
                    <IconTrash />
                  </Button>
                </>
              ) : null}
            </div>

            <Textarea
              value={body}
              onChange={(event) => {
                setBody(event.target.value)
              }}
              placeholder="Whatever it is."
              aria-label="Note"
              className="min-h-0 flex-1 text-[15px]"
            />

            <div className="flex items-center gap-2">
              <span className="text-xs text-muted-foreground tabular-nums">
                {body.length} characters
              </span>
              <div className="ml-auto flex items-center gap-2">
                {/* The wiring that makes notes part of the app rather than a
                    side pocket: a note becomes a draft without being retyped. */}
                {selected ? (
                  <Button
                    variant="outline"
                    onClick={() => {
                      void (async () => {
                        // Saved first rather than asked about: the composer and
                        // the assistant read the STORED note, and this click
                        // asks for what is on screen to become a post.
                        if (dirty && !(await save())) return
                        allowNextNavigation()
                        void navigate({
                          to: '/compose',
                          search: aiOn ? { noteId: selected, suggest: true } : { noteId: selected },
                        })
                      })()
                    }}
                    {...draftHover}
                  >
                    {aiOn ? <SparklesIcon ref={draftRef} data-icon="inline-start" /> : null}
                    {aiOn ? 'Draft from this' : 'Turn into a post'}
                  </Button>
                ) : null}
                <Kbd aria-hidden>{IS_MACOS ? '⌘S' : 'Ctrl+S'}</Kbd>
                <Button
                  disabled={!canSave}
                  aria-keyshortcuts={IS_MACOS ? 'Meta+S' : 'Control+S'}
                  onClick={() => {
                    void save()
                  }}
                >
                  Save
                </Button>
              </div>
            </div>
          </div>
        )}
      </section>
    </div>
  )
}

/** A title for an untitled note: its first line, trimmed to something listable. */
function firstLine(note: Note): string {
  const line = note.body.split('\n').find((value) => value.trim()) ?? 'Untitled'
  return line.length > 48 ? `${line.slice(0, 48)}…` : line
}
