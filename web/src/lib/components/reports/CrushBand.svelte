<script lang="ts">
	/**
	 * "Artist crush of the month" — the accent band.
	 *
	 * The blurb is built from what the data says and nothing more. The design's
	 * copy ("the discovery that defined the month") is the kind of sentence that
	 * reads well once and wrong every time after, so this states the plays and
	 * the date it started and leaves the verdict to the reader.
	 */
	import { openArtist } from '$lib/entities';
	import { nf } from './bits';

	export let crush: { name: string; plays: number; since: number };
	export let unit: 'week' | 'month' | 'year';
	export let compact = false;

	$: since = new Date(crush.since * 1000).toLocaleDateString(undefined, {
		day: 'numeric',
		month: 'short'
	});
</script>

<div class="crush" class:compact>
	<div class="badge">
		<svg viewBox="0 0 24 24" width={compact ? 26 : 34} fill="currentColor" aria-hidden="true"><path d="M12 21s-7.5-4.6-9.8-9C.9 8.8 2.4 5.5 5.6 5.1c1.9-.2 3.5.9 4.4 2.3.9-1.4 2.5-2.5 4.4-2.3 3.2.4 4.7 3.7 3.4 6.9C19.5 16.4 12 21 12 21z"/></svg>
	</div>
	<div class="body">
		<div class="k">Artist crush of the {unit}</div>
		<button class="name h-display" on:click={() => openArtist(crush.name)}>{crush.name}</button>
		<p class="blurb">
			{nf(crush.plays)} plays since first hearing them on {since} — your most-played
			new artist this {unit}.
		</p>
	</div>
	<div class="count">
		<div class="h-display n">{nf(crush.plays)}</div>
		<div class="cl">plays</div>
	</div>
</div>

<style>
	.crush {
		position: relative; overflow: hidden; border-radius: 20px;
		background: linear-gradient(135deg, var(--td-accent), var(--td-accent-hi));
		padding: 22px 24px; margin-bottom: 26px;
		display: flex; align-items: center; gap: 20px; flex-wrap: wrap;
	}
	.crush.compact { padding: 18px 22px; gap: 18px; margin-bottom: 24px; }
	.badge {
		width: 72px; height: 72px; flex: none; border-radius: 50%;
		background: rgba(15, 11, 7, 0.22); color: #fff;
		display: flex; align-items: center; justify-content: center;
	}
	.compact .badge { width: 56px; height: 56px; }
	.body { flex: 1; min-width: 200px; color: #fff; }
	.k {
		font-size: 11px; letter-spacing: 0.16em; text-transform: uppercase;
		color: rgba(255, 255, 255, 0.8); margin-bottom: 6px;
	}
	.name {
		font-size: 30px; line-height: 1; color: #fff;
		background: none; border: none; padding: 0; cursor: pointer; text-align: left;
	}
	.compact .name { font-size: 26px; }
	.name:hover { text-decoration: underline; text-underline-offset: 4px; }
	.blurb {
		font-size: 13.5px; color: rgba(255, 255, 255, 0.9);
		margin: 8px 0 0; max-width: 52ch;
	}
	.count { color: #fff; text-align: right; }
	.n { font-size: 40px; line-height: 1; }
	.compact .n { font-size: 34px; }
	.cl { font-size: 12px; color: rgba(255, 255, 255, 0.8); }
</style>
