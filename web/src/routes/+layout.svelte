<script lang="ts">
	import '../app.css';
	import Nav from '$lib/components/Nav.svelte';
	import { onMount } from 'svelte';

	let state: 'loading' | 'public' | 'authenticated' | 'error' = 'loading';
	let errorMsg = '';

	// Check path immediately (before onMount) so public pages don't flash a spinner
	const path = typeof window !== 'undefined' ? window.location.pathname : '/';
	const isPublic = path === '/login' || path === '/setup';
	if (isPublic) state = 'public';

	onMount(async () => {
		if (isPublic) return;

		try {
			const res = await fetch('/api/v1/auth/status', {
				credentials: 'same-origin',
			});
			if (!res.ok) throw new Error(`HTTP ${res.status}`);
			const data = await res.json();

			if (data.needs_setup) {
				window.location.href = '/setup';
				return;
			}
			if (!data.authenticated) {
				window.location.href = '/login';
				return;
			}

			state = 'authenticated';
		} catch (err: any) {
			state = 'error';
			errorMsg = err?.message ?? String(err);
		}
	});
</script>

{#if state === 'public'}
	<main class="min-h-screen">
		<slot />
	</main>
{:else if state === 'loading'}
	<div class="min-h-screen bg-rp-base flex items-center justify-center">
		<div class="w-5 h-5 border-2 border-rp-iris/30 border-t-rp-iris rounded-full animate-spin"></div>
	</div>
{:else if state === 'error'}
	<div class="min-h-screen bg-rp-base flex items-center justify-center px-4">
		<div class="max-w-md rounded-xl bg-rp-surface border border-rp-love/30 p-6 text-center">
			<p class="text-rp-love text-sm mb-2">Auth Error</p>
			<p class="text-rp-muted text-xs font-mono break-all">{errorMsg}</p>
			<div class="flex gap-3 mt-4 justify-center">
				<a href="/login" class="text-xs text-rp-iris hover:underline">Go to Login</a>
				<a href="/setup" class="text-xs text-rp-iris hover:underline">Go to Setup</a>
			</div>
		</div>
	</div>
{:else}
	<div class="flex min-h-screen">
		<Nav />
		<main class="flex-1 overflow-y-auto">
			<slot />
		</main>
	</div>
{/if}
