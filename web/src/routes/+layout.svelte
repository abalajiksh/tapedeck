<script lang="ts">
	import '../app.css';
	import { onMount } from 'svelte';
	import { page } from '$app/stores';
	import { theme, initTheme, setTheme, type ThemeMode } from '$lib/theme';

	let state: 'loading' | 'public' | 'authenticated' | 'error' = 'loading';
	let errorMsg = '';
	let navOpen = false;
	let isMobile = false;
	let isAdmin = false;

	// Check path immediately (before onMount) so public pages don't flash a spinner
	const path = typeof window !== 'undefined' ? window.location.pathname : '/';
	const isPublic = path === '/login' || path === '/setup';
	if (isPublic) state = 'public';

	const nav = [
		{ href: '/', label: 'Dashboard', icon: 'dashboard' },
		{ href: '/history', label: 'History', icon: 'history' },
		{ href: '/stats', label: 'Statistics', icon: 'stats' },
		{ href: '/import', label: 'Import', icon: 'import' },
		{ href: '/chains', label: 'Signal Chains', icon: 'chains' },
		{ href: '/settings', label: 'Settings', icon: 'settings' }
	];

	$: current = $page.url.pathname;
	function isActive(href: string) {
		return href === '/' ? current === '/' : current.startsWith(href);
	}

	function toggleTheme() {
		// cycle light ↔ dark from whatever is currently showing
		const dark = document.documentElement.dataset.theme === 'dark';
		setTheme(dark ? 'light' : 'dark');
	}

	onMount(() => {
		initTheme();
		const mq = matchMedia('(max-width: 900px)');
		isMobile = mq.matches;
		mq.addEventListener('change', (e) => (isMobile = e.matches));

		if (isPublic) return;
		(async () => {
			try {
				const res = await fetch('/api/v1/auth/status', { credentials: 'same-origin' });
				if (!res.ok) throw new Error(`HTTP ${res.status}`);
				const data = await res.json();
				if (data.needs_setup) return (window.location.href = '/setup');
				if (!data.authenticated) return (window.location.href = '/login');
				isAdmin = !!data.is_admin;
				state = 'authenticated';
			} catch (err) {
				state = 'error';
				errorMsg = err instanceof Error ? err.message : String(err);
			}
		})();
	});
</script>

{#if state === 'public'}
	<slot />
{:else if state === 'loading'}
	<div style="min-height:100vh;display:flex;align-items:center;justify-content:center;">
		<div class="spin" style="width:22px;height:22px;border:2px solid var(--td-track);border-top-color:var(--td-accent);border-radius:50%;"></div>
	</div>
{:else if state === 'error'}
	<div style="min-height:100vh;display:flex;align-items:center;justify-content:center;padding:16px;">
		<div class="panel" style="max-width:28rem;padding:24px;text-align:center;">
			<p style="color:var(--td-accent-hi);font-size:14px;margin:0 0 8px;">Auth error</p>
			<p style="color:var(--td-muted);font-size:12px;font-family:monospace;word-break:break-all;">{errorMsg}</p>
			<div style="display:flex;gap:12px;justify-content:center;margin-top:16px;">
				<a href="/login" style="font-size:13px;">Go to Login</a>
				<a href="/setup" style="font-size:13px;">Go to Setup</a>
			</div>
		</div>
	</div>
{:else}
	<div style="display:flex;min-height:100vh;">
		<!-- SIDEBAR -->
		<aside
			style="background:var(--td-sidebar);border-right:1px solid var(--td-border);display:flex;flex-direction:column;
			{isMobile
				? `position:fixed;top:0;left:0;z-index:60;width:260px;max-width:82vw;height:100vh;transform:translateX(${navOpen ? '0' : '-102%'});transition:transform .3s cubic-bezier(.4,0,.2,1);box-shadow:${navOpen ? '0 0 40px rgba(46,43,37,.3)' : 'none'};`
				: 'width:250px;flex:none;position:sticky;top:0;height:100vh;'}"
		>
			<div style="display:flex;align-items:center;gap:12px;padding:22px 22px 18px;">
				<div style="display:inline-flex;align-items:center;justify-content:center;width:42px;height:42px;border-radius:13px;background:var(--td-panel);">
					<svg viewBox="0 0 32 20" width="24" fill="none"><rect x="1" y="1" width="30" height="18" rx="4" stroke="#f4e7cf" stroke-width="1.7"/><circle cx="11" cy="11" r="3.2" stroke="#c67139" stroke-width="1.7"/><circle cx="21" cy="11" r="3.2" stroke="#7a8a5e" stroke-width="1.7"/></svg>
				</div>
				<div style="flex:1;min-width:0;">
					<div style="font-family:'Caprasimo',serif;font-size:21px;line-height:1;letter-spacing:-.02em;">Tapedeck</div>
					<div style="font-size:11px;color:var(--td-muted-2);margin-top:3px;">listening journal</div>
				</div>
				{#if isMobile}
					<button on:click={() => (navOpen = false)} aria-label="Close menu" style="border:none;background:transparent;cursor:pointer;color:var(--td-nav-fg);padding:4px;display:flex;">
						<svg viewBox="0 0 24 24" width="22" fill="none" stroke="currentColor" stroke-width="2.75" stroke-linecap="round"><path d="M6 6l12 12M18 6L6 18"/></svg>
					</button>
				{/if}
			</div>

			<nav style="display:flex;flex-direction:column;gap:3px;padding:8px 12px;flex:1;">
				{#each nav as item}
					<a
						href={item.href}
						on:click={() => (navOpen = false)}
						class="navlink"
						class:active={isActive(item.href)}
					>
						{#if item.icon === 'dashboard'}<svg viewBox="0 0 24 24" width="20" fill="none" stroke="currentColor" stroke-width="2.6" stroke-linecap="round"><circle cx="12" cy="12" r="9"/><circle cx="12" cy="12" r="2.4"/></svg>
						{:else if item.icon === 'history'}<svg viewBox="0 0 24 24" width="20" fill="none" stroke="currentColor" stroke-width="2.6" stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="12" r="9"/><path d="M12 7v5l3.5 2"/></svg>
						{:else if item.icon === 'stats'}<svg viewBox="0 0 24 24" width="20" fill="none" stroke="currentColor" stroke-width="2.6" stroke-linecap="round" stroke-linejoin="round"><path d="M4 20V4"/><path d="M4 20h16"/><rect x="7" y="11" width="3.2" height="6" rx="1"/><rect x="13.5" y="7" width="3.2" height="10" rx="1"/></svg>
						{:else if item.icon === 'import'}<svg viewBox="0 0 24 24" width="20" fill="none" stroke="currentColor" stroke-width="2.6" stroke-linecap="round" stroke-linejoin="round"><path d="M12 3v12"/><path d="M8 11l4 4 4-4"/><path d="M5 21h14"/></svg>
						{:else if item.icon === 'chains'}<svg viewBox="0 0 24 24" width="20" fill="none" stroke="currentColor" stroke-width="2.6" stroke-linecap="round" stroke-linejoin="round"><path d="M9 12a3 3 0 0 1 3-3h2a3 3 0 0 1 0 6"/><path d="M15 12a3 3 0 0 1-3 3h-2a3 3 0 0 1 0-6"/></svg>
						{:else}<svg viewBox="0 0 24 24" width="20" fill="none" stroke="currentColor" stroke-width="2.6" stroke-linecap="round" stroke-linejoin="round"><line x1="4" y1="8" x2="20" y2="8"/><circle cx="15" cy="8" r="2.4" fill="var(--td-sidebar)"/><line x1="4" y1="16" x2="20" y2="16"/><circle cx="9" cy="16" r="2.4" fill="var(--td-sidebar)"/></svg>
						{/if}
						{item.label}
					</a>
				{/each}
				{#if isAdmin}
					<a href="/users" on:click={() => (navOpen = false)} class="navlink" class:active={isActive('/users')}>
						<svg viewBox="0 0 24 24" width="20" fill="none" stroke="currentColor" stroke-width="2.6" stroke-linecap="round" stroke-linejoin="round"><path d="M17 21v-2a4 4 0 0 0-4-4H5a4 4 0 0 0-4 4v2"/><circle cx="9" cy="7" r="4"/><path d="M23 21v-2a4 4 0 0 0-3-3.87"/><path d="M16 3.13a4 4 0 0 1 0 7.75"/></svg>Users
					</a>
				{/if}
			</nav>

			<div style="padding:2px 12px 14px;display:flex;align-items:center;justify-content:space-between;gap:10px;">
				<span style="font-size:11px;color:var(--td-muted-2);padding-left:10px;">ui v2.0</span>
				<button on:click={toggleTheme} aria-label="Toggle dark mode" class="theme-toggle">
					{#if $theme === 'dark' || (typeof document !== 'undefined' && document.documentElement.dataset.theme === 'dark')}
						<svg viewBox="0 0 24 24" width="18" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="12" r="4.2"/><path d="M12 2v2M12 20v2M4.9 4.9l1.4 1.4M17.7 17.7l1.4 1.4M2 12h2M20 12h2M4.9 19.1l1.4-1.4M17.7 6.3l1.4-1.4"/></svg>
					{:else}
						<svg viewBox="0 0 24 24" width="18" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round"><path d="M21 12.8A9 9 0 1 1 11.2 3a7 7 0 0 0 9.8 9.8z"/></svg>
					{/if}
				</button>
			</div>
		</aside>

		{#if isMobile && navOpen}
			<div on:click={() => (navOpen = false)} on:keydown={() => {}} role="presentation" style="position:fixed;inset:0;z-index:50;background:rgba(30,27,22,.4);"></div>
		{/if}

		<!-- MAIN -->
		<main style="flex:1;min-width:0;overflow-x:hidden;">
			{#if isMobile}
				<div style="position:sticky;top:0;z-index:40;display:flex;align-items:center;gap:12px;padding:12px 16px;background:var(--td-topbar);backdrop-filter:blur(8px);border-bottom:1px solid var(--td-border);">
					<button on:click={() => (navOpen = true)} aria-label="Open menu" style="border:none;background:var(--td-sidebar);cursor:pointer;color:var(--td-text);width:40px;height:40px;border-radius:12px;display:flex;align-items:center;justify-content:center;">
						<svg viewBox="0 0 24 24" width="22" fill="none" stroke="currentColor" stroke-width="2.75" stroke-linecap="round"><path d="M4 7h16M4 12h16M4 17h16"/></svg>
					</button>
					<div style="display:inline-flex;align-items:center;justify-content:center;width:34px;height:34px;border-radius:10px;background:var(--td-panel);">
						<svg viewBox="0 0 32 20" width="20" fill="none"><rect x="1" y="1" width="30" height="18" rx="4" stroke="#f4e7cf" stroke-width="1.8"/><circle cx="11" cy="11" r="3" stroke="#c67139" stroke-width="1.8"/><circle cx="21" cy="11" r="3" stroke="#7a8a5e" stroke-width="1.8"/></svg>
					</div>
					<div style="font-family:'Caprasimo',serif;font-size:19px;letter-spacing:-.02em;">Tapedeck</div>
				</div>
			{/if}
			<div style="max-width:1100px;margin:0 auto;padding:28px 22px 60px;">
				<slot />
			</div>
		</main>
	</div>
{/if}

<style>
	.navlink {
		display: flex;
		align-items: center;
		gap: 12px;
		padding: 11px 14px;
		border-radius: 13px;
		font-size: 14.5px;
		font-weight: 500;
		color: var(--td-nav-fg);
		text-decoration: none;
	}
	.navlink:hover {
		background: var(--td-nav-hover);
		color: var(--td-text);
	}
	.navlink.active {
		background: var(--td-panel);
		color: #f4e7cf;
	}
	.theme-toggle {
		border: none;
		background: var(--td-sidebar);
		cursor: pointer;
		color: var(--td-nav-fg);
		width: 34px;
		height: 34px;
		border-radius: 10px;
		display: flex;
		align-items: center;
		justify-content: center;
		flex: none;
	}
	.theme-toggle:hover {
		background: var(--td-nav-hover);
		color: var(--td-text);
	}
	.spin {
		animation: td-spin 0.7s linear infinite;
	}
</style>
