import { IconAlertTriangle, IconPlus, IconTrash, IconUsers } from '@tabler/icons-react'
import { Link, createFileRoute } from '@tanstack/react-router'
import { openUrl } from '@tauri-apps/plugin-opener'
import * as React from 'react'
import { toast } from 'sonner'

import { EmptyState } from '@/components/shell/empty-state'
import { QueryErrorState } from '@/components/shell/error-screen'
import { Button } from '@/components/ui/button'
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog'
import { Input } from '@/components/ui/input'
import { Select } from '@/components/ui/select'
import { Skeleton } from '@/components/ui/skeleton'
import { brandOf } from '@/lib/platform-brand'
import { useAccounts, useConnectAccount, useDisconnectAccount, usePlatforms } from '@/lib/query'
import { humanMessage } from '@/lib/tauri/client'
import type { Account, PlatformInfo } from '@/lib/tauri/types'
import { cn, formatAbsolute, repeatKeys } from '@/lib/utils'

export const Route = createFileRoute('/accounts')({ component: AccountsScreen })

function AccountsScreen() {
  const accounts = useAccounts()
  const platforms = usePlatforms()
  const disconnect = useDisconnectAccount()
  // `account` is set when this is a RE-connect: the dialog then opens on that
  // account's platform, pre-filled with what identifies it.
  const [connecting, setConnecting] = React.useState<{
    info: PlatformInfo
    account?: Account
  } | null>(null)

  return (
    <div className="mx-auto flex max-w-3xl flex-col gap-6 p-4">
      <section>
        <h2 className="font-display mb-2 text-xs font-semibold tracking-wide text-muted-foreground uppercase">
          Connected
        </h2>
        {accounts.isError ? (
          <QueryErrorState
            what="your accounts"
            queries={[accounts]}
            className="rounded-lg border border-dashed border-border/60 py-12"
          />
        ) : accounts.isLoading ? (
          <div className="flex flex-col gap-2">
            {repeatKeys(2, 'account').map((key) => (
              <Skeleton key={key} className="h-[3.25rem] w-full rounded-lg" />
            ))}
          </div>
        ) : accounts.data && accounts.data.length > 0 ? (
          <ul className="flex flex-col gap-2">
            {accounts.data.map((account) => {
              const brand = brandOf(account.platform)
              const Icon = brand.icon
              const stale = account.status === 'needs_reauth'
              return (
                <li
                  key={account.id}
                  className={cn(
                    'flex items-center gap-3 rounded-lg border bg-card/60 px-3 py-2.5',
                    stale ? 'border-destructive/40' : 'border-border/60',
                  )}
                >
                  {account.avatarUrl ? (
                    <img
                      src={account.avatarUrl}
                      alt=""
                      className="size-8 shrink-0 rounded-full object-cover ring-1"
                      style={{ ['--tw-ring-color' as string]: brand.tone }}
                    />
                  ) : (
                    <span
                      className="grid size-8 shrink-0 place-items-center rounded-full bg-muted"
                      style={{ color: brand.tone }}
                    >
                      <Icon className="size-4" />
                    </span>
                  )}

                  <div className="min-w-0 flex-1">
                    <p className="flex items-center gap-1.5 truncate text-sm font-medium">
                      {account.displayName ?? account.handle}
                      <Icon className="size-3.5 shrink-0" style={{ color: brand.tone }} />
                    </p>
                    <p className="truncate text-xs text-muted-foreground">
                      {account.handle}
                      {account.instance ? ` · ${account.instance.replace('https://', '')}` : ''}
                      {account.charLimit ? ` · ${account.charLimit} chars` : ''}
                    </p>
                  </div>

                  {stale ? (
                    <Button
                      size="xs"
                      variant="destructive"
                      disabled={!platforms.data}
                      onClick={() => {
                        const info = platforms.data?.find((item) => item.id === account.platform)
                        if (info) setConnecting({ info, account })
                      }}
                    >
                      <IconAlertTriangle data-icon="inline-start" />
                      Reconnect
                    </Button>
                  ) : account.tokenExpiresAt ? (
                    <span
                      className="hidden text-xs text-muted-foreground sm:block"
                      title={`Access token expires ${formatAbsolute(account.tokenExpiresAt)}`}
                    >
                      Signed in
                    </span>
                  ) : null}

                  <Button
                    size="icon-sm"
                    variant="ghost"
                    aria-label={`Disconnect ${account.handle}`}
                    onClick={() => {
                      void (async () => {
                        try {
                          await disconnect.mutateAsync(account.id)
                          toast.success(`Disconnected ${account.handle}`)
                        } catch (err) {
                          toast.error(humanMessage(err))
                        }
                      })()
                    }}
                  >
                    <IconTrash />
                  </Button>
                </li>
              )
            })}
          </ul>
        ) : (
          <EmptyState
            icon={IconUsers}
            title="No accounts yet"
            description="Bluesky is the quickest — an app password and you are done. The rest need a developer app you register yourself."
            className="rounded-lg border border-dashed border-border/60 py-12"
          />
        )}
      </section>

      <section>
        <h2 className="font-display mb-2 text-xs font-semibold tracking-wide text-muted-foreground uppercase">
          Add
        </h2>
        {platforms.isError ? (
          <QueryErrorState
            compact
            what="the platform list"
            queries={[platforms]}
            className="rounded-lg border border-border/60 px-3 py-2.5"
          />
        ) : null}
        <div className="grid gap-2 sm:grid-cols-2">
          {(platforms.data ?? []).map((info) => {
            const brand = brandOf(info.id)
            const Icon = brand.icon
            return (
              <button
                key={info.id}
                type="button"
                onClick={() => {
                  setConnecting({ info })
                }}
                className="flex items-start gap-3 rounded-lg border border-border/60 bg-card/50 px-3 py-2.5 text-left transition-colors hover:border-border hover:bg-card"
              >
                <span
                  className="mt-0.5 grid size-7 shrink-0 place-items-center rounded-md bg-muted"
                  style={{ color: brand.tone }}
                >
                  <Icon className="size-4" />
                </span>
                <span className="min-w-0">
                  <span className="flex items-center gap-1.5 text-sm font-medium">
                    {info.name}
                    <IconPlus className="size-3 text-muted-foreground" />
                  </span>
                  <span className="mt-0.5 block text-xs leading-relaxed text-muted-foreground">
                    {info.notes}
                  </span>
                </span>
              </button>
            )
          })}
        </div>
      </section>

      {connecting ? (
        <ConnectDialog
          info={connecting.info}
          {...(connecting.account ? { account: connecting.account } : {})}
          onClose={() => {
            setConnecting(null)
          }}
        />
      ) : null}
    </div>
  )
}

/**
 * The connect dialog, drawn entirely from the platform's own [`PlatformInfo`].
 *
 * Nothing here knows what Bluesky or Reddit needs — Rust says which fields to ask for, so adding an
 * adapter needs no change in this file.
 */
function ConnectDialog({
  info,
  account,
  onClose,
}: {
  info: PlatformInfo
  account?: Account
  onClose: () => void
}) {
  const connect = useConnectAccount()
  // A reconnect starts from the server the account lives on (Mastodon's
  // instance), so signing in again cannot land on a different one by default.
  const [fields, setFields] = React.useState((): Record<string, string> =>
    account?.instance && info.connectFields.some((field) => field.key === 'instance')
      ? { instance: account.instance.replace(/^https?:\/\//, '') }
      : {},
  )
  const [busy, setBusy] = React.useState(false)

  const missing = info.connectFields.filter((field) => field.required && !fields[field.key]?.trim())

  // Bluesky offers both a browser handoff and a typed credential, so what the
  // button promises follows the PICKED method rather than the platform's
  // nominal auth kind.
  const method = fields.method ?? info.connectFields.find((f) => f.key === 'method')?.placeholder
  const opensBrowser = info.auth === 'oAuth2' || method === 'oauth'

  // Only a REQUIRED app field is a precondition — the same rule the backend
  // applies. Bluesky's optional client-metadata override must not make it look
  // like it needs a developer app when it does not.
  const needsDeveloperApp = info.appFields.some((field) => field.required)

  const submit = React.useCallback(async () => {
    setBusy(true)
    try {
      await connect.mutateAsync({ platform: info.id, fields })
      if (opensBrowser) {
        toast.info(`Finish signing in to ${info.name} in your browser`)
      }
      onClose()
    } catch (err) {
      toast.error(humanMessage(err))
    } finally {
      setBusy(false)
    }
  }, [connect, info, fields, onClose])

  return (
    <Dialog
      open
      onOpenChange={(open) => {
        if (!open) onClose()
      }}
    >
      <DialogContent className="sm:max-w-md">
        <DialogHeader>
          <DialogTitle>
            {account ? `Reconnect ${account.handle}` : `Connect ${info.name}`}
          </DialogTitle>
          <DialogDescription>
            {account
              ? `Sign in to ${info.name} as ${account.handle} again. The account is updated in place, so posts scheduled to it keep their destination.`
              : info.notes}
          </DialogDescription>
        </DialogHeader>

        {needsDeveloperApp ? (
          <p className="rounded-md bg-muted/60 px-3 py-2 text-xs leading-relaxed text-muted-foreground">
            This needs your own developer app. Add its details in{' '}
            <Link to="/settings" className="text-primary underline-offset-2 hover:underline">
              Settings → Platform apps
            </Link>{' '}
            first, then come back here.
          </p>
        ) : null}

        {/* A form so Enter in any field connects, behind the same guard as the
            button: a disabled submit button also blocks implicit submission. */}
        <form
          className="flex flex-col gap-4"
          onSubmit={(event) => {
            event.preventDefault()
            if (missing.length > 0 || busy) return
            void submit()
          }}
        >
          <div className="flex flex-col gap-3">
            {info.connectFields.map((field) => (
              <label key={field.key} className="flex flex-col gap-1 text-xs">
                <span className="font-medium">
                  {field.label}
                  {field.required ? <span className="text-destructive"> *</span> : null}
                </span>
                {field.choices.length > 0 ? (
                  <Select
                    value={fields[field.key] ?? field.placeholder}
                    onChange={(event) => {
                      setFields((current) => ({ ...current, [field.key]: event.target.value }))
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
                    type={field.secret ? 'password' : 'text'}
                    value={fields[field.key] ?? ''}
                    placeholder={field.placeholder}
                    autoComplete="off"
                    onChange={(event) => {
                      setFields((current) => ({ ...current, [field.key]: event.target.value }))
                    }}
                  />
                )}
                <span className="leading-relaxed text-muted-foreground">{field.help}</span>
              </label>
            ))}
          </div>

          <div className="flex items-center gap-2">
            {info.setupUrl ? (
              <Button
                type="button"
                size="sm"
                variant="ghost"
                onClick={() => {
                  void openUrl(info.setupUrl ?? '')
                }}
              >
                Open {info.name} setup
              </Button>
            ) : null}
            <Button
              type="submit"
              className="ml-auto"
              size="sm"
              disabled={missing.length > 0 || busy}
            >
              {opensBrowser ? 'Continue in browser' : 'Connect'}
            </Button>
          </div>
        </form>
      </DialogContent>
    </Dialog>
  )
}
