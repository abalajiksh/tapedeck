import { sveltekit } from '@sveltejs/kit/vite';
import { defineConfig } from 'vite';
import { readFileSync } from 'fs';

const pkg = JSON.parse(readFileSync('./package.json', 'utf-8'));

export default defineConfig({
	plugins: [sveltekit()],
	define: {
		__UI_VERSION__: JSON.stringify(pkg.version),
	},
	server: {
		port: 5173,
		proxy: {
			// Proxy API calls to the Rust backend during development
			'/1': 'http://localhost:8080',
			'/api': 'http://localhost:8080',
			'/admin': 'http://localhost:8080',
			'/health': 'http://localhost:8080'
		}
	}
});
