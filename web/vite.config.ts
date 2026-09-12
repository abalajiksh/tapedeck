import { sveltekit } from '@sveltejs/kit/vite';
import { defineConfig } from 'vite';
import { readFileSync } from 'fs';

/** The version, from the one place that holds it.
 *
 *  It used to come out of `web/package.json`, which is a second copy kept by
 *  hand — and it drifted **eighteen releases** behind (ui 0.84.0 against server
 *  0.102.0, v0.102.1), so the one readout anybody would use to check whether
 *  their frontend was stale was itself the stalest thing on the screen. Bumping
 *  it into line fixed the number and left the shape: two versions, always
 *  equal, one of them maintained by remembering.
 *
 *  And the question it answered cannot arise. Since v0.14.0 `build.rs` builds
 *  this app into `static/` and `web_assets.rs` embeds that in the binary, so
 *  there is **one artifact** — a deployed instance structurally cannot have a UI
 *  version that differs from its server version. `bun run dev` is the only case
 *  where the two are separate processes, and they come from the same tree.
 *
 *  So it is derived, which is the rule v0.102.0 earned on the MusicBrainz
 *  User-Agent: *a version a human has to remember to bump is one that is always
 *  wrong.* `web/package.json` carries no version at all now — it is `private`,
 *  so the field never had a packaging meaning; it existed only to feed this.
 *
 *  Parsed out of `[package]` specifically rather than by taking the first
 *  `^version` line, so a `[dependencies.x]` block added later cannot become the
 *  answer. **Throws rather than falling back**: a wrong version on screen is
 *  the thing being fixed here, and a build that stops is better than one that
 *  quietly ships a lie.
 */
function crateVersion(): string {
	const toml = readFileSync('../Cargo.toml', 'utf-8');
	const pkgSection = /^\[package\]([\s\S]*?)(?=^\[|\Z)/m.exec(toml)?.[1] ?? '';
	const v = /^\s*version\s*=\s*"([^"]+)"/m.exec(pkgSection)?.[1];
	if (!v) throw new Error('vite.config.ts: no [package] version in ../Cargo.toml');
	return v;
}

export default defineConfig({
	plugins: [sveltekit()],
	define: {
		__UI_VERSION__: JSON.stringify(crateVersion()),
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
