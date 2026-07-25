<script lang="ts">
	import { onMount, onDestroy } from 'svelte';
	import { api } from '$lib/api';
	import type { TokenInfo } from '$lib/api';
	import { formatDateTime } from '$lib/utils';
	import { theme, setTheme, type ThemeMode } from '$lib/theme';
	import { showArtwork, setShowArtwork } from '$lib/prefs';
	import type { Connection, PlexSettings, PlexTestResult } from '$lib/api';

	const uiVersion = __UI_VERSION__;
	let backendVersion = '…';
	let origin = typeof window !== 'undefined' ? window.location.origin : 'http://your-server:8080';

	let tokens: TokenInfo[] = [];
	let loading = true;
	let listenCount = 0;

	let newTokenName = '';
	let createdToken = '';
	let tokenCopied = false;
	let creating = false;

	let healthStatus = '';
	let checking = false;

	let confirmClear = false;
	let clearing = false;
	let exportMsg = '';

	let timezone = '';
	let tzSaved = false;

	// language enrichment
	let isAdmin = false;
	let langStatus: { total: number; resolved: number; pending: number } | null = null;
	let enriching = false;
	let langPoll: ReturnType<typeof setInterval> | null = null;
	async function loadLangStatus() {
		try { langStatus = await api.getLanguageStatus(); } catch { /* optional */ }
	}
	async function runEnrichment() {
		enriching = true;
		try {
			await api.backfillLanguages();
			// poll progress until it stops moving
			langPoll = setInterval(async () => {
				const prev = langStatus?.pending ?? -1;
				await loadLangStatus();
				if (langStatus && (langStatus.pending === 0 || langStatus.pending === prev)) {
					if (langPoll) clearInterval(langPoll);
					langPoll = null;
					enriching = false;
				}
			}, 2000);
		} catch (e) { enriching = false; alert(String(e).replace(/^API \d+: /, '')); }
	}
	// artwork
	let artStatus: { total: number; resolved: number; pending: number } | null = null;
	let fetchingArt = false;
	let artPoll: ReturnType<typeof setInterval> | null = null;
	async function loadArtStatus() {
		try { artStatus = await api.getArtworkStatus(); } catch { /* optional */ }
	}
	async function runArtworkBackfill() {
		fetchingArt = true;
		try {
			await api.backfillArtwork();
			artPoll = setInterval(async () => {
				const prev = artStatus?.pending ?? -1;
				await loadArtStatus();
				// Stop when it's done, or when a pass made no progress at all.
				if (artStatus && (artStatus.pending === 0 || artStatus.pending === prev)) {
					if (artPoll) clearInterval(artPoll);
					artPoll = null;
					fetchingArt = false;
				}
			}, 2500);
		} catch (e) { fetchingArt = false; alert(String(e).replace(/^API \d+: /, '')); }
	}

	async function saveTimezone() {
		try { await api.putPreferences({ timezone: timezone.trim() }); tzSaved = true; setTimeout(() => (tzSaved = false), 2000); }
		catch { /* ignore */ }
	}

	// Typeahead over the browser's own IANA zone list — no bundled table to go
	// stale. Older engines lack supportedValuesOf; there the field stays free text.
	const allZones: string[] = (() => {
		try { return (Intl as any).supportedValuesOf?.('timeZone') ?? []; }
		catch { return []; }
	})();
	let tzFocused = false;
	let tzIndex = 0;

	function matchZones(query: string, focused: boolean): string[] {
		if (!focused || !allZones.length) return [];
		const q = query.trim().toLowerCase().replace(/\s+/g, '_');
		const pool = q ? allZones.filter((z) => z.toLowerCase().includes(q)) : allZones;
		// An exact hit means they've already chosen — don't hover a list over it.
		if (pool.length === 1 && pool[0].toLowerCase() === q) return [];
		return pool.slice(0, 60);
	}

	$: tzMatches = matchZones(timezone, tzFocused);
	$: if (tzIndex >= tzMatches.length) tzIndex = 0;

	function pickZone(z: string) {
		timezone = z;
		tzFocused = false;
		saveTimezone();
	}

	function tzKeydown(e: KeyboardEvent) {
		if (!tzMatches.length) return;
		if (e.key === 'ArrowDown') { e.preventDefault(); tzIndex = (tzIndex + 1) % tzMatches.length; }
		else if (e.key === 'ArrowUp') { e.preventDefault(); tzIndex = (tzIndex - 1 + tzMatches.length) % tzMatches.length; }
		else if (e.key === 'Enter') { e.preventDefault(); pickZone(tzMatches[tzIndex]); }
		else if (e.key === 'Escape') { tzFocused = false; }
	}

	// connections
	let connections: Connection[] = [];
	let lbToken = '';
	let lbBusy = false;
	// Last.fm and Libre.fm share the same approve-then-finish flow, so their
	// in-flight state is keyed by service rather than duplicated.
	type Scrobbler = 'lastfm' | 'librefm';
	let fmBusy: Scrobbler | null = null;
	let fmPendingToken: Record<Scrobbler, string> = { lastfm: '', librefm: '' };
	let connError = '';
	$: conn = (svc: string) => connections.find((c) => c.service === svc) ?? null;

	async function loadConnections() {
		try {
			const r = await api.getConnections();
			connections = r.connections;
		} catch { /* optional */ }
	}
	async function connectLB() {
		connError = '';
		if (!lbToken.trim()) return;
		lbBusy = true;
		try {
			await api.connectListenBrainz(lbToken.trim());
			lbToken = '';
			await loadConnections();
		} catch (e) { connError = String(e).replace(/^API \d+: /, ''); }
		lbBusy = false;
	}
	async function scrobblerConnect(svc: Scrobbler) {
		connError = '';
		fmBusy = svc;
		try {
			if (!fmPendingToken[svc]) {
				const { token, auth_url } = await api.scrobblerStart(svc);
				fmPendingToken[svc] = token;
				window.open(auth_url, '_blank', 'noopener');
			} else {
				await api.scrobblerComplete(svc, fmPendingToken[svc]);
				fmPendingToken[svc] = '';
				await loadConnections();
			}
		} catch (e) { connError = String(e).replace(/^API \d+: /, ''); }
		fmBusy = null;
	}
	async function disconnectSvc(svc: string) {
		try { await api.disconnect(svc); await loadConnections(); } catch (e) { connError = String(e).replace(/^API \d+: /, ''); }
		if (svc === 'lastfm' || svc === 'librefm') fmPendingToken[svc as Scrobbler] = '';
	}

	// ── Plex source (admin only, server-wide) ──
	let plex: PlexSettings | null = null;
	let plexUrl = '';
	let plexToken = '';        // blank = keep the stored one; never sent back to us
	let plexEnabled = false;
	let plexFilters = {
		users_allow: '', users_block: '',
		devices_allow: '', devices_block: '',
		libraries_allow: '', libraries_block: ''
	};
	let plexAdvanced = false;
	let plexBusy = false;
	let plexMsg = '';
	let plexError = '';
	let plexTest: PlexTestResult | null = null;

	function applyPlex(s: PlexSettings) {
		plex = s;
		plexUrl = s.url;
		plexEnabled = s.enabled;
		plexToken = '';
		plexFilters = {
			users_allow: s.users_allow.join(', '),
			users_block: s.users_block.join(', '),
			devices_allow: s.devices_allow.join(', '),
			devices_block: s.devices_block.join(', '),
			libraries_allow: s.libraries_allow.join(', '),
			libraries_block: s.libraries_block.join(', ')
		};
	}

	const splitList = (s: string) => s.split(',').map((x) => x.trim()).filter(Boolean);

	async function loadPlex() {
		try { applyPlex(await api.getPlexSettings()); } catch { /* non-admin or unreachable */ }
	}

	async function savePlex() {
		plexBusy = true; plexError = ''; plexMsg = ''; plexTest = null;
		try {
			applyPlex(await api.putPlexSettings({
				enabled: plexEnabled,
				url: plexUrl.trim(),
				...(plexToken.trim() ? { token: plexToken.trim() } : {}),
				users_allow: splitList(plexFilters.users_allow),
				users_block: splitList(plexFilters.users_block),
				devices_allow: splitList(plexFilters.devices_allow),
				devices_block: splitList(plexFilters.devices_block),
				libraries_allow: splitList(plexFilters.libraries_allow),
				libraries_block: splitList(plexFilters.libraries_block)
			}));
			plexMsg = 'Saved — the poll engine picks this up within a few seconds.';
		} catch (e) {
			plexError = String(e).replace(/^API \d+: /, '');
		}
		plexBusy = false;
	}

	async function testPlex() {
		plexBusy = true; plexError = ''; plexMsg = ''; plexTest = null;
		try {
			plexTest = await api.testPlex({
				url: plexUrl.trim(),
				...(plexToken.trim() ? { token: plexToken.trim() } : {})
			});
		} catch (e) {
			plexError = String(e).replace(/^API \d+: /, '');
		}
		plexBusy = false;
	}

	const themes: { v: ThemeMode; label: string }[] = [
		{ v: 'light', label: 'Light' },
		{ v: 'dark', label: 'Dark' },
		{ v: 'system', label: 'System' }
	];

	onMount(async () => {
		try {
			const [health, tokensRes] = await Promise.all([api.health(), api.getTokens()]);
			backendVersion = health.version;
			tokens = tokensRes.tokens;
		} catch { /* fallback */ }
		try { listenCount = (await api.getDashboardStats()).total; } catch { /* optional */ }
		try {
			const prefs = await api.getPreferences();
			timezone = (prefs.timezone as string) || Intl.DateTimeFormat().resolvedOptions().timeZone || 'UTC';
		} catch { timezone = 'UTC'; }
		try { isAdmin = (await api.me()).is_admin; } catch { /* optional */ }
		if (isAdmin) loadPlex();
		loadArtStatus();
		loadConnections();
		loadLangStatus();
		loading = false;
	});

	onDestroy(() => {
		if (langPoll) clearInterval(langPoll);
		if (artPoll) clearInterval(artPoll);
	});

	async function createToken() {
		if (!newTokenName.trim()) return;
		creating = true;
		createdToken = '';
		try {
			const result = await api.createToken(newTokenName.trim());
			createdToken = result.token;
			newTokenName = '';
			tokens = (await api.getTokens()).tokens;
		} catch (e) { console.error('Create token failed:', e); }
		creating = false;
	}

	async function revokeToken(id: number) {
		if (!confirm('Revoke this token? Any clients using it will stop working.')) return;
		try {
			await api.revokeToken(id);
			tokens = tokens.filter((t) => t.id !== id);
		} catch (e) { console.error('Revoke failed:', e); }
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
		} catch (e) { healthStatus = `✗ ${e instanceof Error ? e.message : e}`; }
		checking = false;
	}

	async function doExport(format: 'json' | 'csv') {
		try {
			const blob = await api.exportBlob(format);
			const url = URL.createObjectURL(blob);
			const a = document.createElement('a');
			a.href = url;
			a.download = `tapedeck-export.${format}`;
			a.click();
			URL.revokeObjectURL(url);
			exportMsg = `Exported ${format.toUpperCase()}`;
			setTimeout(() => (exportMsg = ''), 3000);
		} catch (e) { exportMsg = 'Export failed'; }
	}

	async function doBackup() {
		try {
			const blob = await api.backupBlob();
			const url = URL.createObjectURL(blob);
			const a = document.createElement('a');
			a.href = url;
			a.download = 'tapedeck-backup.db';
			a.click();
			URL.revokeObjectURL(url);
			exportMsg = 'Backup downloaded';
			setTimeout(() => (exportMsg = ''), 3000);
		} catch (e) { exportMsg = 'Backup failed (admin only)'; }
	}

	async function doClear() {
		clearing = true;
		try {
			await api.clearScrobbles();
			listenCount = 0;
			confirmClear = false;
		} catch (e) { alert('Clear failed: ' + e); }
		clearing = false;
	}

	async function handleLogout() {
		await api.logout();
		window.location.href = '/login';
	}
</script>

<svelte:head><title>Settings — Tapedeck</title></svelte:head>

<h1 class="h-display" style="font-size:38px;margin-bottom:6px;">Settings</h1>
<p style="margin:0 0 24px;color:var(--td-muted);font-size:15px;">Tokens, connections, and the machinery behind the deck.</p>

<!-- Appearance -->
<div class="panel" style="border-radius:22px;padding:24px;margin-bottom:18px;">
	<h3 class="h-display" style="font-size:19px;margin-bottom:4px;">Appearance</h3>
	<p style="font-size:13px;color:var(--td-muted);margin:0 0 16px;">Choose a theme, or follow your system. Saved to your profile so every device matches.</p>
	<div style="display:flex;background:var(--td-sidebar);border-radius:999px;padding:4px;gap:2px;max-width:360px;">
		{#each themes as o}
			<button class="seg-btn" class:on={$theme === o.v} on:click={() => setTheme(o.v)} style="flex:1;">{o.label}</button>
		{/each}
	</div>

	<div style="margin-top:18px;padding-top:18px;border-top:1px solid var(--td-border);">
		<label style="display:flex;align-items:center;gap:10px;cursor:pointer;font-size:14px;">
			<input type="checkbox" checked={$showArtwork} on:change={(e) => setShowArtwork(e.currentTarget.checked)}
				style="width:16px;height:16px;accent-color:var(--td-accent);cursor:pointer;">
			Show album art
		</label>
		<p style="font-size:12.5px;color:var(--td-muted);margin:6px 0 0;">
			Covers come from the Cover Art Archive, matched via MusicBrainz. Listens without a match keep the lettered tile.
		</p>

		{#if artStatus}
			<div style="display:flex;align-items:center;gap:12px;flex-wrap:wrap;margin-top:12px;">
				<div style="flex:1;min-width:180px;">
					<div style="height:8px;border-radius:999px;background:var(--td-track);overflow:hidden;">
						<div style="height:100%;width:{artStatus.total ? (artStatus.resolved / artStatus.total) * 100 : 0}%;background:linear-gradient(90deg,var(--td-accent-2),var(--td-accent));border-radius:999px;transition:width .4s;"></div>
					</div>
					<div style="font-size:12.5px;color:var(--td-muted-2);margin-top:6px;">
						{artStatus.resolved} / {artStatus.total} listens have artwork
					</div>
				</div>
				{#if isAdmin}
					<button class="btn btn-secondary" on:click={runArtworkBackfill} disabled={fetchingArt || artStatus.pending === 0}>
						{fetchingArt ? 'Fetching…' : artStatus.pending === 0 ? 'All fetched' : 'Fetch missing art'}
					</button>
				{/if}
			</div>
			{#if fetchingArt}
				<p style="font-size:12px;color:var(--td-muted-3);margin:8px 0 0;">
					Rate-limited by MusicBrainz, so a large history takes a while. It also runs on its own in the background — you can leave this page.
				</p>
			{/if}
		{/if}
	</div>

	<div style="margin-top:18px;">
		<div class="field" style="max-width:360px;"><label for="tz">Timezone <span style="opacity:.6">(for local-time analytics)</span></label>
			<div style="display:flex;gap:8px;position:relative;">
				<div style="flex:1;position:relative;">
					<input
						id="tz" class="input" bind:value={timezone} placeholder="Type to search — e.g. Berlin"
						autocomplete="off" role="combobox" aria-expanded={tzMatches.length > 0} aria-controls="tz-list"
						on:focus={() => (tzFocused = true)}
						on:blur={() => setTimeout(() => (tzFocused = false), 120)}
						on:keydown={tzKeydown}
					>
					{#if tzMatches.length}
						<ul id="tz-list" class="tz-list">
							{#each tzMatches as z, i}
								<li>
									<button
										type="button"
										class="tz-option"
										class:active={i === tzIndex}
										on:mouseenter={() => (tzIndex = i)}
										on:click={() => pickZone(z)}
									>{z.replace(/_/g, ' ')}</button>
								</li>
							{/each}
						</ul>
					{/if}
				</div>
				<button class="btn btn-secondary" on:click={saveTimezone}>{tzSaved ? '✓' : 'Save'}</button>
			</div>
		</div>
	</div>
</div>

<!-- Language enrichment (admin) -->
{#if isAdmin}
	<div class="panel" style="border-radius:22px;padding:24px;margin-bottom:18px;">
		<h3 class="h-display" style="font-size:19px;margin-bottom:4px;">Language Enrichment</h3>
		<p style="font-size:13px;color:var(--td-muted);margin:0 0 16px;">Classify listens by language for the Analytics breakdown — instant for non-Latin scripts, MusicBrainz (rate-limited) for the rest. Runs automatically in the background; use this to fill the backlog now.</p>
		<div style="display:flex;align-items:center;gap:14px;flex-wrap:wrap;">
			<button class="btn btn-primary" on:click={runEnrichment} disabled={enriching}>{enriching ? 'Enriching…' : 'Enrich languages'}</button>
			{#if langStatus}
				<span style="font-size:13.5px;color:var(--td-muted);">{langStatus.resolved} / {langStatus.total} classified{langStatus.pending ? ` · ${langStatus.pending} pending` : ''}</span>
			{/if}
		</div>
	</div>
{/if}

<!-- Connections -->
<div class="panel" style="border-radius:22px;padding:24px;margin-bottom:18px;">
	<h3 class="h-display" style="font-size:19px;margin-bottom:4px;">Scrobble Connections</h3>
	<p style="font-size:13px;color:var(--td-muted);margin:0 0 16px;">Forward <em>your</em> listens to your own accounts. Nothing is shared across users, and a service you don't connect simply isn't forwarded to — listens are always kept locally either way.</p>

	{#if connError}<div style="font-size:13px;color:#b2622d;margin-bottom:12px;">{connError}</div>{/if}

	<!-- ListenBrainz -->
	<div style="display:flex;align-items:center;gap:12px;flex-wrap:wrap;padding:12px 0;border-bottom:1px solid var(--td-border);">
		<div style="flex:1;min-width:160px;">
			<div style="font-weight:600;font-size:14px;">ListenBrainz</div>
			{#if conn('listenbrainz')}
				<div style="font-size:12.5px;color:var(--td-good-fg);">Connected{conn('listenbrainz')?.username ? ` as ${conn('listenbrainz')?.username}` : ''}</div>
			{:else}
				<div style="font-size:12.5px;color:var(--td-muted-2);">Paste your user token from listenbrainz.org → Settings.</div>
			{/if}
		</div>
		{#if conn('listenbrainz')}
			<button class="btn btn-secondary" style="color:#b2622d;" on:click={() => disconnectSvc('listenbrainz')}>Disconnect</button>
		{:else}
			<input class="input" style="width:220px;" type="password" bind:value={lbToken} placeholder="ListenBrainz token">
			<button class="btn btn-primary" on:click={connectLB} disabled={lbBusy || !lbToken.trim()}>{lbBusy ? '…' : 'Connect'}</button>
		{/if}
	</div>

	<!-- Last.fm and Libre.fm: same approve-then-finish flow -->
	{#each [{ id: 'lastfm', label: 'Last.fm' }, { id: 'librefm', label: 'Libre.fm' }] as svc, i}
		{@const key = svc.id as 'lastfm' | 'librefm'}
		<div style="display:flex;align-items:center;gap:12px;flex-wrap:wrap;padding:12px 0;{i === 0 ? 'border-bottom:1px solid var(--td-border);' : ''}">
			<div style="flex:1;min-width:160px;">
				<div style="font-weight:600;font-size:14px;">{svc.label}</div>
				{#if conn(key)}
					<div style="font-size:12.5px;color:var(--td-good-fg);">Connected{conn(key)?.username ? ` as ${conn(key)?.username}` : ''}</div>
				{:else if fmPendingToken[key]}
					<div style="font-size:12.5px;color:var(--td-accent);">Approve Tapedeck in the {svc.label} tab, then click “Finish”.</div>
				{:else}
					<div style="font-size:12.5px;color:var(--td-muted-2);">Click Connect, approve on {svc.label}, then finish.</div>
				{/if}
			</div>
			{#if conn(key)}
				<button class="btn btn-secondary" style="color:#b2622d;" on:click={() => disconnectSvc(key)}>Disconnect</button>
			{:else}
				<button class="btn btn-primary" on:click={() => scrobblerConnect(key)} disabled={fmBusy !== null}>
					{fmBusy === key ? '…' : fmPendingToken[key] ? 'Finish connecting' : `Connect ${svc.label}`}
				</button>
				{#if fmPendingToken[key]}<button class="btn btn-secondary" on:click={() => (fmPendingToken[key] = '')}>Cancel</button>{/if}
			{/if}
		</div>
	{/each}
</div>

<!-- Plex source — admin only, and server-wide rather than per-user -->
{#if isAdmin && plex}
	<div class="panel" style="border-radius:22px;padding:24px;margin-bottom:18px;">
		<h3 class="h-display" style="font-size:19px;margin-bottom:4px;">Plex</h3>
		<p style="font-size:13px;color:var(--td-muted);margin:0 0 16px;">
			Where listens come <em>from</em>. Tapedeck polls this server and records what everyone plays,
			matching each Plex account to a Tapedeck user by name. This is instance-wide, not per-account —
			only admins can change it, and it applies without a restart.
		</p>

		{#if plexError}<div style="font-size:13px;color:#b2622d;margin-bottom:12px;">{plexError}</div>{/if}

		<div style="display:flex;align-items:center;gap:12px;padding:4px 0 14px;">
			<label style="display:flex;align-items:center;gap:10px;cursor:pointer;font-size:14px;">
				<input type="checkbox" bind:checked={plexEnabled} style="width:16px;height:16px;accent-color:var(--td-accent);cursor:pointer;">
				Poll this Plex server
			</label>
			{#if plex.enabled && plex.url}
				<span class="tag tag-accent-2" style="font-size:11px;">active</span>
			{/if}
		</div>

		<div style="display:flex;gap:12px;flex-wrap:wrap;">
			<div class="field" style="flex:2;min-width:240px;">
				<label for="plex-url">Server URL</label>
				<input id="plex-url" class="input" bind:value={plexUrl} placeholder="http://192.168.1.10:32400">
			</div>
			<div class="field" style="flex:1;min-width:200px;">
				<label for="plex-token">
					X-Plex-Token {#if plex.has_token}<span style="color:var(--td-good-fg);">· stored</span>{/if}
				</label>
				<input id="plex-token" class="input" type="password" bind:value={plexToken}
					placeholder={plex.has_token ? 'Leave blank to keep' : 'Paste your token'}>
			</div>
		</div>

		<button class="btn btn-ghost" style="margin-top:12px;font-size:13px;" on:click={() => (plexAdvanced = !plexAdvanced)}>
			{plexAdvanced ? '− Hide' : '+ Show'} filters
		</button>

		{#if plexAdvanced}
			<p style="font-size:12.5px;color:var(--td-muted-2);margin:8px 0 12px;">
				Comma-separated. Leave a list empty to allow everything; a block list always wins over an allow list.
			</p>
			<div style="display:grid;grid-template-columns:repeat(auto-fit,minmax(220px,1fr));gap:12px;">
				<div class="field"><label for="pf-ua">Users — allow</label><input id="pf-ua" class="input" bind:value={plexFilters.users_allow} placeholder="ashwin, partner"></div>
				<div class="field"><label for="pf-ub">Users — block</label><input id="pf-ub" class="input" bind:value={plexFilters.users_block} placeholder="guest"></div>
				<div class="field"><label for="pf-da">Devices — allow</label><input id="pf-da" class="input" bind:value={plexFilters.devices_allow} placeholder="Living Room"></div>
				<div class="field"><label for="pf-db">Devices — block</label><input id="pf-db" class="input" bind:value={plexFilters.devices_block} placeholder="Chrome"></div>
				<div class="field"><label for="pf-la">Libraries — allow</label><input id="pf-la" class="input" bind:value={plexFilters.libraries_allow} placeholder="Music"></div>
				<div class="field"><label for="pf-lb">Libraries — block</label><input id="pf-lb" class="input" bind:value={plexFilters.libraries_block} placeholder="Podcasts"></div>
			</div>
		{/if}

		<div style="display:flex;align-items:center;gap:12px;flex-wrap:wrap;margin-top:16px;">
			<button class="btn btn-primary" on:click={savePlex} disabled={plexBusy}>{plexBusy ? '…' : 'Save'}</button>
			<button class="btn btn-secondary" on:click={testPlex} disabled={plexBusy}>Test connection</button>
			{#if plexMsg}
				<span style="font-size:13px;color:var(--td-good-fg);">✓ {plexMsg}</span>
			{/if}
		</div>

		{#if plexTest}
			<div style="margin-top:14px;font-size:13px;">
				{#if plexTest.ok}
					<span style="color:var(--td-good-fg);">✓ Connected.</span>
					<span style="color:var(--td-muted);">
						{plexTest.libraries?.length ? `Libraries: ${plexTest.libraries.join(', ')}` : 'No libraries reported.'}
					</span>
				{:else}
					<span style="color:#b2622d;">✗ {plexTest.error}</span>
				{/if}
			</div>
		{/if}
	</div>
{/if}

<!-- API Tokens -->
<div class="panel" style="border-radius:22px;padding:24px;margin-bottom:18px;">
	<h3 class="h-display" style="font-size:19px;margin-bottom:4px;">API Tokens</h3>
	<p style="font-size:13px;color:var(--td-muted);margin:0 0 16px;">Tokens let Pano Scrobbler and other clients submit listens via <code class="chip">Authorization: Token td_…</code></p>

	{#if loading}
		<div style="display:flex;justify-content:center;padding:16px;"><div class="spin" style="width:18px;height:18px;border:2px solid var(--td-track);border-top-color:var(--td-accent);border-radius:50%;"></div></div>
	{:else}
		{#if tokens.length}
			<div style="display:flex;flex-direction:column;gap:8px;margin-bottom:16px;">
				{#each tokens as t (t.id)}
					<div style="display:flex;align-items:center;justify-content:space-between;gap:12px;background:var(--td-surface2);border:1px solid var(--td-border);border-radius:14px;padding:12px 16px;">
						<div style="min-width:0;">
							<div><span style="font-weight:600;font-size:14px;">{t.name}</span> <span style="font-size:10px;letter-spacing:.08em;text-transform:uppercase;color:var(--td-muted-3);margin-left:6px;">{t.scopes}</span></div>
							<div style="font-size:12px;color:var(--td-muted-2);margin-top:2px;">Created {formatDateTime(t.created_at)} · {t.last_used_at ? `Last used ${formatDateTime(t.last_used_at)}` : 'Never used'}</div>
						</div>
						<button class="btn btn-secondary" style="font-size:12.5px;color:#b2622d;" on:click={() => revokeToken(t.id)}>Revoke</button>
					</div>
				{/each}
			</div>
		{/if}

		{#if createdToken}
			<div style="background:var(--td-panel);border-radius:16px;padding:16px;margin-bottom:16px;">
				<div style="font-size:10.5px;letter-spacing:.1em;text-transform:uppercase;color:#e0a06a;margin-bottom:8px;">New token — copy it now, it won't be shown again</div>
				<div style="display:flex;gap:8px;align-items:center;">
					<code style="flex:1;background:var(--td-panel2);color:#a9c48a;border-radius:10px;padding:10px 12px;font-size:13px;word-break:break-all;">{createdToken}</code>
					<button class="btn btn-primary" on:click={copyToken}>{tokenCopied ? '✓ Copied' : 'Copy'}</button>
				</div>
			</div>
		{/if}

		<div style="display:flex;gap:8px;flex-wrap:wrap;">
			<input class="input" style="flex:1;min-width:180px;" bind:value={newTokenName} placeholder="Token name (e.g. walkman, phone)">
			<button class="btn btn-primary" on:click={createToken} disabled={creating || !newTokenName.trim()}>{creating ? '…' : 'Create'}</button>
		</div>
		<p style="font-size:12px;color:var(--td-muted-2);margin:10px 0 0;">Tokens can only submit listens. Reading, editing, and admin actions require signing in here.</p>
	{/if}
</div>

<!-- Connection -->
<div class="panel" style="border-radius:22px;padding:24px;margin-bottom:18px;">
	<h3 class="h-display" style="font-size:19px;margin-bottom:4px;">Connection</h3>
	<p style="font-size:13px;color:var(--td-muted);margin:0 0 16px;">Test connectivity to the Tapedeck backend.</p>
	<div style="display:flex;align-items:center;gap:14px;">
		<button class="btn btn-secondary" on:click={checkConnection} disabled={checking}>{checking ? 'Checking…' : 'Test connection'}</button>
		{#if healthStatus}<span style="font-size:14px;font-weight:600;color:{healthStatus.startsWith('✓') ? 'var(--td-good-fg)' : '#b2622d'};">{healthStatus}</span>{/if}
	</div>
</div>

<!-- Pano setup -->
<div class="panel" style="border-radius:22px;padding:24px;margin-bottom:18px;">
	<h3 class="h-display" style="font-size:19px;margin-bottom:4px;">Pano Scrobbler Setup</h3>
	<p style="font-size:13px;color:var(--td-muted);margin:0 0 16px;">Point every device at your instance in one step.</p>
	<ol style="margin:0;padding-left:22px;font-size:14px;color:var(--td-nav-fg);display:flex;flex-direction:column;gap:8px;">
		<li>Open Pano Scrobbler → Settings → Scrobble services</li>
		<li>Add a custom ListenBrainz server</li>
		<li>URL: <code class="chip">{origin}</code></li>
		<li>Token: one of your <code class="chip">td_…</code> tokens above</li>
	</ol>
</div>

<!-- Data & Backups -->
<div class="panel" style="border-radius:22px;padding:24px;margin-bottom:18px;">
	<h3 class="h-display" style="font-size:19px;margin-bottom:4px;">Data &amp; Backups</h3>
	<p style="font-size:13px;color:var(--td-muted);margin:0 0 16px;">{listenCount} listens recorded. Export a full backup or wipe the deck clean.</p>
	<div style="display:flex;gap:10px;flex-wrap:wrap;align-items:center;">
		<button class="btn btn-secondary" on:click={() => doExport('json')}>Export JSON</button>
		<button class="btn btn-secondary" on:click={() => doExport('csv')}>Export CSV</button>
		<button class="btn btn-secondary" on:click={doBackup}>Download DB backup</button>
		<span style="flex:1;"></span>
		<button class="btn btn-ghost" style="color:#b2622d;" on:click={() => (confirmClear = true)}>Clear history</button>
	</div>
	{#if exportMsg}
		<div style="margin-top:14px;display:inline-flex;align-items:center;gap:8px;font-size:13px;color:var(--td-good-fg);background:var(--td-good-bg);padding:7px 14px;border-radius:999px;">✓ {exportMsg}</div>
	{/if}
</div>

<!-- About -->
<div class="panel" style="border-radius:22px;padding:24px;display:flex;align-items:flex-start;justify-content:space-between;gap:16px;">
	<div>
		<h3 class="h-display" style="font-size:19px;margin-bottom:6px;">About</h3>
		<div style="font-size:13px;color:var(--td-muted);display:flex;flex-direction:column;gap:3px;">
			<span>Server v{backendVersion} · UI v{uiVersion}</span>
			<span>A self-hosted music listening journal.</span>
			<a href="https://codeberg.org/abksh/tapedeck" target="_blank" rel="noopener">View on Codeberg →</a>
		</div>
	</div>
	<button class="btn btn-secondary" style="color:#b2622d;" on:click={handleLogout}>Sign out</button>
</div>

{#if confirmClear}
	<div class="dialog-backdrop" on:click={() => (confirmClear = false)} on:keydown={() => {}} role="presentation">
		<div class="dialog" on:click|stopPropagation on:keydown|stopPropagation role="presentation">
			<div class="dialog-title">Clear listen history?</div>
			<div class="dialog-body" style="color:var(--td-nav-fg);">This permanently deletes all {listenCount} recorded listens from this instance. Export a backup first if you might want them back — this can't be undone.</div>
			<div class="dialog-actions">
				<button class="btn btn-secondary" on:click={() => (confirmClear = false)}>Cancel</button>
				<button class="btn btn-primary" style="background:#b2622d;" on:click={doClear} disabled={clearing}>{clearing ? 'Clearing…' : 'Clear everything'}</button>
			</div>
		</div>
	</div>
{/if}

<style>
	.tz-list {
		position: absolute;
		z-index: 30;
		top: calc(100% + 4px);
		left: 0;
		right: 0;
		max-height: 240px;
		overflow-y: auto;
		margin: 0;
		padding: 5px;
		list-style: none;
		background: var(--td-surface);
		border: 1px solid var(--td-border);
		border-radius: 16px;
		box-shadow: 0 12px 28px rgba(46, 43, 37, 0.18);
	}
	.tz-option {
		display: block;
		width: 100%;
		text-align: left;
		border: none;
		background: transparent;
		cursor: pointer;
		font: inherit;
		font-size: 13.5px;
		color: var(--td-text);
		padding: 7px 12px;
		border-radius: 10px;
	}
	.tz-option.active {
		background: var(--td-nav-hover);
	}
	.spin { animation: td-spin 0.7s linear infinite; }
	.seg-btn { border: none; background: transparent; cursor: pointer; color: var(--td-nav-fg); font-size: 13px; padding: 7px 14px; border-radius: 999px; font-family: inherit; }
	.seg-btn.on { background: var(--td-accent); color: #fbf5ea; }
	.chip { background: var(--td-sidebar); padding: 2px 6px; border-radius: 6px; font-size: 12px; }
</style>
