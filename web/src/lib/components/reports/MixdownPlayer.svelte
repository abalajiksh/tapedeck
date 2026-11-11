<script lang="ts">
	/**
	 * A report chapter played as a mixdown, and saved as a video.
	 *
	 * **Nothing is laid out here.** The server draws every layer — fitted text,
	 * embedded-font metrics, the period's colour — and this component only
	 * moves them, by evaluating the tracks in `$lib/mixdown` on each animation
	 * frame. The MP4 is made by the server from the same mixdown with the same
	 * arithmetic, which is what keeps the two from drifting.
	 *
	 * The layers are written with `innerHTML` rather than `{@html}`: they go
	 * inside an `<svg>`, and setting `innerHTML` on an SVG element parses in
	 * the SVG namespace, which is the only way the markup becomes shapes
	 * rather than unknown HTML elements.
	 */
	import { onDestroy, tick } from 'svelte';
	import { api, humanError, type ReportMixdown, type MixdownVideo } from '$lib/api';
	import { COUNT_TOKEN, countAt, envelope, layerState, thousands } from '$lib/mixdown';

	export let unit: 'week' | 'month' | 'year';
	/** Periods back from the newest, as the Reports stepper counts. */
	export let offset = 0;
	export let label = '';
	export let open = false;

	type Theme = 'dark' | 'light';
	const THEMES: { k: Theme; label: string }[] = [
		{ k: 'dark', label: 'Dark' },
		{ k: 'light', label: 'Light' }
	];

	let theme: Theme = 'dark';
	let mixdown: ReportMixdown | null = null;
	let loading = false;
	let error = '';

	/** Which scene is on, and how far into it, in milliseconds. */
	let scene = 0;
	let elapsed = 0;
	let paused = false;
	let ended = false;

	let ground: SVGGElement;
	let stage: SVGGElement;
	let groups: SVGGElement[] = [];
	/** The last number written into each counting layer, to skip rewrites. */
	let shown: (number | null)[] = [];
	let raf = 0;
	let last = 0;

	let video: MixdownVideo | null = null;
	let videoError = '';
	let saving = false;
	let poll: ReturnType<typeof setTimeout> | null = null;

	$: if (open) void load(unit, offset, theme);
	$: if (!open) stop();

	async function load(u: typeof unit, o: number, th: Theme) {
		stop();
		loading = true;
		error = '';
		mixdown = null;
		try {
			const s = await api.getReportMixdown(u, o, th);
			// A slower answer for a look that is no longer selected is dropped.
			if (!open || u !== unit || o !== offset || th !== theme) return;
			mixdown = s;
			await tick();
			if (ground) ground.innerHTML = s.background;
			show(0);
			last = performance.now();
			raf = requestAnimationFrame(frame);
		} catch (e) {
			error = humanError(e, 'The Mixdown could not be loaded');
		} finally {
			loading = false;
		}
	}

	function stop() {
		if (raf) cancelAnimationFrame(raf);
		raf = 0;
	}

	/** Put a scene's layers on the stage, at its first moment. */
	function show(i: number) {
		if (!mixdown || !stage) return;
		scene = i;
		elapsed = 0;
		ended = false;
		stage.innerHTML = '';
		groups = [];
		shown = [];
		for (const layer of mixdown.scenes[i].layers) {
			const g = document.createElementNS('http://www.w3.org/2000/svg', 'g');
			if (!layer.count) g.innerHTML = layer.svg;
			stage.appendChild(g);
			groups.push(g);
			shown.push(null);
		}
		draw();
	}

	function draw() {
		if (!mixdown) return;
		const s = mixdown.scenes[scene];
		stage.setAttribute('opacity', String(envelope(s, elapsed)));
		s.layers.forEach((layer, k) => {
			const g = groups[k];
			if (!g) return;
			const st = layerState(layer, elapsed);
			g.setAttribute('opacity', String(st.opacity));
			if (st.transform) g.setAttribute('transform', st.transform);
			else g.removeAttribute('transform');
			if (layer.count) {
				const n = countAt(layer.count, elapsed);
				if (shown[k] !== n) {
					g.innerHTML = layer.svg.replace(COUNT_TOKEN, thousands(n));
					shown[k] = n;
				}
			}
		});
	}

	function frame(now: number) {
		raf = requestAnimationFrame(frame);
		const dt = Math.min(100, now - last);
		last = now;
		if (!mixdown || paused || ended) return;
		elapsed += dt;
		const s = mixdown.scenes[scene];
		if (elapsed >= s.duration_ms) {
			if (scene + 1 < mixdown.scenes.length) {
				show(scene + 1);
				return;
			}
			// The last scene is held, as it is in the video.
			elapsed = s.duration_ms;
			ended = true;
		}
		draw();
	}

	function go(by: number) {
		if (!mixdown) return;
		const next = scene + by;
		if (next < 0) show(0);
		else if (next < mixdown.scenes.length) show(next);
		else close();
	}

	function replay() {
		paused = false;
		show(0);
	}

	function onKey(e: KeyboardEvent) {
		if (!open) return;
		if (e.key === 'Escape') close();
		else if (e.key === 'ArrowRight') go(1);
		else if (e.key === 'ArrowLeft') go(-1);
		else if (e.key === ' ') {
			e.preventDefault();
			paused = !paused;
		}
	}

	/** How full each scene's bar is, 0 to 1. */
	function fill(i: number, sc: number, el: number): number {
		if (!mixdown || i > sc) return 0;
		if (i < sc) return 1;
		return Math.min(1, el / mixdown.scenes[i].duration_ms);
	}

	async function makeVideo() {
		videoError = '';
		try {
			video = await api.startMixdownVideo(unit, offset, theme);
			watch();
		} catch (e) {
			videoError = humanError(e, 'The video could not be started');
		}
	}

	function watch() {
		if (poll) clearTimeout(poll);
		poll = setTimeout(async () => {
			if (!video) return;
			try {
				video = await api.getMixdownVideo(video.id);
				if (video.state === 'done') await saveVideo();
				else if (video.state === 'failed') videoError = video.error ?? 'The video could not be made';
				else watch();
			} catch (e) {
				videoError = humanError(e, 'The video could not be made');
			}
		}, 1000);
	}

	async function saveVideo() {
		if (!video) return;
		saving = true;
		try {
			const blob = await api.getMixdownVideoFile(video.id);
			const url = URL.createObjectURL(blob);
			const a = document.createElement('a');
			a.href = url;
			a.download = `tapedeck-${label.toLowerCase().replace(/[^a-z0-9]+/g, '-')}-mixdown.mp4`;
			a.click();
			setTimeout(() => URL.revokeObjectURL(url), 60_000);
		} catch (e) {
			videoError = humanError(e, 'The video could not be saved');
		} finally {
			saving = false;
		}
	}

	function setTheme(t: Theme) {
		if (t === theme) return;
		theme = t;
		// A video being made is of the other look; its button goes back to
		// offering one of this look once it finishes.
	}

	function close() {
		open = false;
		paused = false;
	}

	$: busy = video != null && (video.state === 'queued' || video.state === 'rendering');
	$: pct =
		video && video.frames_total > 0
			? Math.round((video.frames_done / video.frames_total) * 100)
			: 0;

	onDestroy(() => {
		stop();
		if (poll) clearTimeout(poll);
	});
</script>

<svelte:window on:keydown={onKey} />

{#if open}
	<div class="dialog-backdrop" on:click={close} on:keydown={() => {}} role="presentation">
		<div
			class="dialog player"
			on:click|stopPropagation
			on:keydown|stopPropagation
			role="dialog"
			aria-modal="true"
			aria-label="{label} as a Mixdown"
			tabindex="-1"
		>
			<div class="top">
				<div class="dialog-title">{label}</div>
				<button class="btn btn-secondary x" on:click={close} aria-label="Close the Mixdown">
					<svg viewBox="0 0 24 24" width="15" fill="none" stroke="currentColor" stroke-width="2.6" stroke-linecap="round"><path d="M6 6l12 12M18 6L6 18"/></svg>
				</button>
			</div>

			<div class="frame">
				{#if mixdown}
					<div class="bars" aria-hidden="true">
						{#each mixdown.scenes as s, i (s.id)}
							<span class="bar"><span style="width:{fill(i, scene, elapsed) * 100}%"></span></span>
						{/each}
					</div>
				{/if}
				<svg
					class="screen"
					viewBox="0 0 {mixdown?.width ?? 1080} {mixdown?.height ?? 1920}"
					role="img"
					aria-label={mixdown ? `Scene ${scene + 1} of ${mixdown.scenes.length}` : 'Loading the Mixdown'}
				>
					<g bind:this={ground}></g>
					<g bind:this={stage}></g>
				</svg>
				{#if mixdown}
					<!-- Two halves over the picture, the way a mixdown is driven on a
					     phone. Real buttons, so a keyboard reaches them too. -->
					<button class="half left" on:click={() => go(-1)} aria-label="Previous scene"></button>
					<button class="half right" on:click={() => go(1)} aria-label="Next scene"></button>
				{:else if loading}
					<div class="wait"><div class="spin"></div></div>
				{/if}
			</div>

			{#if error}<p class="bad">{error}</p>{/if}

			<div class="controls">
				<div class="seg" aria-label="Look">
					{#each THEMES as t (t.k)}
						<button class="seg-btn" class:on={theme === t.k} on:click={() => setTheme(t.k)}>{t.label}</button>
					{/each}
				</div>
				{#if mixdown}
					{#if ended}
						<button class="btn btn-secondary" on:click={replay}>Play again</button>
					{:else}
						<button class="btn btn-secondary" on:click={() => (paused = !paused)}>
							{paused ? 'Play' : 'Pause'}
						</button>
					{/if}
					{#if mixdown.video_export}
						<button class="btn btn-primary" on:click={makeVideo} disabled={busy || saving}>
							{#if video?.state === 'queued'}
								Waiting for a turn…
							{:else if video?.state === 'rendering'}
								Drawing {pct}%
							{:else if saving}
								Saving…
							{:else}
								Save the video
							{/if}
						</button>
					{/if}
				{/if}
			</div>

			{#if mixdown && !mixdown.video_export}
				<p class="hint">
					This server can't make videos: it needs ffmpeg installed. The Mixdown plays here
					all the same.
				</p>
			{/if}
			{#if mixdown && !mixdown.fully_covered && mixdown.video_export}
				<p class="hint">
					Some characters here aren't in Tapedeck's own fonts, so in the video they may
					come out as boxes. On this screen your browser draws them.
				</p>
			{/if}
			{#if videoError}<p class="bad">{videoError}</p>{/if}
		</div>
	</div>
{/if}

<style>
	.player {
		width: min(430px, 100%);
		max-height: calc(100vh - 36px);
		gap: 12px;
	}
	.top {
		display: flex;
		align-items: center;
		justify-content: space-between;
		gap: 12px;
	}
	.x {
		padding: 8px 10px;
	}
	.frame {
		position: relative;
		border-radius: 18px;
		overflow: hidden;
		border: 1px solid var(--td-border);
		background: var(--td-surface2);
		/* The mixdown's own shape, as tall as the window allows. */
		aspect-ratio: 9 / 16;
		max-height: calc(100vh - 250px);
		margin: 0 auto;
	}
	.screen {
		display: block;
		width: 100%;
		height: 100%;
	}
	.bars {
		position: absolute;
		top: 10px;
		left: 10px;
		right: 10px;
		display: flex;
		gap: 4px;
		z-index: 2;
		pointer-events: none;
	}
	.bar {
		flex: 1;
		height: 3px;
		border-radius: 2px;
		background: color-mix(in srgb, var(--td-muted) 35%, transparent);
		overflow: hidden;
	}
	.bar span {
		display: block;
		height: 100%;
		background: var(--td-accent);
	}
	.half {
		position: absolute;
		top: 0;
		bottom: 0;
		width: 50%;
		border: none;
		background: none;
		cursor: pointer;
		padding: 0;
	}
	.half.left {
		left: 0;
	}
	.half.right {
		right: 0;
	}
	.half:focus-visible {
		outline: 2px solid var(--td-accent);
		outline-offset: -4px;
	}
	.wait {
		position: absolute;
		inset: 0;
		display: grid;
		place-items: center;
	}
	.spin {
		width: 22px;
		height: 22px;
		border: 2px solid var(--td-track);
		border-top-color: var(--td-accent);
		border-radius: 50%;
		animation: turn 0.8s linear infinite;
	}
	@keyframes turn {
		to {
			transform: rotate(360deg);
		}
	}
	.controls {
		display: flex;
		gap: 8px;
		align-items: center;
		justify-content: flex-end;
		flex-wrap: wrap;
	}
	/* Repeated per component, as ShareChapter and Reports repeat it: Svelte
	   scopes styles, and matching the existing control matters more than
	   deduplicating it. */
	.seg {
		display: inline-flex;
		gap: 2px;
		padding: 3px;
		border-radius: 999px;
		background: var(--td-surface2);
		border: 1px solid var(--td-border);
		margin-right: auto;
	}
	.seg-btn {
		border: none;
		background: none;
		cursor: pointer;
		padding: 6px 14px;
		border-radius: 999px;
		font: inherit;
		font-size: 13px;
		color: var(--td-muted);
	}
	.seg-btn.on {
		background: var(--td-accent);
		color: #fbf5ea;
	}
	.hint {
		font-size: 12px;
		color: var(--td-muted-2);
		margin: 0;
	}
	.bad {
		font-size: 12.5px;
		margin: 0;
		padding: 9px 12px;
		border-radius: 0 12px 12px 0;
		color: var(--td-text);
		background: var(--td-bad-bg);
		border-left: 3px solid var(--td-bad-fg);
	}
</style>
