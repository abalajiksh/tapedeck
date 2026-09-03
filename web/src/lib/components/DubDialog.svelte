<script lang="ts">
	/**
	 * Passing a record to somebody — a **dub**.
	 *
	 * Its own component because the gesture belongs wherever you are looking at
	 * a record you want to pass on: an artist page, an album page, a Patch
	 * card. The endpoint has existed since v0.80.0 and **nothing called it**,
	 * so dubs could be received and never sent — the same shape as the Shared
	 * Spool's missing entry point, and the second time in this feature.
	 *
	 * Two of the server's three guards are surfaced here rather than left to
	 * come back as errors, because both are things the sender can act on:
	 * `because` is required, and a record already in their history is refused
	 * with the play count. The third — that you must be patched with them — is
	 * enforced by the picker having nobody else in it.
	 */
	import { api, humanError, type PatchPeer } from '$lib/api';
	import { createEventDispatcher } from 'svelte';

	export let open = false;
	export let kind: 'artist' | 'album' | 'track';
	export let artist: string;
	/** The album or track title. Absent for an artist dub. */
	export let name: string | null = null;

	const dispatch = createEventDispatcher();

	let peers: PatchPeer[] = [];
	let loaded = false;
	let to = '';
	let because = '';
	let busy = false;
	let error = '';
	let done = '';

	async function load() {
		loaded = true;
		try {
			const d = await api.getPatchDecks();
			const out = new Map<string, PatchPeer>();
			// Either direction, the rule `dub` itself enforces.
			for (const p of [...d.following, ...d.followers]) {
				if (p.state === 'accepted') out.set(p.username, p);
			}
			peers = [...out.values()].sort((a, b) =>
				(a.display_name || a.username).localeCompare(b.display_name || b.username)
			);
		} catch {
			/* the dialog says there is nobody to pass it to */
		}
	}

	$: if (open && !loaded) void load();

	async function send() {
		if (!to || !because.trim()) return;
		busy = true;
		error = '';
		try {
			await api.dub({ to, kind, artist, name, because: because.trim() });
			done = `Passed to ${peers.find((p) => p.username === to)?.display_name || to}.`;
			to = '';
			because = '';
			dispatch('sent');
		} catch (e) {
			// The already-heard refusal names the play count, which is the half
			// that tells the sender what to do differently — so it is shown, but
			// unwrapped: the client throws `API 409: {json}`, and putting that
			// envelope in front of a person is showing them our plumbing.
			error = humanError(e);
		}
		busy = false;
	}

	function close() {
		open = false;
		error = '';
		done = '';
	}
</script>

{#if open}
	<div class="dialog-backdrop" on:click={close} on:keydown={() => {}} role="presentation">
		<div class="dialog" on:click|stopPropagation on:keydown|stopPropagation role="presentation">
			<div class="dialog-title">Dub it to a deck</div>
			<div class="dialog-body">
				<p style="margin:0 0 6px;">
					<strong>{name || artist}</strong>
					{#if name}<span style="color:var(--td-muted);"> — {artist}</span>{/if}
				</p>
				<p class="sub">
					A record passed hand to hand. It lands in their Crate, with everything else they're
					still flipping through — <strong>not in their history</strong>.
				</p>

				{#if !peers.length}
					<p class="sub" style="margin:0;">
						You aren't patched with anyone yet. A dub travels along a patch, in either
						direction — <a href="/patch?tab=decks">find a deck</a>.
					</p>
				{:else}
					<div class="field">
						<label for="dub-to">Pass it to</label>
						<select id="dub-to" class="input" bind:value={to}>
							<option value="">— pick a deck —</option>
							{#each peers as p (p.username)}
								<option value={p.username}>{p.display_name || p.username}</option>
							{/each}
						</select>
					</div>
					<div class="field">
						<label for="dub-why">Why them</label>
						<input
							id="dub-why"
							class="input"
							bind:value={because}
							maxlength="300"
							placeholder="you wore out Laughing Stock — this is the same room, colder"
						/>
						<!--
							Required by the server, and said here rather than left to
							come back as a 400: a suggestion with no root in their
							listening is free association, which is the whole reason
							the column exists.
						-->
						<div class="hint">
							Required. Say what in <em>their</em> listening this comes from.
						</div>
					</div>
				{/if}

				{#if error}<div class="dubbad">{error}</div>{/if}
				{#if done}<div class="dubgood">{done}</div>{/if}
			</div>
			<div class="dialog-actions">
				<button class="btn btn-secondary" on:click={close}>{done ? 'Done' : 'Cancel'}</button>
				{#if peers.length && !done}
					<button class="btn btn-primary" disabled={busy || !to || !because.trim()} on:click={send}>
						{busy ? 'Passing…' : 'Dub it'}
					</button>
				{/if}
			</div>
		</div>
	</div>
{/if}

<style>
	.sub { font-size: 13px; color: var(--td-muted); margin: 0 0 14px; text-wrap: pretty; }
	.hint { font-size: 12px; color: var(--td-muted-2); margin-top: 5px; }
	.dubbad,
	.dubgood {
		font-size: 12.5px;
		margin-top: 12px;
		padding: 9px 12px;
		border-radius: 0 12px 12px 0;
		color: var(--td-text);
	}
	.dubbad {
		background: var(--td-bad-bg);
		border-left: 3px solid var(--td-bad-fg);
	}
	.dubgood {
		background: var(--td-good-bg);
		border-left: 3px solid var(--td-good-fg);
	}
</style>
