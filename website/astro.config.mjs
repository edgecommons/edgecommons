// @ts-check
import { defineConfig } from 'astro/config';
import starlight from '@astrojs/starlight';

// https://astro.build/config
export default defineConfig({
	site: 'https://docs.edgecommons.mbreissi.com',
	integrations: [
		starlight({
			title: 'EdgeCommons',
			description:
				'EdgeCommons — one library in four languages (Java, Python, Rust, TypeScript) for building edge components that run on AWS IoT Greengrass, Docker, or Kubernetes.',
			social: [
				{
					icon: 'github',
					label: 'GitHub',
					href: 'https://github.com/edgecommons/ggcommons',
				},
			],
			// Position within each group is controlled by every page's
			// `sidebar.order` frontmatter (autogenerate sorts by order, then title).
			sidebar: [
				{ label: 'Getting Started', items: [{ autogenerate: { directory: 'start' } }] },
				{ label: 'Guides', items: [{ autogenerate: { directory: 'guides' } }] },
				{ label: 'API Reference', items: [{ autogenerate: { directory: 'reference' } }] },
				{ label: 'Deployment', items: [{ autogenerate: { directory: 'deploy' } }] },
			],
		}),
	],
});
