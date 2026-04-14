<script lang="ts">
	import { onMount } from 'svelte';
	import { api } from '$lib/api';

	let username = 'admin';
	let displayName = '';
	let password = '';
	let confirmPassword = '';
	let error = '';
	let loading = false;

	// After setup
	let setupComplete = false;
	let generatedToken = '';

	onMount(async () => {
		try {
			const status = await api.authStatus();
			if (!status.needs_setup) {
				// Setup already done — go to dashboard or login
				window.location.href = status.authenticated ? '/' : '/login';
			}
		} catch { /* proceed to setup form */ }
	});

	async function handleSetup() {
		error = '';

		if (password !== confirmPassword) {
			error = 'Passwords do not match';
			return;
		}
		if (password.length < 8) {
			error = 'Password must be at least 8 characters';
			return;
		}
		if (!username.trim()) {
			error = 'Username is required';
			return;
		}

		loading = true;
		try {
			const result = await api.setup(
				username.trim(),
				password,
				displayName.trim() || undefined,
			);
			generatedToken = result.token;
			setupComplete = true;
		} catch (e: any) {
			error = e.message ?? 'Setup failed';
		}
		loading = false;
	}

	function goToDashboard() {
		window.location.href = '/';
	}

	let tokenCopied = false;
	function copyToken() {
		navigator.clipboard.writeText(generatedToken);
		tokenCopied = true;
		setTimeout(() => (tokenCopied = false), 2000);
	}
</script>

<svelte:head>
	<title>Setup — Tapedeck</title>
</svelte:head>

<div class="min-h-screen bg-rp-base flex items-center justify-center px-4">
	<div class="w-full max-w-md">
		<!-- Logo -->
		<div class="text-center mb-8">
			<span class="text-4xl">📼</span>
			<h1 class="text-2xl font-light text-rp-text mt-2">Tapedeck</h1>
			<p class="text-sm text-rp-muted mt-1">Welcome! Set up your admin account.</p>
		</div>

		{#if setupComplete}
			<!-- Success: show token -->
			<div class="rounded-xl bg-rp-surface border border-rp-hl-med p-6 space-y-5">
				<div class="text-center">
					<span class="text-3xl">✅</span>
					<h2 class="text-lg font-medium text-rp-text mt-2">Setup Complete</h2>
					<p class="text-sm text-rp-muted mt-1">Your admin account is ready.</p>
				</div>

				<!-- API Token -->
				<div class="rounded-lg bg-rp-base border border-rp-gold/30 p-4">
					<p class="text-xs text-rp-gold uppercase tracking-wider font-medium mb-2">
						Your API Token — save this now!
					</p>
					<p class="text-xs text-rp-muted mb-3">
						Use this token in Pano Scrobbler and other clients. It won't be shown again.
					</p>
					<div class="flex gap-2">
						<code class="flex-1 bg-rp-overlay rounded px-3 py-2 text-sm text-rp-foam font-mono break-all select-all">
							{generatedToken}
						</code>
						<button
							on:click={copyToken}
							class="shrink-0 px-3 py-2 rounded-lg text-xs font-medium
							       bg-rp-gold/15 text-rp-gold border border-rp-gold/25
							       hover:bg-rp-gold/25 transition-colors"
						>
							{tokenCopied ? '✓ Copied' : 'Copy'}
						</button>
					</div>
				</div>

				<button
					on:click={goToDashboard}
					class="w-full px-4 py-2.5 rounded-lg text-sm font-medium transition-colors
					       bg-rp-iris/20 text-rp-iris border border-rp-iris/30 hover:bg-rp-iris/30"
				>
					Go to Dashboard →
				</button>
			</div>
		{:else}
			<!-- Setup form -->
			<div class="rounded-xl bg-rp-surface border border-rp-hl-med p-6">
				{#if error}
					<div class="rounded-lg bg-rp-love/10 border border-rp-love/20 px-4 py-2.5 mb-4">
						<p class="text-sm text-rp-love">{error}</p>
					</div>
				{/if}

				<div class="space-y-4">
					<div>
						<label for="username" class="block text-xs text-rp-muted uppercase tracking-wider mb-1.5">Username</label>
						<input
							id="username"
							type="text"
							bind:value={username}
							autocomplete="username"
							class="w-full bg-rp-base border border-rp-hl-med rounded-lg px-3.5 py-2.5 text-sm text-rp-text
							       placeholder:text-rp-muted focus:outline-none focus:border-rp-iris/50 transition-colors"
						/>
					</div>

					<div>
						<label for="display_name" class="block text-xs text-rp-muted uppercase tracking-wider mb-1.5">Display Name <span class="normal-case text-rp-muted">(optional)</span></label>
						<input
							id="display_name"
							type="text"
							bind:value={displayName}
							class="w-full bg-rp-base border border-rp-hl-med rounded-lg px-3.5 py-2.5 text-sm text-rp-text
							       placeholder:text-rp-muted focus:outline-none focus:border-rp-iris/50 transition-colors"
							placeholder="Ashwin"
						/>
					</div>

					<div>
						<label for="password" class="block text-xs text-rp-muted uppercase tracking-wider mb-1.5">Password</label>
						<input
							id="password"
							type="password"
							bind:value={password}
							autocomplete="new-password"
							class="w-full bg-rp-base border border-rp-hl-med rounded-lg px-3.5 py-2.5 text-sm text-rp-text
							       placeholder:text-rp-muted focus:outline-none focus:border-rp-iris/50 transition-colors"
							placeholder="Min. 8 characters"
						/>
					</div>

					<div>
						<label for="confirm" class="block text-xs text-rp-muted uppercase tracking-wider mb-1.5">Confirm Password</label>
						<input
							id="confirm"
							type="password"
							bind:value={confirmPassword}
							autocomplete="new-password"
							class="w-full bg-rp-base border border-rp-hl-med rounded-lg px-3.5 py-2.5 text-sm text-rp-text
							       placeholder:text-rp-muted focus:outline-none focus:border-rp-iris/50 transition-colors"
							placeholder="••••••••"
						/>
					</div>

					<button
						on:click={handleSetup}
						disabled={loading || !username || !password || !confirmPassword}
						class="w-full px-4 py-2.5 rounded-lg text-sm font-medium transition-colors
						       bg-rp-iris/20 text-rp-iris border border-rp-iris/30
						       hover:bg-rp-iris/30 disabled:opacity-40 disabled:cursor-not-allowed"
					>
						{loading ? 'Creating account…' : 'Create Admin Account'}
					</button>
				</div>
			</div>
		{/if}
	</div>
</div>
