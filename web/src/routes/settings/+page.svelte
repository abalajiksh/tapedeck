<script lang="ts">
	import { onMount } from 'svelte';
	import { api } from '$lib/api';

	let token = '';
	let saved = false;
	let healthStatus = '';
	let checking = false;

	onMount(() => {
		token = api.getToken();
	});

	function saveToken() {
		api.setToken(token.trim());
		saved = true;
		setTimeout(() => (saved = false), 2000);
	}

	async function checkConnection() {
		checking = true;
		healthStatus = '';
		try {
			const h = await api.health();
			healthStatus = h.status === 'ok' ? '✓ Connected' : `⚠ Status: ${h.status}`;
		} catch (e: any) {
			healthStatus = `✗ ${e.message}`;
		}
		checking = false;
	}
</script>

<svelte:head>
	<title>Settings — Tapedeck</title>
</svelte:head>

<div class="max-w-2xl mx-auto px-6 py-8">
	<h1 class="text-2xl font-light text-rp-text mb-6">Settings</h1>

	<!-- API Token -->
	<section class="rounded-xl bg-rp-surface border border-rp-hl-med p-5 mb-6">
		<h2 class="text-sm font-medium text-rp-text mb-1">API Token</h2>
		<p class="text-xs text-rp-muted mb-4">
			Your Tapedeck token (<code class="text-rp-foam">td_…</code>). Stored in your browser only.
		</p>

		<div class="flex gap-2">
			<input
				type="password"
				bind:value={token}
				placeholder="td_xxxxxxxxxxxx"
				class="flex-1 bg-rp-base border border-rp-hl-med rounded-lg px-3.5 py-2 text-sm text-rp-text font-mono
				       placeholder:text-rp-muted focus:outline-none focus:border-rp-iris/50 transition-colors"
			/>
			<button
				on:click={saveToken}
				class="px-4 py-2 rounded-lg text-sm font-medium transition-colors
				       bg-rp-iris/20 text-rp-iris border border-rp-iris/30
				       hover:bg-rp-iris/30"
			>
				{saved ? '✓ Saved' : 'Save'}
			</button>
		</div>
	</section>

	<!-- Connection check -->
	<section class="rounded-xl bg-rp-surface border border-rp-hl-med p-5 mb-6">
		<h2 class="text-sm font-medium text-rp-text mb-1">Connection</h2>
		<p class="text-xs text-rp-muted mb-4">
			Test connectivity to the Tapedeck backend.
		</p>

		<div class="flex items-center gap-3">
			<button
				on:click={checkConnection}
				disabled={checking}
				class="px-4 py-2 rounded-lg text-sm font-medium transition-colors
				       bg-rp-pine/20 text-rp-foam border border-rp-pine/30
				       hover:bg-rp-pine/30 disabled:opacity-50"
			>
				{checking ? 'Checking…' : 'Test Connection'}
			</button>

			{#if healthStatus}
				<span class="text-sm {healthStatus.startsWith('✓') ? 'text-rp-foam' : 'text-rp-love'}">
					{healthStatus}
				</span>
			{/if}
		</div>
	</section>

	<!-- Pano Scrobbler setup -->
	<section class="rounded-xl bg-rp-surface border border-rp-hl-med p-5 mb-6">
		<h2 class="text-sm font-medium text-rp-text mb-1">Pano Scrobbler Setup</h2>
		<p class="text-xs text-rp-muted mb-4">
			Connect all your devices in one step.
		</p>

		<ol class="text-sm text-rp-subtle space-y-2">
			<li class="flex gap-2">
				<span class="text-rp-iris shrink-0">1.</span>
				Open Pano Scrobbler → Settings → Scrobble services
			</li>
			<li class="flex gap-2">
				<span class="text-rp-iris shrink-0">2.</span>
				Add a custom ListenBrainz server
			</li>
			<li class="flex gap-2">
				<span class="text-rp-iris shrink-0">3.</span>
				<span>
					URL: <code class="text-rp-foam bg-rp-hl-low px-1.5 py-0.5 rounded text-xs">{typeof window !== 'undefined' ? window.location.origin : 'http://your-server:8080'}</code>
				</span>
			</li>
			<li class="flex gap-2">
				<span class="text-rp-iris shrink-0">4.</span>
				<span>Token: your <code class="text-rp-foam bg-rp-hl-low px-1.5 py-0.5 rounded text-xs">td_…</code> token</span>
			</li>
		</ol>
	</section>

	<!-- About -->
	<section class="rounded-xl bg-rp-surface border border-rp-hl-med p-5">
		<h2 class="text-sm font-medium text-rp-text mb-1">About</h2>
		<div class="text-xs text-rp-muted space-y-1">
			<p>Tapedeck v0.5.2</p>
			<p>A self-hosted music intelligence hub.</p>
			<p>
				<a href="https://github.com/abalajiksh/tapedeck" target="_blank" rel="noopener"
				   class="text-rp-iris hover:underline">GitHub</a>
			</p>
		</div>
	</section>
</div>
