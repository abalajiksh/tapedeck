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
		}
	}
};

export default config;
