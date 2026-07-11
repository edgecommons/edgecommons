// @ts-check
import { defineConfig } from 'astro/config';
import starlight from '@astrojs/starlight';
import starlightLinksValidator from 'starlight-links-validator';
import mermaid from 'astro-mermaid';

// https://astro.build/config
export default defineConfig({
	site: 'https://docs.edgecommons.mbreissi.com',
	integrations: [
		// Renders ```mermaid code blocks as diagrams (client-side; follows the light/dark theme).
		// Must come BEFORE starlight() so its markdown transform runs ahead of Expressive Code.
		mermaid({ autoTheme: true, enableLog: false }),
		starlight({
			// Fails the build on broken internal links / heading anchors — guards against the
			// stale-slug class of bug (e.g. the old /reference/configuration/ links).
			plugins: [starlightLinksValidator()],
			// Brand tokens first; the second file maps them onto Starlight's own variables.
			customCss: [
				'./src/styles/edgecommons-tokens.css',
				'./src/styles/edgecommons.css',
			],
			title: 'EdgeCommons',
			// public/favicon.svg is the brand mark, vendored from brand/logos/favicon.svg by
			// `npm run sync` in brand/. Stated explicitly rather than relying on Starlight's
			// implicit /favicon.svg default, so the brand dependency is visible here.
			favicon: '/favicon.svg',
			description:
				'EdgeCommons — one library in four languages (Java, Python, Rust, TypeScript) for building edge components that run on AWS IoT Greengrass, Docker, or Kubernetes.',
			social: [
				{
					icon: 'github',
					label: 'GitHub',
					href: 'https://github.com/edgecommons/edgecommons',
				},
			],
			// Position within each group is controlled by every page's
			// `sidebar.order` frontmatter (autogenerate sorts by order, then title).
			sidebar: [
				{ label: 'Getting Started', items: [{ autogenerate: { directory: 'start' } }] },
				{ label: 'Guides', items: [{ autogenerate: { directory: 'guides' } }] },
				{ label: 'Components', items: [{ autogenerate: { directory: 'components' } }] },
				{ label: 'API Reference', items: [{ autogenerate: { directory: 'reference' } }] },
				{ label: 'Deployment', items: [{ autogenerate: { directory: 'deploy' } }] },
			],
		}),
	],
});
