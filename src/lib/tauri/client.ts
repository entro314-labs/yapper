import { invoke } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'

import type { IpcCommand, IpcEvent } from './ipc'

/**
 * Rust sends every error as `"[CODE] message"` (see `src-tauri/src/error.rs`). The code is what the
 * query client reads to decide whether retrying could possibly help, so it has to survive the trip
 * intact.
 */
const ERROR_CODE = /^\[([A-Z_]+)]\s*/

export function errorCode(error: unknown): string | null {
  const message = error instanceof Error ? error.message : String(error)
  return ERROR_CODE.exec(message)?.[1] ?? null
}

/** The message with its `[CODE]` prefix stripped — what a human should read. */
export function humanMessage(error: unknown): string {
  const message = error instanceof Error ? error.message : String(error)
  return message.replace(ERROR_CODE, '')
}

function normalizeError(error: unknown): Error {
  if (error instanceof Error) return error
  if (typeof error === 'string') return new Error(error)
  // Anything else is a bug on the Rust side: every command returns AppError,
  // which serializes as a string.
  return new Error(`Unexpected IPC failure: ${JSON.stringify(error)}`)
}

export async function invokeCommand<TResult>(
  command: IpcCommand,
  args?: Record<string, unknown>,
): Promise<TResult> {
  try {
    return await invoke<TResult>(command, args)
  } catch (err) {
    throw normalizeError(err)
  }
}

// The generic forwards Tauri's `listen<T>` payload typing to subscribers;
// inlining `unknown` would force an unsafe cast at every call site instead of
// one typed boundary here.
// oxlint-disable-next-line typescript/no-unnecessary-type-parameters
export async function subscribeEvent<TPayload>(
  event: IpcEvent,
  handler: (payload: TPayload) => void,
): Promise<() => void> {
  return listen<TPayload>(event, (message) => {
    handler(message.payload)
  })
}
