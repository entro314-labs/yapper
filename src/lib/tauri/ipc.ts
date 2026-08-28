/**
 * The IPC registry. Every command and event name lives here rather than as a string literal at the
 * call site, so a rename in Rust breaks the build in one place instead of failing silently at
 * runtime.
 */
export const IPC_COMMANDS = {
  listPlatforms: 'list_platforms',
  listAccounts: 'list_accounts',
  connectAccount: 'connect_account',
  disconnectAccount: 'disconnect_account',
  getAppCredentials: 'get_app_credentials',
  saveAppCredentials: 'save_app_credentials',
  forgetAppCredentials: 'forget_app_credentials',

  listPosts: 'list_posts',
  listAttempts: 'list_attempts',
  savePost: 'save_post',
  deletePost: 'delete_post',
  publishNow: 'publish_now',
  reschedulePost: 'reschedule_post',
  retryTarget: 'retry_target',

  checkPost: 'check_post',
  resolveMedia: 'resolve_media',

  getSettings: 'get_settings',
  updateSettings: 'update_settings',
  oauthRedirectUri: 'oauth_redirect_uri',
  setWindowMaterial: 'set_window_material',
} as const

export const IPC_EVENTS = {
  /** A post or one of its destinations changed. Payload: the post id, or nothing. */
  queueChanged: 'yapper://queue-changed',
  accountsChanged: 'yapper://accounts-changed',
  /** A destination is being sent right now. Payload: the target id. */
  publishing: 'yapper://publishing',
  /** A connect flow finished, either way. Payload: `AuthOutcome`. */
  auth: 'yapper://auth',
} as const

type ValueOf<T> = T[keyof T]

export type IpcCommand = ValueOf<typeof IPC_COMMANDS>
export type IpcEvent = ValueOf<typeof IPC_EVENTS>
