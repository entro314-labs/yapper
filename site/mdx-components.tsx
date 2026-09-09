import type { MDXComponents } from 'mdx/types'

/**
 * Required by `@next/mdx` in the App Router: every MDX file renders through this map.
 *
 * It deliberately overrides nothing. The document styles live in one `.prose` rule in
 * `globals.css`, so an MDX file stays plain prose that reads as prose in a diff — the moment
 * headings need a className here, writing a new legal page stops being "add a file".
 */
export function useMDXComponents(components: MDXComponents): MDXComponents {
  return components
}
