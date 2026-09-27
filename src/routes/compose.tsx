import { IconTrash, IconUsers } from '@tabler/icons-react'
import { Link, createFileRoute, useNavigate } from '@tanstack/react-router'
import { open } from '@tauri-apps/plugin-dialog'
import * as React from 'react'
import { toast } from 'sonner'

import { SuggestPanel } from '@/components/compose/suggest-panel'
import { AttachFileIcon } from '@/components/icons/attach-file'
import { SendIcon } from '@/components/icons/send'
import { SparklesIcon } from '@/components/icons/sparkles'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Select } from '@/components/ui/select'
import { Textarea } from '@/components/ui/textarea'
import { useAnimatedIcon } from '@/lib/animated-icon'
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
import type { PlatformInfo, Suggestion } from '@/lib/tauri/types'
import { cn, formatBytes, fromLocalInputValue, toLocalInputValue } from '@/lib/utils'

export const Route = createFileRoute('/compose')({
  component: ComposeScreen,
  validateSearch: (
    search: Record<string, unknown>,
  ): { id?: number; noteId?: number; suggest?: boolean } => {
    const positive = (value: unknown) => {
      const parsed = Number(value)
      return Number.isInteger(parsed) && parsed > 0 ? parsed : undefined
    }
    return {
      ...(positive(search.id) === undefined ? {} : { id: positive(search.id) }),
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

function ComposeScreen() {
  const { id, noteId, suggest: openSuggest } = Route.useSearch()
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

  const editing = React.useMemo(
    () => (id ? posts.data?.find((post) => post.id === id) : undefined),
    [id, posts.data],
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

  // Loads an existing post exactly once per id. A plain effect on `editing`
  // would re-seed the form every time the queue refetched and throw away
  // whatever was being typed.
  React.useEffect(() => {
    if (!editing || loadedId === editing.id) return
    setBody(editing.body)
    setTitle(editing.title ?? '')
    setLink(editing.link ?? '')
    setWhen(editing.scheduledAt ? toLocalInputValue(new Date(editing.scheduledAt)) : '')
    setSelected(editing.targets.map((target) => target.accountId))
    setOptions(
      Object.fromEntries(editing.targets.map((target) => [target.accountId, target.options ?? {}])),
    )
    setMedia(
      editing.media.map((item) => ({
        path: item.path,
        altText: item.altText ?? '',
        mime: item.mime,
        bytes: item.bytes,
      })),
    )
    setLoadedId(editing.id)
  }, [editing, loadedId])

  // Arriving from a note: seed the body once, so "Turn into a post" does not
  // mean retyping. Only when composing something NEW — an existing post's own
  // text must never be replaced by a note's.
  React.useEffect(() => {
    if (id !== undefined || noteId === undefined || seededNote === noteId) return
    const note = notes.data?.find((candidate) => candidate.id === noteId)
    if (!note) return
    setSeededNote(noteId)
    if (openSuggest) return
    setTitle((current) => current || note.title)
    setBody((current) => current || note.body)
  }, [id, noteId, seededNote, notes.data, openSuggest])

  const checks = useCheckPost(
    body,
    title || null,
    link.trim() || null,
    media.map(({ mime, bytes }) => ({ mime, bytes })),
    selected,
  )
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
        void navigate({ to: '/' })
      } catch (err) {
        toast.error(humanMessage(err))
      }
    },
    [collect, savePost, publishNow, navigate, when],
  )

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
          <Button
            variant="outline"
            disabled={!canSave || savePost.isPending}
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
}: {
  media: Attachment[]
  onAdd: () => void
  onRemove: (path: string) => void
  onAlt: (path: string, value: string) => void
}) {
  const [attachRef, attachHover] = useAnimatedIcon()
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

      {media.map((item) => (
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
              disabled={stale}
              title={stale ? 'This account needs reconnecting before it can post' : undefined}
              onClick={() => {
                onToggle(account.id)
              }}
              className={cn(
                'inline-flex items-center gap-1.5 rounded-4xl border px-2.5 py-1 text-xs transition-colors',
                on
                  ? 'border-primary/50 bg-primary/10 text-foreground'
                  : 'border-border/60 text-muted-foreground hover:border-border hover:text-foreground',
                stale && 'cursor-not-allowed opacity-50',
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
