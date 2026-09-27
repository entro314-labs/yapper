import { createHmac, timingSafeEqual } from 'node:crypto'

import { NextResponse } from 'next/server'

/**
 * `POST /api/data-deletion` — Meta's data deletion callback.
 *
 * Meta requires either this endpoint or an instructions page; this deployment publishes both, since
 * the honest answer needs explaining and a machine-readable confirmation is still expected.
 *
 * The substantive answer is that there is nothing here to delete. Windbag keeps posts and tokens on
 * the user's own machine; the only thing this deployment ever stores is a publish attachment,
 * anonymously, for as long as it takes Meta to fetch it, expired by a bucket lifecycle rule. No
 * object is associated with a user id, so there is nothing to look up and nothing to remove.
 *
 * The signature is still verified rather than waved through: an endpoint that returns a
 * confirmation code to anyone who posts to it is not a deletion endpoint, it is a decoration.
 *
 * Every Meta app a user registers points its callback here — Threads needs an app of its own beside
 * the one Facebook and Instagram share — and each signs with its own secret. Meta's payload does
 * not say which app sent it, so `META_APP_SECRETS` lists them all, comma-separated, and a request
 * is accepted when it verifies against any one.
 */
export const runtime = 'nodejs'
export const dynamic = 'force-dynamic'

interface SignedRequest {
  user_id?: string
  algorithm?: string
}

export async function POST(request: Request): Promise<NextResponse> {
  const appSecrets = (process.env.META_APP_SECRETS ?? '')
    .split(',')
    .map((secret) => secret.trim())
    .filter((secret) => secret !== '')
  if (appSecrets.length === 0) {
    // Not a 200: reporting success for a request that could not be verified
    // would be a lie about having done something.
    return NextResponse.json(
      {
        error:
          "This deployment has no META_APP_SECRETS set, so the callback cannot verify Meta's signature.",
      },
      { status: 503 },
    )
  }

  const form = await request.formData().catch(() => null)
  const signed = form?.get('signed_request')
  if (typeof signed !== 'string') {
    return NextResponse.json({ error: 'Missing signed_request.' }, { status: 400 })
  }

  const payload = appSecrets.reduce<SignedRequest | null>(
    (found, secret) => found ?? verify(signed, secret),
    null,
  )
  if (!payload) {
    return NextResponse.json({ error: 'Bad signature.' }, { status: 401 })
  }

  const origin = originOf(request)
  // Meta requires both fields: a URL the user can visit to see the status, and
  // a code they can quote. The code identifies this confirmation, not a job —
  // there is no queue behind it because there is nothing to delete.
  return NextResponse.json({
    url: `${origin}/legal/data-deletion`,
    confirmation_code: `nodata-${payload.user_id ?? 'unknown'}-${Date.now().toString(36)}`,
  })
}

/**
 * Meta's `signed_request`: `<base64url signature>.<base64url json payload>`, signed with
 * HMAC-SHA256 over the raw payload segment using the app secret.
 */
function verify(signed: string, appSecret: string): SignedRequest | null {
  const [encodedSignature, encodedPayload] = signed.split('.')
  if (!encodedSignature || !encodedPayload) return null

  const expected = createHmac('sha256', appSecret).update(encodedPayload).digest()
  const offered = Buffer.from(encodedSignature, 'base64url')
  // Length is checked first because timingSafeEqual throws on a mismatch.
  if (offered.length !== expected.length || !timingSafeEqual(offered, expected)) {
    return null
  }

  try {
    const parsed: unknown = JSON.parse(Buffer.from(encodedPayload, 'base64url').toString('utf8'))
    if (typeof parsed !== 'object' || parsed === null) return null
    const payload = parsed as SignedRequest
    // Meta signs with HMAC-SHA256 and says so in the payload; anything else is
    // a payload this code did not actually verify the way it claims to.
    if (payload.algorithm && payload.algorithm.toUpperCase() !== 'HMAC-SHA256') return null
    return payload
  } catch {
    return null
  }
}

function originOf(request: Request): string {
  const host = request.headers.get('x-forwarded-host') ?? request.headers.get('host')
  const proto = request.headers.get('x-forwarded-proto') ?? 'https'
  return host ? `${proto}://${host}` : new URL(request.url).origin
}
