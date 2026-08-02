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
	// Per-edit review: every proposed change is its own row with its own
	// checkbox, because a majority vote of 4-of-29 is a guess and only the
	// person who did the listening can tell. Keyed by identity, not index, so a
	// refreshed preview doesn't silently re-point a tick at something else.
	let pickedAlbums = new Set<string>();
	let pickedCredits = new Set<string>();
	let pickedVariants = new Set<string>();
	/** Which release each album should unify onto — defaults to the plurality. */
	let albumChoice: Record<string, string> = {};
	const albumKey = (a: { artist: string; album: string }) => `${a.artist}\u0001${a.album}`;
	const variantKey = (from: string, to: string) => `${from}\u0001${to}`;

	/** Everything ticked, which is genuinely the common case. */
	function selectAll(on: boolean) {
		if (!sanitize) return;
		pickedAlbums = new Set(on ? sanitize.mixed_albums.map(albumKey) : []);
		pickedCredits = new Set(on ? sanitize.featured_artists.map((a) => a.artist) : []);
		pickedVariants = new Set(
			on
				? sanitize.artist_variants.flatMap((v) =>
						v.variants.map((x) => variantKey(x[0], v.primary))
					)
				: []
		);
	}

	$: pickedCount = pickedAlbums.size + pickedCredits.size + pickedVariants.size;

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
		try {
			sanitize = await api.sanitizePreview();
			// Default each album to the plurality; the operator overrules it per row.
			albumChoice = {};
			for (const a of sanitize.mixed_albums) {
				if (a.winner) albumChoice[albumKey(a)] = a.winner;
			}
			selectAll(true);
		} catch { sanitize = null; }
	}
	async function runSanitize() {
		if (!sanitize || !pickedCount) return;
		sanitizing = true;
		sanitizeMsg = '';
		try {
			const r = await api.sanitizeApply({
				album_edits: sanitize.mixed_albums
					.filter((a) => pickedAlbums.has(albumKey(a)) && albumChoice[albumKey(a)])
					.map((a) => ({
						artist: a.artist,
						album: a.album,
						mbid_release: albumChoice[albumKey(a)]
					})),
				credit_edits: sanitize.featured_artists
					.filter((a) => pickedCredits.has(a.artist))
					.map((a) => ({ artist: a.artist })),
				variant_edits: sanitize.artist_variants.flatMap((v) =>
					v.variants
						.filter((x) => pickedVariants.has(variantKey(x[0], v.primary)))
						.map((x) => ({ from: x[0], to: v.primary }))
				)
			});
			sanitizeMsg =
				`Fixed ${r.albums_fixed} album${r.albums_fixed === 1 ? '' : 's'} (${r.album_listens_changed} listens), ` +
				`folded ${r.artists_folded} featured credit${r.artists_folded === 1 ? '' : 's'} (${r.artist_listens_changed} listens) ` +
				`and merged ${r.variants_merged} spelling${r.variants_merged === 1 ? '' : 's'} (${r.variant_listens_changed} listens). ` +
				`Artwork will re-resolve in the background.`;
			await loadSanitize();
		} catch (e) {
			sanitizeMsg = 'Failed: ' + String(e).replace(/^API \d+: /, '');
		}
		sanitizing = false;
	}

	/** Set-in-a-let needs the reassignment to publish. */
	function toggle(set: Set<string>, key: string): Set<string> {
		const next = new Set(set);
		if (next.has(key)) next.delete(key);
		else next.add(key);
		return next;
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

	/** Lost the token? Only its hash is stored, so there is nothing to show —
	 *  re-issuing in place is the recovery path. The row keeps its chain. */
	async function resetToken(t: TokenInfo) {
		if (!confirm(`Re-issue "${t.name}"? The old token stops working immediately and you'll need to paste the new one into that client.`)) return;
		try {
			createdToken = (await api.rotateToken(t.id)).token;
			tokens = (await api.getTokens()).tokens;
		} catch (e) { alert('Reset failed: ' + e); }
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
									<button class="btn btn-secondary" style="font-size:12.5px;" on:click={() => resetToken(t)}>Reset</button>
									<button class="btn btn-secondary" style="font-size:12.5px;color:#b2622d;" on:click={() => revokeToken(t.id)}>Revoke</button>
								</div>
							{/each}
						</div>
						<p style="font-size:12px;color:var(--td-muted-2);margin:-6px 0 16px;">
							Changing a chain here takes effect immediately — the token itself doesn't change, so there's nothing to re-paste into the app.
							<strong>Reset</strong> is for a token you've lost: only its hash is stored, so it can't be shown again — it's re-issued instead, and the old one stops working.
							{#if !chains.length}<a href="/chains">Create a chain first →</a>{/if}
						</p>
					{/if}

					{#if createdToken}
						<div style="background:var(--td-panel);border-radius:16px;padding:16px;margin-bottom:16px;">
							<div style="font-size:10.5px;letter-spacing:.1em;text-transform:uppercase;color:#e0a06a;margin-bottom:8px;">Copy it now — it won't be shown again</div>
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
					{@const total = sanitize.mixed_albums.length + sanitize.featured_artists.length + sanitize.artist_variants.length}
					{#if total}
						<div class="san-bar">
							<span style="font-weight:600;font-size:14px;">{pickedCount} of {total} selected</span>
							<span style="flex:1;"></span>
							<button class="btn btn-ghost" style="font-size:12.5px;" on:click={() => selectAll(true)}>Select all</button>
							<button class="btn btn-ghost" style="font-size:12.5px;" on:click={() => selectAll(false)}>None</button>
						</div>

						{#if sanitize.mixed_albums.length}
							<div class="san-group">Albums on more than one release</div>
							{#each sanitize.mixed_albums as a (albumKey(a))}
								{@const key = albumKey(a)}
								{@const chosen = albumChoice[key]}
								{@const chosenPlays = a.candidates.find((c) => c.mbid === chosen)?.plays ?? 0}
								{@const weak = chosenPlays * 2 <= a.plays}
								<label class="san-row" class:on={pickedAlbums.has(key)}>
									<input type="checkbox" checked={pickedAlbums.has(key)}
										on:change={() => (pickedAlbums = toggle(pickedAlbums, key))}>
									<span style="flex:1;min-width:0;">
										<span class="san-name">{a.album}</span>
										<span class="san-sub">
											{a.artist} · {a.plays} plays across {a.variants} releases{#if a.untagged}, {a.untagged} with none{/if}
										</span>
										<span class="san-controls">
											<select class="input san-select" value={chosen ?? ''}
												on:change={(e) => (albumChoice = { ...albumChoice, [key]: e.currentTarget.value })}
												aria-label="Release to unify onto">
												{#each a.candidates as c}
													<option value={c.mbid}>{c.mbid.slice(0, 8)}… — {c.plays} play{c.plays === 1 ? '' : 's'}</option>
												{/each}
											</select>
											{#if weak}
												<span class="san-warn" title="No release has a clear majority — worth checking on MusicBrainz first">
													weak majority
												</span>
											{/if}
											{#if chosen}
												<a class="san-link" href={`https://musicbrainz.org/release/${chosen}`} target="_blank" rel="noopener">check →</a>
											{/if}
										</span>
									</span>
								</label>
							{/each}
						{/if}

						{#if sanitize.featured_artists.length}
							<div class="san-group">Featured credits splitting an artist</div>
							{#each sanitize.featured_artists as a (a.artist)}
								<label class="san-row" class:on={pickedCredits.has(a.artist)}>
									<input type="checkbox" checked={pickedCredits.has(a.artist)}
										on:change={() => (pickedCredits = toggle(pickedCredits, a.artist))}>
									<span style="flex:1;min-width:0;">
										<span class="san-name">{a.artist}</span>
										<span class="san-sub">→ {a.primary} · {a.plays} play{a.plays === 1 ? '' : 's'} · full credit is kept</span>
									</span>
								</label>
							{/each}
						{/if}

						{#if sanitize.artist_variants.length}
							<div class="san-group">One artist spelled several ways</div>
							{#each sanitize.artist_variants as v}
								{#each v.variants as x}
									{@const key = variantKey(x[0], v.primary)}
									<label class="san-row" class:on={pickedVariants.has(key)}>
										<input type="checkbox" checked={pickedVariants.has(key)}
											on:change={() => (pickedVariants = toggle(pickedVariants, key))}>
										<span style="flex:1;min-width:0;">
											<span class="san-name">{x[0]}</span>
											<span class="san-sub">
												→ {v.primary} · {x[1]} play{x[1] === 1 ? '' : 's'} moved to the spelling with {v.primary_plays}
												· not reversible
											</span>
										</span>
									</label>
								{/each}
							{/each}
						{/if}

						<button class="btn btn-primary" style="margin-top:16px;" on:click={runSanitize}
							disabled={sanitizing || !pickedCount}>
							{sanitizing ? 'Sanitising…' : `Apply ${pickedCount} change${pickedCount === 1 ? '' : 's'}`}
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
	.san-bar {
		display: flex; align-items: center; gap: 8px; flex-wrap: wrap;
		padding: 10px 0 12px; border-bottom: 1px solid var(--td-border);
	}
	.san-group {
		font-size: 11px; letter-spacing: .1em; text-transform: uppercase;
		color: var(--td-muted-3); margin: 16px 0 6px;
	}
	.san-row {
		display: flex; align-items: flex-start; gap: 10px;
		padding: 9px 10px; border-radius: 12px; cursor: pointer;
	}
	.san-row:hover { background: var(--td-nav-hover); }
	.san-row.on { background: color-mix(in srgb, var(--td-accent) 7%, transparent); }
	.san-row input { width: 16px; height: 16px; margin-top: 2px; accent-color: var(--td-accent); cursor: pointer; flex: none; }
	.san-name { display: block; font-size: 14px; font-weight: 600; overflow-wrap: anywhere; }
	.san-sub { display: block; font-size: 12.5px; color: var(--td-muted-2); margin-top: 2px; }
	.san-controls { display: flex; align-items: center; gap: 8px; flex-wrap: wrap; margin-top: 7px; }
	.san-select { width: auto; font-size: 12px; padding: 4px 10px; }
	/* Named rather than coloured-only: "the plurality is a minority" is the one
	   thing the operator most needs to notice before applying. */
	.san-warn {
		font-size: 11px; font-weight: 600; color: #b2622d;
		background: color-mix(in srgb, #b2622d 12%, transparent);
		padding: 3px 8px; border-radius: 999px; white-space: nowrap;
	}
	.san-link { font-size: 11.5px; }

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
