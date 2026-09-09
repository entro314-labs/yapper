/**
 * The document frame every legal page renders into.
 *
 * `.prose` is applied here, once, rather than in `mdx-components.tsx` or in each file — which is
 * what keeps a new document to "drop in a `page.mdx`" with no imports and no wrapper of its own.
 */
export default function LegalLayout({ children }: { children: React.ReactNode }) {
  return <article className="prose">{children}</article>
}
