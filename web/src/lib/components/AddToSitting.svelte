<script lang="ts">
	/**
	 * Adding somebody to a sitting that has already started.
	 *
	 * **One dialog, two callers**, and that is the point rather than tidiness.
	 * It is reached from the running banner on Patch — the moment somebody
	 * actually walks in — and from the sitting's own page afterwards. A second
	 * copy would be a second place for the consent guard and the "from when"
	 * question to drift, which is the objection that kept the shelf reporting a
	 * sitting rather than starting one (v0.110.0).
	 *
	 * Everything it decides is enforced server-side regardless: `start_spool`'s
	 * rule that you may only name a deck *you* are patched with lives in
	 * `db::add_spool_members`, and one stranger in the list refuses the whole
	 * call with nothing written. The picker having nobody else in it is the
	 * courtesy, not the guard.
	 */
	import { createEventDispatcher } from 'svelte';
	import { api, type PatchPeer, type SpoolMember } from '$lib/api';

	/** The sitting. `members` is only ever read to leave people out of the
	 *  picker, so the banner's `SpoolView` and the page's `SpoolDetail` both
	 *  serve — they carry the same roster. */
	export let uuid: string;
	export let members: SpoolMember[] = [];

	const dispatch = createEventDispatcher<{ added: { added: number }; close: void }>();

	let candidates: PatchPeer[] = [];
	let picked: Record<string, boolean> = {};
	let arrivedNow = false;
	let busy = false;
	let error = '';
	let loading = true;

	$: chosen = Object.keys(picked).filter((u) => picked[u]);

	async function load() {
		loading = true;
		try {
			const d = await api.getPatchDecks();
			const here = new Set(members.map((m) => m.username));
			candidates = [...d.following, ...d.followers].filter(
				(p, i, all) =>
					p.state === 'accepted' &&
					!here.has(p.username) &&
					all.findIndex((q) => q.username === p.username) === i
			);
		} catch (e) {
			error = e instanceof Error ? e.message : 'Could not read your decks';
		} finally {
			loading = false;
		}
	}
	void load();

	async function save() {
		if (!chosen.length) return;
		busy = true;
		error = '';
		try {
			// `present_from` absent means the whole evening. A real mid-evening
			// arrival says so — and "now" is the honest reading of that,
			// because the host is adding them as they walk in.
			const r = await api.addSpoolMembers(uuid, {
				with: chosen,
				present_from: arrivedNow ? Math.floor(Date.now() / 1000) : null
			});
			dispatch('added', r);
		} catch (e) {
			error = e instanceof Error ? e.message : 'Could not add them';
		} finally {
			busy = false;
		}
	}
</script>

<div class="dialog-backdrop" role="presentation" on:click={() => dispatch('close')} on:keydown={() => {}}>
	<div
		class="dialog"
		role="dialog"
		aria-modal="true"
		tabindex="-1"
		aria-label="Add somebody to this sitting"
		on:click|stopPropagation
		on:keydown|stopPropagation
	>
		<div class="dialog-title">Add somebody</div>
		<div class="dialog-body">
			Nothing reaches their deck until they say yes, and then only plays through
			chains their owner shares.
		</div>

		{#if error}<p class="hint bad">{error}</p>{/if}

		{#if loading}
			<p class="hint">Reading your decks…</p>
		{:else if candidates.length}
			<div class="picks">
				{#each candidates as c (c.username)}
					<label class="pick">
						<input type="checkbox" bind:checked={picked[c.username]} />
						<span>{c.display_name || c.username}</span>
					</label>
				{/each}
			</div>
			<!--
				The "from when" question, and the default is the common case: a host
				adding somebody an hour in has usually just remembered the person
				sitting in front of them. A segmented pair rather than a checkbox,
				because what this holds is a state (v0.83.0).

				`align-self` is load-bearing. `.dialog` is a global flex column, so a
				flex item is stretched to its width whatever its own `display` says —
				which blew the pair out to the full dialog with 200px of dead space
				inside its own border.
			-->
			<div class="seg">
				<button class="seg-btn" class:on={!arrivedNow} on:click={() => (arrivedNow = false)}>
					Here all along
				</button>
				<button class="seg-btn" class:on={arrivedNow} on:click={() => (arrivedNow = true)}>
					Just arrived
				</button>
			</div>
			<p class="hint">
				{arrivedNow
					? 'They will only be offered what plays from now on.'
					: 'They will be offered the whole evening.'}
			</p>
		{:else}
			<p class="hint">Everybody you are patched with is already in this sitting.</p>
		{/if}

		<div class="dialog-actions">
			<button class="btn btn-secondary" on:click={() => dispatch('close')}>Close</button>
			<button class="btn btn-primary" disabled={busy || !chosen.length} on:click={save}>
				{chosen.length ? `Add ${chosen.length}` : 'Add'}
			</button>
		</div>
	</div>
</div>

<style>
	/* Only what is local to this dialog. The surface, its backdrop, the title,
	   the body text and the action row are all global (`app.css`) — a local copy
	   of `.dialog` would not replace the global one, it would *collide* with it,
	   which is what left the segmented pair stretched across the whole card.

	   Svelte scopes styles per component, so anything not global has to be
	   declared here rather than inherited from whichever page opened the dialog —
	   the v0.42.0 trap, where an undeclared class renders as a bare div and
	   `svelte-check` stays clean either way. */
	.picks {
		display: flex;
		flex-direction: column;
		gap: 6px;
	}
	.pick {
		display: flex;
		align-items: center;
		gap: 8px;
		font-size: 14px;
	}
	.seg {
		align-self: flex-start;
		display: inline-flex;
		border: 1px solid var(--td-border);
		border-radius: 8px;
		overflow: hidden;
	}
	.seg-btn {
		background: none;
		border: none;
		padding: 6px 12px;
		font: inherit;
		font-size: 13px;
		color: var(--td-muted);
		cursor: pointer;
	}
	.seg-btn.on {
		background: var(--td-accent);
		color: #fff;
	}
	.hint {
		margin: 0;
		font-size: 12px;
		color: var(--td-muted);
	}
	.hint.bad {
		color: var(--td-bad-fg);
	}
</style>
