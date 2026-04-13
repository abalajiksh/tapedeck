import { sveltekit } from '@sveltejs/kit/vite';
import { defineConfig } from 'vite';

export default defineConfig({
	plugins: [sveltekit()],
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
