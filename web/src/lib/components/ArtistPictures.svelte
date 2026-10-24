<script lang="ts">
	/**
	 * Choosing what an artist looks like — **to you**.
	 *
	 * Plex's *Edit → Poster / Background*, for a journal: every picture your
	 * own servers hold or can find, Deezer's, and anything you upload or link,
	 * each tile saying where it came from. The choice is yours alone — the
	 * shared picture the backfill found stays underneath as everyone's default,
	 * and is a tile here so going back to it is one click.
	 *
	 * Nothing is applied until **Save changes**, the way Plex works: clicking
	 * a tile, dropping a file and pasting a link all only *select*. That keeps
	 * an upload you then cancel from leaving a file on the server.
	 *
	 * Two tabs because they are two photographs, not two crops: the portrait is
	 * square and goes everywhere an artist is listed; the wide one fills a
	 * Reports chapter tile.
	 */
	import { createEventDispatcher } from 'svelte';
	import { api, humanError, type PictureCandidate, type PictureChoices, type PictureKind } from '$lib/api';
	import SourceIcon from './SourceIcon.svelte';

	export let open = false;
	export let artistRowId: number;
	export let artistName: string;
	export let tab: PictureKind = 'portrait';

	const dispatch = createEventDispatcher<{ saved: { portrait?: string | null; wide?: string | null } }>();

	type Pick =
		| { type: 'default' }
		| { type: 'current' }
		| { type: 'candidate'; c: PictureCandidate }
		| { type: 'file'; file: File; url: string }
		| { type: 'link'; url: string };

	let choices: Partial<Record<PictureKind, PictureChoices>> = {};
	let loadErr: Partial<Record<PictureKind, string>> = {};
	let loadingTab: PictureKind | null = null;
	/** What is selected and not yet saved, per tab. Absent means unchanged. */
	let pending: Partial<Record<PictureKind, Pick>> = {};
	let linkOpen = false;
	let linkText = '';
	let dragging = false;
	let saving = false;
	let saveErr = '';
	let fileInput: HTMLInputElement;
	let failed = new Set<string>();

	const LIBRARY = ['plex', 'jellyfin', 'emby', 'navidrome'];
	const TABS: { kind: PictureKind; label: string }[] = [
		{ kind: 'portrait', label: 'Portrait' },
		{ kind: 'wide', label: 'Wide' }
	];

	async function load(kind: PictureKind) {
		if (choices[kind] || loadingTab === kind) return;
		loadingTab = kind;
		loadErr = { ...loadErr, [kind]: '' };
		try {
			choices = { ...choices, [kind]: await api.getArtistPictures(artistRowId, kind) };
		} catch (e) {
			loadErr = { ...loadErr, [kind]: humanError(e) };
		}
		if (loadingTab === kind) loadingTab = null;
	}

	$: if (open) void load(tab);

	$: c = choices[tab];
	$: sel = pending[tab] ?? null;

	/** Is this tile the one that will be in force after saving? */
	function isOn(p: Pick, sel: Pick | null, c: PictureChoices | undefined): boolean {
		if (sel) {
			if (p.type !== sel.type) return false;
			if (p.type === 'candidate' && sel.type === 'candidate') return p.c.key === sel.c.key && p.c.source_ref === sel.c.source_ref;
			return true;
		}
		if (!c) return false;
		if (p.type === 'default') return !c.current;
		if (p.type === 'current') return !!c.current && !c.candidates.some((x) => x.chosen);
		if (p.type === 'candidate') return p.c.chosen;
		return false;
	}

	function select(p: Pick) {
		// Dropping the object URL of a file that is no longer selected, or it
		// stays alive for as long as the page does.
		const old = pending[tab];
		if (old?.type === 'file' && !(p.type === 'file' && p.url === old.url)) URL.revokeObjectURL(old.url);
		pending = { ...pending, [tab]: p };
		saveErr = '';
	}

	function takeFile(file: File | undefined | null) {
		if (!file) return;
		if (!file.type.startsWith('image/')) {
			saveErr = 'That is not an image.';
			return;
		}
		if (file.size > 8 * 1024 * 1024) {
			saveErr = 'That picture is larger than 8 MB.';
			return;
		}
		select({ type: 'file', file, url: URL.createObjectURL(file) });
	}

	function onPick(e: Event) {
		const input = e.target as HTMLInputElement;
		takeFile(input.files?.[0]);
		// Cleared so choosing the same file again still fires `change`.
		input.value = '';
	}

	function onDrop(e: DragEvent) {
		dragging = false;
		const file = e.dataTransfer?.files?.[0];
		if (file) return takeFile(file);
		// A picture dragged from another tab arrives as a URL, not a file.
		const url = e.dataTransfer?.getData('text/uri-list') || e.dataTransfer?.getData('text/plain');
		if (url && /^https?:\/\//.test(url.trim())) select({ type: 'link', url: url.trim() });
	}

	function useLink() {
		const url = linkText.trim();
		if (!/^https?:\/\//.test(url)) {
			saveErr = 'A link starts with http:// or https://.';
			return;
		}
		select({ type: 'link', url });
		linkText = '';
		linkOpen = false;
	}

	async function apply(kind: PictureKind, p: Pick): Promise<string | null | undefined> {
		switch (p.type) {
			case 'current':
				return undefined;
			case 'default':
				return (await api.clearArtistPicture(artistRowId, kind)).url;
			case 'file':
				return (await api.uploadArtistPicture(artistRowId, kind, p.file)).url;
			case 'link':
				return (await api.chooseArtistPicture(artistRowId, kind, { source: 'link', key: p.url })).url;
			case 'candidate':
				return (
					await api.chooseArtistPicture(artistRowId, kind, {
						source: p.c.source,
						source_ref: p.c.source_ref,
						key: p.c.key
					})
				).url;
		}
	}

	async function save() {
		saving = true;
		saveErr = '';
		const out: { portrait?: string | null; wide?: string | null } = {};
		try {
			for (const { kind } of TABS) {
				const p = pending[kind];
				if (!p) continue;
				const url = await apply(kind, p);
				if (url !== undefined) out[kind] = url;
				if (p.type === 'file') URL.revokeObjectURL(p.url);
				const rest = { ...pending };
				delete rest[kind];
				pending = rest;
				// What is chosen changed, so the next open asks again.
				const left = { ...choices };
				delete left[kind];
				choices = left;
			}
			dispatch('saved', out);
			close();
		} catch (e) {
			saveErr = humanError(e);
			// Whatever did save is already in force; say so to the page.
			if (Object.keys(out).length) dispatch('saved', out);
		}
		saving = false;
	}

	function close() {
		for (const p of Object.values(pending)) if (p?.type === 'file') URL.revokeObjectURL(p.url);
		pending = {};
		linkOpen = false;
		saveErr = '';
		open = false;
	}

	function onKey(e: KeyboardEvent) {
		if (open && e.key === 'Escape') close();
	}

	function sourceName(source: string, label: string | null): string {
		if (source === 'upload') return 'Your upload';
		if (source === 'link') return 'Your link';
		if (source === 'deezer') return 'Deezer';
		return label || source.charAt(0).toUpperCase() + source.slice(1);
	}

	function size(cand: PictureCandidate): string {
		return cand.width && cand.height ? `${cand.width}×${cand.height}` : '';
	}

	function broken(src: string) {
		failed.add(src);
		failed = failed;
	}

	$: dirty = Object.keys(pending).length > 0;
	// The current choice gets its own tile only when it is not one of the
	// candidates — an upload, a link, or a library picture the server no
	// longer lists.
	$: currentTile = c?.current && !c.candidates.some((x) => x.chosen) ? c.current : null;
</script>

<svelte:window on:keydown={onKey} />

{#if open}
	<div class="dialog-backdrop" on:click={close} on:keydown={() => {}} role="presentation">
		<div class="pics" on:click|stopPropagation on:keydown|stopPropagation role="dialog" aria-modal="true" tabindex="-1"
			aria-labelledby="pics-title">
			<header>
				<svg viewBox="0 0 24 24" width="20" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M12 20h9"/><path d="M16.5 3.5a2.1 2.1 0 0 1 3 3L7 19l-4 1 1-4z"/></svg>
				<h2 id="pics-title">{artistName}</h2>
				<button class="x" on:click={close} aria-label="Close">
					<svg viewBox="0 0 24 24" width="18" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round"><path d="M6 6l12 12M18 6L6 18"/></svg>
				</button>
			</header>

			<div class="body">
				<nav aria-label="Which picture">
					{#each TABS as t (t.kind)}
						<button class="tab" class:on={tab === t.kind} on:click={() => (tab = t.kind)}>
							{#if t.kind === 'portrait'}
								<svg viewBox="0 0 24 24" width="17" fill="none" stroke="currentColor" stroke-width="2" stroke-linejoin="round" aria-hidden="true"><rect x="4" y="4" width="16" height="16" rx="3"/><circle cx="12" cy="10" r="3"/><path d="M6.5 19a6 6 0 0 1 11 0"/></svg>
							{:else}
								<svg viewBox="0 0 24 24" width="17" fill="none" stroke="currentColor" stroke-width="2" stroke-linejoin="round" aria-hidden="true"><rect x="2.5" y="6" width="19" height="12" rx="2.5"/><path d="M2.5 15l5-4 4 3 3-2 7 4"/></svg>
							{/if}
							{t.label}
							{#if pending[t.kind]}<span class="dot" aria-label="unsaved"></span>{/if}
						</button>
					{/each}
					<p class="mine">Only you see your choice. Everyone else keeps theirs, or the shared picture.</p>
				</nav>

				<section
					class:dragging
					on:dragover|preventDefault={() => (dragging = true)}
					on:dragleave={() => (dragging = false)}
					on:drop|preventDefault={onDrop}
					aria-label="Pictures"
				>
					<div class="ways">
						<button class="link" on:click={() => fileInput.click()}>choose an image</button>
						<span aria-hidden="true">·</span>
						<span>drag and drop</span>
						<span aria-hidden="true">·</span>
						<button class="link" on:click={() => (linkOpen = !linkOpen)}>enter a url</button>
						<input bind:this={fileInput} type="file" accept="image/*" hidden on:change={onPick} />
					</div>
					{#if linkOpen}
						<form class="linkrow" on:submit|preventDefault={useLink}>
							<!-- svelte-ignore a11y-autofocus -->
							<input class="input" bind:value={linkText} placeholder="https://… an image" autofocus />
							<button class="btn btn-secondary" type="submit">Use it</button>
						</form>
					{/if}

					{#if loadingTab === tab && !c}
						<p class="note">Asking your servers…</p>
					{:else if loadErr[tab]}
						<p class="note bad">{loadErr[tab]}</p>
					{/if}

					{#if c || sel}
						<div class="grid" class:wide={tab === 'wide'}>
							{#if sel?.type === 'file' || sel?.type === 'link'}
								<button class="tile on" aria-pressed="true">
									<span class="frame"><img src={sel.url} alt="" /></span>
									<span class="cap"><strong>{sel.type === 'file' ? 'Your upload' : 'Your link'}</strong><em>not saved yet</em></span>
								</button>
							{/if}

							{#if c}
								<!-- The shared picture, so going back to it is a click. -->
								<button class="tile" class:on={isOn({ type: 'default' }, sel, c)}
									aria-pressed={isOn({ type: 'default' }, sel, c)}
									on:click={() => select({ type: 'default' })}>
									<span class="frame">
										{#if c.shared_url && !failed.has(c.shared_url)}
											<img src={c.shared_url} alt="" loading="lazy" on:error={() => c?.shared_url && broken(c.shared_url)} />
										{:else}
											<span class="blank">{artistName.charAt(0).toUpperCase()}</span>
										{/if}
									</span>
									<span class="cap"><strong>Shared default</strong><em>{c.shared_url ? 'what everyone sees' : 'nothing found yet'}</em></span>
								</button>

								{#if currentTile}
									<button class="tile" class:on={isOn({ type: 'current' }, sel, c)}
										aria-pressed={isOn({ type: 'current' }, sel, c)}
										on:click={() => select({ type: 'current' })}>
										<span class="frame">
											{#if !failed.has(currentTile.url)}
												<img src={currentTile.url} alt="" loading="lazy" on:error={() => currentTile && broken(currentTile.url)} />
											{:else}
												<span class="blank">?</span>
											{/if}
										</span>
										<span class="cap">
											<strong>
												{#if LIBRARY.includes(currentTile.source)}<SourceIcon source={currentTile.source} size={13} />{/if}
												{sourceName(currentTile.source, currentTile.source_label)}
											</strong>
											<em>your choice</em>
										</span>
									</button>
								{/if}

								{#each c.candidates as cand (`${cand.source_ref}|${cand.key}`)}
									{#if !failed.has(cand.preview)}
										<button class="tile" class:on={isOn({ type: 'candidate', c: cand }, sel, c)}
											aria-pressed={isOn({ type: 'candidate', c: cand }, sel, c)}
											on:click={() => select({ type: 'candidate', c: cand })}>
											<span class="frame"><img src={cand.preview} alt="" loading="lazy" on:error={() => broken(cand.preview)} /></span>
											<span class="cap">
												<strong>
													{#if LIBRARY.includes(cand.source)}<SourceIcon source={cand.source} size={13} />{/if}
													{cand.source_label}
												</strong>
												<em>
													{[cand.provider ? `via ${cand.provider}` : '', cand.server_selected && LIBRARY.includes(cand.source) ? `showing in ${cand.source_label}` : '', size(cand)].filter(Boolean).join(' · ')}
												</em>
											</span>
										</button>
									{/if}
								{/each}
							{/if}
						</div>

						{#if c && !c.candidates.length && loadingTab !== tab}
							<p class="note">
								None of your servers has a {tab === 'wide' ? 'wide picture' : 'picture'} of {artistName}{tab === 'portrait' ? ', and Deezer had nothing' : ''} — upload one or paste a link.
							</p>
						{/if}
						{#if c?.unreachable.length}
							<p class="note">Couldn't reach {c.unreachable.join(', ')} — {c.unreachable.length === 1 ? 'its' : 'their'} pictures aren't listed.</p>
						{/if}
					{/if}
				</section>
			</div>

			<footer>
				{#if saveErr}<span class="err">{saveErr}</span>{/if}
				<button class="btn btn-secondary" on:click={close}>Cancel</button>
				<button class="btn btn-primary" disabled={!dirty || saving} on:click={save}>
					{saving ? 'Saving…' : 'Save changes'}
				</button>
			</footer>
		</div>
	</div>
{/if}

<style>
	.pics {
		width: min(880px, 100%);
		max-height: min(720px, calc(100vh - 36px));
		display: flex;
		flex-direction: column;
		border-radius: 22px;
		overflow: hidden;
		background: var(--td-surface);
		color: var(--td-text);
		box-shadow: 0 12px 32px rgba(46, 43, 37, 0.22);
	}
	header {
		display: flex;
		align-items: center;
		gap: 12px;
		padding: 18px 22px;
		border-bottom: 1px solid var(--td-border);
		color: var(--td-muted);
	}
	h2 {
		margin: 0;
		flex: 1;
		min-width: 0;
		font-family: 'Caprasimo', serif;
		font-size: 22px;
		font-weight: 400;
		color: var(--td-text);
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}
	.x {
		display: grid;
		place-items: center;
		width: 32px;
		height: 32px;
		border: 0;
		border-radius: 999px;
		background: transparent;
		color: var(--td-muted);
		cursor: pointer;
	}
	.x:hover { background: var(--td-surface2); color: var(--td-text); }

	.body { display: flex; flex: 1; min-height: 0; }
	nav {
		width: 190px;
		flex: none;
		display: flex;
		flex-direction: column;
		gap: 4px;
		padding: 14px 10px;
		background: var(--td-surface2);
		border-right: 1px solid var(--td-border);
	}
	.tab {
		display: flex;
		align-items: center;
		gap: 10px;
		padding: 10px 12px;
		border: 0;
		border-radius: 12px;
		background: transparent;
		color: var(--td-muted);
		font: inherit;
		font-size: 14.5px;
		text-align: left;
		cursor: pointer;
	}
	.tab:hover { color: var(--td-text); }
	.tab.on { background: var(--td-surface); color: var(--td-accent); }
	.dot { width: 7px; height: 7px; margin-left: auto; border-radius: 999px; background: var(--td-accent); }
	.mine { margin: auto 4px 0; font-size: 12px; line-height: 1.45; color: var(--td-muted-2); text-wrap: pretty; }

	section {
		flex: 1;
		min-width: 0;
		overflow-y: auto;
		padding: 0 20px 20px;
		outline: 2px dashed transparent;
		outline-offset: -8px;
	}
	section.dragging { outline-color: var(--td-accent); }
	.ways {
		position: sticky;
		top: 0;
		z-index: 1;
		display: flex;
		flex-wrap: wrap;
		justify-content: center;
		gap: 8px;
		margin: 0 -20px 16px;
		padding: 14px 20px;
		background: var(--td-surface);
		border-bottom: 1px solid var(--td-border);
		font-size: 14.5px;
		color: var(--td-muted);
	}
	.link { border: 0; padding: 0; background: none; font: inherit; color: var(--td-accent); cursor: pointer; }
	.link:hover { text-decoration: underline; }
	.linkrow { display: flex; gap: 8px; margin-bottom: 16px; }
	.linkrow .input { flex: 1; min-width: 0; }

	.grid {
		display: grid;
		grid-template-columns: repeat(auto-fill, minmax(150px, 1fr));
		gap: 16px;
	}
	.grid.wide { grid-template-columns: repeat(auto-fill, minmax(230px, 1fr)); }
	.tile {
		display: flex;
		flex-direction: column;
		gap: 7px;
		padding: 0;
		border: 0;
		background: none;
		color: inherit;
		font: inherit;
		text-align: left;
		cursor: pointer;
	}
	.frame {
		position: relative;
		display: block;
		aspect-ratio: 1;
		border-radius: 12px;
		overflow: hidden;
		background: var(--td-panel2);
		outline: 3px solid transparent;
		outline-offset: 2px;
		transition: outline-color 0.12s;
	}
	.wide .frame { aspect-ratio: 16 / 9; }
	.tile:hover .frame { outline-color: var(--td-border); }
	.tile.on .frame { outline-color: var(--td-accent); }
	.tile.on .frame::after {
		content: '';
		position: absolute;
		top: 0;
		right: 0;
		width: 34px;
		height: 34px;
		background: var(--td-accent) url("data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 24 24' fill='none' stroke='white' stroke-width='3' stroke-linecap='round' stroke-linejoin='round'%3E%3Cpath d='M5 12l5 5 9-10'/%3E%3C/svg%3E") 60% 40% / 16px no-repeat;
		clip-path: polygon(0 0, 100% 0, 100% 100%);
	}
	.frame img { width: 100%; height: 100%; object-fit: cover; display: block; }
	.blank {
		display: grid;
		place-items: center;
		width: 100%;
		height: 100%;
		font-family: 'Caprasimo', serif;
		font-size: 44px;
		color: var(--td-muted-3);
	}
	.cap { display: flex; flex-direction: column; gap: 1px; min-width: 0; font-size: 12.5px; }
	.cap strong { display: flex; align-items: center; gap: 5px; font-weight: 600; color: var(--td-text); }
	.cap em { font-style: normal; color: var(--td-muted); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }

	.note { font-size: 13px; color: var(--td-muted); margin: 14px 0 0; text-wrap: pretty; }
	.note.bad { color: var(--td-bad-fg); }

	footer {
		display: flex;
		align-items: center;
		justify-content: flex-end;
		gap: 10px;
		padding: 14px 22px;
		border-top: 1px solid var(--td-border);
	}
	.err { margin-right: auto; font-size: 13px; color: var(--td-bad-fg); }

	@media (max-width: 640px) {
		.body { flex-direction: column; }
		nav { width: auto; flex-direction: row; flex-wrap: wrap; border-right: 0; border-bottom: 1px solid var(--td-border); padding: 8px; }
		.mine { flex-basis: 100%; margin: 4px 6px 2px; }
		.grid { grid-template-columns: repeat(auto-fill, minmax(120px, 1fr)); gap: 12px; }
		.grid.wide { grid-template-columns: 1fr; }
		header, footer { padding-left: 16px; padding-right: 16px; }
		section { padding: 0 16px 16px; }
		.ways { margin: 0 -16px 14px; }
	}
</style>
