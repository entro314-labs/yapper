import { createHash, timingSafeEqual } from 'node:crypto'

import { PutObjectCommand } from '@aws-sdk/client-s3'
import { getSignedUrl } from '@aws-sdk/s3-request-presigner'
import { NextResponse } from 'next/server'

import { ALLOWED_TYPES, MAX_BYTES, r2 } from '@/lib/r2'

/**
 * `POST /api/media` — reserve a public URL for one attachment, and hand back a one-time upload
 * address for it.
 *
 * The desktop app calls this immediately before building a Threads or Instagram media container,
 * because those endpoints fetch media from a URL and will not accept an upload. The bytes do NOT
 * pass through here: a Vercel function refuses any request body over 4.5 MB, which is smaller than
 * an ordinary phone video. So the app declares what it is about to send, `{ contentType, size }`,
 * and gets back a presigned R2 PUT URL bound to exactly that type and length, which it uploads to
 * directly.
 *
 * The response is `{ uploadUrl, url }`. Nothing is recorded about who uploaded it, and the object
 * is expired by a bucket lifecycle rule rather than deleted here — see the note in `.env.example`.
 */
export const runtime = 'nodejs'
// Every answer carries a freshly signed URL and must never come from a cache.
export const dynamic = 'force-dynamic'

/**
 * How long the upload address stays usable. It only has to cover the gap between this answer and
 * the app starting its PUT, which is immediate; R2 checks expiry when a request begins, so a slow
 * upload of a large file is not cut off by it.
 */
const UPLOAD_URL_TTL_SECONDS = 10 * 60

interface UploadRequest {
  contentType: string
  size: number
}

export async function POST(request: Request): Promise<NextResponse> {
  const expected = process.env.WINDBAG_UPLOAD_TOKEN
  if (!expected) {
    return NextResponse.json(
      { error: 'This deployment has no WINDBAG_UPLOAD_TOKEN set, so uploads are disabled.' },
      { status: 503 },
    )
  }

  if (!tokenMatches(request.headers.get('authorization'), `Bearer ${expected}`)) {
    return NextResponse.json({ error: 'Bad upload token.' }, { status: 401 })
  }

  const store = r2()
  if (!store) {
    return NextResponse.json(
      { error: 'This deployment has no R2 bucket configured, so uploads are disabled.' },
      { status: 503 },
    )
  }

  const declared = readUploadRequest(await request.text())
  if (!declared) {
    return NextResponse.json(
      { error: 'Expected a JSON body of the form { "contentType": string, "size": number }.' },
      { status: 400 },
    )
  }

  const extension = ALLOWED_TYPES[declared.contentType]
  if (!extension) {
    return NextResponse.json(
      { error: `Unsupported media type \`${declared.contentType || 'none'}\`.` },
      { status: 415 },
    )
  }
  if (declared.size > MAX_BYTES) {
    return NextResponse.json(
      { error: `Attachment is larger than the ${MAX_BYTES / 1024 / 1024} MB limit.` },
      { status: 413 },
    )
  }

  // 32 random bytes. The object is publicly readable by anyone holding the URL —
  // it has to be, since Meta fetches it anonymously — so the key is the only
  // thing standing between it and enumeration, and it is sized accordingly.
  const key = `${crypto.randomUUID().replaceAll('-', '')}${randomHex(16)}.${extension}`

  // Signed locally; R2 is not contacted until the app uploads. Content-Type and
  // Content-Length are both part of the signature, so the declared type and
  // size checked above are the only upload this URL will accept.
  const uploadUrl = await getSignedUrl(
    store.client,
    new PutObjectCommand({
      Bucket: store.bucket,
      Key: key,
      ContentType: declared.contentType,
      ContentLength: declared.size,
    }),
    {
      expiresIn: UPLOAD_URL_TTL_SECONDS,
      signableHeaders: new Set(['content-type', 'content-length']),
    },
  )

  // With a public bucket domain Meta fetches straight from R2 and this
  // deployment never serves the bytes; without one they come back through /m.
  const url = store.publicBaseUrl
    ? `${store.publicBaseUrl}/${key}`
    : `${originOf(request)}/m/${key}`

  return NextResponse.json({ uploadUrl, url }, { status: 201 })
}

/**
 * Constant-time, so response timing cannot be used to recover the token a byte at a time. Both
 * sides are hashed first because `timingSafeEqual` requires equal lengths, and comparing lengths
 * directly would leak the token's.
 */
function tokenMatches(offered: string | null, expected: string): boolean {
  if (offered === null) return false
  const digest = (value: string) => createHash('sha256').update(value).digest()
  return timingSafeEqual(digest(offered), digest(expected))
}

function readUploadRequest(body: string): UploadRequest | null {
  let parsed: unknown
  try {
    parsed = JSON.parse(body)
  } catch {
    return null
  }
  if (typeof parsed !== 'object' || parsed === null) return null
  const contentType = 'contentType' in parsed ? parsed.contentType : undefined
  const size = 'size' in parsed ? parsed.size : undefined
  if (typeof contentType !== 'string' || typeof size !== 'number') return null
  // An empty file is not an attachment, and a fractional or unsafe size cannot
  // be a Content-Length.
  if (!Number.isSafeInteger(size) || size <= 0) return null
  return { contentType: contentType.split(';')[0]?.trim() ?? '', size }
}

function randomHex(bytes: number): string {
  return Array.from(crypto.getRandomValues(new Uint8Array(bytes)), (byte) =>
    byte.toString(16).padStart(2, '0'),
  ).join('')
}

/**
 * The public origin, preferring the forwarded headers a proxy sets — behind one, `request.url` is
 * the internal address and Meta would be handed a URL it cannot reach.
 */
function originOf(request: Request): string {
  const host = request.headers.get('x-forwarded-host') ?? request.headers.get('host')
  const proto = request.headers.get('x-forwarded-proto') ?? 'https'
  return host ? `${proto}://${host}` : new URL(request.url).origin
}
