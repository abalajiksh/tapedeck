<script lang="ts">
	import { createEventDispatcher } from 'svelte';

	/** 1-based. */
	export let page: number;
	export let size: number;
	export let total: number;
	/** What is being counted, for the range label: "listens", "sessions". */
	export let noun = 'listens';
	export let busy = false;

	const dispatch = createEventDispatcher<{ go: number }>();
	$: pages = Math.max(1, Math.ceil(total / size));
	$: from = total ? (page - 1) * size + 1 : 0;
	$: to = Math.min(page * size, total);
	const nf = (n: number) => n.toLocaleString();
	function go(p: number) {
		p = Math.min(Math.max(1, p), pages);
		if (p !== page) dispatch('go', p);
	}
</script>

<nav class="pager" aria-label="Pages">
	<span class="range">{nf(from)}–{nf(to)} of {nf(total)} {noun}</span>
	<span style="flex:1;"></span>
	<button class="btn btn-secondary" on:click={() => go(1)} disabled={busy || page <= 1} aria-label="First page">«</button>
	<button class="btn btn-secondary" on:click={() => go(page - 1)} disabled={busy || page <= 1} aria-label="Newer page">‹ Newer</button>
	<span class="range">Page {nf(page)} of {nf(pages)}</span>
	<button class="btn btn-secondary" on:click={() => go(page + 1)} disabled={busy || page >= pages} aria-label="Older page">Older ›</button>
	<button class="btn btn-secondary" on:click={() => go(pages)} disabled={busy || page >= pages} aria-label="Last page">»</button>
</nav>

<style>
	.pager { display: flex; align-items: center; gap: 8px; flex-wrap: wrap; margin: 22px 0 0; }
	.range { font-size: 13px; color: var(--td-muted); }
</style>
