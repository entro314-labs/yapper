import type { MetadataRoute } from 'next'

/**
 * The legal pages are meant to be indexed — an app reviewer or a user should be able to find the
 * privacy policy without being handed the link.
 *
 * `/api/` and `/oauth/` are excluded because they are not documents: the latter only ever carries a
 * single-use authorization code in its query string.
 *
 * `/m/` is deliberately NOT excluded, though it is just as much a non-document. Meta's own servers
 * fetch attachments from there, and whether that fetcher consults robots.txt is not something this
 * repo can know. The two outcomes are wildly asymmetric: disallowing it risks every Threads and
 * Instagram post with media failing at the fetch — the core feature — while allowing it risks a
 * crawler indexing a random-keyed image that the bucket lifecycle rule deletes within two days. The
 * second is not a real cost.
 */
export default function robots(): MetadataRoute.Robots {
  return {
    rules: { userAgent: '*', allow: '/', disallow: ['/api/', '/oauth/'] },
    sitemap: 'https://windbag.social/sitemap.xml',
  }
}
