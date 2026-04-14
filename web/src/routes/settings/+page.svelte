<script lang="ts">
	import { onMount } from 'svelte';
	import { api } from '$lib/api';
	import type { TokenInfo } from '$lib/api';
	import { formatDateTime } from '$lib/utils';

	declare const __UI_VERSION__: string;
	const uiVersion = __UI_VERSION__;
	let backendVersion = '…';

	let tokens: TokenInfo[] = [];
	let loading = true;

	// New token form
	let newTokenName = '';
	let newTokenScopes = 'submit';
	let createdToken = '';
	let tokenCopied = false;
	let creating = false;

	let healthStatus = '';
	let checking = false;

	onMount(async () => {
		try {
			const [health, tokensRes] = await Promise.all([
				api.health(),
				api.getTokens(),
			]);
			backendVersion = health.version;
			tokens = tokensRes.tokens;
		} catch { /* shown as fallback */ }
		loading = false;
	});

	async function createToken() {
		if (!newTokenName.trim()) return;
		creating = true;
		createdToken = '';
		try {
			const result = await api.createToken(newTokenName.trim(), newTokenScopes);
			createdToken = result.token;
			newTokenName = '';
			// Refresh token list
			const res = await api.getTokens();
			tokens = res.tokens;
		} catch (e: any) {
			console.error('Create token failed:', e);
		}
		creating = false;
	}

	async function revokeToken(id: number) {
		if (!confirm('Revoke this token? Any clients using it will stop working.')) return;
		try {
			await api.revokeToken(id);
			tokens = tokens.filter(t => t.id !== id);
		} catch (e: any) {
			console.error('Revoke failed:', e);
		}
	}

	function copyToken() {
		navigator.clipboard.writeText(createdToken);
		tokenCopied = true;
		setTimeout(() => (tokenCopied = false), 2000);
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

	async function handleLogout() {
		await api.logout();
		window.location.href = '/login';
	}
</script>

<svelte:head>
	<title>Settings — Tapedeck</title>
</svelte:head>

<div class="max-w-2xl mx-auto px-6 py-8">
	<h1 class="text-2xl font-light text-rp-text mb-6">Settings</h1>

	<!-- API Tokens -->
	<section class="rounded-xl bg-rp-surface border border-rp-hl-med p-5 mb-6">
		<h2 class="text-sm font-medium text-rp-text mb-1">API Tokens</h2>
		<p class="text-xs text-rp-muted mb-4">
			Tokens for Pano Scrobbler and other external clients. Each token authenticates via
			<code class="text-rp-foam">Authorization: Token td_…</code>
		</p>

		{#if loading}
			<div class="flex justify-center py-4">
				<div class="w-4 h-4 border-2 border-rp-iris/30 border-t-rp-iris rounded-full animate-spin"></div>
			</div>
		{:else}
			<!-- Token list -->
			{#if tokens.length > 0}
				<div class="space-y-2 mb-4">
					{#each tokens as token (token.id)}
						<div class="flex items-center justify-between bg-rp-base rounded-lg px-3.5 py-2.5 border border-rp-hl-low">
							<div class="min-w-0">
								<span class="text-sm text-rp-text font-medium">{token.name}</span>
								<span class="text-[10px] text-rp-muted ml-2 uppercase">{token.scopes}</span>
								<p class="text-xs text-rp-muted mt-0.5">
									Created {formatDateTime(token.created_at)}
									{#if token.last_used_at}
										 · Last used {formatDateTime(token.last_used_at)}
									{:else}
										 · Never used
									{/if}
								</p>
							</div>
							<button
								on:click={() => revokeToken(token.id)}
								class="shrink-0 text-xs text-rp-love/70 hover:text-rp-love transition-colors ml-3"
							>
								Revoke
							</button>
						</div>
					{/each}
				</div>
			{:else}
				<p class="text-sm text-rp-muted mb-4">No tokens yet.</p>
			{/if}

			<!-- New token created banner -->
			{#if createdToken}
				<div class="rounded-lg bg-rp-base border border-rp-gold/30 p-4 mb-4">
					<p class="text-xs text-rp-gold uppercase tracking-wider font-medium mb-2">New token created — copy it now!</p>
					<div class="flex gap-2">
						<code class="flex-1 bg-rp-overlay rounded px-3 py-2 text-sm text-rp-foam font-mono break-all select-all">
							{createdToken}
						</code>
						<button
							on:click={copyToken}
							class="shrink-0 px-3 py-2 rounded-lg text-xs font-medium
							       bg-rp-gold/15 text-rp-gold border border-rp-gold/25 hover:bg-rp-gold/25"
						>
							{tokenCopied ? '✓' : 'Copy'}
						</button>
					</div>
				</div>
			{/if}

			<!-- Create new token -->
			<div class="flex gap-2">
				<input
					type="text"
					bind:value={newTokenName}
					placeholder="Token name (e.g. walkman, phone)"
					class="flex-1 bg-rp-base border border-rp-hl-med rounded-lg px-3.5 py-2 text-sm text-rp-text
					       placeholder:text-rp-muted focus:outline-none focus:border-rp-iris/50 transition-colors"
				/>
				<select
					bind:value={newTokenScopes}
					class="bg-rp-base border border-rp-hl-med rounded-lg px-2 py-2 text-sm text-rp-text
					       focus:outline-none focus:border-rp-iris/50"
				>
					<option value="submit">submit</option>
					<option value="submit,read">submit + read</option>
					<option value="submit,read,admin">all</option>
				</select>
				<button
					on:click={createToken}
					disabled={creating || !newTokenName.trim()}
					class="px-4 py-2 rounded-lg text-sm font-medium transition-colors
					       bg-rp-iris/20 text-rp-iris border border-rp-iris/30
					       hover:bg-rp-iris/30 disabled:opacity-40"
				>
					{creating ? '…' : 'Create'}
				</button>
			</div>
		{/if}
	</section>

	<!-- Connection check -->
	<section class="rounded-xl bg-rp-surface border border-rp-hl-med p-5 mb-6">
		<h2 class="text-sm font-medium text-rp-text mb-1">Connection</h2>
		<p class="text-xs text-rp-muted mb-4">Test connectivity to the Tapedeck backend.</p>
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
				<span class="text-sm {healthStatus.startsWith('✓') ? 'text-rp-foam' : 'text-rp-love'}">{healthStatus}</span>
			{/if}
		</div>
	</section>

	<!-- Pano Scrobbler setup -->
	<section class="rounded-xl bg-rp-surface border border-rp-hl-med p-5 mb-6">
		<h2 class="text-sm font-medium text-rp-text mb-1">Pano Scrobbler Setup</h2>
		<p class="text-xs text-rp-muted mb-4">Connect all your devices in one step.</p>
		<ol class="text-sm text-rp-subtle space-y-2">
			<li class="flex gap-2"><span class="text-rp-iris shrink-0">1.</span> Open Pano Scrobbler → Settings → Scrobble services</li>
			<li class="flex gap-2"><span class="text-rp-iris shrink-0">2.</span> Add a custom ListenBrainz server</li>
			<li class="flex gap-2"><span class="text-rp-iris shrink-0">3.</span>
				<span>URL: <code class="text-rp-foam bg-rp-hl-low px-1.5 py-0.5 rounded text-xs">{typeof window !== 'undefined' ? window.location.origin : 'http://your-server:8080'}</code></span>
			</li>
			<li class="flex gap-2"><span class="text-rp-iris shrink-0">4.</span>
				<span>Token: one of your <code class="text-rp-foam bg-rp-hl-low px-1.5 py-0.5 rounded text-xs">td_…</code> tokens from above</span>
			</li>
		</ol>
	</section>

	<!-- About + Logout -->
	<section class="rounded-xl bg-rp-surface border border-rp-hl-med p-5">
		<div class="flex items-start justify-between">
			<div>
				<h2 class="text-sm font-medium text-rp-text mb-1">About</h2>
				<div class="text-xs text-rp-muted space-y-1">
					<p>Server v{backendVersion} · UI v{uiVersion}</p>
					<p>A self-hosted music intelligence hub.</p>
					<p>
						<a href="https://github.com/abalajiksh/tapedeck" target="_blank" rel="noopener"
						   class="text-rp-iris hover:underline">GitHub</a>
					</p>
				</div>
			</div>
			<button
				on:click={handleLogout}
				class="px-4 py-2 rounded-lg text-sm font-medium transition-colors
				       bg-rp-love/10 text-rp-love border border-rp-love/20
				       hover:bg-rp-love/20"
			>
				Sign out
			</button>
		</div>
	</section>
</div>
