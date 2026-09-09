import type { Metadata } from 'next'
import Link from 'next/link'

export const metadata: Metadata = {
  alternates: { canonical: '/' },
  openGraph: { url: '/' },
}

const DOCUMENTS = [
  {
    href: '/legal/privacy',
    title: 'Privacy Policy',
    blurb: 'What the app stores, where it stores it, and the two things this site briefly touches.',
  },
  {
    href: '/legal/terms',
    title: 'Terms of Service',
    blurb: 'The terms the software is offered under, and what you remain responsible for.',
  },
  {
    href: '/legal/data-deletion',
    title: 'Data Deletion',
    blurb: 'How to remove a connection and everything associated with it.',
  },
] as const

export default function Home() {
  return (
    <div className="prose">
      <h1>Windbag</h1>
      <p>
        A desktop composer and scheduler for social platforms — Bluesky, Mastodon, Reddit, X,
        LinkedIn, Threads, Instagram and Facebook Pages. Your posts, your accounts and your
        credentials stay on your own machine.
      </p>
      <p className="text-[color:var(--color-muted)]">
        This site exists for three narrow reasons: it publishes the documents below, it provides the
        HTTPS sign-in redirect Meta requires and a desktop app cannot serve, and it briefly holds
        attachments at a public URL so Meta can fetch them — the Threads and Instagram APIs will not
        accept an uploaded file.
      </p>

      <h2>Documents</h2>
      <ul className="not-prose mt-4 grid gap-3">
        {DOCUMENTS.map((doc) => (
          <li key={doc.href}>
            <Link
              href={doc.href}
              className="block rounded-xl border border-[color:var(--color-line)] bg-[color:var(--color-surface)] px-4 py-3.5 no-underline transition-colors hover:border-[color:var(--color-accent)]"
            >
              <span className="block font-medium text-[color:var(--color-ink)]">{doc.title}</span>
              <span className="mt-1 block text-sm leading-relaxed text-[color:var(--color-muted)]">
                {doc.blurb}
              </span>
            </Link>
          </li>
        ))}
      </ul>
    </div>
  )
}
