import { PutObjectCommand } from '@aws-sdk/client-s3'
import { NextResponse } from 'next/server'

import { ALLOWED_TYPES, MAX_BYTES, r2 } from '@/lib/r2'

/**
 * `POST /api/media` — park one attachment at a public URL.
 *
 * The desktop app calls this immediately before building a Threads or Instagram media container,
 * because those endpoints fetch media from a URL and will not accept an upload. The body is the raw
 * file and `Content-Type` is the only other input; there is no multipart envelope because there is
 * exactly one file and nothing else to carry.
 *
 * The response is `{ url, key }`. Nothing is recorded about who uploaded it, and the object is
 * expired by a bucket lifecycle rule rather than deleted here — see the note in `.env.example`.
 */
export const runtime = 'nodejs'
// Reading a body and putting it to R2 must never be answered from a cache.
export const dynamic = 'force-dynamic'

export async function POST(request: Request): Promise<NextResponse> {
  const expected = process.env.WINDBAG_UPLOAD_TOKEN
  if (!expected) {
    return NextResponse.json(
      { error: 'This deployment has no WINDBAG_UPLOAD_TOKEN set, so uploads are disabled.' },
      { status: 503 },
    )
  }

  // Compared whole rather than by prefix: a partial match is not a match.
  const offered = request.headers.get('authorization')
  if (offered !== `Bearer ${expected}`) {
    return NextResponse.json({ error: 'Bad upload token.' }, { status: 401 })
  }

  const store = r2()
  if (!store) {
    return NextResponse.json(
      { error: 'This deployment has no R2 bucket configured, so uploads are disabled.' },
      { status: 503 },
    )
  }

  const contentType = request.headers.get('content-type')?.split(';')[0]?.trim() ?? ''
  const extension = ALLOWED_TYPES[contentType]
  if (!extension) {
    return NextResponse.json(
      { error: `Unsupported media type \`${contentType || 'none'}\`.` },
      { status: 415 },
    )
  }

  const body = new Uint8Array(await request.arrayBuffer())
  if (body.byteLength === 0) {
    return NextResponse.json({ error: 'Empty upload.' }, { status: 400 })
  }
  if (body.byteLength > MAX_BYTES) {
    return NextResponse.json(
      { error: `Attachment is larger than the ${MAX_BYTES / 1024 / 1024} MB limit.` },
      { status: 413 },
    )
  }

  // 32 random bytes. The object is publicly readable by anyone holding the URL —
  // it has to be, since Meta fetches it anonymously — so the key is the only
  // thing standing between it and enumeration, and it is sized accordingly.
  const key = `${crypto.randomUUID().replaceAll('-', '')}${randomHex(16)}.${extension}`

  try {
    await store.client.send(
      new PutObjectCommand({
        Bucket: store.bucket,
        Key: key,
        Body: body,
        ContentType: contentType,
        ContentLength: body.byteLength,
      }),
    )
  } catch (err) {
    // The one place a server-side log earns its keep: the desktop app is told
    // only that the upload failed, and the operator needs R2's actual reason.
    // oxlint-disable-next-line no-console
    console.error('media upload failed', err)
    return NextResponse.json({ error: 'Could not store the attachment.' }, { status: 502 })
  }

  // With a public bucket domain Meta fetches straight from R2 and this
  // deployment never serves the bytes; without one they come back through /m.
  const url = store.publicBaseUrl
    ? `${store.publicBaseUrl}/${key}`
    : `${originOf(request)}/m/${key}`

  return NextResponse.json({ url, key }, { status: 201 })
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
