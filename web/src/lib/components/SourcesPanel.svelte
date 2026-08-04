<script lang="ts">
	import { onMount } from 'svelte';
	import { api, type UserSource, type SourceTestResult } from '$lib/api';
	import { timeAgo } from '$lib/utils';

	/**
	 * Where this user's listens come *from*. Several servers of the same kind are
	 * allowed — two Plex boxes, a Navidrome and a Jellyfin — so everything here is
	 * keyed by source id, and each row carries a label to tell them apart.
	 *
	 * Credentials are never sent back by the API; a stored one shows as "stored"
	 * and leaving the field blank keeps it.
	 */

	type Kind = 'plex' | 'navidrome' | 'jellyfin';

	const KINDS: { v: Kind; label: string; blurb: string }[] = [
		{
			v: 'plex',
			label: 'Plex',
			blurb: 'Paste your own Plex token — Tapedeck works out which Plex account it belongs to and records only that account, so nobody else on the server ends up in your history.'
		},
		{
			v: 'navidrome',
			label: 'Navidrome',
			blurb: 'Signs in as you and backfills the play history Navidrome already keeps, then keeps up live. Your library’s MusicBrainz IDs come along, sparing those listens a lookup.'
		},
		{
			v: 'jellyfin',
			label: 'Jellyfin',
			blurb: 'Watches your sessions and records what you finish. Mint a key under Dashboard → API Keys. Jellyfin keeps no play history, so listens start from when you connect.'
		}
	];

	let sources: UserSource[] = [];
	let loading = true;
	let error = '';

	// The row being edited, or a blank draft when adding. `id: null` = new.
	type Draft = {
		id: number | null;
		kind: Kind;
		label: string;
		url: string;
		enabled: boolean;
		secret: string;
		username: string;
		hasSecret: boolean;
		/** Plex only. Which account on the server to scrobble; '' means the
		 *  token's own, which is the right default for a personal token. */
		plexAccount: string;
		filters: { libraries_allow: string; libraries_block: string; devices_allow: string; devices_block: string };
	};
	let draft: Draft | null = null;
	let advanced = false;
	let busy = false;
	let message = '';
	let test: SourceTestResult | null = null;
	let confirmDelete: UserSource | null = null;

	const blank = (kind: Kind): Draft => ({
		id: null, kind, label: '', url: '', enabled: true,
		secret: '', username: '', hasSecret: false, plexAccount: '',
		filters: { libraries_allow: '', libraries_block: '', devices_allow: '', devices_block: '' }
	});

	function edit(s: UserSource) {
		draft = {
			id: s.id,
			kind: s.kind as Kind,
			label: s.label,
			url: s.url,
			enabled: s.enabled,
			secret: '',
			username: s.username ?? '',
			hasSecret: s.has_token,
			// The stored account is the current choice; blank means "whoever the
			// token belongs to", which is what an un-picked source has always done.
			plexAccount: s.remote_account ?? '',
			filters: {
				libraries_allow: (s.libraries_allow ?? []).join(', '),
				libraries_block: (s.libraries_block ?? []).join(', '),
				devices_allow: (s.devices_allow ?? []).join(', '),
				devices_block: (s.devices_block ?? []).join(', ')
			}
		};
		advanced = false;
		test = null;
		message = '';
		error = '';
	}

	function add(kind: Kind) {
		draft = blank(kind);
		advanced = false;
		test = null;
		message = '';
		error = '';
	}

	const split = (s: string) => s.split(',').map((x) => x.trim()).filter(Boolean);

	/** Map the draft's single secret field onto whichever credential the kind uses. */
	function secretPayload(d: Draft) {
		if (!d.secret.trim()) return {};
		if (d.kind === 'plex') return { token: d.secret.trim() };
		if (d.kind === 'jellyfin') return { api_key: d.secret.trim() };
		return { password: d.secret };
	}

	async function load() {
		try {
			sources = (await api.getSources()).sources;
		} catch (e) {
			error = String(e).replace(/^API \d+: /, '');
		}
		loading = false;
	}

	async function save() {
		if (!draft) return;
		busy = true; error = ''; message = ''; test = null;
		const d = draft;
		const body = {
			label: d.label.trim(),
			enabled: d.enabled,
			url: d.url.trim(),
			...(d.kind === 'navidrome' ? { username: d.username.trim() } : {}),
			...(d.kind === 'plex' && d.plexAccount.trim() ? { plex_account: d.plexAccount.trim() } : {}),
			...secretPayload(d),
			libraries_allow: split(d.filters.libraries_allow),
			libraries_block: split(d.filters.libraries_block),
			devices_allow: split(d.filters.devices_allow),
			devices_block: split(d.filters.devices_block)
		};
		try {
			if (d.id == null) {
				await api.createSource({ kind: d.kind, ...body });
			} else {
				await api.updateSource(d.id, body);
			}
			await load();
			draft = null;
			message = 'Saved — polling picks this up within a few seconds.';
		} catch (e) {
			error = String(e).replace(/^API \d+: /, '');
		}
		busy = false;
	}

	async function runTest() {
		if (!draft) return;
		busy = true; error = ''; message = ''; test = null;
		const d = draft;
		try {
			test = await api.testSource({
				...(d.id == null ? { kind: d.kind } : { id: d.id }),
				url: d.url.trim(),
				...(d.kind === 'navidrome' ? { username: d.username.trim() } : {}),
				...secretPayload(d)
			});
		} catch (e) {
			error = String(e).replace(/^API \d+: /, '');
		}
		busy = false;
	}

	async function remove(s: UserSource) {
		busy = true; error = '';
		try {
			await api.deleteSource(s.id);
			await load();
			if (draft?.id === s.id) draft = null;
			message = 'Removed.';
		} catch (e) {
			error = String(e).replace(/^API \d+: /, '');
		}
		confirmDelete = null;
		busy = false;
	}

	const kindLabel = (k: string) => KINDS.find((x) => x.v === k)?.label ?? k;
	const secretLabel = (k: Kind) =>
		k === 'plex' ? 'X-Plex-Token' : k === 'jellyfin' ? 'API key' : 'Password';

	$: draftKind = draft?.kind;
	$: draftBlurb = KINDS.find((k) => k.v === draftKind)?.blurb ?? '';

	onMount(load);
</script>

<div class="panel" style="border-radius:22px;padding:24px;margin-bottom:18px;">
	<h3 class="h-display" style="font-size:19px;margin-bottom:4px;">Sources</h3>
	<p style="font-size:13px;color:var(--td-muted);margin:0 0 18px;">
		Where <em>your</em> listens come from. Each source is yours alone — connect as many servers as
		you like, including more than one of the same kind.
	</p>

	{#if error}<div style="font-size:13px;color:#b2622d;margin-bottom:12px;">{error}</div>{/if}
	{#if message}<div style="font-size:13px;color:var(--td-good-fg);margin-bottom:12px;">✓ {message}</div>{/if}

	{#if loading}
		<div style="display:flex;justify-content:center;padding:16px;"><div class="spin" style="width:18px;height:18px;border:2px solid var(--td-track);border-top-color:var(--td-accent);border-radius:50%;"></div></div>
	{:else}
		{#if sources.length}
			<div style="display:flex;flex-direction:column;gap:10px;margin-bottom:18px;">
				{#each sources as s (s.id)}
					<div style="display:flex;align-items:center;gap:12px;flex-wrap:wrap;padding:12px 14px;border:1px solid var(--td-border);border-radius:16px;background:var(--td-surface2);">
						<span style="width:8px;height:8px;border-radius:50%;flex:none;background:{s.last_error ? '#b2622d' : s.enabled ? '#7a8a5e' : 'var(--td-muted-3)'};"></span>
						<div style="flex:1;min-width:150px;">
							<div style="font-size:14px;font-weight:600;">
								{s.label || kindLabel(s.kind)}
								<span style="font-weight:400;color:var(--td-muted-3);font-size:12.5px;">· {kindLabel(s.kind)}</span>
							</div>
							<div style="font-size:12.5px;color:var(--td-muted-2);white-space:nowrap;overflow:hidden;text-overflow:ellipsis;">
								{s.url}{s.remote_account ? ` · ${s.remote_account}` : ''}
							</div>
						</div>
						{#if s.last_error}
							<span class="tag tag-outline" style="font-size:11px;color:#b2622d;">failing</span>
						{:else if !s.enabled}
							<span class="tag tag-neutral" style="font-size:11px;">paused</span>
						{:else if s.last_polled_at}
							<span style="font-size:12px;color:var(--td-muted-3);">{timeAgo(s.last_polled_at)}</span>
						{/if}
						<button class="btn btn-secondary" style="font-size:12.5px;padding:6px 14px;" on:click={() => edit(s)}>Edit</button>
						<button class="btn btn-secondary" style="font-size:12.5px;padding:6px 14px;color:#b2622d;" on:click={() => (confirmDelete = s)}>Remove</button>
					</div>
					{#if s.last_error}
						<p style="font-size:12px;color:#b2622d;margin:-4px 0 0 22px;font-family:monospace;word-break:break-word;">{s.last_error}</p>
					{/if}
				{/each}
			</div>
		{:else}
			<p style="color:var(--td-muted);font-size:14px;margin:0 0 18px;">
				No sources yet. Tapedeck still accepts listens pushed by scrobble clients — these are for
				pulling from a media server you run.
			</p>
		{/if}

		{#if !draft}
			<div style="display:flex;gap:10px;flex-wrap:wrap;">
				{#each KINDS as k}
					<button class="btn btn-secondary" on:click={() => add(k.v)}>+ Add {k.label}</button>
				{/each}
			</div>
		{/if}
	{/if}

	{#if draft}
		<div style="margin-top:18px;padding-top:18px;border-top:1px solid var(--td-border);">
			<h4 class="h-display" style="font-size:16px;margin:0 0 4px;">
				{draft.id == null ? `Add ${kindLabel(draft.kind)}` : `Edit ${draft.label || kindLabel(draft.kind)}`}
			</h4>
			<p style="font-size:12.5px;color:var(--td-muted);margin:0 0 16px;">{draftBlurb}</p>

			<label style="display:flex;align-items:center;gap:10px;cursor:pointer;font-size:14px;margin-bottom:14px;">
				<input type="checkbox" bind:checked={draft.enabled} style="width:16px;height:16px;accent-color:var(--td-accent);cursor:pointer;">
				Poll this server
			</label>

			<div style="display:flex;gap:12px;flex-wrap:wrap;">
				<div class="field" style="flex:1;min-width:160px;">
					<label for="src-label">Name <span style="opacity:.6">(optional)</span></label>
					<input id="src-label" class="input" bind:value={draft.label} placeholder="Living Room">
				</div>
				<div class="field" style="flex:2;min-width:220px;">
					<label for="src-url">Server URL</label>
					<input id="src-url" class="input" bind:value={draft.url}
						placeholder={draft.kind === 'navidrome' ? 'http://192.168.1.10:4533' : draft.kind === 'jellyfin' ? 'http://192.168.1.10:8096' : 'http://192.168.1.10:32400'}>
				</div>
			</div>

			<div style="display:flex;gap:12px;flex-wrap:wrap;margin-top:12px;">
				{#if draft.kind === 'navidrome'}
					<div class="field" style="flex:1;min-width:150px;">
						<label for="src-user">Username</label>
						<input id="src-user" class="input" bind:value={draft.username} placeholder="ashwin">
					</div>
				{/if}
				<div class="field" style="flex:1;min-width:180px;">
					<label for="src-secret">
						{secretLabel(draft.kind)}
						{#if draft.hasSecret}<span style="color:var(--td-good-fg);">· stored</span>{/if}
					</label>
					<input id="src-secret" class="input" type="password" bind:value={draft.secret}
						placeholder={draft.hasSecret ? 'Leave blank to keep' : `Your ${secretLabel(draft.kind)}`}>
				</div>
			</div>

			{#if draft.kind === 'navidrome'}
				<p style="font-size:12.5px;color:var(--td-muted-2);margin:10px 0 0;">
					Navidrome's Subsonic API hashes your password on every request, so unlike a token it has
					to be stored rather than exchanged. It's encrypted at rest with a key kept outside the
					database.
				</p>
			{/if}

			{#if draft.kind === 'plex'}
				<button class="btn btn-ghost" style="margin-top:12px;font-size:13px;" on:click={() => (advanced = !advanced)}>
					{advanced ? '− Hide' : '+ Show'} filters
				</button>
				{#if advanced}
					<p style="font-size:12.5px;color:var(--td-muted-2);margin:8px 0 12px;">
						Comma-separated. Empty allows everything; a block list always wins. Blocking Podcasts
						and Audiobooks is the usual reason to touch this.
					</p>
					<div style="display:grid;grid-template-columns:repeat(auto-fit,minmax(220px,1fr));gap:12px;">
						<div class="field"><label for="f-la">Libraries — allow</label><input id="f-la" class="input" bind:value={draft.filters.libraries_allow} placeholder="Music"></div>
						<div class="field"><label for="f-lb">Libraries — block</label><input id="f-lb" class="input" bind:value={draft.filters.libraries_block} placeholder="Podcasts, Audiobooks"></div>
						<div class="field"><label for="f-da">Devices — allow</label><input id="f-da" class="input" bind:value={draft.filters.devices_allow} placeholder="Living Room"></div>
						<div class="field"><label for="f-db">Devices — block</label><input id="f-db" class="input" bind:value={draft.filters.devices_block} placeholder="Chrome"></div>
					</div>
				{/if}
			{/if}

			<div style="display:flex;align-items:center;gap:12px;flex-wrap:wrap;margin-top:16px;">
				<button class="btn btn-primary" on:click={save} disabled={busy}>{busy ? '…' : 'Save'}</button>
				<button class="btn btn-secondary" on:click={runTest} disabled={busy}>Test connection</button>
				<button class="btn btn-secondary" on:click={() => (draft = null)} disabled={busy}>Cancel</button>
			</div>

			{#if test}
				<div style="margin-top:14px;font-size:13px;">
					{#if test.ok}
						<span style="color:var(--td-good-fg);">✓ Connected{test.account ? ` as ${test.account}` : ''}.</span>
						{#if test.libraries?.length}
							<span style="color:var(--td-muted);">Libraries: {test.libraries.join(', ')}</span>
						{/if}
						{#if test.account_error}
							<div style="color:#b2622d;margin-top:6px;">✗ Could not identify the account: {test.account_error}</div>
						{/if}
						{#if draft && draft.kind === 'plex' && test.accounts && test.accounts.length > 1}
							<div class="field" style="margin-top:12px;max-width:340px;">
								<label for="f-plexacct">Scrobble which Plex account?</label>
								<select id="f-plexacct" class="input" bind:value={draft.plexAccount}>
									<option value="">{test.account ? `${test.account} (this token)` : 'Whoever this token belongs to'}</option>
									{#each test.accounts as a (a.id)}
										<option value={a.name}>{a.name}</option>
									{/each}
								</select>
								<p style="color:var(--td-muted);font-size:12px;margin-top:6px;">
									Only this account's sessions are recorded. Pick a different one when
									the token is the server owner's but the listening isn't.
								</p>
							</div>
						{/if}
					{:else}
						<span style="color:#b2622d;">✗ {test.error}</span>
					{/if}
				</div>
			{/if}
		</div>
	{/if}
</div>

{#if confirmDelete}
	<div class="dialog-backdrop" on:click={() => (confirmDelete = null)} on:keydown={() => {}} role="presentation">
		<div class="dialog" on:click|stopPropagation on:keydown|stopPropagation role="presentation">
			<div class="dialog-title">Remove this source?</div>
			<div class="dialog-body" style="color:var(--td-nav-fg);">
				“{confirmDelete.label || kindLabel(confirmDelete.kind)}” will stop being polled and its
				credentials are deleted. Listens already recorded from it are kept.
			</div>
			<div class="dialog-actions">
				<button class="btn btn-secondary" on:click={() => (confirmDelete = null)}>Cancel</button>
				<button class="btn btn-primary" style="background:#b2622d;" on:click={() => confirmDelete && remove(confirmDelete)}>Remove</button>
			</div>
		</div>
	</div>
{/if}

<style>
	.spin { animation: td-spin 0.7s linear infinite; }
</style>
