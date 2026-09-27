import { S3Client } from '@aws-sdk/client-s3'

/**
 * Cloudflare R2 through its S3-compatible API.
 *
 * R2 is the store for one narrow job: holding a publish attachment at a public URL for as long as
 * it takes Meta to come and fetch it. See the privacy policy for why that indirection is forced —
 * the Threads and Instagram publishing endpoints take a URL and fetch it themselves, and offer no
 * upload path at all.
 *
 * The client is created lazily and cached on the module, so a cold serverless invocation that never
 * touches media never builds one.
 */
let cached: S3Client | null = null

export interface R2Config {
  client: S3Client
  bucket: string
  /** A public bucket domain, when one is bound. Absent means media is served back through `/m`. */
  publicBaseUrl: string | null
}

/**
 * `null` when R2 is not configured. Deliberately not a throw: the legal pages and the OAuth bounce
 * are the whole point of this deployment and must work without any storage at all, so a site
 * deployed only for those is a valid deployment rather than a broken one.
 */
export function r2(): R2Config | null {
  const accountId = process.env.R2_ACCOUNT_ID
  const accessKeyId = process.env.R2_ACCESS_KEY_ID
  const secretAccessKey = process.env.R2_SECRET_ACCESS_KEY
  const bucket = process.env.R2_BUCKET

  if (!accountId || !accessKeyId || !secretAccessKey || !bucket) return null

  cached ??= new S3Client({
    // R2 has no regions; the S3 client insists on one, and `auto` is what
    // Cloudflare documents for this.
    region: 'auto',
    endpoint: `https://${accountId}.r2.cloudflarestorage.com`,
    credentials: { accessKeyId, secretAccessKey },
    // The SDK's default flexible checksums write an `x-amz-checksum-crc32`
    // into every presigned PUT URL, computed over the EMPTY body it signs —
    // so the real upload would fail the check. Checksums only where the S3
    // API itself demands one.
    requestChecksumCalculation: 'WHEN_REQUIRED',
    responseChecksumValidation: 'WHEN_REQUIRED',
  })

  return {
    client: cached,
    bucket,
    publicBaseUrl: process.env.R2_PUBLIC_BASE_URL?.replace(/\/+$/, '') ?? null,
  }
}

/**
 * The media types that may be stored, mapped to the extension the key gets.
 *
 * A closed set, matching what the desktop app will resolve off disk. It is not a general file host:
 * an open one on a public URL is an abuse vector, and nothing here needs to accept anything else.
 */
export const ALLOWED_TYPES: Readonly<Record<string, string>> = {
  'image/jpeg': 'jpg',
  'image/png': 'png',
  'image/gif': 'gif',
  'image/webp': 'webp',
  'video/mp4': 'mp4',
}

/** Mirrors the desktop app's own attachment ceiling. */
export const MAX_BYTES = 40 * 1024 * 1024
