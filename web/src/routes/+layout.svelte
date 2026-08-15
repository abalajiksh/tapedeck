<script lang="ts">
	import '../app.css';
	import { onMount } from 'svelte';
	import { page } from '$app/stores';
	import { api } from '$lib/api';
	import { deck, watchNowPlaying } from '$lib/nowplaying';
	import { initPrefs } from '$lib/prefs';
	import { theme, initTheme, setTheme, type ThemeMode } from '$lib/theme';

	let state: 'loading' | 'public' | 'authenticated' | 'error' = 'loading';
	let errorMsg = '';
	let navOpen = false;
	let isMobile = false;
	let isAdmin = false;
	let serverVersion = '';

	// Check path immediately (before onMount) so public pages don't flash a spinner
	const path = typeof window !== 'undefined' ? window.location.pathname : '/';
	// `/authorize` is public *to the layout gate only* — the OAuth consent page
	// gates itself. Letting the gate hard-redirect would throw away the query
	// string the whole authorization request lives in.
	const isPublic = path === '/login' || path === '/setup' || path === '/authorize';
	if (isPublic) state = 'public';

	const nav = [
		{ href: '/', label: 'Dashboard', icon: 'dashboard' },
		{ href: '/history', label: 'History', icon: 'history' },
		{ href: '/search', label: 'Search', icon: 'search' },
		{ href: '/charts', label: 'Top Charts', icon: 'charts' },
		{ href: '/stats', label: 'Statistics', icon: 'stats' },
		{ href: '/reports', label: 'Reports', icon: 'reports' },
		{ href: '/loved', label: 'Loved', icon: 'loved' },
		{ href: '/notes', label: 'Liner Notes', icon: 'notes' },
		{ href: '/shelf', label: 'The Shelf', icon: 'shelf' },
		{ href: '/crate', label: 'The Crate', icon: 'crate' },
		{ href: '/rediscover', label: 'Rediscovery', icon: 'rediscover' },
		{ href: '/playlists/saved', label: 'Playlists', icon: 'playlists' },
		{ href: '/playlists', label: 'Playlist Lab', icon: 'playlists' },
		{ href: '/map', label: 'Genre Map', icon: 'map' },
		{ href: '/import', label: 'Import', icon: 'import' },
		{ href: '/chains', label: 'Signal Chains', icon: 'chains' },
		{ href: '/settings', label: 'Settings', icon: 'settings' }
	];

	$: current = $page.url.pathname;
	function isActive(href: string) {
		if (href === '/') return current === '/';
		// Match on a segment boundary, and let a more specific entry win. Adding
		// /playlists/saved beside /playlists otherwise lights both, and a bare
		// `startsWith` would also match /chartsomething against /charts.
		const onPath = (h: string) => current === h || current.startsWith(h + '/');
		if (nav.some((n) => n.href !== href && n.href.startsWith(href + '/') && onPath(n.href)))
			return false;
		return onPath(href);
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

		// The sidebar card polls alongside whatever the page does; the store
		// reference-counts so this and the dashboard share one request.
		const stopDeck = watchNowPlaying();

		(async () => {
			try {
				const res = await fetch('/api/v1/auth/status', { credentials: 'same-origin' });
				if (!res.ok) throw new Error(`HTTP ${res.status}`);
				const data = await res.json();
				if (data.needs_setup) return (window.location.href = '/setup');
				if (!data.authenticated) return (window.location.href = '/login');
				isAdmin = !!data.is_admin;
				state = 'authenticated';

				try {
					serverVersion = (await api.health()).version;
				} catch { /* footer just shows the UI version */ }

				initPrefs();

				// Record the browser timezone once, so analytics use local time.
				try {
					const prefs = await api.getPreferences();
					if (!prefs.timezone) {
						const tz = Intl.DateTimeFormat().resolvedOptions().timeZone;
						if (tz) await api.putPreferences({ timezone: tz });
					}
				} catch { /* non-fatal */ }
			} catch (err) {
				state = 'error';
				errorMsg = err instanceof Error ? err.message : String(err);
			}
		})();

		return stopDeck;
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
		<!-- `overflow:hidden` plus a scrolling `nav` is what makes this usable on a
		     short screen. The aside is a fixed `100vh` flex column, so before this
		     anything past the fold simply painted outside it with no way to reach
		     it — on a 1366×768 laptop that was the version line and the theme
		     toggle. Only the nav scrolls: the brand stays put at the top and the
		     footer stays put at the bottom, which is where people look for them. -->
		<aside
			style="background:var(--td-sidebar);border-right:1px solid var(--td-border);display:flex;flex-direction:column;overflow:hidden;
			{isMobile
				? `position:fixed;top:0;left:0;z-index:60;width:260px;max-width:82vw;height:100vh;transform:translateX(${navOpen ? '0' : '-102%'});transition:transform .3s cubic-bezier(.4,0,.2,1);box-shadow:${navOpen ? '0 0 40px rgba(46,43,37,.3)' : 'none'};`
				: 'width:250px;flex:none;position:sticky;top:0;height:100vh;'}"
		>
			<div style="flex:none;display:flex;align-items:center;gap:12px;padding:22px 22px 18px;">
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

			<!-- `min-height:0` is the load-bearing half: without it a flex child
			     refuses to shrink below its content, so `overflow-y:auto` never
			     engages and the links still overflow the column. -->
			<nav class="nav-scroll" style="display:flex;flex-direction:column;gap:3px;padding:8px 12px;flex:1;min-height:0;overflow-y:auto;">
				{#each nav as item}
					<a
						href={item.href}
						on:click={() => (navOpen = false)}
						class="navlink"
						class:active={isActive(item.href)}
					>
						{#if item.icon === 'dashboard'}<svg viewBox="0 0 24 24" width="20" fill="none" stroke="currentColor" stroke-width="2.6" stroke-linecap="round"><circle cx="12" cy="12" r="9"/><circle cx="12" cy="12" r="2.4"/></svg>
						{:else if item.icon === 'history'}<svg viewBox="0 0 24 24" width="20" fill="none" stroke="currentColor" stroke-width="2.6" stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="12" r="9"/><path d="M12 7v5l3.5 2"/></svg>
						{:else if item.icon === 'search'}<svg viewBox="0 0 24 24" width="20" fill="none" stroke="currentColor" stroke-width="2.6" stroke-linecap="round" stroke-linejoin="round"><circle cx="11" cy="11" r="7"/><path d="M20 20l-3.6-3.6"/></svg>
						{:else if item.icon === 'charts'}<svg viewBox="0 0 24 24" width="20" fill="none" stroke="currentColor" stroke-width="2.6" stroke-linecap="round" stroke-linejoin="round"><path d="M8 21h8"/><path d="M12 17v4"/><path d="M7 4h10v4a5 5 0 0 1-10 0z"/><path d="M17 5h3v2a3 3 0 0 1-3 3"/><path d="M7 5H4v2a3 3 0 0 0 3 3"/></svg>
					{:else if item.icon === 'stats'}<svg viewBox="0 0 24 24" width="20" fill="none" stroke="currentColor" stroke-width="2.6" stroke-linecap="round" stroke-linejoin="round"><path d="M4 20V4"/><path d="M4 20h16"/><rect x="7" y="11" width="3.2" height="6" rx="1"/><rect x="13.5" y="7" width="3.2" height="10" rx="1"/></svg>
					{:else if item.icon === 'reports'}<svg viewBox="0 0 24 24" width="20" fill="none" stroke="currentColor" stroke-width="2.6" stroke-linecap="round" stroke-linejoin="round"><path d="M6 3h9l4 4v14a1 1 0 0 1-1 1H6a1 1 0 0 1-1-1V4a1 1 0 0 1 1-1z"/><path d="M14 3v5h5"/><path d="M9 13h6"/><path d="M9 17h4"/></svg>
						{:else if item.icon === 'import'}<svg viewBox="0 0 24 24" width="20" fill="none" stroke="currentColor" stroke-width="2.6" stroke-linecap="round" stroke-linejoin="round"><path d="M12 3v12"/><path d="M8 11l4 4 4-4"/><path d="M5 21h14"/></svg>
						{:else if item.icon === 'notes'}<svg viewBox="0 0 24 24" width="20" fill="none" stroke="currentColor" stroke-width="2.6" stroke-linecap="round" stroke-linejoin="round"><path d="M6 4h9l4 4v12H6z"/><path d="M9 12h7"/><path d="M9 16h5"/></svg>
						{:else if item.icon === 'playlists'}<svg viewBox="0 0 24 24" width="20" fill="none" stroke="currentColor" stroke-width="2.6" stroke-linecap="round" stroke-linejoin="round"><path d="M5 17.5l5.5-8.5 4 5 4.5-8"/><circle cx="5" cy="17.5" r="1.9" fill="currentColor" stroke="none"/><circle cx="10.5" cy="9" r="1.9" fill="currentColor" stroke="none"/><circle cx="14.5" cy="14" r="1.9" fill="currentColor" stroke="none"/><circle cx="19" cy="6" r="1.9" fill="currentColor" stroke="none"/></svg>
					{:else if item.icon === 'map'}<svg viewBox="0 0 24 24" width="20" fill="none" stroke="currentColor" stroke-width="2.6" stroke-linecap="round" stroke-linejoin="round"><path d="M9 4 3 6.5v13L9 17l6 2.5 6-2.5v-13L15 7z"/><path d="M9 4v13"/><path d="M15 7v12.5"/></svg>
						{:else if item.icon === 'loved'}<svg viewBox="0 0 24 24" width="20" fill="none" stroke="currentColor" stroke-width="2.6" stroke-linecap="round" stroke-linejoin="round"><path d="M20.8 6.6a5 5 0 0 0-7.3-.5L12 7.6l-1.5-1.5a5 5 0 0 0-7.3 6.8L12 21l8.8-8.1a5 5 0 0 0 0-6.3z"/></svg>
						{:else if item.icon === 'rediscover'}<svg viewBox="0 0 24 24" width="20" fill="none" stroke="currentColor" stroke-width="2.6" stroke-linecap="round" stroke-linejoin="round"><path d="M3 5v5h5"/><path d="M4.2 14a8 8 0 1 0 1.3-6.3L3 10"/></svg>
						{:else if item.icon === 'shelf'}<svg viewBox="0 0 24 24" width="20" fill="none" stroke="currentColor" stroke-width="2.6" stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="12" r="9"/><circle cx="12" cy="12" r="3"/></svg>
						{:else if item.icon === 'crate'}<svg viewBox="0 0 24 24" width="20" fill="none" stroke="currentColor" stroke-width="2.6" stroke-linecap="round" stroke-linejoin="round"><rect x="3" y="7" width="18" height="12.6" rx="1.5"/><path d="M7.6 10.6v5.6"/><path d="M11.4 10.6v5.6"/><path d="M16.6 11l-2.2 5.2"/></svg>
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

			{#if $deck.playing && $deck.track}
				<a
					href="/"
					on:click={() => (navOpen = false)}
					style="flex:none;margin:8px 12px 14px;padding:12px 14px;border-radius:18px;background:var(--td-panel);color:#f4e7cf;text-decoration:none;display:block;"
				>
					<div style="display:flex;align-items:center;gap:8px;margin-bottom:8px;">
						<span style="width:7px;height:7px;border-radius:50%;background:#e07a55;animation:td-blink 1.6s ease-in-out infinite;"></span>
						<span style="font-size:10px;letter-spacing:.14em;text-transform:uppercase;color:#c9a678;">Now Playing</span>
					</div>
					<div style="font-size:13.5px;font-weight:600;white-space:nowrap;overflow:hidden;text-overflow:ellipsis;">{$deck.track.title}</div>
					<div style="font-size:12px;color:#b3a487;white-space:nowrap;overflow:hidden;text-overflow:ellipsis;">{$deck.track.artist}</div>
				</a>
			{/if}

			<div style="flex:none;padding:2px 12px 14px;display:flex;align-items:center;justify-content:space-between;gap:10px;border-top:1px solid var(--td-border);padding-top:10px;">
				<span style="font-size:11px;color:var(--td-muted-2);padding-left:10px;">
					{serverVersion ? `server v${serverVersion} · ` : ''}ui v{__UI_VERSION__}
				</span>
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
	/* A visible scrollbar in a 250px sidebar is mostly noise, but it must not be
	   invisible either — the whole bug was not knowing there was more below.
	   A thin, muted track appears on hover and when scrolling. */
	.nav-scroll {
		scrollbar-width: thin;
		scrollbar-color: transparent transparent;
		transition: scrollbar-color 0.2s;
	}
	.nav-scroll:hover {
		scrollbar-color: var(--td-track) transparent;
	}
	.nav-scroll::-webkit-scrollbar {
		width: 8px;
	}
	.nav-scroll::-webkit-scrollbar-thumb {
		background: transparent;
		border-radius: 999px;
		border: 2px solid transparent;
		background-clip: content-box;
	}
	.nav-scroll:hover::-webkit-scrollbar-thumb {
		background: var(--td-track);
		background-clip: content-box;
	}

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
