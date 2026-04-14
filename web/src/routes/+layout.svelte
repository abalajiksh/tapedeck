<script lang="ts">
	import '../app.css';
	import Nav from '$lib/components/Nav.svelte';
	import { onMount } from 'svelte';
	import { page } from '$app/stores';
	import { goto } from '$app/navigation';
	import { api } from '$lib/api';

	let ready = false;
	const publicPaths = ['/login', '/setup'];

	onMount(async () => {
		const path = $page.url.pathname;

		// Public paths render immediately — they handle their own auth logic
		if (publicPaths.includes(path)) {
			ready = true;
			return;
		}

		// Check auth status before rendering protected content
		try {
			const status = await api.authStatus();
			if (status.needs_setup) {
				goto('/setup');
				return;
			}
			if (!status.authenticated) {
				goto('/login');
				return;
			}
		} catch {
			goto('/login');
			return;
		}

		ready = true;
	});
</script>

{#if $page.url.pathname === '/login' || $page.url.pathname === '/setup'}
	<main class="min-h-screen">
		<slot />
	</main>
{:else if !ready}
	<div class="min-h-screen bg-rp-base flex items-center justify-center">
		<div class="w-5 h-5 border-2 border-rp-iris/30 border-t-rp-iris rounded-full animate-spin"></div>
	</div>
{:else}
	<div class="flex min-h-screen">
		<Nav />
		<main class="flex-1 overflow-y-auto">
			<slot />
		</main>
	</div>
{/if}
