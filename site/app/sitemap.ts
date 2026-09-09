import type { MetadataRoute } from 'next'

/** Every readable page. Four documents, listed rather than crawled for. */
export default function sitemap(): MetadataRoute.Sitemap {
  const lastModified = new Date('2026-09-09')
  return ['/', '/legal/privacy', '/legal/terms', '/legal/data-deletion'].map((path) => ({
    url: `https://windbag.social${path}`,
    lastModified,
  }))
}
