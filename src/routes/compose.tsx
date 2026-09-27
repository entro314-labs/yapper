import { IconArrowDown, IconArrowUp, IconFileOff, IconTrash, IconUsers } from '@tabler/icons-react'
import { Link, createFileRoute, useNavigate } from '@tanstack/react-router'
import { open } from '@tauri-apps/plugin-dialog'
import * as React from 'react'
import { toast } from 'sonner'

import { SuggestPanel } from '@/components/compose/suggest-panel'
import { AttachFileIcon } from '@/components/icons/attach-file'
import { SendIcon } from '@/components/icons/send'
import { SparklesIcon } from '@/components/icons/sparkles'
import { EmptyState } from '@/components/shell/empty-state'
import { QueryErrorState } from '@/components/shell/error-screen'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Kbd } from '@/components/ui/kbd'
import { Select } from '@/components/ui/select'
import { Skeleton } from '@/components/ui/skeleton'
import { Textarea } from '@/components/ui/textarea'
import { useAnimatedIcon } from '@/lib/animated-icon'
import { IS_MACOS } from '@/lib/chrome'
import { brandOf } from '@/lib/platform-brand'
import {
  useAccounts,
  useCheckPost,
  useNotes,
  usePlatforms,
  usePosts,
  usePublishNow,
  useResolveMedia,
  useSavePost,
  useSettings,
} from '@/lib/query'
import { humanMessage } from '@/lib/tauri/client'
import type { PlatformInfo, PostDetail, Suggestion } from '@/lib/tauri/types'
import { useUnsavedGuard } from '@/lib/unsaved'
import { cn, formatBytes, fromLocalInputValue, toLocalInputValue } from '@/lib/utils'

export const Route = createFileRoute('/compose')({
  component: ComposeRoute,
  validateSearch: (
    search: Record<string, unknown>,
  ): { id?: number; from?: number; noteId?: number; suggest?: boolean } => {
    const positive = (value: unknown) => {
      const parsed = Number(value)
      return Number.isInteger(parsed) && parsed > 0 ? parsed : undefined
    }
    return {
      ...(positive(search.id) === undefined ? {} : { id: positive(search.id) }),
      ...(positive(search.from) === undefined ? {} : { from: positive(search.from) }),
      ...(positive(search.noteId) === undefined ? {} : { noteId: positive(search.noteId) }),
      ...(search.suggest ? { suggest: true } : {}),
    }
  },
})

interface Attachment {
  path: string
  altText: string
  mime: string
  bytes: number
}

/** Everything the composer edits — what a post seeds, and what "unsaved" is measured against. */
interface Form {
  body: string
  title: string
  link: string
  when: string
  selected: number[]
  options: Record<number, Record<string, string>>
  media: Attachment[]
}

const EMPTY_FORM: Form = {
  body: '',
  title: '',
  link: '',
  when: '',
  selected: [],
  options: {},
  media: [],
}

/**
 * The form a stored post opens as. `keepTime` is false for a copy, which starts as a draft: the
 * original's time is the original's, and on a published post it is already in the past.
 */
function formFromPost(post: PostDetail, keepTime: boolean): Form {
  return {
    body: post.body,
    title: post.title ?? '',
    link: post.link ?? '',
    when: keepTime && post.scheduledAt ? toLocalInputValue(new Date(post.scheduledAt)) : '',
    selected: post.targets.map((target) => target.accountId),
    options: Object.fromEntries(
      post.targets.map((target) => [target.accountId, target.options ?? {}]),
    ),
    media: post.media.map((item) => ({
      path: item.path,
      altText: item.altText ?? '',
      mime: item.mime,
      bytes: item.bytes,
    })),
  }
}

/**
 * A different post, copy or note is a different form, so the screen is remounted per visit. Staying
 * mounted would carry one visit's edits into the next — leaving an edit for a blank compose, even
 * after "Discard", would keep the old text and save it as a new post.
 */
function ComposeRoute() {
  const { id, from, noteId } = Route.useSearch()
  return <ComposeScreen key={`${id ?? ''}:${from ?? ''}:${noteId ?? ''}`} />
}

function ComposeScreen() {
  const { id, from, noteId, suggest: openSuggest } = Route.useSearch()
  const navigate = useNavigate()
  const accounts = useAccounts()
  const platforms = usePlatforms()
  const posts = usePosts()
  const savePost = useSavePost()
  const publishNow = usePublishNow()
  const resolveMedia = useResolveMedia()
  const notes = useNotes()
  const settings = useSettings()
  const [assistantRef, assistantHover] = useAnimatedIcon()
  const [sendRef, sendHover] = useAnimatedIcon()

  // `from` opens a COPY: the source seeds the form, but the post stays
  // unsaved until the user saves it as a new one — nothing ever writes back
  // to the original, which is what makes this safe for a published post.
  const sourceId = id ?? from
  const source = React.useMemo(
    () => (sourceId ? posts.data?.find((post) => post.id === sourceId) : undefined),
    [sourceId, posts.data],
  )

  const [body, setBody] = React.useState('')
  const [title, setTitle] = React.useState('')
  const [link, setLink] = React.useState('')
  const [when, setWhen] = React.useState('')
  const [selected, setSelected] = React.useState<number[]>([])
  const [options, setOptions] = React.useState<Record<number, Record<string, string>>>({})
  const [media, setMedia] = React.useState<Attachment[]>([])
  const [loadedId, setLoadedId] = React.useState<number | null>(null)
  const [suggesting, setSuggesting] = React.useState(Boolean(openSuggest))
  const [seededNote, setSeededNote] = React.useState<number | null>(null)

  // Loads an existing post exactly once per id. A plain effect on `source`
  // would re-seed the form every time the queue refetched and throw away
  // whatever was being typed.
  React.useEffect(() => {
    if (!source || loadedId === source.id) return
    const form = formFromPost(source, id !== undefined)
    setBody(form.body)
    setTitle(form.title)
    setLink(form.link)
    setWhen(form.when)
    setSelected(form.selected)
    setOptions(form.options)
    setMedia(form.media)
    setLoadedId(source.id)
  }, [source, loadedId, id])

  // Arriving from a note: seed the body once, so "Turn into a post" does not
  // mean retyping. Only when composing something NEW — an existing post's own
  // text must never be replaced by a note's.
  React.useEffect(() => {
    if (sourceId !== undefined || noteId === undefined || seededNote === noteId) return
    const note = notes.data?.find((candidate) => candidate.id === noteId)
    if (!note) return
    setSeededNote(noteId)
    if (openSuggest) return
    setTitle((current) => current || note.title)
    setBody((current) => current || note.body)
  }, [sourceId, noteId, seededNote, notes.data, openSuggest])

  // Unsaved means "differs from what this visit opened with" — the stored post,
  // the note it was seeded from, or nothing — derived rather than tracked, so
  // no edit path can forget to mark the form dirty.
  const seedNote =
    sourceId === undefined && noteId !== undefined && !openSuggest
      ? notes.data?.find((note) => note.id === noteId)
      : undefined
  const pristine: Form = source
    ? formFromPost(source, id !== undefined)
    : seedNote
      ? { ...EMPTY_FORM, title: seedNote.title, body: seedNote.body }
      : EMPTY_FORM
  const edited: Form = { body, title, link, when, selected, options, media }
  const dirty = JSON.stringify(edited) !== JSON.stringify(pristine)
  const { allowNextNavigation } = useUnsavedGuard(dirty, 'this post')

  const checks = useCheckPost(body, title || null, media.length, selected)
  const platformById = React.useMemo(
    () => new Map((platforms.data ?? []).map((info) => [info.id, info])),
    [platforms.data],
  )

  const blocking = checks.data?.filter((check) => check.error) ?? []
  const canSave = selected.length > 0 && blocking.length === 0

  const collect = React.useCallback(
    () => ({
      id: id ?? null,
      body,
      title: title.trim() || null,
      link: link.trim() || null,
      scheduledAt: when ? fromLocalInputValue(when) : null,
      targets: selected.map((accountId) => ({
        accountId,
        options: options[accountId] ?? {},
      })),
      media: media.map((item) => ({
        path: item.path,
        altText: item.altText.trim() || null,
      })),
    }),
    [id, body, title, link, when, selected, options, media],
  )

  const save = React.useCallback(
    async (thenPublish: boolean) => {
      try {
        const savedId = await savePost.mutateAsync(collect())
        if (thenPublish) {
          await publishNow.mutateAsync(savedId)
          toast.success('Sending now')
        } else {
          toast.success(when ? 'Scheduled' : 'Saved as a draft')
        }
        allowNextNavigation()
        void navigate({ to: '/' })
      } catch (err) {
        toast.error(humanMessage(err))
      }
    },
    [collect, savePost, publishNow, navigate, when, allowNextNavigation],
  )

  // Cmd/Ctrl+Enter is the Schedule / Save draft button, behind the same guard.
  // Never "Post now": a keystroke that publishes immediately is too easy to
  // hit while still writing.
  const canSubmit = canSave && !savePost.isPending
  React.useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (event.key !== 'Enter' || !(event.metaKey || event.ctrlKey)) return
      event.preventDefault()
      if (canSubmit) void save(false)
    }
    window.addEventListener('keydown', onKey)
    return () => {
      window.removeEventListener('keydown', onKey)
    }
  }, [canSubmit, save])

  const attach = React.useCallback(async () => {
    const picked = await open({
      multiple: true,
      filters: [
        { name: 'Images and video', extensions: ['png', 'jpg', 'jpeg', 'gif', 'webp', 'mp4'] },
      ],
    })
    if (!picked) return
    const paths = (Array.isArray(picked) ? picked : [picked]).filter(
      (path) => !media.some((item) => item.path === path),
    )
    if (paths.length === 0) return

    try {
      // Resolved through Rust at PICK time rather than at save: an unreadable
      // file, or a type none of the platforms take, should be a message now and
      // not a rejected schedule five minutes later.
      const resolved = await resolveMedia.mutateAsync(paths)
      setMedia((current) => [
        ...current,
        ...resolved.map((item) => ({
          path: item.path,
          altText: '',
          mime: item.mime,
          bytes: item.bytes,
        })),
      ])
    } catch (err) {
      toast.error(humanMessage(err))
    }
  }, [media, resolveMedia])

  const assistantOn = settings.data ? settings.data.aiBackend !== 'off' : false

  const applySuggestion = React.useCallback((suggestion: Suggestion) => {
    // Fills the composer and nothing else: from here the draft is subject to
    // the same counters, the same validation and the same human click as
    // anything typed by hand.
    setBody(suggestion.body)
    if (suggestion.title) setTitle(suggestion.title)
    setSuggesting(false)
  }, [])

  // Only the reads this visit actually depends on: the queue matters when a
  // post is being opened, the notes when one is being drafted from.
  const reads = [
    accounts,
    platforms,
    ...(sourceId === undefined ? [] : [posts]),
    ...(noteId === undefined ? [] : [notes]),
  ]
  if (reads.some((query) => query.isError)) {
    return (
      <QueryErrorState
        what={
          id !== undefined ? 'this post' : from !== undefined ? 'the post to copy' : 'the composer'
        }
        queries={reads}
      />
    )
  }

  // An edit opened before the queue arrives would otherwise show an empty
  // form, indistinguishable from the post having no text.
  if (sourceId !== undefined && !source) {
    return posts.isLoading ? (
      <div className="mx-auto flex max-w-3xl flex-col gap-4 p-4">
        <Skeleton className="h-52 w-full rounded-lg" />
        <Skeleton className="h-8 w-full rounded-md" />
      </div>
    ) : (
      <EmptyState
        icon={IconFileOff}
        title="This post no longer exists"
        description={`It was deleted from the queue, so there is nothing here to ${id === undefined ? 'copy' : 'edit'}.`}
        action={<Button render={<Link to="/" />}>Back to the queue</Button>}
      />
    )
  }

  if (accounts.data?.length === 0) {
    return (
      <div className="grid h-full place-items-center px-6 text-center">
        <div className="max-w-sm">
          <div className="mx-auto mb-4 grid size-11 place-items-center rounded-xl border border-border/60 bg-muted/40">
            <IconUsers className="size-5 text-muted-foreground" />
          </div>
          <h2 className="font-display text-base font-semibold tracking-tight">
            Nowhere to post yet
          </h2>
          <p className="mt-1.5 text-sm leading-relaxed text-muted-foreground">
            Connect at least one account. Bluesky needs only an app password — no developer app, no
            review.
          </p>
          <Button className="mt-5" render={<Link to="/accounts" />}>
            Connect an account
          </Button>
        </div>
      </div>
    )
  }

  // Reddit is the only destination that requires a title, so the field appears
  // only once a Reddit account is picked rather than sitting on every draft.
  const needsTitle = selected.some((accountId) => {
    const account = accounts.data?.find((candidate) => candidate.id === accountId)
    return account ? platformById.get(account.platform)?.limits.requiresTitle : false
  })

  return (
    <div className="mx-auto flex max-w-3xl flex-col gap-4 p-4">
      {suggesting ? (
        <SuggestPanel
          accountIds={selected}
          {...(noteId === undefined ? {} : { initialNoteId: noteId })}
          onApply={applySuggestion}
          onClose={() => {
            setSuggesting(false)
          }}
        />
      ) : assistantOn ? (
        <Button
          size="sm"
          variant="outline"
          className="self-start"
          onClick={() => {
            setSuggesting(true)
          }}
          {...assistantHover}
        >
          <SparklesIcon ref={assistantRef} data-icon="inline-start" />
          Draft with the assistant
        </Button>
      ) : null}

      {needsTitle ? (
        <Input
          value={title}
          onChange={(event) => {
            setTitle(event.target.value)
          }}
          placeholder="Title — required by Reddit"
          aria-label="Title"
          className="h-9 text-sm"
        />
      ) : null}

      <Textarea
        value={body}
        onChange={(event) => {
          setBody(event.target.value)
        }}
        rows={9}
        placeholder="What are you posting?"
        aria-label="Post text"
        className="text-[15px]"
      />

      {/* The counters. One per destination, because 280 and 3,000 and 300
          graphemes are not the same number and a single "characters left" would
          be wrong for four of the five. */}
      {selected.length > 0 ? (
        <div className="flex flex-wrap gap-1.5">
          {(checks.data ?? []).map((check) => {
            const brand = brandOf(check.platform)
            const Icon = brand.icon
            const over = check.used > check.limit
            return (
              <span
                key={check.accountId}
                title={check.error ?? `${check.used} of ${check.limit}`}
                className={cn(
                  'inline-flex items-center gap-1.5 rounded-4xl border px-2 py-1 text-xs tabular-nums',
                  check.error
                    ? 'border-destructive/40 bg-destructive/10 text-destructive'
                    : 'border-border/60 text-muted-foreground',
                )}
              >
                <Icon
                  className="size-3.5"
                  style={{ color: check.error ? undefined : brand.tone }}
                />
                <span className="max-w-28 truncate">{check.handle}</span>
                <span className={cn(over && 'font-semibold')}>
                  {check.used}/{check.limit}
                </span>
              </span>
            )
          })}
        </div>
      ) : null}

      {blocking.length > 0 ? (
        <ul className="flex flex-col gap-1 rounded-md bg-destructive/10 px-3 py-2 text-xs text-destructive">
          {blocking.map((check) => (
            <li key={check.accountId}>{humanMessage(check.error ?? '')}</li>
          ))}
        </ul>
      ) : null}

      <Input
        value={link}
        onChange={(event) => {
          setLink(event.target.value)
        }}
        placeholder="Link (optional) — becomes a card on LinkedIn and a link post on Reddit"
        aria-label="Link"
      />

      <AttachmentList
        media={media}
        onAdd={() => {
          void attach()
        }}
        onRemove={(path) => {
          setMedia((current) => current.filter((item) => item.path !== path))
        }}
        onAlt={(path, value) => {
          setMedia((current) =>
            current.map((item) => (item.path === path ? { ...item, altText: value } : item)),
          )
        }}
        onMove={(index, delta) => {
          // Array order IS the saved order: save_post writes each item's
          // position from its index, which is how a carousel is sequenced.
          setMedia((current) => {
            const next = [...current]
            const [item] = next.splice(index, 1)
            if (item) next.splice(index + delta, 0, item)
            return next
          })
        }}
      />

      <Destinations
        selected={selected}
        options={options}
        platformById={platformById}
        onToggle={(accountId) => {
          setSelected((current) =>
            current.includes(accountId)
              ? current.filter((id_) => id_ !== accountId)
              : [...current, accountId],
          )
        }}
        onOption={(accountId, key, value) => {
          setOptions((current) => ({
            ...current,
            [accountId]: { ...current[accountId], [key]: value },
          }))
        }}
      />

      <div className="flex flex-wrap items-center gap-2 border-t border-border/50 pt-4">
        <label htmlFor="scheduled-at" className="text-xs text-muted-foreground">
          When
        </label>
        <Input
          id="scheduled-at"
          type="datetime-local"
          value={when}
          onChange={(event) => {
            setWhen(event.target.value)
          }}
          className="h-8 w-52"
        />
        {/* Times are entered and shown in the machine's own zone and converted to
            UTC on the way in — a scheduling field that silently means UTC is the
            classic way this kind of app lies about when something fires. */}
        <span className="text-xs text-muted-foreground/70">
          {Intl.DateTimeFormat().resolvedOptions().timeZone}
        </span>

        <div className="ml-auto flex items-center gap-2">
          <Kbd aria-hidden>{IS_MACOS ? '⌘↵' : 'Ctrl+↵'}</Kbd>
          <Button
            variant="outline"
            disabled={!canSubmit}
            aria-keyshortcuts={IS_MACOS ? 'Meta+Enter' : 'Control+Enter'}
            onClick={() => {
              void save(false)
            }}
          >
            {when ? 'Schedule' : 'Save draft'}
          </Button>
          <Button
            disabled={!canSave || savePost.isPending || publishNow.isPending}
            onClick={() => {
              void save(true)
            }}
            {...sendHover}
          >
            <SendIcon ref={sendRef} data-icon="inline-start" />
            Post now
          </Button>
        </div>
      </div>
    </div>
  )
}

function AttachmentList({
  media,
  onAdd,
  onRemove,
  onAlt,
  onMove,
}: {
  media: Attachment[]
  onAdd: () => void
  onRemove: (path: string) => void
  onAlt: (path: string, value: string) => void
  onMove: (index: number, delta: -1 | 1) => void
}) {
  const [attachRef, attachHover] = useAnimatedIcon()
  const moveButtons = React.useRef(new Map<string, HTMLButtonElement>())
  const refocus = React.useRef<{ path: string; delta: -1 | 1 } | null>(null)

  // Keeps the keyboard on the item that moved. Reordering can detach the
  // focused row from the DOM, and once it reaches an end its button in that
  // direction is disabled, so focus falls back to the other one.
  React.useEffect(() => {
    const target = refocus.current
    if (!target) return
    refocus.current = null
    const same = moveButtons.current.get(`${target.delta}:${target.path}`)
    const other = moveButtons.current.get(`${-target.delta}:${target.path}`)
    const button = same && !same.disabled ? same : other
    button?.focus()
  }, [media])

  return (
    <div className="flex flex-col gap-2">
      <div className="flex items-center gap-2">
        <Button size="sm" variant="outline" onClick={onAdd} {...attachHover}>
          <AttachFileIcon ref={attachRef} data-icon="inline-start" />
          Attach
        </Button>
        {media.length > 0 ? (
          <span className="text-xs text-muted-foreground">
            {media.length} file{media.length === 1 ? '' : 's'}
          </span>
        ) : null}
      </div>

      {media.map((item, index) => (
        <div
          key={item.path}
          className="flex items-center gap-2 rounded-md border border-border/60 bg-card/50 px-2.5 py-2"
        >
          <span className="max-w-40 shrink-0 truncate text-xs" title={item.path}>
            {item.path.split('/').pop()}
          </span>
          <span className="shrink-0 text-[11px] tabular-nums text-muted-foreground">
            {formatBytes(item.bytes)}
          </span>
          <Input
            value={item.altText}
            onChange={(event) => {
              onAlt(item.path, event.target.value)
            }}
            // Not optional-feeling by accident: alt text is the difference
            // between a post that is readable to everyone and one that is not,
            // so it sits in the row rather than behind a disclosure.
            placeholder="Describe this image"
            aria-label={`Alt text for ${item.path}`}
            className="h-7 flex-1 text-xs"
          />
          {media.length > 1
            ? ([-1, 1] as const).map((delta) => (
                <Button
                  key={delta}
                  ref={(element: HTMLButtonElement | null) => {
                    const key = `${delta}:${item.path}`
                    if (element) moveButtons.current.set(key, element)
                    else moveButtons.current.delete(key)
                  }}
                  size="icon-xs"
                  variant="ghost"
                  aria-label={`Move ${item.path.split('/').pop() ?? ''} ${delta < 0 ? 'up' : 'down'}`}
                  disabled={delta < 0 ? index === 0 : index === media.length - 1}
                  onClick={() => {
                    refocus.current = { path: item.path, delta }
                    onMove(index, delta)
                  }}
                >
                  {delta < 0 ? <IconArrowUp /> : <IconArrowDown />}
                </Button>
              ))
            : null}
          <Button
            size="icon-xs"
            variant="ghost"
            aria-label="Remove"
            onClick={() => {
              onRemove(item.path)
            }}
          >
            <IconTrash />
          </Button>
        </div>
      ))}
    </div>
  )
}

function Destinations({
  selected,
  options,
  platformById,
  onToggle,
  onOption,
}: {
  selected: number[]
  options: Record<number, Record<string, string>>
  platformById: Map<string, PlatformInfo>
  onToggle: (accountId: number) => void
  onOption: (accountId: number, key: string, value: string) => void
}) {
  const accounts = useAccounts()

  return (
    <div className="flex flex-col gap-2">
      <h2 className="font-display text-xs font-semibold tracking-wide text-muted-foreground uppercase">
        Destinations
      </h2>
      <div className="flex flex-wrap gap-1.5">
        {(accounts.data ?? []).map((account) => {
          const brand = brandOf(account.platform)
          const Icon = brand.icon
          const on = selected.includes(account.id)
          const stale = account.status === 'needs_reauth'
          return (
            <button
              key={account.id}
              type="button"
              aria-pressed={on}
              // A stale account cannot be ADDED, but one already picked (an
              // older post, a copy) must stay removable, or the post is stuck
              // aimed at a destination that cannot take it.
              disabled={stale && !on}
              title={stale ? 'This account needs reconnecting before it can post' : undefined}
              onClick={() => {
                onToggle(account.id)
              }}
              className={cn(
                'inline-flex items-center gap-1.5 rounded-4xl border px-2.5 py-1 text-xs transition-colors',
                on
                  ? 'border-primary/50 bg-primary/10 text-foreground'
                  : 'border-border/60 text-muted-foreground hover:border-border hover:text-foreground',
                stale && !on && 'cursor-not-allowed opacity-50',
                stale && on && 'border-destructive/50',
              )}
            >
              <Icon className="size-3.5" style={{ color: brand.tone }} />
              {account.handle}
            </button>
          )
        })}
      </div>

      {/* Per-destination options appear only for the destinations actually
          picked: a subreddit field on a Bluesky-only post is noise. */}
      {selected.map((accountId) => {
        const account = accounts.data?.find((item) => item.id === accountId)
        const info = account ? platformById.get(account.platform) : undefined
        if (!account || !info || info.targetFields.length === 0) return null
        return (
          <div
            key={accountId}
            className="flex flex-col gap-2 rounded-md border border-border/60 bg-card/50 p-2.5"
          >
            <span className="text-xs font-medium">{account.handle}</span>
            {info.targetFields.map((field) => (
              <label key={field.key} className="flex flex-col gap-1 text-xs">
                <span className="text-muted-foreground">
                  {field.label}
                  {field.required ? <span className="text-destructive"> *</span> : null}
                </span>
                {field.choices.length > 0 ? (
                  <Select
                    value={options[accountId]?.[field.key] ?? field.placeholder}
                    onChange={(event) => {
                      onOption(accountId, field.key, event.target.value)
                    }}
                  >
                    {field.choices.map((choice) => (
                      <option key={choice} value={choice}>
                        {choice}
                      </option>
                    ))}
                  </Select>
                ) : (
                  <Input
                    value={options[accountId]?.[field.key] ?? ''}
                    onChange={(event) => {
                      onOption(accountId, field.key, event.target.value)
                    }}
                    placeholder={field.placeholder}
                    className="h-7 text-xs"
                  />
                )}
                <span className="text-muted-foreground/70">{field.help}</span>
              </label>
            ))}
          </div>
        )
      })}
    </div>
  )
}
