import createMDX from '@next/mdx'
import type { NextConfig } from 'next'

/**
 * MDX is the whole point of this config: a legal page is prose, and prose belongs in a file you can
 * read in a diff rather than in a string inside a component. Adding `app/legal/<name>/page.mdx` is
 * how a new document gets published — there is no registry to update and no CMS to run.
 */
const nextConfig: NextConfig = {
  pageExtensions: ['ts', 'tsx', 'mdx'],
}

export default createMDX()(nextConfig)
