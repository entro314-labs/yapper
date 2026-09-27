/**
 * The IPC registry. Every command and event name lives here rather than as a string literal at the
 * call site, so a rename in Rust breaks the build in one place instead of failing silently at
 * runtime.
 */
export const IPC_COMMANDS = {
  listPlatforms: 'list_platforms',
  listAccounts: 'list_accounts',
  connectAccount: 'connect_account',
  deliverAuthCallback: 'deliver_auth_callback',
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

  listNotes: 'list_notes',
  saveNote: 'save_note',
  deleteNote: 'delete_note',

  aiAvailability: 'ai_availability',
  suggestPosts: 'suggest_posts',

  getStats: 'get_stats',
  getRefreshCost: 'get_refresh_cost',
  refreshEngagement: 'refresh_engagement',

  getSettings: 'get_settings',
  updateSettings: 'update_settings',
  oauthRedirectUri: 'oauth_redirect_uri',
  mcpCommand: 'mcp_command',

  getWebHost: 'get_web_host',
  saveWebHost: 'save_web_host',
  forgetWebHost: 'forget_web_host',

  checkForUpdate: 'check_for_update',
  installUpdate: 'install_update',
  restartAndInstall: 'restart_and_install',
  updateStaged: 'update_staged',
  updateInstallSupport: 'update_install_support',

  setWindowMaterial: 'set_window_material',
} as const

export const IPC_EVENTS = {
  /** A post or one of its destinations changed. Payload: the post id, or nothing. */
  queueChanged: 'windbag://queue-changed',
  accountsChanged: 'windbag://accounts-changed',
  /** A connect flow finished, either way. Payload: `AuthOutcome`. */
  auth: 'windbag://auth',
  notesChanged: 'windbag://notes-changed',
  /** An update download is in flight. Payload: `{ downloaded, total }` in bytes. */
  updateProgress: 'windbag://update-progress',
} as const

type ValueOf<T> = T[keyof T]

export type IpcCommand = ValueOf<typeof IPC_COMMANDS>
export type IpcEvent = ValueOf<typeof IPC_EVENTS>
