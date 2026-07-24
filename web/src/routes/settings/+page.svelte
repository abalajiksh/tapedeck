<script lang="ts">
	import { onMount } from 'svelte';
	import { api } from '$lib/api';
	import type { TokenInfo } from '$lib/api';
	import { formatDateTime } from '$lib/utils';
	import { theme, setTheme, type ThemeMode } from '$lib/theme';
	import type { Connection } from '$lib/api';

	const uiVersion = __UI_VERSION__;
	let backendVersion = '…';
	let origin = typeof window !== 'undefined' ? window.location.origin : 'http://your-server:8080';

	let tokens: TokenInfo[] = [];
	let loading = true;
	let listenCount = 0;

	let newTokenName = '';
	let newTokenScopes = 'submit';
	let createdToken = '';
	let tokenCopied = false;
	let creating = false;

	let healthStatus = '';
	let checking = false;

	let confirmClear = false;
	let clearing = false;
	let exportMsg = '';

	// connections
	let connections: Connection[] = [];
	let lastfmAvailable = false;
	let lbToken = '';
	let lbBusy = false;
	let fmBusy = false;
	let fmPendingToken = ''; // set after 'start', cleared after 'complete'
	let connError = '';
	$: conn = (svc: string) => connections.find((c) => c.service === svc) ?? null;

	async function loadConnections() {
		try {
			const r = await api.getConnections();
			connections = r.connections;
			lastfmAvailable = r.lastfm_available;
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
	async function lastfmConnect() {
		connError = '';
		fmBusy = true;
		try {
			if (!fmPendingToken) {
				const { token, auth_url } = await api.lastfmStart();
				fmPendingToken = token;
				window.open(auth_url, '_blank', 'noopener');
			} else {
				await api.lastfmComplete(fmPendingToken);
				fmPendingToken = '';
				await loadConnections();
			}
		} catch (e) { connError = String(e).replace(/^API \d+: /, ''); }
		fmBusy = false;
	}
	async function disconnectSvc(svc: string) {
		try { await api.disconnect(svc); await loadConnections(); } catch (e) { connError = String(e).replace(/^API \d+: /, ''); }
		if (svc === 'lastfm') fmPendingToken = '';
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
		loadConnections();
		loading = false;
	});

	async function createToken() {
		if (!newTokenName.trim()) return;
		creating = true;
		createdToken = '';
		try {
			const result = await api.createToken(newTokenName.trim(), newTokenScopes);
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
</div>

<!-- Connections -->
<div class="panel" style="border-radius:22px;padding:24px;margin-bottom:18px;">
	<h3 class="h-display" style="font-size:19px;margin-bottom:4px;">Scrobble Connections</h3>
	<p style="font-size:13px;color:var(--td-muted);margin:0 0 16px;">Forward <em>your</em> listens to your own Last.fm and ListenBrainz. Until you connect here, listens use the server's default configuration.</p>

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

	<!-- Last.fm -->
	<div style="display:flex;align-items:center;gap:12px;flex-wrap:wrap;padding:12px 0;">
		<div style="flex:1;min-width:160px;">
			<div style="font-weight:600;font-size:14px;">Last.fm</div>
			{#if conn('lastfm')}
				<div style="font-size:12.5px;color:var(--td-good-fg);">Connected{conn('lastfm')?.username ? ` as ${conn('lastfm')?.username}` : ''}</div>
			{:else if !lastfmAvailable}
				<div style="font-size:12.5px;color:var(--td-muted-2);">Not available — the server needs LASTFM_API_KEY and LASTFM_SECRET set.</div>
			{:else if fmPendingToken}
				<div style="font-size:12.5px;color:var(--td-accent);">Approve Tapedeck in the Last.fm tab, then click “Finish”.</div>
			{:else}
				<div style="font-size:12.5px;color:var(--td-muted-2);">Click Connect, approve on Last.fm, then finish.</div>
			{/if}
		</div>
		{#if conn('lastfm')}
			<button class="btn btn-secondary" style="color:#b2622d;" on:click={() => disconnectSvc('lastfm')}>Disconnect</button>
		{:else if lastfmAvailable}
			<button class="btn btn-primary" on:click={lastfmConnect} disabled={fmBusy}>{fmBusy ? '…' : (fmPendingToken ? 'Finish connecting' : 'Connect Last.fm')}</button>
			{#if fmPendingToken}<button class="btn btn-secondary" on:click={() => (fmPendingToken = '')}>Cancel</button>{/if}
		{/if}
	</div>
</div>

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
			<select class="input" style="width:auto;" bind:value={newTokenScopes}>
				<option value="submit">submit</option>
				<option value="submit,read">submit + read</option>
				<option value="submit,read,admin">all</option>
			</select>
			<button class="btn btn-primary" on:click={createToken} disabled={creating || !newTokenName.trim()}>{creating ? '…' : 'Create'}</button>
		</div>
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
			<a href="https://github.com/abalajiksh/tapedeck" target="_blank" rel="noopener">View on GitHub →</a>
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
	.spin { animation: td-spin 0.7s linear infinite; }
	.seg-btn { border: none; background: transparent; cursor: pointer; color: var(--td-nav-fg); font-size: 13px; padding: 7px 14px; border-radius: 999px; font-family: inherit; }
	.seg-btn.on { background: var(--td-accent); color: #fbf5ea; }
	.chip { background: var(--td-sidebar); padding: 2px 6px; border-radius: 6px; font-size: 12px; }
</style>
