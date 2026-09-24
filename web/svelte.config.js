import adapter from '@sveltejs/adapter-static';
import { vitePreprocess } from '@sveltejs/vite-plugin-svelte';

/** @type {import('@sveltejs/kit').Config} */
const config = {
	preprocess: vitePreprocess(),

	kit: {
		adapter: adapter({
			// Build into ../static/ so rust-embed can include it
			pages: '../static',
			assets: '../static',
			fallback: 'index.html',
			precompress: false,
			strict: true
		}),
		// All API calls go through /api/ or /1/ — prefix with the backend
		paths: {
			base: ''
		},
		// SvelteKit's default is Date.now(), which is written into version.json
		// and a chunk, so every chunk hash changes between two builds of the same
		// commit. scripts/make_obs_source.sh sets this to the commit SHA so that a
		// release tarball is reproducible; everything else keeps the default.
		version: {
			name: process.env.TAPEDECK_UI_VERSION_NAME || Date.now().toString()
		}
	}
};

export default config;
