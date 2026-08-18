<script lang="ts">
	import { onMount } from 'svelte';
	import { api } from '$lib/api';
	import type { Scrobble } from '$lib/api';
	import ScrobbleRow from '$lib/components/ScrobbleRow.svelte';
	import { deck, watchNowPlaying } from '$lib/nowplaying';
	import { formatDuration, qualityLabel, tierTagClass, mediumLabel } from '$lib/utils';
	import { loadLoves, lovedKeys, isLovedNamed, toggleLoveByName } from '$lib/loves';
	import { openAlbum, openArtist } from '$lib/entities';
	import { fidelity, fidelityLabel } from '$lib/fidelity';

	let recent: Scrobble[] = [];
	let loading = true;
	let connected = false;
	let today = 0, thisWeek = 0, losslessPct = 0;
	// The fidelity card needs both halves: the share, and how much listening it
	// was computed from. Below a quarter coverage it reports the coverage
	// instead — see `$lib/fidelity`.
	let fidelityKnown = 0, totalPlays = 0;
	let todayHours = 0, weekHours = 0;

	// The deck shows what's playing when a source is reporting, and falls back
	// to the most recent listen otherwise.
	$: fid = fidelity(losslessPct, fidelityKnown, totalPlays, 'lossless');

	$: live = $deck.playing ? $deck.track : null;
	$: hero = live ?? recent[0] ?? null;
	$: heroQ = hero ? qualityLabel(hero) : null;
	// Same rule as the history row: an object gets named, a file gets its codec,
	// and the two are never both true.
	$: heroMedium = hero ? mediumLabel(hero) : null;
	$: progressPct = live?.duration ? Math.min(100, ($deck.position / live.duration) * 100) : 0;
	// A tape counter is just a position readout — four digits, like the real thing.
	$: counter = String(Math.floor($deck.position) % 10000).padStart(4, '0');

	// Whether the number under the bar is a measurement or our own arithmetic.
	// Plex, Jellyfin, Roon and the shelf report a real playhead; a scrobble
	// client submitting `playing_now` has no position in the format to send, and
	// Subsonic answers "minutes ago" — for those this is time since we were
	// told, and a pause or a seek makes it arbitrarily wrong. It is drawn as an
	// estimate rather than corrected, because there is nothing to correct it
	// with: the source genuinely does not know.
	$: estimated = !!live && live.position_known === false;

	// 14 bars, staggered so the meter doesn't pulse in unison.
	const vuBars = Array.from({ length: 14 }, (_, i) => ({
		duration: 0.55 + (i % 5) * 0.13,
		delay: i * 0.05
	}));

	// Purely cosmetic — digital audio has no sides. It's a nod to the format,
	// so it only changes the label and which way the reels turn.
	let side: 'A' | 'B' = 'A';
	function flip() {
		side = side === 'A' ? 'B' : 'A';
	}

	function playsLabel(plays: number, hours: number): string {
		return hours >= 0.1 ? `plays · ${hours} hrs` : 'plays';
	}

	// The deck updates itself every few seconds, so a stale list of recent
	// listens underneath it was the one part of this page you had to reload by
	// hand. Refreshed on a slow timer *and* the moment the playing track
	// changes — a change means the previous one has just been scrobbled, which
	// is exactly when the list below is wrong.
	const REFRESH_MS = 30_000;
	let refreshing = false;

	async function refreshRecent() {
		if (refreshing) return;
		refreshing = true;
		try {
			const [s, r] = await Promise.all([
				api.getDashboardStats(),
				api.getScrobbles({ limit: 8 })
			]);
			today = s.today; thisWeek = s.this_week; losslessPct = s.lossless_pct;
			fidelityKnown = s.fidelity_known; totalPlays = s.total;
			todayHours = s.today_hours; weekHours = s.week_hours;
			recent = r.scrobbles;
		} catch {
			// Leave what's on screen; the next tick tries again.
		}
		refreshing = false;
	}

	// `$deck.track` is replaced wholesale by the poll, so compare identity by
	// name rather than by reference or it fires on every poll.
	$: nowKey = $deck.track ? `${$deck.track.artist}|${$deck.track.title}` : '';

	// The heart on the deck shares `lovedKeys` with the history rows, so loving
	// what's playing fills the heart on its row too. `hero` may be a real
	// scrobble (the fallback) or the in-memory deck track, and the key is
	// derived from names either way — so both cases work without caring which.
	/** The template's `{#if hero.album}` doesn't narrow inside a click handler,
	 *  and a listen without an album is normal — guard here instead. */
	function openAlbumOf(artist: string, album: string | null | undefined): void {
		if (album) void openAlbum(artist, album);
	}

	$: heroLoved = hero ? isLovedNamed($lovedKeys, 'track', hero.artist, hero.title) : false;
	let lastKey: string | null = null;
	$: if (!loading && nowKey !== lastKey) {
		// The scrobble lands when the *previous* track ends, which the source
		// reports a moment before the new one shows up here.
		const first = lastKey === null;
		lastKey = nowKey;
		if (!first) setTimeout(refreshRecent, 1500);
	}

	onMount(() => {
		const stop = watchNowPlaying();
		loadLoves();

		(async () => {
			try {
				connected = (await api.health()).status === 'ok';
			} catch { connected = false; }

			try {
				const s = await api.getDashboardStats();
				today = s.today; thisWeek = s.this_week; losslessPct = s.lossless_pct;
			fidelityKnown = s.fidelity_known; totalPlays = s.total;
				todayHours = s.today_hours; weekHours = s.week_hours;
			} catch (e) { console.warn('stats', e); }

			try {
				recent = (await api.getScrobbles({ limit: 8 })).scrobbles;
			} catch (e) { console.warn('scrobbles', e); }

			loading = false;
		})();

		const timer = setInterval(refreshRecent, REFRESH_MS);
		return () => { clearInterval(timer); stop(); };
	});
</script>

<svelte:head><title>Dashboard — Tapedeck</title></svelte:head>

<div style="display:flex;align-items:flex-end;justify-content:space-between;gap:16px;margin-bottom:24px;">
	<div style="flex:1;min-width:0;">
		<h1 class="h-display" style="font-size:38px;">Now Rolling</h1>
		<p style="margin:6px 0 0;color:var(--td-muted);font-size:15px;">Your listening, one side at a time.</p>
	</div>
	<span style="display:inline-flex;align-items:center;gap:8px;font-size:12.5px;color:var(--td-good-fg);background:var(--td-good-bg);padding:7px 14px;border-radius:999px;">
		<span style="width:7px;height:7px;border-radius:50%;background:{connected ? '#7a8a5e' : '#b2622d'};"></span>{connected ? 'Connected' : 'Offline'}</span>
</div>

{#if loading}
	<div style="display:flex;justify-content:center;padding:80px;"><div class="spin" style="width:22px;height:22px;border:2px solid var(--td-track);border-top-color:var(--td-accent);border-radius:50%;"></div></div>
{:else}
	<!-- Cassette hero -->
	<div class="panel" style="border-radius:28px;box-shadow:0 10px 30px rgba(46,43,37,.09);padding:24px;margin-bottom:22px;display:flex;gap:28px;flex-wrap:wrap;align-items:center;">
		<div style="width:340px;max-width:100%;flex:none;">
			<svg viewBox="0 0 340 210" style="width:100%;height:auto;display:block;">
				<rect x="6" y="6" width="328" height="198" rx="22" fill="#2b2721"/>
				<rect x="6" y="6" width="328" height="198" rx="22" fill="none" stroke="#c67139" stroke-width="1.6" opacity=".55"/>
				<rect x="28" y="20" width="284" height="50" rx="10" fill="#f4e7cf"/>
				<line x1="40" y1="33" x2="300" y2="33" stroke="#c67139" stroke-width="3" opacity=".55"/>
				<line x1="40" y1="45" x2="240" y2="45" stroke="#7a8a5e" stroke-width="3" opacity=".45"/>
				<rect x="62" y="88" width="216" height="92" rx="12" fill="#1b1712"/>
				<rect x="120" y="120" width="100" height="28" rx="3" fill="#3a3128"/>
				{#each [120, 220] as cx}
					<g class="reel" class:rolling={$deck.playing} class:reverse={side === 'B'} style="transform-box:fill-box;transform-origin:center;">
						<circle {cx} cy="134" r="33" fill="none" stroke="#e9dcc4" stroke-width="3"/>
						<circle {cx} cy="134" r="22" fill="none" stroke="#7a6f5c" stroke-width="2" opacity=".6"/>
						<circle {cx} cy="134" r="8" fill="#e9dcc4"/>
						<line x1={cx - 30} y1="134" x2={cx + 30} y2="134" stroke="#e9dcc4" stroke-width="2.4"/>
						<line x1={cx - 15} y1="108" x2={cx + 15} y2="160" stroke="#e9dcc4" stroke-width="2.4"/>
						<line x1={cx - 15} y1="160" x2={cx + 15} y2="108" stroke="#e9dcc4" stroke-width="2.4"/>
					</g>
				{/each}
			</svg>
		</div>
		<div style="flex:1;min-width:260px;">
			{#if hero}
				<div style="display:flex;align-items:center;gap:8px;margin-bottom:8px;">
					<span style="width:8px;height:8px;border-radius:50%;background:{live ? '#e07a55' : 'var(--td-muted-3)'};{live ? 'animation:td-blink 1.6s ease-in-out infinite;' : ''}"></span>
					<span style="font-size:11px;letter-spacing:.16em;text-transform:uppercase;color:#c67139;font-weight:600;">{live ? `Now Playing · Side ${side}` : 'Last Played'}</span>
				</div>
				<div style="display:flex;align-items:flex-start;gap:12px;margin:0 0 4px;">
					<h2 class="h-display" style="font-size:28px;margin:0;flex:1;min-width:0;">{hero.title}</h2>
					<button
						class="deck-heart"
						class:loved={heroLoved}
						title={heroLoved ? 'Loved' : 'Love this track'}
						aria-label={heroLoved ? 'Remove love' : 'Love this track'}
						aria-pressed={heroLoved}
						on:click={() => hero && toggleLoveByName('track', hero.artist, hero.title)}
					>
						<svg viewBox="0 0 24 24" width="21" fill={heroLoved ? 'currentColor' : 'none'} stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M20.8 4.6a5.5 5.5 0 0 0-7.8 0L12 5.7l-1-1.1a5.5 5.5 0 0 0-7.8 7.8l1.1 1L12 21l7.7-7.6 1.1-1a5.5 5.5 0 0 0 0-7.8z"/></svg>
					</button>
				</div>
				<p style="margin:0 0 14px;color:var(--td-muted);font-size:15.5px;">
					<button class="deck-link" on:click={() => hero && openArtist(hero.artist)}>{hero.artist}</button>{#if hero.album} — <button class="deck-link" on:click={() => hero && openAlbumOf(hero.artist, hero.album)}>{hero.album}</button>{/if}
				</p>

				<div style="display:flex;align-items:center;gap:8px;flex-wrap:wrap;{live ? 'margin-bottom:16px;' : ''}">
					{#if heroMedium}<span class="tag tag-medium">{heroMedium}</span>
					{:else if heroQ?.text}<span class={tierTagClass(heroQ.tier)}>{heroQ.text}</span>{/if}
					{#if hero.listening_context && !heroMedium}<span class="tag tag-neutral">{hero.listening_context}</span>{/if}
					{#if live}
						<div style="display:flex;align-items:flex-end;gap:3px;height:34px;margin-left:4px;">
							{#each vuBars as bar}
								<span class="vu" style="animation-duration:{bar.duration}s;animation-delay:{bar.delay}s;"></span>
							{/each}
						</div>
					{/if}
				</div>

				{#if live}
					<div class="bar" class:est={estimated} style="height:8px;border-radius:999px;background:var(--td-track);overflow:hidden;margin-bottom:8px;">
						<div style="height:100%;width:{progressPct}%;border-radius:999px;background:linear-gradient(90deg,var(--td-accent-2),var(--td-accent-hi));transition:width .9s linear;"></div>
					</div>
					<div style="display:flex;align-items:center;justify-content:space-between;gap:12px;flex-wrap:wrap;">
						<div style="display:flex;align-items:center;gap:14px;flex-wrap:wrap;">
							<div
								style="font-variant-numeric:tabular-nums;font-size:13.5px;color:var(--td-muted);{estimated ? 'cursor:help;' : ''}"
								title={estimated
									? 'This player reports no playhead, so the position is time since the track was submitted — pausing or seeking makes it wrong.'
									: ''}
							>
								{estimated ? '≈ ' : ''}{formatDuration($deck.position)}{live.duration ? ` / ${formatDuration(live.duration)}` : ''}
							</div>
							<button class="btn btn-secondary" on:click={flip} title="Cosmetic — flips the side label" style="font-size:12.5px;padding:6px 14px;gap:7px;">
								<svg viewBox="0 0 24 24" width="15" fill="none" stroke="currentColor" stroke-width="2.6" stroke-linecap="round" stroke-linejoin="round"><path d="M3 12a9 9 0 0 1 15-6.7L21 8"/><path d="M21 12a9 9 0 0 1-15 6.7L3 16"/></svg>Flip · Side {side}
							</button>
						</div>
						<div style="display:flex;align-items:center;gap:8px;background:var(--td-panel2);color:#e9dcc4;padding:6px 12px;border-radius:9px;font-variant-numeric:tabular-nums;letter-spacing:.14em;font-size:15px;">
							<span style="font-size:9px;letter-spacing:.14em;color:#9a8c72;">TAPE</span>{counter}
						</div>
					</div>
				{/if}
			{:else}
				<h2 class="h-display" style="font-size:26px;margin:0 0 6px;">The deck is empty</h2>
				<p style="margin:0;color:var(--td-muted);font-size:15px;">Nothing recorded yet — <a href="/import">import a backup</a> or start scrobbling.</p>
			{/if}
		</div>
	</div>

	<!-- Quick stats -->
	<div style="display:grid;grid-template-columns:repeat(auto-fit,minmax(150px,1fr));gap:16px;margin-bottom:30px;">
		<div class="panel" style="border-radius:22px;padding:20px 22px;">
			<div style="font-size:11px;letter-spacing:.12em;text-transform:uppercase;color:var(--td-muted-2);margin-bottom:6px;">Today</div>
			<div class="h-display" style="font-size:36px;line-height:1;">{today}</div>
			<div style="font-size:12.5px;color:var(--td-muted-2);margin-top:6px;">{playsLabel(today, todayHours)}</div>
		</div>
		<div class="panel" style="border-radius:22px;padding:20px 22px;">
			<div style="font-size:11px;letter-spacing:.12em;text-transform:uppercase;color:var(--td-muted-2);margin-bottom:6px;">This Week</div>
			<div class="h-display" style="font-size:36px;line-height:1;">{thisWeek}</div>
			<div style="font-size:12.5px;color:var(--td-muted-2);margin-top:6px;">{playsLabel(thisWeek, weekHours)}</div>
		</div>
		<!-- Not "of all listens": most of an imported history has no format at
		     all, so dividing by everything reported the import ratio. -->
		<div style="background:var(--td-panel);color:#f4e7cf;border-radius:22px;padding:20px 22px;">
			<div style="font-size:11px;letter-spacing:.12em;text-transform:uppercase;color:#c9a678;margin-bottom:6px;">{fidelityLabel(fid, 'Lossless')}</div>
			<div class="h-display" style="font-size:36px;line-height:1;">{fid.value}</div>
			<div style="font-size:12.5px;color:#b3a487;margin-top:6px;line-height:1.35;">{fid.sub}</div>
		</div>
		<a class="charts-card" href="/charts">
			<svg viewBox="0 0 24 24" width="22" fill="none" stroke="currentColor" stroke-width="2.4" stroke-linecap="round" stroke-linejoin="round"><path d="M8 21h8"/><path d="M12 17v4"/><path d="M7 4h10v4a5 5 0 0 1-10 0z"/><path d="M17 5h3v2a3 3 0 0 1-3 3"/><path d="M7 5H4v2a3 3 0 0 0 3 3"/></svg>
			<span>
				<span class="charts-title">Top Charts</span>
				<span class="charts-sub">Your most played, ranked</span>
			</span>
		</a>
	</div>

	<!-- Recent -->
	<div style="display:flex;align-items:center;justify-content:space-between;margin-bottom:12px;">
		<h3 class="h-display" style="font-size:20px;">Recent Listens</h3>
		<a href="/history" style="font-size:13.5px;">View all →</a>
	</div>
	{#if recent.length}
		<div class="panel" style="border-radius:22px;overflow:hidden;">
			{#each recent as s (s.id)}<ScrobbleRow scrobble={s} />{/each}
		</div>
	{:else}
		<div class="panel" style="border-radius:22px;padding:44px;text-align:center;color:var(--td-muted);">No listens recorded yet.</div>
	{/if}
{/if}

<style>
	/* A bar drawn from a position nobody reported. Faded rather than hidden —
	   the track really is playing and the length really is known, so removing
	   it would say less than showing it hesitantly does. */
	.bar.est { opacity: .5; }
	.bar.est > :global(div) { background: var(--td-muted-3) !important; }

	/* Unlike the history row's heart, this one is always visible: the deck has
	   no hover affordance to reveal it and it's the primary action on the card. */
	.deck-heart {
		border: none;
		background: transparent;
		cursor: pointer;
		color: var(--td-muted-3);
		padding: 4px;
		border-radius: 8px;
		flex: none;
		display: flex;
		align-items: center;
		transition: color .15s, transform .15s;
	}
	.deck-heart:hover { color: #b2622d; transform: scale(1.08); }
	.deck-heart.loved { color: #c67139; }

	/* Anchors would be wrong here — the deck knows names, not entity ids, so
	   opening one is a resolve round-trip (see $lib/entities). Styled to read
	   as a link anyway, since that is what it behaves like. */
	.deck-link {
		border: none;
		background: none;
		padding: 0;
		font: inherit;
		color: inherit;
		cursor: pointer;
		border-bottom: 1px solid transparent;
	}
	.deck-link:hover,
	.deck-link:focus-visible { color: var(--td-fg); border-bottom-color: currentColor; }

	/* The reels only turn while something is actually playing — a still deck
	   is the honest signal that no source is reporting. */
	.reel.rolling {
		animation: td-spin 4s linear infinite;
	}
	.reel.rolling.reverse {
		animation-direction: reverse;
	}
	.spin {
		animation: td-spin 0.7s linear infinite;
	}
	/* The dashboard had no route to the charts at all; the redesign puts a
	   "view full charts" affordance here. */
	.charts-card {
		display: flex;
		align-items: center;
		gap: 14px;
		background: var(--td-surface);
		border: 1px solid var(--td-border);
		border-radius: 22px;
		padding: 20px 22px;
		text-decoration: none;
		color: inherit;
	}
	.charts-card:hover { border-color: var(--td-accent); color: inherit; }
	.charts-card svg { flex: none; color: var(--td-accent); }
	.charts-title { display: block; font-weight: 600; font-size: 15px; }
	.charts-sub { display: block; font-size: 12.5px; color: var(--td-muted-2); margin-top: 2px; }
	.vu {
		width: 4px;
		height: 34px;
		border-radius: 2px;
		background: linear-gradient(var(--td-accent-2), var(--td-accent));
		transform-origin: bottom;
		animation-name: td-vu;
		animation-timing-function: ease-in-out;
		animation-iteration-count: infinite;
		animation-direction: alternate;
	}
</style>
