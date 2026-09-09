import './globals.css'

import type { Metadata } from 'next'
import Link from 'next/link'

/**
 * `metadataBase` is what makes every canonical and Open Graph URL below absolute. Without it Next
 * emits relative ones, and a Meta app reviewer following a shared link to the privacy policy — the
 * exact journey this deployment exists for — can land on an unresolvable URL.
 */
export const metadata: Metadata = {
  metadataBase: new URL('https://windbag.social'),
  title: { default: 'Windbag', template: '%s · Windbag' },
  description:
    'The companion site for Windbag, a desktop composer and scheduler for social platforms.',
  robots: { index: true, follow: true },
  // No `canonical` or `og:url` here: metadata is inherited, so a root-level one
  // would make every legal page declare itself a duplicate of the home page —
  // de-indexing the exact documents this deployment exists to publish. Each page
  // sets its own.
  openGraph: {
    type: 'website',
    siteName: 'Windbag',
    title: 'Windbag',
    description:
      'A desktop composer and scheduler for social platforms. Your posts and credentials stay on your own machine.',
  },
}

/**
 * One shell for every page.
 *
 * The footer carries the three links a Meta app review asks for by name — privacy policy, terms,
 * data deletion — on every page rather than only on an index, because a reviewer arrives at a deep
 * link and should not have to go hunting for the other two.
 */
export default function RootLayout({ children }: { children: React.ReactNode }) {
  return (
    <html lang="en">
      <body className="min-h-dvh antialiased">
        <div className="mx-auto flex min-h-dvh max-w-[46rem] flex-col px-6 py-10 sm:px-8 sm:py-14">
          <header className="mb-12">
            <Link
              href="/"
              className="font-semibold tracking-tight text-[color:var(--color-ink)] no-underline"
            >
              Windbag
            </Link>
          </header>

          <main className="flex-1">{children}</main>

          <footer className="mt-20 border-t border-[color:var(--color-line)] pt-6 text-sm text-[color:var(--color-muted)]">
            <nav className="flex flex-wrap gap-x-5 gap-y-2">
              <Link href="/legal/privacy" className="hover:text-[color:var(--color-ink)]">
                Privacy
              </Link>
              <Link href="/legal/terms" className="hover:text-[color:var(--color-ink)]">
                Terms
              </Link>
              <Link href="/legal/data-deletion" className="hover:text-[color:var(--color-ink)]">
                Data deletion
              </Link>
              <a
                href="https://github.com/entro314-labs/yapper"
                className="hover:text-[color:var(--color-ink)]"
              >
                Source
              </a>
            </nav>
          </footer>
        </div>
      </body>
    </html>
  )
}
