<script lang="ts">
	import { page } from '$app/stores';
	import { onMount } from 'svelte';
	import { api } from '$lib/api';

	declare const __UI_VERSION__: string;
	const uiVersion = __UI_VERSION__;
	let backendVersion = '…';

	onMount(async () => {
		try {
			const health = await api.health();
			backendVersion = health.version;
		} catch {
			backendVersion = '?';
		}
	});

	const navItems = [
		{ href: '/',        label: 'Dashboard',  icon: '⏺' },
		{ href: '/history', label: 'History',    icon: '📜' },
		{ href: '/stats',   label: 'Statistics', icon: '📊' },
		{ href: '/chains',  label: 'Chains',     icon: '🔗' },
		{ href: '/settings',label: 'Settings',   icon: '⚙' },
	];
</script>

<nav class="w-56 shrink-0 border-r border-rp-hl-med bg-rp-surface flex flex-col h-screen sticky top-0">
	<!-- Logo -->
	<a href="/" class="flex items-center gap-2.5 px-5 py-5 border-b border-rp-hl-med hover:bg-rp-hl-low transition-colors">
		<span class="text-xl">📼</span>
		<span class="text-lg font-medium tracking-tight text-rp-text">Tapedeck</span>
	</a>

	<!-- Links -->
	<div class="flex flex-col gap-0.5 px-3 py-3 flex-1">
		{#each navItems as item}
			<a
				href={item.href}
				class="flex items-center gap-3 px-3 py-2 rounded-md text-sm transition-colors
				       {$page.url.pathname === item.href
				         ? 'bg-rp-hl-med text-rp-text font-medium'
				         : 'text-rp-subtle hover:text-rp-text hover:bg-rp-hl-low'}"
			>
				<span class="text-base w-5 text-center">{item.icon}</span>
				{item.label}
			</a>
		{/each}
	</div>

	<!-- Footer -->
	<div class="px-5 py-3 border-t border-rp-hl-med">
		<p class="text-[11px] text-rp-muted">server v{backendVersion} · ui v{uiVersion}</p>
	</div>
</nav>
