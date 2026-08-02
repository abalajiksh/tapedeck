<script lang="ts">
	import { onMount, onDestroy } from 'svelte';
	import { api } from '$lib/api';
	import type { TokenInfo } from '$lib/api';
	import { formatDateTime } from '$lib/utils';
	import { theme, setTheme, type ThemeMode } from '$lib/theme';
	import { showArtwork, setShowArtwork } from '$lib/prefs';
	import type { Connection, SignalChain } from '$lib/api';
	import SourcesPanel from '$lib/components/SourcesPanel.svelte';

	const uiVersion = __UI_VERSION__;
	let backendVersion = '…';
	let origin = typeof window !== 'undefined' ? window.location.origin : 'http://your-server:8080';

	// ── Sub-sections ──
	// The page had grown to nine stacked panels; splitting it means the thing you
	// came for is one click away instead of a scroll hunt. The section lives in the
	// hash so a link like /settings#sources lands where it says it will.
	const SECTIONS = [
		{ id: 'appearance', label: 'Appearance', hint: 'Theme, artwork, timezone' },
		{ id: 'sources', label: 'Sources', hint: 'Plex, Navidrome, Jellyfin' },
		{ id: 'connections', label: 'Connections', hint: 'Where listens are forwarded' },
		{ id: 'tokens', label: 'API Tokens', hint: 'Scrobble clients' },
		{ id: 'data', label: 'Data', hint: 'Export, backup, wipe' },
		{ id: 'maintenance', label: 'Maintenance', hint: 'Enrichment and health' },
		{ id: 'about', label: 'About', hint: 'Version, sign out' }
	];
	let section = 'appearance';
	function go(id: string) {
		section = id;
		if (typeof history !== 'undefined') history.replaceState(null, '', `#${id}`);
	}

	let tokens: TokenInfo[] = [];
	let chains: SignalChain[] = [];
	let loading = true;
	let listenCount = 0;

	let newTokenName = '';
	let newTokenChain = ''; // '' = no chain; otherwise the chain id as a string
	let createdToken = '';
	let tokenCopied = false;
	let creating = false;

	let healthStatus = '';
	let checking = false;

	// ── Metadata sanitiser ──
	// Preview first, always. Scrobble metadata that came from a real MusicBrainz
	// mapping is still data; rewriting it without showing what changes would be
	// a worse failure than the mismatch being fixed.
	let sanitize: import('$lib/api').SanitizePreview | null = null;
	let sanitizing = false;
	let sanitizeMsg = '';
	let fixAlbums = true;
	let fixArtists = true;
	let fixVariants = true;

	// ── Rebuilding sessions ──
	// Needed on any existing install: `assign_session` kept the session rows but
	// nothing ever wrote `scrobbles.session_id`, so the sessions had no contents
	// until this runs once. Also the only way an imported history gets grouped.
	let rebuildingSessions = false;
	let sessionMsg = '';
	let sessionGapMinutes = 30;
	async function rebuildSessions() {
		if (!confirm('Regroup every listen into sessions?\n\nThis discards any sessions you have merged or split by hand.')) return;
		rebuildingSessions = true;
		sessionMsg = '';
		try {
			const r = await api.rebuildSessions(Math.max(1, sessionGapMinutes) * 60);
			sessionMsg = `Grouped ${r.listens.toLocaleString()} listens into ${r.sessions.toLocaleString()} sessions.`;
		} catch (e) {
			sessionMsg = 'Failed: ' + String(e).replace(/^API \d+: /, '');
		}
		rebuildingSessions = false;
	}

	// ── Pulling loves from connected services ──
	let pullingLoves = false;
	let lovesMsg = '';
	async function pullLoves() {
		pullingLoves = true;
		lovesMsg = '';
		try {
			const r = await api.pullLoves();
			const parts = r.services.map((s) =>
				s.error
					? `${s.service}: ${s.error}`
					: `${s.service}: ${s.loved} new${s.skipped ? `, ${s.skipped} not in your history` : ''}`
			);
			lovesMsg = parts.length ? parts.join(' · ') : 'No connected services to pull from.';
		} catch (e) {
			lovesMsg = 'Failed: ' + String(e).replace(/^API \d+: /, '');
		}
		pullingLoves = false;
	}

	async function loadSanitize() {
		try { sanitize = await api.sanitizePreview(); } catch { sanitize = null; }
	}
	async function runSanitize() {
		sanitizing = true;
		sanitizeMsg = '';
		try {
			const r = await api.sanitizeApply({ albums: fixAlbums, artists: fixArtists, variants: fixVariants });
			sanitizeMsg = `Fixed ${r.albums_fixed} album${r.albums_fixed === 1 ? '' : 's'} (${r.album_listens_changed} listens), folded ${r.artists_folded} featured credit${r.artists_folded === 1 ? '' : 's'} (${r.artist_listens_changed} listens) and merged ${r.variants_merged} artist spelling${r.variants_merged === 1 ? '' : 's'} (${r.variant_listens_changed} listens). Artwork will re-resolve in the background.`;
			await loadSanitize();
		} catch (e) {
			sanitizeMsg = 'Failed: ' + e;
		}
		sanitizing = false;
	}

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

	const themes: { v: ThemeMode; label: string }[] = [
		{ v: 'light', label: 'Light' },
		{ v: 'dark', label: 'Dark' },
		{ v: 'system', label: 'System' }
	];

	function sectionFromHash() {
		const h = window.location.hash.slice(1);
		if (SECTIONS.some((s) => s.id === h)) section = h;
	}

	onMount(async () => {
		sectionFromHash();
		// A hash-only navigation (a link from elsewhere in the SPA, or Back after
		// clicking through sections) doesn't remount this page, so follow it.
		window.addEventListener('hashchange', sectionFromHash);

		try {
			const [health, tokensRes] = await Promise.all([api.health(), api.getTokens()]);
			backendVersion = health.version;
			tokens = tokensRes.tokens;
		} catch { /* fallback */ }
		// Chains drive the per-token dropdown; an instance with none simply
		// doesn't show it, so this failing is not worth surfacing.
		try { chains = (await api.getChains()).chains; } catch { /* optional */ }
		try { listenCount = (await api.getDashboardStats()).total; } catch { /* optional */ }
		try {
			const prefs = await api.getPreferences();
			timezone = (prefs.timezone as string) || Intl.DateTimeFormat().resolvedOptions().timeZone || 'UTC';
		} catch { timezone = 'UTC'; }
		try { isAdmin = (await api.me()).is_admin; } catch { /* optional */ }
		loadArtStatus();
		loadSanitize();
		loadConnections();
		loadLangStatus();
		loading = false;
	});

	onDestroy(() => {
		if (langPoll) clearInterval(langPoll);
		if (artPoll) clearInterval(artPoll);
		if (typeof window !== 'undefined') window.removeEventListener('hashchange', sectionFromHash);
	});

	async function createToken() {
		if (!newTokenName.trim()) return;
		creating = true;
		createdToken = '';
		try {
			const result = await api.createToken(newTokenName.trim(), {
				default_chain_id: newTokenChain ? Number(newTokenChain) : null
			});
			createdToken = result.token;
			newTokenName = '';
			newTokenChain = '';
			tokens = (await api.getTokens()).tokens;
		} catch (e) { console.error('Create token failed:', e); }
		creating = false;
	}

	/** Repoint a token at a different chain. Optimistic — the row is the only
	 *  place this value is shown, so a failure just puts it back. */
	async function setTokenChain(t: TokenInfo, value: string) {
		const chain_id = value ? Number(value) : null;
		const before = t.default_chain_id;
		tokens = tokens.map((x) => (x.id === t.id ? { ...x, default_chain_id: chain_id } : x));
		try {
			await api.updateToken(t.id, { default_chain_id: chain_id });
		} catch (e) {
			tokens = tokens.map((x) => (x.id === t.id ? { ...x, default_chain_id: before } : x));
			alert('Failed to set chain: ' + e);
		}
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

<div class="settings-layout">
	<nav class="settings-nav" aria-label="Settings sections">
		{#each SECTIONS as s}
			<button class="nav-item" class:on={section === s.id} on:click={() => go(s.id)}>
				<span class="nav-label">{s.label}</span>
				<span class="nav-hint">{s.hint}</span>
			</button>
		{/each}
	</nav>

	<div class="settings-body">
		{#if section === 'appearance'}
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
						Covers come from your own library where the listen came from one, then Deezer, then the
						Cover Art Archive. Listens with no match anywhere keep the lettered tile.
						{#if artStatus}
							<a href="#maintenance" on:click|preventDefault={() => go('maintenance')}>Fetch missing art →</a>
						{/if}
					</p>
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
		{/if}

		{#if section === 'sources'}
			<SourcesPanel />
		{/if}

		{#if section === 'connections'}
			<div class="panel" style="border-radius:22px;padding:24px;margin-bottom:18px;">
				<h3 class="h-display" style="font-size:19px;margin-bottom:4px;">Scrobble Connections</h3>
				<p style="font-size:13px;color:var(--td-muted);margin:0 0 16px;">Forward <em>your</em> listens to your own accounts. Nothing is shared across users, and a service you don't connect simply isn't forwarded to — listens are always kept locally either way.</p>

				{#if connError}<div style="font-size:13px;color:#b2622d;margin-bottom:12px;">{connError}</div>{/if}

				{#if connections.length}
					<div style="display:flex;align-items:center;gap:12px;flex-wrap:wrap;padding:12px 14px;margin-bottom:14px;background:var(--td-surface2);border:1px solid var(--td-border);border-radius:14px;">
						<div style="flex:1;min-width:180px;">
							<div style="font-weight:600;font-size:14px;">Pull loved tracks</div>
							<div style="font-size:12.5px;color:var(--td-muted-2);margin-top:2px;">
								Brings your existing loves across from the services above. A track you've never
								played here is skipped rather than invented.
							</div>
						</div>
						<button class="btn btn-secondary" on:click={pullLoves} disabled={pullingLoves}>
							{pullingLoves ? 'Pulling…' : 'Pull loves'}
						</button>
					</div>
					{#if lovesMsg}
						<p style="font-size:12.5px;color:var(--td-good-fg);margin:-6px 0 14px;">{lovesMsg}</p>
					{/if}
				{/if}

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
		{/if}

		{#if section === 'tokens'}
			<div class="panel" style="border-radius:22px;padding:24px;margin-bottom:18px;">
				<h3 class="h-display" style="font-size:19px;margin-bottom:4px;">API Tokens</h3>
				<p style="font-size:13px;color:var(--td-muted);margin:0 0 16px;">Tokens let Pano Scrobbler and other clients submit listens via <code class="chip">Authorization: Token td_…</code></p>

				{#if loading}
					<div style="display:flex;justify-content:center;padding:16px;"><div class="spin" style="width:18px;height:18px;border:2px solid var(--td-track);border-top-color:var(--td-accent);border-radius:50%;"></div></div>
				{:else}
					{#if tokens.length}
						<div style="display:flex;flex-direction:column;gap:8px;margin-bottom:16px;">
							{#each tokens as t (t.id)}
								<div style="display:flex;align-items:center;justify-content:space-between;gap:12px;flex-wrap:wrap;background:var(--td-surface2);border:1px solid var(--td-border);border-radius:14px;padding:12px 16px;">
									<div style="min-width:0;flex:1;">
										<div><span style="font-weight:600;font-size:14px;">{t.name}</span> <span style="font-size:10px;letter-spacing:.08em;text-transform:uppercase;color:var(--td-muted-3);margin-left:6px;">{t.scopes}</span></div>
										<div style="font-size:12px;color:var(--td-muted-2);margin-top:2px;">Created {formatDateTime(t.created_at)} · {t.last_used_at ? `Last used ${formatDateTime(t.last_used_at)}` : 'Never used'}</div>
									</div>
									<select class="input" style="width:auto;min-width:170px;font-size:12.5px;" value={t.default_chain_id ?? ''}
										on:change={(e) => setTokenChain(t, e.currentTarget.value)} aria-label="Default signal chain">
										<option value="">— no chain —</option>
										{#each chains as c}<option value={c.id}>{c.name}</option>{/each}
									</select>
									<button class="btn btn-secondary" style="font-size:12.5px;color:#b2622d;" on:click={() => revokeToken(t.id)}>Revoke</button>
								</div>
							{/each}
						</div>
						<p style="font-size:12px;color:var(--td-muted-2);margin:-6px 0 16px;">
							Changing a chain here takes effect immediately — the token itself doesn't change, so there's nothing to re-paste into the app.
							{#if !chains.length}<a href="/chains">Create a chain first →</a>{/if}
						</p>
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
						<input class="input" style="flex:1;min-width:180px;" bind:value={newTokenName} placeholder="Token name (e.g. fooyin, Pano on Android)">
						{#if chains.length}
							<select class="input" style="width:auto;min-width:170px;" bind:value={newTokenChain} aria-label="Default signal chain for the new token">
								<option value="">— no chain —</option>
								{#each chains as c}<option value={String(c.id)}>{c.name}</option>{/each}
							</select>
						{/if}
						<button class="btn btn-primary" on:click={createToken} disabled={creating || !newTokenName.trim()}>{creating ? '…' : 'Create'}</button>
					</div>
					<p style="font-size:12px;color:var(--td-muted-2);margin:10px 0 0;">
						One token per app is the idea: give it a chain here and every listen it submits is attributed to that setup, with nothing to configure in the app itself.
						A listen that carries its own chain, or arrives through a mapped output, still wins.
					</p>
					<p style="font-size:12px;color:var(--td-muted-2);margin:8px 0 0;">Tokens can only submit listens. Reading, editing, and admin actions require signing in here.</p>
				{/if}
			</div>

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
		{/if}

		{#if section === 'data'}
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
				<p style="font-size:12px;color:var(--td-muted-2);margin:14px 0 0;">
					The DB backup contains every user's listens and their encrypted credentials — treat it
					like a password file. The encryption key is kept outside the database and is
					<em>not</em> in the backup, so a leaked copy alone won't hand over your tokens.
				</p>
			</div>
		{/if}

		{#if section === 'maintenance'}
			<div class="panel" style="border-radius:22px;padding:24px;margin-bottom:18px;">
				<h3 class="h-display" style="font-size:19px;margin-bottom:4px;">Metadata Sanitiser</h3>
				<p style="font-size:13px;color:var(--td-muted);margin:0 0 16px;">
					Scrobble clients tag each track on its own, so one song from an album routinely arrives
					carrying a compilation's release — wrong cover, and it drops out of the album's stats.
					This finds those and puts them back on the release the rest of the album agrees on.
				</p>

				{#if sanitize}
					{#if sanitize.mixed_albums.length || sanitize.featured_artists.length}
						<div style="display:flex;flex-direction:column;gap:14px;margin-bottom:16px;">
							{#if sanitize.mixed_albums.length}
								<div>
									<label style="display:flex;align-items:center;gap:9px;cursor:pointer;font-size:14px;font-weight:600;">
										<input type="checkbox" bind:checked={fixAlbums} style="width:16px;height:16px;accent-color:var(--td-accent);cursor:pointer;">
										{sanitize.mixed_albums.length} album{sanitize.mixed_albums.length === 1 ? '' : 's'} with mixed releases
									</label>
									<div style="margin:8px 0 0 25px;font-size:12.5px;color:var(--td-muted-2);display:flex;flex-direction:column;gap:4px;">
										{#each sanitize.mixed_albums.slice(0, 6) as a}
											<div>{a.album} — {a.artist} <span style="color:var(--td-muted-3);">· {a.variants} releases across {a.plays} plays</span></div>
										{/each}
										{#if sanitize.mixed_albums.length > 6}
											<div style="color:var(--td-muted-3);">…and {sanitize.mixed_albums.length - 6} more</div>
										{/if}
									</div>
								</div>
							{/if}
							{#if sanitize.featured_artists.length}
								<div>
									<label style="display:flex;align-items:center;gap:9px;cursor:pointer;font-size:14px;font-weight:600;">
										<input type="checkbox" bind:checked={fixArtists} style="width:16px;height:16px;accent-color:var(--td-accent);cursor:pointer;">
										{sanitize.featured_artists.length} featured credit{sanitize.featured_artists.length === 1 ? '' : 's'} splitting an artist
									</label>
									<div style="margin:8px 0 0 25px;font-size:12.5px;color:var(--td-muted-2);display:flex;flex-direction:column;gap:4px;">
										{#each sanitize.featured_artists.slice(0, 6) as a}
											<div>{a.artist} <span style="color:var(--td-muted-3);">→ {a.primary} · {a.plays} plays</span></div>
										{/each}
										{#if sanitize.featured_artists.length > 6}
											<div style="color:var(--td-muted-3);">…and {sanitize.featured_artists.length - 6} more</div>
										{/if}
									</div>
									<p style="margin:8px 0 0 25px;font-size:12px;color:var(--td-muted-3);">
										The full credit is kept, so the featured performer isn't lost — it just stops
										fragmenting the album and artist pages. Names joined by “&amp;” or “,” are left alone.
									</p>
								</div>
							{/if}
							{#if sanitize.artist_variants.length}
								<div>
									<label style="display:flex;align-items:center;gap:9px;cursor:pointer;font-size:14px;font-weight:600;">
										<input type="checkbox" bind:checked={fixVariants} style="width:16px;height:16px;accent-color:var(--td-accent);cursor:pointer;">
										{sanitize.artist_variants.length} artist{sanitize.artist_variants.length === 1 ? '' : 's'} spelled more than one way
									</label>
									<div style="margin:8px 0 0 25px;font-size:12.5px;color:var(--td-muted-2);display:flex;flex-direction:column;gap:4px;">
										{#each sanitize.artist_variants.slice(0, 6) as v}
											<div>
												{v.variants.map((x) => x[0]).join(', ')}
												<span style="color:var(--td-muted-3);">→ {v.primary} ({v.primary_plays} plays)</span>
											</div>
										{/each}
										{#if sanitize.artist_variants.length > 6}
											<div style="color:var(--td-muted-3);">…and {sanitize.artist_variants.length - 6} more</div>
										{/if}
									</div>
									<p style="margin:8px 0 0 25px;font-size:12px;color:var(--td-muted-3);">
										Same artist, different punctuation — “A.R. Rahman” and “A. R. Rahman” chart as two
										people and split their play count. The most-played spelling wins.
									</p>
								</div>
							{/if}
						</div>
						<button class="btn btn-primary" on:click={runSanitize} disabled={sanitizing || (!fixAlbums && !fixArtists && !fixVariants)}>
							{sanitizing ? 'Sanitising…' : 'Apply fixes'}
						</button>
					{:else}
						<p style="font-size:14px;color:var(--td-muted);margin:0;">Nothing to fix — every album agrees with itself.</p>
					{/if}
				{:else}
					<p style="font-size:14px;color:var(--td-muted);margin:0;">Checking…</p>
				{/if}
				{#if sanitizeMsg}
					<p style="font-size:13px;color:var(--td-good-fg);margin:12px 0 0;">{sanitizeMsg}</p>
				{/if}
			</div>

			<div class="panel" style="border-radius:22px;padding:24px;margin-bottom:18px;">
				<h3 class="h-display" style="font-size:19px;margin-bottom:4px;">Listening Sessions</h3>
				<p style="font-size:13px;color:var(--td-muted);margin:0 0 16px;">
					Listens are grouped into sessions as they arrive — contiguous listening, split by a
					long gap or a change of app. Rebuilding regroups your whole history from scratch,
					which is what an imported backlog needs, and lets you reconsider the gap.
				</p>
				<div style="display:flex;align-items:center;gap:12px;flex-wrap:wrap;">
					<label style="font-size:13px;color:var(--td-muted);display:flex;align-items:center;gap:8px;">
						New session after
						<input class="input" type="number" min="1" max="1440" bind:value={sessionGapMinutes} style="width:80px;">
						minutes
					</label>
					<button class="btn btn-secondary" on:click={rebuildSessions} disabled={rebuildingSessions}>
						{rebuildingSessions ? 'Regrouping…' : 'Rebuild sessions'}
					</button>
				</div>
				<p style="font-size:12px;color:var(--td-muted-3);margin:10px 0 0;">
					Discards sessions you've merged or split by hand — those choices live in the same
					place this rewrites.
				</p>
				{#if sessionMsg}
					<p style="font-size:13px;color:var(--td-good-fg);margin:12px 0 0;">{sessionMsg}</p>
				{/if}
			</div>

			<div class="panel" style="border-radius:22px;padding:24px;margin-bottom:18px;">
				<h3 class="h-display" style="font-size:19px;margin-bottom:4px;">Album Art</h3>
				<p style="font-size:13px;color:var(--td-muted);margin:0 0 16px;">
					Art is resolved in order: your own library (free and exact), then Deezer, then the Cover
					Art Archive. It runs in the background — this fills the backlog now.
				</p>
				{#if artStatus}
					<div style="display:flex;align-items:center;gap:12px;flex-wrap:wrap;">
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
						<p style="font-size:12px;color:var(--td-muted-3);margin:10px 0 0;">
							MusicBrainz is rate-limited, so a large history takes a while. You can leave this page.
						</p>
					{/if}
				{:else}
					<p style="font-size:13px;color:var(--td-muted-3);margin:0;">No status available.</p>
				{/if}
			</div>

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

			<div class="panel" style="border-radius:22px;padding:24px;margin-bottom:18px;">
				<h3 class="h-display" style="font-size:19px;margin-bottom:4px;">Connection</h3>
				<p style="font-size:13px;color:var(--td-muted);margin:0 0 16px;">Test connectivity to the Tapedeck backend.</p>
				<div style="display:flex;align-items:center;gap:14px;">
					<button class="btn btn-secondary" on:click={checkConnection} disabled={checking}>{checking ? 'Checking…' : 'Test connection'}</button>
					{#if healthStatus}<span style="font-size:14px;font-weight:600;color:{healthStatus.startsWith('✓') ? 'var(--td-good-fg)' : '#b2622d'};">{healthStatus}</span>{/if}
				</div>
			</div>
		{/if}

		{#if section === 'about'}
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
		{/if}
	</div>
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
	.settings-layout {
		display: grid;
		grid-template-columns: 208px minmax(0, 1fr);
		gap: 24px;
		align-items: start;
	}
	.settings-nav {
		display: flex;
		flex-direction: column;
		gap: 2px;
		position: sticky;
		top: 24px;
	}
	.nav-item {
		display: flex;
		flex-direction: column;
		gap: 2px;
		text-align: left;
		border: none;
		background: transparent;
		font: inherit;
		cursor: pointer;
		padding: 9px 14px;
		border-radius: 12px;
		color: var(--td-nav-fg);
	}
	.nav-item:hover { background: var(--td-nav-hover); }
	.nav-item.on { background: var(--td-sidebar); color: var(--td-text); }
	.nav-label { font-size: 14px; font-weight: 600; }
	.nav-hint { font-size: 11.5px; color: var(--td-muted-3); }
	.nav-item.on .nav-hint { color: var(--td-muted-2); }

	/* Narrow screens get the same list as a scrolling strip above the content —
	   a 208px rail would eat half a phone. */
	@media (max-width: 720px) {
		.settings-layout { grid-template-columns: minmax(0, 1fr); gap: 16px; }
		.settings-nav {
			position: static;
			flex-direction: row;
			gap: 6px;
			overflow-x: auto;
			padding-bottom: 4px;
		}
		.nav-item { flex: none; padding: 7px 14px; border-radius: 999px; background: var(--td-surface2); }
		/* The rail's subtle tint isn't enough to read as "selected" on a row of
		   pills, so the active one takes the accent outright. */
		.nav-item.on { background: var(--td-accent); color: #fbf5ea; }
		.nav-hint { display: none; }
	}

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
