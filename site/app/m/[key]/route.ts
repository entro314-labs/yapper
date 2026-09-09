import { GetObjectCommand } from '@aws-sdk/client-s3'
import { NextResponse } from 'next/server'

import { r2 } from '@/lib/r2'

/**
 * `GET /m/<key>` — serve a parked attachment.
 *
 * Only used when no public bucket domain is bound. With `R2_PUBLIC_BASE_URL` set, Meta fetches
 * straight from R2 and this route is never hit.
 *
 * Anonymous by necessity: Meta's servers fetch the URL with no credentials, so nothing here can
 * check one. The unguessable key is the protection, and the object is short-lived.
 */
export const runtime = 'nodejs'

const KEY_PATTERN = /^[a-f0-9]{32,96}\.(jpg|png|gif|webp|mp4)$/

export async function GET(
  _request: Request,
  { params }: { params: Promise<{ key: string }> },
): Promise<NextResponse> {
  const { key } = await params

  // Shape-checked before it reaches the bucket: this is a path segment from an
  // anonymous caller, and only keys this route itself minted can be valid.
  if (!KEY_PATTERN.test(key)) {
    return NextResponse.json({ error: 'Not found.' }, { status: 404 })
  }

  const store = r2()
  if (!store) {
    return NextResponse.json({ error: 'Not found.' }, { status: 404 })
  }

  try {
    const object = await store.client.send(new GetObjectCommand({ Bucket: store.bucket, Key: key }))
    if (!object.Body) {
      return NextResponse.json({ error: 'Not found.' }, { status: 404 })
    }

    return new NextResponse(object.Body.transformToWebStream(), {
      headers: {
        'content-type': object.ContentType ?? 'application/octet-stream',
        ...(object.ContentLength ? { 'content-length': String(object.ContentLength) } : {}),
        // Meta may fetch the same container's media more than once while it
        // processes a video, and the object is immutable for its short life.
        'cache-control': 'public, max-age=3600, immutable',
      },
    })
  } catch {
    // An expired object is the ordinary case here, not an incident: the
    // lifecycle rule removes it once the post has gone out.
    return NextResponse.json({ error: 'Not found.' }, { status: 404 })
  }
}
