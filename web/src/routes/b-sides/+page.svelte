<script lang="ts">
	/**
	 * B-Sides: the other ways of looking at your listening. Classical and
	 * Scores & Ambient have tabs of their own; the seasons come out as tabs
	 * while they are on and wait here the rest of the year.
	 *
	 * A lens takes nothing out of Top Charts. It only adds a view.
	 */
	import { onMount } from 'svelte';
	import { api, humanError, type LensList } from '$lib/api';

	let data: LensList | null = null;
	let error = '';

	const BLURB: Record<string, string> = {
		christmas: 'Your season, the day it opened, and every version of every carol.',
		halloween: 'October, and whatever you reach for when it gets dark early.',
		easter: 'Holy Week to Easter Monday.'
	};

	function when(starts: string, ends: string) {
		const f = (d: string) => new Date(`${d}T12:00:00`).toLocaleDateString(undefined, { day: 'numeric', month: 'long' });
		return `${f(starts)} – ${f(ends)}`;
	}
	function backOn(starts: string) {
		const d = new Date(`${starts}T12:00:00`);
		const next = new Date(d);
		next.setFullYear(d.getFullYear() + 1);
		const target = d.getTime() > Date.now() ? d : next;
		return target.toLocaleDateString(undefined, { day: 'numeric', month: 'long' });
	}

	onMount(async () => {
		try {
			data = await api.getLenses();
		} catch (e) {
			error = humanError(e, 'B-Sides could not be loaded');
		}
	});
</script>

<svelte:head><title>B-Sides — Tapedeck</title></svelte:head>

<div style="margin-bottom:22px;">
	<div class="eyebrow">B-Sides</div>
	<h1 class="h-display" style="font-size:38px;margin:0;">The other sides</h1>
	<p class="lede">Ways of looking at your listening that sit beside the charts. The seasons come out as their own tab while they're on.</p>
</div>

{#if error}
	<div class="panel empty">{error}</div>
{:else if !data}
	<div class="centre"><div class="spin"></div></div>
{:else}
	<div class="grid">
		<a class="card panel" href="/classical">
			<div class="kicker">Its own tab</div>
			<div class="h-display title">Classical</div>
			<p>Pieces, composers, and who played them.</p>
			<div class="count">{data.classical.toLocaleString()} listens</div>
		</a>
		<a class="card panel" href="/scores">
			<div class="kicker">Its own tab</div>
			<div class="h-display title">Scores & Ambient</div>
			<p>Film, game and TV score, and ambient records.</p>
			<div class="count">{(data.score + data.ambient).toLocaleString()} listens</div>
		</a>
		{#each data.lenses as l}
			<a class="card panel" class:live={l.in_season} href="/b-sides/{l.id}">
				<div class="kicker">{l.in_season ? 'In season now' : `Back on ${backOn(l.starts)}`}</div>
				<div class="h-display title">{l.label}</div>
				<p>{BLURB[l.id] ?? ''}</p>
				<div class="count">{l.listens.toLocaleString()} listens · {when(l.starts, l.ends)}</div>
			</a>
		{/each}
	</div>
	{#if data.pending}
		<p class="foot">Tapedeck is still sorting {data.pending.toLocaleString()} listens into these.</p>
	{/if}
{/if}

<style>
	.eyebrow { font-size: 11px; letter-spacing: .16em; text-transform: uppercase; color: var(--td-accent); font-weight: 600; margin-bottom: 8px; }
	.lede { margin: 6px 0 0; color: var(--td-muted); font-size: 15px; max-width: 640px; }
	.grid { display: grid; grid-template-columns: repeat(auto-fill, minmax(240px, 1fr)); gap: 16px; }
	.card { display: block; border-radius: 22px; padding: 20px; color: inherit; text-decoration: none; border: 1px solid transparent; }
	.card:hover { border-color: var(--td-accent); }
	.card.live { border-color: var(--td-accent); }
	.kicker { font-size: 11px; letter-spacing: .12em; text-transform: uppercase; color: var(--td-muted-2); font-weight: 600; }
	.card.live .kicker { color: var(--td-accent); }
	.title { font-size: 26px; margin: 6px 0 4px; }
	.card p { margin: 0 0 14px; color: var(--td-muted); font-size: 13.5px; }
	.count { font-size: 12.5px; color: var(--td-muted-2); font-variant-numeric: tabular-nums; }
	.foot { text-align: center; margin-top: 18px; font-size: 12.5px; color: var(--td-muted-3); }
	.empty { border-radius: 22px; padding: 44px; text-align: center; color: var(--td-muted); }
	.centre { display: flex; justify-content: center; padding: 70px; }
	.spin { width: 22px; height: 22px; border: 2px solid var(--td-track); border-top-color: var(--td-accent); border-radius: 50%; animation: td-spin 0.7s linear infinite; }
</style>
