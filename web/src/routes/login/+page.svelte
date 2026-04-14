<script lang="ts">
	import { onMount } from 'svelte';
	import { api } from '$lib/api';

	let username = '';
	let password = '';
	let error = '';
	let loading = false;

	onMount(async () => {
		// If already authenticated, go to dashboard
		try {
			const status = await api.authStatus();
			if (status.needs_setup) {
				window.location.href = '/setup';
				return;
			}
			if (status.authenticated) {
				window.location.href = '/';
				return;
			}
		} catch { /* proceed to login form */ }
	});

	async function handleLogin() {
		error = '';
		loading = true;
		try {
			await api.login(username, password);
			// Full reload to re-run layout auth check with the new session cookie
			window.location.href = '/';
		} catch (e: any) {
			error = e.message?.includes('401') ? 'Invalid username or password' : 'Connection error';
		}
		loading = false;
	}

	function onKeydown(e: KeyboardEvent) {
		if (e.key === 'Enter' && username && password) handleLogin();
	}
</script>

<svelte:head>
	<title>Login — Tapedeck</title>
</svelte:head>

<div class="min-h-screen bg-rp-base flex items-center justify-center px-4">
	<div class="w-full max-w-sm">
		<!-- Logo -->
		<div class="text-center mb-8">
			<span class="text-4xl">📼</span>
			<h1 class="text-2xl font-light text-rp-text mt-2">Tapedeck</h1>
			<p class="text-sm text-rp-muted mt-1">Sign in to your instance</p>
		</div>

		<!-- Login form -->
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
						on:keydown={onKeydown}
						autocomplete="username"
						class="w-full bg-rp-base border border-rp-hl-med rounded-lg px-3.5 py-2.5 text-sm text-rp-text
						       placeholder:text-rp-muted focus:outline-none focus:border-rp-iris/50 transition-colors"
						placeholder="admin"
					/>
				</div>

				<div>
					<label for="password" class="block text-xs text-rp-muted uppercase tracking-wider mb-1.5">Password</label>
					<input
						id="password"
						type="password"
						bind:value={password}
						on:keydown={onKeydown}
						autocomplete="current-password"
						class="w-full bg-rp-base border border-rp-hl-med rounded-lg px-3.5 py-2.5 text-sm text-rp-text
						       placeholder:text-rp-muted focus:outline-none focus:border-rp-iris/50 transition-colors"
						placeholder="••••••••"
					/>
				</div>

				<button
					on:click={handleLogin}
					disabled={loading || !username || !password}
					class="w-full px-4 py-2.5 rounded-lg text-sm font-medium transition-colors
					       bg-rp-iris/20 text-rp-iris border border-rp-iris/30
					       hover:bg-rp-iris/30 disabled:opacity-40 disabled:cursor-not-allowed"
				>
					{loading ? 'Signing in…' : 'Sign in'}
				</button>
			</div>
		</div>
	</div>
</div>
