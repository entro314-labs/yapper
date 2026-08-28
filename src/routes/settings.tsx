import {
  IconAlertTriangle,
  IconCheck,
  IconCircleCheck,
  IconCopy,
  IconExternalLink,
} from '@tabler/icons-react'
import { createFileRoute } from '@tanstack/react-router'
import { openUrl } from '@tauri-apps/plugin-opener'
import * as React from 'react'
import { toast } from 'sonner'

import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Select } from '@/components/ui/select'
import { Switch } from '@/components/ui/switch'
import { brandOf } from '@/lib/platform-brand'
import {
  useAiAvailability,
  useAppCredentials,
  useForgetAppCredentials,
  usePlatforms,
  useRedirectUri,
  useSaveAppCredentials,
  useSettings,
  useUpdateSettings,
} from '@/lib/query'
import { humanMessage } from '@/lib/tauri/client'
import type { AiBackend, PlatformInfo, Settings } from '@/lib/tauri/types'

export const Route = createFileRoute('/settings')({ component: SettingsScreen })

function SettingsScreen() {
  const settings = useSettings()
  const update = useUpdateSettings()
  const platforms = usePlatforms()
  const redirectUri = useRedirectUri()

  const patch = React.useCallback(
    (change: Partial<Settings>) => {
      if (!settings.data) return
      void update.mutateAsync({ ...settings.data, ...change }).catch((err: unknown) => {
        toast.error(humanMessage(err))
      })
    },
    [settings.data, update],
  )

  const current = settings.data
  if (!current) return null

  return (
    <div className="mx-auto flex max-w-2xl flex-col gap-8 p-4">
      <Section title="Appearance">
        <Row label="Theme" hint="System follows your desktop.">
          <Select
            value={current.theme}
            onChange={(event) => {
              patch({ theme: event.target.value as Settings['theme'] })
            }}
            className="w-36"
          >
            <option value="system">System</option>
            <option value="light">Light</option>
            <option value="dark">Dark</option>
          </Select>
        </Row>
        <Row
          label="Window material"
          hint="Frosts the window chrome. macOS and Windows only — Linux compositors mostly refuse, and Yapper falls back to solid."
        >
          <Select
            value={current.windowMaterial}
            onChange={(event) => {
              patch({ windowMaterial: event.target.value as Settings['windowMaterial'] })
            }}
            className="w-36"
          >
            <option value="off">Off</option>
            <option value="standard">Standard</option>
            <option value="strong">Strong</option>
          </Select>
        </Row>
      </Section>

      <Section
        title="Scheduling"
        note="Yapper posts from this machine, so it has to be running when a post is due. Launching at login keeps it in the background."
      >
        <Row label="Launch at login" hint="Starts hidden, with the scheduler running.">
          <Switch
            checked={current.launchAtLogin}
            onCheckedChange={(checked) => {
              patch({ launchAtLogin: checked })
            }}
          />
        </Row>
        <Row
          label="If a post was missed"
          hint="What to do with a post whose time passed while Yapper was closed."
        >
          <Select
            value={current.missedPolicy}
            onChange={(event) => {
              patch({ missedPolicy: event.target.value as Settings['missedPolicy'] })
            }}
            className="w-44"
          >
            <option value="skip">Mark it missed</option>
            <option value="post_late">Post it late</option>
          </Select>
        </Row>
        <Row
          label="Grace window"
          hint="Minutes past due that still count as on time. Closing the laptop briefly should not cost a post."
        >
          <Input
            type="number"
            min={0}
            max={720}
            value={current.graceMinutes}
            onChange={(event) => {
              patch({ graceMinutes: Number(event.target.value) })
            }}
            className="w-20"
          />
        </Row>
      </Section>

      <Section
        title="Assistant"
        note="Yapper ships no API key and no model of its own — it borrows an assistant you already have. It only ever hands you drafts: they land in the composer, where the same limits and the same click still apply before anything goes out."
      >
        <AssistantRow
          value={current.aiBackend}
          onChange={(aiBackend) => {
            patch({ aiBackend })
          }}
        />
        {current.aiBackend === 'claude' || current.aiBackend === 'codex' ? (
          <>
            <Row
              label="Model"
              hint="Passed straight to the CLI. Leave blank for its own default, which is usually right."
            >
              <Input
                value={current.aiModel}
                placeholder="default"
                onChange={(event) => {
                  patch({ aiModel: event.target.value })
                }}
                className="w-40"
              />
            </Row>
            <Row label="Effort" hint="How hard it should think. Blank uses the tool's default.">
              <Select
                value={current.aiEffort}
                onChange={(event) => {
                  patch({ aiEffort: event.target.value })
                }}
                className="w-32"
              >
                <option value="">Default</option>
                <option value="low">Low</option>
                <option value="medium">Medium</option>
                <option value="high">High</option>
              </Select>
            </Row>
          </>
        ) : null}
      </Section>

      <Section
        title="Agent door"
        note="Yapper ships an MCP server so an agent host can read your queue and schedule posts directly — where you see and approve each tool call. Build it with `cargo build --release --bin yapper-mcp`, then register the binary:"
      >
        <AgentDoorRow />
      </Section>

      <Section
        title="Platform apps"
        note="X, Reddit and LinkedIn all gate posting behind a developer app that has to be registered to a person. Yapper cannot ship one, so you register your own and paste its client id here — it is stored in your OS keychain, never in the app's database."
      >
        {redirectUri.data ? <RedirectUriRow uri={redirectUri.data} /> : null}
        {(platforms.data ?? [])
          .filter((info) => info.appFields.length > 0)
          .map((info) => (
            <AppCredentialsRow key={info.id} info={info} />
          ))}
      </Section>
    </div>
  )
}

function Section({
  title,
  note,
  children,
}: {
  title: string
  note?: string
  children: React.ReactNode
}) {
  return (
    <section>
      <h2 className="font-display text-xs font-semibold tracking-wide text-muted-foreground uppercase">
        {title}
      </h2>
      {note ? <p className="mt-1.5 text-xs leading-relaxed text-muted-foreground">{note}</p> : null}
      <div className="mt-3 flex flex-col divide-y divide-border/50 rounded-lg border border-border/60 bg-card/50">
        {children}
      </div>
    </section>
  )
}

function Row({
  label,
  hint,
  children,
}: {
  label: string
  hint?: string
  children: React.ReactNode
}) {
  return (
    <div className="flex items-start gap-4 px-3 py-2.5">
      <div className="min-w-0 flex-1">
        <p className="text-sm">{label}</p>
        {hint ? (
          <p className="mt-0.5 text-xs leading-relaxed text-muted-foreground">{hint}</p>
        ) : null}
      </div>
      <div className="shrink-0 pt-0.5">{children}</div>
    </div>
  )
}

/**
 * The one string every OAuth app must have registered verbatim. Copyable rather than retypeable: a
 * single wrong character here fails the token exchange with a message that names nothing useful.
 */
function RedirectUriRow({ uri }: { uri: string }) {
  const [copied, setCopied] = React.useState(false)
  return (
    <Row
      label="Redirect URI"
      hint="Register this exactly, including the port, on every developer app below."
    >
      <button
        type="button"
        onClick={() => {
          void (async () => {
            await navigator.clipboard.writeText(uri)
            setCopied(true)
            window.setTimeout(() => {
              setCopied(false)
            }, 1600)
          })()
        }}
        className="flex items-center gap-1.5 rounded-md border border-border/60 bg-background/40 px-2 py-1 font-mono text-xs hover:border-border"
      >
        {uri}
        {copied ? (
          <IconCheck className="size-3.5 text-success" />
        ) : (
          <IconCopy className="size-3.5 text-muted-foreground" />
        )}
      </button>
    </Row>
  )
}

/**
 * The assistant picker, with each backend's live availability beside it.
 *
 * The reason a backend is unavailable is shown verbatim — Apple's own words when the model is not
 * resident, the CLI's absence from PATH — because "unavailable" with no reason is a dead end rather
 * than a setting.
 */
function AssistantRow({
  value,
  onChange,
}: {
  value: AiBackend
  onChange: (backend: AiBackend) => void
}) {
  const availability = useAiAvailability()

  return (
    <div className="px-3 py-2.5">
      <Row label="Use" hint="Off by default. Nothing is sent anywhere until you pick one.">
        <Select
          value={value}
          onChange={(event) => {
            onChange(event.target.value as AiBackend)
          }}
          className="w-44"
        >
          <option value="off">Off</option>
          <option value="apple">Apple Intelligence</option>
          <option value="claude">Claude Code</option>
          <option value="codex">Codex</option>
        </Select>
      </Row>

      <ul className="mt-1 flex flex-col gap-1 border-t border-border/50 pt-2.5">
        {(availability.data ?? []).map((entry) => (
          <li key={entry.backend} className="flex items-start gap-1.5 text-xs">
            {entry.available ? (
              <IconCircleCheck className="mt-px size-3.5 shrink-0 text-success" />
            ) : (
              <IconAlertTriangle className="mt-px size-3.5 shrink-0 text-muted-foreground" />
            )}
            <span className="font-medium">{entry.label}</span>
            <span className="min-w-0 flex-1 leading-relaxed text-muted-foreground">
              {entry.reason}
            </span>
          </li>
        ))}
      </ul>
    </div>
  )
}

/** The one command that registers the MCP server, copyable rather than retyped. */
function AgentDoorRow() {
  const [copied, setCopied] = React.useState(false)
  const command = 'claude mcp add yapper -- /path/to/yapper-mcp'

  return (
    <div className="flex flex-col gap-2 px-3 py-2.5">
      <button
        type="button"
        onClick={() => {
          void (async () => {
            await navigator.clipboard.writeText(command)
            setCopied(true)
            window.setTimeout(() => {
              setCopied(false)
            }, 1600)
          })()
        }}
        className="flex items-center gap-1.5 self-start rounded-md border border-border/60 bg-background/40 px-2 py-1 font-mono text-xs hover:border-border"
      >
        {command}
        {copied ? (
          <IconCheck className="size-3.5 text-success" />
        ) : (
          <IconCopy className="size-3.5 text-muted-foreground" />
        )}
      </button>
      <p className="text-xs leading-relaxed text-muted-foreground">
        It reads the same store this app uses, so it works whether or not Yapper is open — but a
        post it schedules still only goes out while Yapper is running. It refuses anything that
        would not fit its destinations, using each platform&apos;s own message.
      </p>
    </div>
  )
}

function AppCredentialsRow({ info }: { info: PlatformInfo }) {
  const stored = useAppCredentials(info.id)
  const save = useSaveAppCredentials()
  const forget = useForgetAppCredentials()
  const [draft, setDraft] = React.useState<Record<string, string>>({})
  const [open, setOpen] = React.useState(false)

  const brand = brandOf(info.id)
  const Icon = brand.icon
  const configured = Boolean(stored.data?.clientId)

  /**
   * What a field shows: the unsaved edit if there is one, otherwise what is stored. Secrets never
   * come back from Rust, so their stored value is ''.
   */
  const value = React.useCallback(
    (key: string) =>
      draft[key] ??
      (key === 'client_id'
        ? (stored.data?.clientId ?? '')
        : key === 'client_secret'
          ? ''
          : (stored.data?.extra[key] ?? '')),
    [draft, stored.data],
  )

  const submit = React.useCallback(() => {
    const clientId = (draft.client_id ?? stored.data?.clientId ?? '').trim()
    if (!clientId) {
      toast.error('The client id cannot be empty.')
      return
    }
    const extra = Object.fromEntries(
      info.appFields
        .filter((field) => field.key !== 'client_id' && field.key !== 'client_secret')
        .map((field) => [field.key, value(field.key)] as const)
        .filter(([, entry]) => entry.trim() !== ''),
    )
    void (async () => {
      try {
        await save.mutateAsync({
          platform: info.id,
          clientId,
          // An empty secret field means "keep what is stored", which is why a
          // blank one is sent as null rather than as an empty string.
          clientSecret: draft.client_secret ?? null,
          extra,
        })
        toast.success(`Saved your ${info.name} app`)
        setDraft({})
        setOpen(false)
      } catch (err) {
        toast.error(humanMessage(err))
      }
    })()
  }, [draft, stored.data, info, save, value])

  return (
    <div className="px-3 py-2.5">
      <div className="flex items-center gap-2.5">
        <Icon className="size-4 shrink-0" style={{ color: brand.tone }} />
        <span className="text-sm">{info.name}</span>
        <span className="text-xs text-muted-foreground">
          {configured ? 'Configured' : 'Not set up'}
        </span>
        <div className="ml-auto flex items-center gap-1.5">
          {info.setupUrl ? (
            <Button
              size="icon-sm"
              variant="ghost"
              aria-label={`Open the ${info.name} developer portal`}
              onClick={() => {
                void openUrl(info.setupUrl ?? '')
              }}
            >
              <IconExternalLink />
            </Button>
          ) : null}
          <Button
            size="sm"
            variant="outline"
            onClick={() => {
              setOpen((current) => !current)
            }}
          >
            {open ? 'Close' : configured ? 'Edit' : 'Set up'}
          </Button>
        </div>
      </div>

      {open ? (
        <div className="mt-3 flex flex-col gap-3 border-t border-border/50 pt-3">
          {info.appFields.map((field) => (
            <label key={field.key} className="flex flex-col gap-1 text-xs">
              <span className="font-medium">
                {field.label}
                {field.required ? <span className="text-destructive"> *</span> : null}
                {field.secret && stored.data?.hasSecret ? (
                  <span className="ml-1.5 font-normal text-muted-foreground">
                    (stored — leave blank to keep it)
                  </span>
                ) : null}
              </span>
              <Input
                type={field.secret ? 'password' : 'text'}
                value={draft[field.key] ?? (field.secret ? '' : value(field.key))}
                placeholder={field.placeholder}
                autoComplete="off"
                onChange={(event) => {
                  setDraft((current) => ({ ...current, [field.key]: event.target.value }))
                }}
              />
              <span className="leading-relaxed text-muted-foreground">{field.help}</span>
            </label>
          ))}
          <div className="flex items-center gap-2">
            <Button size="sm" onClick={submit}>
              Save
            </Button>
            {configured ? (
              <Button
                size="sm"
                variant="destructive"
                onClick={() => {
                  void (async () => {
                    try {
                      // Removes the keychain item only. Accounts already
                      // connected with this app keep working until their tokens
                      // expire — there is nothing to refresh them with after
                      // that, which is what the account list will then say.
                      await forget.mutateAsync({ platform: info.id })
                      toast.success(`Removed your ${info.name} app`)
                      setDraft({})
                      setOpen(false)
                    } catch (err) {
                      toast.error(humanMessage(err))
                    }
                  })()
                }}
              >
                Remove
              </Button>
            ) : null}
          </div>
        </div>
      ) : null}
    </div>
  )
}
