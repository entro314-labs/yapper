import { NextResponse } from 'next/server'

/**
 * `GET /oauth/meta` — the HTTPS redirect Meta insists on, bounced back to the desktop app.
 *
 * Threads, Instagram and Facebook all refuse to register `http://127.0.0.1:8917/callback` as a
 * redirect URI; they require HTTPS. Every other platform Windbag supports accepts the loopback
 * directly. So a Meta app registers THIS url, and this route forwards the browser to the loopback
 * listener the app already has open.
 *
 * It reads nothing and stores nothing. The authorization code passes through in a redirect and is
 * only ever exchanged by the application on the user's own machine, which is also the only party
 * holding the app secret.
 */
export const runtime = 'nodejs'
export const dynamic = 'force-dynamic'

/** Must match `oauth::REDIRECT_PORT` in `src-tauri/src/oauth.rs`. */
const LOOPBACK = 'http://127.0.0.1:8917/callback'

export function GET(request: Request): NextResponse {
  const incoming = new URL(request.url)
  const target = new URL(LOOPBACK)
  // Forwarded verbatim: `code` and `state` are what the app is waiting for, and
  // `error`/`error_description` are what a declined consent screen sends
  // instead — dropping those would leave the app timing out on a sign-in the
  // user already cancelled.
  for (const [key, value] of incoming.searchParams) {
    target.searchParams.set(key, value)
  }

  // An interstitial rather than a bare 302, because the failure mode of a 302
  // here is a browser error page about a refused connection — which tells the
  // user nothing about the actual cause, that Windbag is not running.
  const escaped = target.toString().replaceAll('&', '&amp;').replaceAll('"', '&quot;')
  const body = `<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<meta name="robots" content="noindex">
<title>Returning to Windbag</title>
<style>
  :root { color-scheme: light dark; }
  body {
    margin: 0; min-height: 100dvh; display: grid; place-items: center;
    font: 15px/1.6 ui-sans-serif, system-ui, -apple-system, "Segoe UI", Roboto, sans-serif;
    background: #fbfbfc; color: #22252b; padding: 2rem; text-align: center;
  }
  @media (prefers-color-scheme: dark) { body { background: #16181d; color: #e9ecf1; } }
  .card { max-width: 32rem; }
  h1 { font-size: 1.15rem; font-weight: 600; margin: 0 0 .5rem; }
  p { margin: .5rem 0; opacity: .75; }
  a { color: inherit; }
</style>
</head>
<body>
  <div class="card">
    <h1>Returning you to Windbag…</h1>
    <p>You can close this tab once the app confirms the connection.</p>
    <p><a href="${escaped}">Continue manually</a> if nothing happens. If that fails, Windbag is not
    running — open it and start the connection again.</p>
  </div>
  <script>location.replace(${JSON.stringify(target.toString())})</script>
</body>
</html>`

  return new NextResponse(body, {
    status: 200,
    headers: {
      'content-type': 'text/html; charset=utf-8',
      // The URL carries a single-use authorization code.
      'cache-control': 'no-store',
      referrer: 'no-referrer',
    },
  })
}
