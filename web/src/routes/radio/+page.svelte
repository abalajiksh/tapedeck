<script lang="ts">
	/**
	 * Radio — artboard **1a, the tuner faceplate**, of `temp/Radio.dc.html`.
	 *
	 * Pick a preset and the server tunes in: it reads the station's own stream
	 * for track names and logs them under the rules in the right rail. The
	 * model, and what does and does not become a listen, is `src/radio/mod.rs`.
	 *
	 * Where this departs from the drawing, and why:
	 *
	 * * **Track names come from the station's stream (ICY)**, not RT+ or DL+
	 *   off the air — there is no SDR feeder yet. So the source badge reads
	 *   `ICY`, the bars show the stream's health rather than RF signal, and a
	 *   news break is recognised from its words rather than a PTY flag.
	 * * **"Tagged" means MusicBrainz recognised the track**, since ICY carries no
	 *   tags. An unrecognised name is a guess and goes to review.
	 * * **The dial, frequency and city are yours to type.** RadioDNS names no
	 *   frequency (ARD publishes FM as `fm:de0.d3c3.*`), no DAB channel and no
	 *   city. A preset without a dial position still tunes; it just sits off
	 *   the dial.
	 * * **Browse is by genre or broadcaster region**, not city — the region is
	 *   the one place fact the station list carries.
	 * * The drawing has no way to add, edit or remove a preset, or to turn the
	 *   tuner off; those are added in the faceplate's own style.
	 *
	 * The faceplate is dark in both themes, like the lyric reader's stage, so
	 * its colours are literal (see the note on `.faceplate`). Everything else
	 * follows the theme.
	 */
	import { onDestroy, onMount } from 'svelte';
	import { api, humanError } from '$lib/api';
	import type {
		AirLogEntry,
		RadioPreset,
		RadioSchedule,
		RadioSession,
		RadioSettings,
		RadioStation,
		SignalChain,
		TunerStatus
	} from '$lib/api';

	let presets: RadioPreset[] = [];
	let stations: RadioStation[] = [];
	let regions: string[] = [];
	let genres: string[] = [];
	let dirNote = '';
	let tuner: TunerStatus | null = null;
	let settings: RadioSettings | null = null;
	let chains: SignalChain[] = [];
	let log: AirLogEntry[] = [];
	let review: AirLogEntry[] = [];
	let recent: RadioSession[] = [];
	let schedule: RadioSchedule | null = null;
	let scheduleFor: number | null = null;
	let selId: number | null = null;
	let loading = true;
	let busy = false;
	let error = '';

	// Browse
	let browse: 'genre' | 'region' = 'genre';
	let chip: string | null = null;
	let query = '';
	let adding: { station: RadioStation; band: 'fm' | 'dab'; dial: string; city: string } | null = null;

	// Preset editor
	let editing = false;
	let editDial = '';
	let editCity = '';

	let now = Date.now() / 1000;
	let clock: ReturnType<typeof setInterval> | undefined;
	let poll: ReturnType<typeof setInterval> | undefined;
	let ticks = 0;

	onMount(async () => {
		await Promise.all([loadPresets(), loadTuner(), loadLog(), loadRecent(), loadSettings(), loadChains()]);
		loading = false;
		// The list is a day-cached fetch of nine broadcasters on the server; it
		// can take a few seconds the first time, so the page does not wait on it.
		void loadStations();
		clock = setInterval(() => (now = Date.now() / 1000), 1000);
		poll = setInterval(async () => {
			ticks++;
			if (document.hidden) return;
			await loadTuner();
			if (ticks % 2 === 0) await loadLog();
		}, 5000);
	});
	onDestroy(() => {
		clearInterval(clock);
		clearInterval(poll);
	});

	async function loadPresets() {
		try {
			presets = (await api.getRadioPresets()).presets;
		} catch (e) {
			error = humanError(e, 'Could not load your presets');
		}
	}
	async function loadStations() {
		try {
			const r = await api.getRadioStations();
			stations = r.stations;
			regions = r.regions;
			genres = r.genres;
			dirNote = r.note;
		} catch (e) {
			error = humanError(e, 'Could not load the station list');
		}
	}
	async function loadTuner() {
		try {
			const prev = tuner;
			tuner = await api.getTuner();
			// A track ending or the tuner stopping by itself changes the log and
			// the recent list; fetch them now rather than on the next slow tick.
			const changed =
				prev?.tuned !== tuner.tuned ||
				prev?.now?.since !== tuner.now?.since ||
				prev?.state !== tuner.state;
			if (prev && changed) {
				await loadLog();
				if (!tuner.tuned) await loadRecent();
			}
		} catch {
			/* the next tick tries again */
		}
	}
	async function loadLog() {
		try {
			const r = await api.getAirLog(50);
			log = r.entries;
			review = r.review;
		} catch {
			/* the next tick tries again */
		}
	}
	async function loadRecent() {
		try {
			recent = (await api.getRecentRadio()).recent;
		} catch {
			/* decoration */
		}
	}
	async function loadSettings() {
		try {
			settings = await api.getRadioSettings();
		} catch (e) {
			error = humanError(e, 'Could not load your radio rules');
		}
	}
	async function loadChains() {
		try {
			chains = (await api.getChains()).chains;
		} catch {
			/* the chain picker is optional */
		}
	}
	async function loadSchedule(p: RadioPreset) {
		scheduleFor = p.id;
		schedule = null;
		try {
			const s = await api.getRadioSchedule(p.id);
			if (scheduleFor === p.id) schedule = s;
		} catch {
			if (scheduleFor === p.id) schedule = null;
		}
	}

	// ── What the faceplate shows ──

	$: tuned = !!tuner?.tuned;
	$: sel = (tuned && tuner?.preset ? tuner.preset : presets.find((p) => p.id === selId) ?? presets[0]) ?? null;
	$: if (sel && sel.id !== scheduleFor) void loadSchedule(sel);
	$: station = sel ? stations.find((s) => s.id === sel?.station_id) ?? null : null;
	$: isDab = sel?.band === 'dab';

	const bandLabel = (b: string | null | undefined) => (b === 'dab' ? 'DAB+' : 'FM');
	const hhmm = (ts: number | null | undefined) =>
		ts ? new Date(ts * 1000).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' }) : '';
	const pad = (n: number) => String(Math.max(0, Math.floor(n))).padStart(2, '0');

	type Tone = 'good' | 'warn' | 'neutral' | 'bad';
	type Display = { title: string; sub: string; status: string; tone: Tone; src: string };

	function display(t: TunerStatus | null, s: RadioSettings | null, p: RadioPreset | null, n: number): Display {
		if (!t?.tuned) {
			return p
				? { title: 'Off', sub: `Press a preset to tune in to ${p.name}`, status: 'Off', tone: 'neutral', src: '' }
				: { title: 'No presets yet', sub: 'Add a station from Browse', status: 'Off', tone: 'neutral', src: '' };
		}
		const name = t.preset?.name ?? 'the station';
		switch (t.state) {
			case 'live': {
				const trusted = t.now?.tagged || !s?.tagged_only;
				return {
					title: t.now?.title ?? '',
					sub: t.now?.artist ?? '',
					status: trusted ? (s?.tag_radio ? 'Scrobbling · radio' : 'Scrobbling') : 'Listening · to review',
					tone: trusted ? 'good' : 'warn',
					src: t.source ?? 'ICY'
				};
			}
			case 'unknown':
				return {
					title: 'No track tags',
					sub: t.line ? `“${t.line}”` : `${name} is talking`,
					status: 'Listening, not logging',
					tone: 'warn',
					src: t.source ?? ''
				};
			case 'break':
				return {
					title: 'Break',
					sub: t.line ? `“${t.line}” · paused until music returns` : 'Paused until music returns',
					status: s?.skip_breaks ? 'Paused · break' : 'Logging break',
					tone: 'neutral',
					src: t.source ?? ''
				};
			case 'dropped': {
				const left = t.retry_at ? t.retry_at - n : 0;
				const last = t.now ? `Last: ${t.now.artist} — ${t.now.title} · ` : '';
				return {
					title: 'Signal lost',
					sub: `${last}retrying in ${pad(left / 60)}:${pad(left % 60)}`,
					status: 'Dropped',
					tone: 'bad',
					src: ''
				};
			}
			default:
				return {
					title: 'Tuning…',
					sub: `Opening ${name}'s stream, waiting for a track name`,
					status: 'Connecting',
					tone: 'neutral',
					src: ''
				};
		}
	}
	$: disp = display(tuner, settings, sel, now);

	$: code = sel?.station_id.split('.')[1]?.toUpperCase() ?? '';
	$: idLine = sel ? `${isDab ? 'SId' : 'PI'} ${code} · ${station?.provider ?? ''}` : '';
	$: dataLine = station
		? station.stream
			? `ICY · ${station.stream.bitrate ? station.stream.bitrate + ' kbit/s ' : ''}stream`
			: 'No stream · cannot log'
		: '';

	// ── The dial ──

	const DAB: string[] = (() => {
		const o: string[] = [];
		for (let b = 5; b <= 13; b++) for (const l of b === 13 ? 'ABCDEF' : 'ABCD') o.push(b + l);
		return o;
	})();
	function dialPos(band: string, dial: string | null): number | null {
		if (!dial) return null;
		if (band === 'fm') {
			const f = parseFloat(dial);
			return Number.isFinite(f) ? ((f - 87.5) / 20.5) * 100 : null;
		}
		const i = DAB.indexOf(dial.toUpperCase());
		return i < 0 ? null : ((i + 0.5) / DAB.length) * 100;
	}
	$: dialTicks = isDab
		? DAB.map((c, i) => ({ left: ((i + 0.5) / DAB.length) * 100, major: c.endsWith('A'), label: c.endsWith('A') ? c.slice(0, -1) : '' }))
		: Array.from({ length: 41 }, (_, i) => 88 + i * 0.5).map((f) => ({
				left: ((f - 87.5) / 20.5) * 100,
				major: f % 2 === 0,
				label: f % 4 === 0 ? String(f) : ''
			}));
	$: markers = presets
		.map((p, i) => ({ p, n: i + 1, left: p.band === (isDab ? 'dab' : 'fm') ? dialPos(p.band, p.dial) : null }))
		.filter((m) => m.left !== null);
	$: needle = sel ? dialPos(sel.band, sel.dial) : null;

	// ── Tonight ──

	$: epg = (() => {
		const progs = schedule?.programmes ?? [];
		if (!progs.length) return [];
		const end = (p: (typeof progs)[number]) => p.start + (p.duration ?? 0);
		let i = progs.findIndex((p) => p.start <= now && now < end(p));
		if (i < 0) i = Math.max(0, progs.findIndex((p) => p.start > now) - 1);
		const from = Math.max(0, i - 1);
		return progs.slice(from, from + 4).map((p, k) => {
			const onNow = p.start <= now && now < end(p);
			return {
				t: hhmm(p.start),
				n: p.names.long ?? p.names.medium ?? p.names.short ?? 'Untitled',
				tag: onNow ? 'On now' : end(p) <= now ? 'Earlier' : '',
				onNow,
				earlier: end(p) <= now,
				key: `${p.start}-${k}`
			};
		});
	})();
	$: epgTitle = new Date(now * 1000).getHours() >= 18 ? 'Tonight' : 'Today';
	$: epgNote = !sel
		? ''
		: schedule?.status === 'not_published'
			? `${sel.name} publishes names and logos but no schedule.`
			: schedule?.status === 'unavailable'
				? 'The schedule could not be fetched just now.'
				: isDab
					? 'DAB carries this guide on air too; Tapedeck reads the same data from RadioDNS online.'
					: 'FM carries no programme guide. This schedule comes from RadioDNS online.';

	// ── Air log ──

	const STATUS: Record<string, [string, 'good' | 'warn' | 'mute']> = {
		playing: ['On air', 'good'],
		scrobbled: ['Scrobbled', 'good'],
		kept: ['Kept · scrobbled', 'good'],
		review: ['In review', 'warn'],
		held: ['Logged, held', 'mute'],
		dropped: ['Dropped', 'mute'],
		partial: ['Partly heard', 'mute'],
		skipped: ['Skipped', 'mute'],
		logged: ['Logged', 'mute']
	};
	const statusOf = (s: string) => STATUS[s] ?? [s, 'mute'];
	const lineTitle = (e: AirLogEntry) => e.title ?? (e.kind === 'break' ? 'Break' : 'Station text');
	const lineSub = (e: AirLogEntry) => e.artist ?? `“${e.raw}”`;
	$: scrobbledCount = log.filter((e) => e.status === 'scrobbled' || e.status === 'kept').length;

	// ── Rules ──

	type RuleKey = 'skip_breaks' | 'tagged_only' | 'review' | 'tag_radio' | 'auto_stop' | 'attach_chain';
	$: chain = settings?.chain_id ? chains.find((c) => c.id === settings?.chain_id) : null;
	$: chainPath = chain
		? chain.components?.map((c) => c.name).filter(Boolean).join(' → ') || chain.name
		: 'Pick a chain below';
	$: rules = [
		{ key: 'skip_breaks', label: 'Skip ads, talk & jingles', sub: 'News, traffic and promo lines are logged as skipped, never as listens' },
		{ key: 'tagged_only', label: 'Forward tagged tracks only', sub: 'Tracks MusicBrainz recognises go straight to your services. Other names wait.' },
		{ key: 'review', label: 'Review queue', sub: 'Unrecognised track names wait here for a yes' },
		{ key: 'tag_radio', label: 'Tag as radio listening', sub: 'Saved with listening_context = radio' },
		{ key: 'auto_stop', label: 'Auto-stop after', sub: 'Ends the session if you walk away with the tuner on' },
		{ key: 'attach_chain', label: 'Attach signal chain', sub: chainPath }
	] as { key: RuleKey; label: string; sub: string }[];

	async function saveSettings(next: RadioSettings) {
		const before = settings;
		settings = next;
		try {
			settings = await api.putRadioSettings(next);
			await loadTuner();
		} catch (e) {
			settings = before;
			error = humanError(e, 'Could not save your radio rules');
		}
	}
	function toggleRule(k: RuleKey) {
		if (settings) void saveSettings({ ...settings, [k]: !settings[k] });
	}
	function stopHours(d: number) {
		if (settings)
			void saveSettings({ ...settings, auto_stop_hours: Math.min(12, Math.max(1, settings.auto_stop_hours + d)) });
	}
	function pickChain(e: Event) {
		const v = (e.target as HTMLSelectElement).value;
		if (settings) void saveSettings({ ...settings, chain_id: v ? Number(v) : null });
	}

	// ── Tuning ──

	async function tune(p: RadioPreset) {
		selId = p.id;
		editing = false;
		busy = true;
		error = '';
		try {
			tuner = await api.tuneIn(p.id);
			await Promise.all([loadLog(), loadRecent()]);
		} catch (e) {
			error = humanError(e, `Could not tune in to ${p.name}`);
		} finally {
			busy = false;
		}
	}
	async function off() {
		busy = true;
		try {
			tuner = await api.tuneOff();
			await Promise.all([loadLog(), loadRecent()]);
		} catch (e) {
			error = humanError(e, 'Could not turn the tuner off');
		} finally {
			busy = false;
		}
	}
	function seek(d: number) {
		if (!presets.length) return;
		const i = Math.max(0, presets.findIndex((p) => p.id === sel?.id));
		void tune(presets[(i + d + presets.length) % presets.length]);
	}
	function tuneRecent(r: RadioSession) {
		const p = presets.find((x) => x.station_id === r.station_id && x.band === r.band);
		if (p) void tune(p);
	}

	$: sessionLine = tuned
		? `On since ${hhmm(tuner?.since)}${tuner?.ends_at ? ` · auto-stop ${hhmm(tuner.ends_at)}` : ''}`
		: 'Off';

	// ── Review ──

	async function decide(e: AirLogEntry, keep: boolean) {
		try {
			if (keep) await api.keepAirLine(e.id);
			else await api.dropAirLine(e.id);
		} catch (err) {
			error = humanError(err, keep ? 'Could not keep that line' : 'Could not drop that line');
		}
		await loadLog();
	}

	// ── Browse & presets ──

	$: chips = browse === 'genre' ? genres.slice(0, 12) : regions;
	$: shownStations = stations
		.filter((s) => {
			if (chip) {
				if (browse === 'genre' && !s.genres.some((g) => g.toLowerCase() === chip?.toLowerCase())) return false;
				if (browse === 'region' && s.region !== chip) return false;
			}
			const q = query.trim().toLowerCase();
			return !q || s.name.toLowerCase().includes(q) || s.provider.toLowerCase().includes(q);
		})
		.slice(0, 40);
	const hasPreset = (s: RadioStation, band: 'fm' | 'dab') =>
		presets.some((p) => p.station_id === s.id && p.band === band);

	function startAdd(s: RadioStation, band: 'fm' | 'dab') {
		adding = { station: s, band, dial: '', city: '' };
	}
	async function saveAdd() {
		if (!adding) return;
		busy = true;
		try {
			const p = await api.addRadioPreset({
				station_id: adding.station.id,
				band: adding.band,
				dial: adding.dial || null,
				city: adding.city || null
			});
			adding = null;
			await loadPresets();
			if (!tuned) selId = p.id;
		} catch (e) {
			error = humanError(e, 'Could not save that preset');
		} finally {
			busy = false;
		}
	}

	function openEditor() {
		if (!sel) return;
		editing = !editing;
		editDial = sel.dial ?? '';
		editCity = sel.city ?? '';
	}
	async function saveEdit() {
		if (!sel) return;
		busy = true;
		try {
			await api.updateRadioPreset(sel.id, { dial: editDial.trim() || null, city: editCity.trim() || null });
			editing = false;
			await Promise.all([loadPresets(), loadTuner()]);
		} catch (e) {
			error = humanError(e, 'Could not save that preset');
		} finally {
			busy = false;
		}
	}
	async function move(d: number) {
		if (!sel) return;
		const i = presets.findIndex((p) => p.id === sel?.id);
		const j = i + d;
		if (i < 0 || j < 0 || j >= presets.length) return;
		const a = presets[i];
		const b = presets[j];
		try {
			await api.updateRadioPreset(a.id, { position: b.position });
			await api.updateRadioPreset(b.id, { position: a.position });
			await loadPresets();
		} catch (e) {
			error = humanError(e, 'Could not move that preset');
		}
	}
	async function removePreset() {
		if (!sel) return;
		if (!confirm(`Remove ${sel.name} (${bandLabel(sel.band)}) from your presets? Its air log stays.`)) return;
		try {
			if (tuned && tuner?.preset?.id === sel.id) await api.tuneOff();
			await api.deleteRadioPreset(sel.id);
			editing = false;
			selId = null;
			await Promise.all([loadPresets(), loadTuner()]);
		} catch (e) {
			error = humanError(e, 'Could not remove that preset');
		}
	}
</script>

<svelte:head><title>Radio · Tapedeck</title></svelte:head>

<div class="page">
	<div class="head">
		<div>
			<h1 class="h-display">Radio</h1>
			<p class="lede">Pick a station and Tapedeck logs what it plays, from the station's own stream.</p>
		</div>
		<div class="head-right">
			<span class="session" class:on={tuned}>{sessionLine}</span>
			{#if tuned}
				<button class="btn btn-secondary" on:click={off} disabled={busy}>Off</button>
			{/if}
		</div>
	</div>

	{#if error}
		<div class="error" role="alert">
			<span>{error}</span>
			<button class="btn btn-ghost" on:click={() => (error = '')}>Dismiss</button>
		</div>
	{/if}

	{#if loading}
		<p class="muted">Loading…</p>
	{:else}
		<div class="grid">
			<div class="main">
				<!-- ═════════ The faceplate ═════════ -->
				<section class="faceplate" aria-label="Tuner">
					<div class="fp-top">
						<div class="fp-freq">
							<div class="fp-band">{sel ? bandLabel(sel.band) : '—'}</div>
							<div class="fp-num">{sel?.dial ?? '—'}</div>
							<div class="fp-city">{sel?.city ?? (sel ? (isDab ? 'channel not set' : 'frequency not set') : '')}</div>
						</div>
						<div class="fp-now">
							<div class="fp-name">
								{#if tuner?.logo && tuned}<img src={tuner.logo} alt="" class="fp-logo" />{/if}
								{sel?.name ?? 'Radio'}
							</div>
							<div class="fp-title">{disp.title}</div>
							<div class="fp-sub">{disp.sub}</div>
							<div class="fp-pills">
								<span class="pill dark {disp.tone}">{disp.status}</span>
								{#if disp.src}<span class="badge dark warn">{disp.src}</span>{/if}
								{#if tuned && tuner?.chain_name}<span class="fp-chain">via {tuner.chain_name}</span>{/if}
							</div>
						</div>
						<div class="fp-side">
							<div class="bars" title="Stream health">
								{#each [0, 1, 2, 3, 4] as b}
									<span style="height:{6 + b * 4}px" class:lit={b < (tuner?.signal ?? 0)}></span>
								{/each}
							</div>
							<div class="fp-id">{idLine}<br />{dataLine}</div>
						</div>
					</div>

					{#if tuned && tuner?.state === 'live'}
						<div class="onair" aria-hidden="true"><div></div></div>
					{/if}

					<div class="dial" aria-hidden="true">
						{#each dialTicks as t}
							<span class="tick" class:major={t.major} style="left:{t.left}%"></span>
							{#if t.label}<span class="tick-label" style="left:{t.left}%">{t.label}</span>{/if}
						{/each}
						{#each markers as m}
							<button
								class="marker"
								class:on={m.p.id === sel?.id}
								style="left:{m.left}%"
								title={m.p.name}
								tabindex="-1"
								on:click={() => tune(m.p)}>{m.n}</button>
						{/each}
						{#if needle !== null}
							<span class="needle" class:dim={tuner?.state === 'dropped'} style="left:{needle}%"></span>
						{/if}
					</div>

					<div class="preset-row">
						<button class="seek" on:click={() => seek(-1)} aria-label="Previous preset" disabled={busy || !presets.length}>‹</button>
						<div class="presets">
							{#each presets as p, i (p.id)}
								<button class="preset" class:on={p.id === sel?.id} on:click={() => tune(p)} disabled={busy} title="{p.name} · {bandLabel(p.band)}{p.dial ? ' ' + p.dial : ''}">
									<span class="pn">{i + 1}</span>
									<span class="ps">{p.short_name ?? p.name}</span>
								</button>
							{:else}
								<span class="fp-empty">No presets yet — add a station from Browse.</span>
							{/each}
						</div>
						<button class="seek" on:click={() => seek(1)} aria-label="Next preset" disabled={busy || !presets.length}>›</button>
					</div>

					{#if sel}
						<div class="fp-tools">
							<button class="fp-link" on:click={openEditor}>{editing ? 'Close' : 'Edit preset'}</button>
						</div>
						{#if editing}
							<div class="editor">
								<label>
									<span>{isDab ? 'DAB channel' : 'FM frequency (MHz)'}</span>
									<input class="fp-input" bind:value={editDial} placeholder={isDab ? '5C' : '95.8'} />
								</label>
								<label>
									<span>City</span>
									<input class="fp-input" bind:value={editCity} placeholder="Leipzig" />
								</label>
								<div class="editor-actions">
									<button class="fp-btn" on:click={() => move(-1)} aria-label="Move left">←</button>
									<button class="fp-btn" on:click={() => move(1)} aria-label="Move right">→</button>
									<button class="fp-btn primary" on:click={saveEdit} disabled={busy}>Save</button>
									<button class="fp-btn" on:click={removePreset}>Remove</button>
								</div>
							</div>
						{/if}
					{/if}
				</section>

				<div class="two">
					<!-- ═════════ Tonight ═════════ -->
					<section class="card">
						<div class="card-head">
							<h3>{epgTitle}</h3>
							{#if schedule}<span class="badge good">{schedule.source}</span>{/if}
						</div>
						{#if !sel}
							<p class="muted small">Pick a preset to see what's on.</p>
						{:else if !schedule && scheduleFor === sel.id}
							<p class="muted small">Fetching the schedule…</p>
						{:else}
							<div class="epg">
								{#each epg as e (e.key)}
									<div class="epg-row" class:now={e.onNow} class:earlier={e.earlier}>
										<span class="epg-t">{e.t}</span>
										<span class="epg-n">{e.n}</span>
										{#if e.tag}<span class="epg-tag" class:now={e.onNow}>{e.tag}</span>{/if}
									</div>
								{/each}
							</div>
						{/if}
						{#if epgNote}<p class="note">{epgNote}</p>{/if}
					</section>

					<!-- ═════════ Air log ═════════ -->
					<section class="card">
						<div class="card-head">
							<h3>Air log</h3>
							<span class="muted small">{log.length} entries · {scrobbledCount} scrobbled</span>
						</div>
						{#if !log.length}
							<p class="muted small">Nothing yet. Tune in and what the station plays shows up here.</p>
						{/if}
						<div class="log">
							{#each log as e (e.id)}
								<div class="log-row">
									<span class="log-t">{hhmm(e.started_at)}</span>
									<div class="log-body" class:faded={statusOf(e.status)[1] === 'mute'}>
										<div class="log-title">{lineTitle(e)}</div>
										<div class="log-sub">{lineSub(e)}</div>
									</div>
									<span class="badge warn">{e.source}</span>
									<span class="pill {statusOf(e.status)[1]}">{statusOf(e.status)[0]}</span>
								</div>
							{/each}
						</div>
					</section>
				</div>
			</div>

			<!-- ═════════ Right rail ═════════ -->
			<aside class="rail">
				<section class="card">
					<h3>Scrobble rules</h3>
					{#if settings}
						{#each rules as r (r.key)}
							<div class="rule">
								<div class="rule-text">
									<div class="rule-label">{r.label}</div>
									<div class="rule-sub">{r.sub}</div>
									{#if r.key === 'auto_stop'}
										<div class="stepper">
											<button on:click={() => stopHours(-1)} aria-label="Fewer hours">−</button>
											<span>{settings.auto_stop_hours}h</span>
											<button on:click={() => stopHours(1)} aria-label="More hours">+</button>
										</div>
									{/if}
									{#if r.key === 'attach_chain' && settings.attach_chain}
										<select class="input chain-pick" value={settings.chain_id ?? ''} on:change={pickChain}>
											<option value="">No chain</option>
											{#each chains as c (c.id)}
												<option value={c.id}>{c.name}</option>
											{/each}
										</select>
									{/if}
								</div>
								<button
									class="switch"
									class:on={settings[r.key]}
									role="switch"
									aria-checked={!!settings[r.key]}
									aria-label={r.label}
									on:click={() => toggleRule(r.key)}><span></span></button>
							</div>
						{/each}
					{/if}
				</section>

				<section class="card">
					<div class="card-head">
						<h3>To review</h3>
						<span class="count">{review.length}</span>
					</div>
					{#if !review.length}
						<p class="muted small">Nothing waiting. Track names MusicBrainz doesn't recognise show up here.</p>
					{/if}
					{#each review as q (q.id)}
						<div class="queue">
							<div class="muted small">{hhmm(q.started_at)} · {q.station_name} · “{q.raw}”</div>
							<div class="queue-name">{q.artist} — {q.title}</div>
							<div class="queue-actions">
								<button class="btn btn-primary" on:click={() => decide(q, true)}>Keep &amp; forward</button>
								<button class="btn btn-secondary" on:click={() => decide(q, false)}>Drop</button>
							</div>
						</div>
					{/each}
				</section>

				<section class="card">
					<h3>Recently tuned</h3>
					{#if !recent.length}<p class="muted small">Nothing yet.</p>{/if}
					{#each recent as r (r.id)}
						<button class="recent" on:click={() => tuneRecent(r)}>
							<span>{r.station_name}</span>
							<span class="muted small">{[`${bandLabel(r.band)}${r.dial ? ' ' + r.dial : ''}`, r.city].filter(Boolean).join(' · ')}</span>
						</button>
					{/each}

					<div class="browse-head">
						<h3>Browse</h3>
						<div class="seg">
							<button class:on={browse === 'genre'} on:click={() => ((browse = 'genre'), (chip = null))}>Genre</button>
							<button class:on={browse === 'region'} on:click={() => ((browse = 'region'), (chip = null))}>Region</button>
						</div>
					</div>
					<div class="chips">
						{#each chips as c}
							<button class="chip" class:on={chip === c} on:click={() => (chip = chip === c ? null : c)}>{c}</button>
						{/each}
					</div>
					<input class="input search" placeholder="Search stations" bind:value={query} />
					{#if !stations.length}
						<p class="muted small">Fetching the station list…</p>
					{/if}
					<div class="stations">
						{#each shownStations as s (s.id)}
							<div class="station">
								<div class="st-text">
									<div class="st-name">{s.name}</div>
									<div class="muted small">{s.provider}{s.genres.length ? ' · ' + s.genres.slice(0, 2).join(', ') : ''}{s.stream ? '' : ' · no stream'}</div>
								</div>
								<div class="st-add">
									{#each ['fm', 'dab'] as b}
										{#if (b === 'fm' ? !!s.fm : s.dab.length > 0)}
											<button
												class="chip"
												disabled={hasPreset(s, b === 'fm' ? 'fm' : 'dab') || !s.stream}
												title={!s.stream
													? 'This station publishes no stream to read track names from'
													: hasPreset(s, b === 'fm' ? 'fm' : 'dab')
														? 'Already a preset'
														: `Add on ${bandLabel(b)}`}
												on:click={() => startAdd(s, b === 'fm' ? 'fm' : 'dab')}>+ {bandLabel(b)}</button>
										{/if}
									{/each}
								</div>
							</div>
							{#if adding && adding.station.id === s.id}
								<form class="adder" on:submit|preventDefault={saveAdd}>
									<label>
										<span>{adding.band === 'dab' ? 'DAB channel' : 'FM frequency (MHz)'} · optional</span>
										<input class="input" bind:value={adding.dial} placeholder={adding.band === 'dab' ? '5C' : '95.8'} />
									</label>
									<label>
										<span>City · optional</span>
										<input class="input" bind:value={adding.city} placeholder="Leipzig" />
									</label>
									<div class="queue-actions">
										<button class="btn btn-primary" type="submit" disabled={busy}>Add preset</button>
										<button class="btn btn-secondary" type="button" on:click={() => (adding = null)}>Cancel</button>
									</div>
								</form>
							{/if}
						{/each}
					</div>
					<p class="note">{dirNote || 'German stations for now. Worldwide comes later.'}</p>
				</section>
			</aside>
		</div>
	{/if}
</div>

<style>
	.page { max-width: 1460px; margin: 0 auto; }
	.head { display: flex; align-items: flex-end; justify-content: space-between; gap: 16px; flex-wrap: wrap; margin-bottom: 18px; }
	.head h1 { font-size: 34px; }
	.lede { margin: 6px 0 0; color: var(--td-muted); font-size: 14px; }
	.head-right { display: flex; align-items: center; gap: 10px; }
	.session { font-size: 12.5px; font-weight: 600; padding: 7px 13px; border-radius: 999px; background: var(--td-surface2); color: var(--td-muted); border: 1px solid var(--td-border); }
	.session.on { color: var(--td-good-fg); background: var(--td-good-bg); }
	.error { display: flex; align-items: center; justify-content: space-between; gap: 12px; padding: 10px 14px; border-radius: 14px; background: var(--td-bad-bg); color: var(--td-bad-fg); margin-bottom: 14px; font-size: 13.5px; }
	.muted { color: var(--td-muted); }
	.small { font-size: 12.5px; }

	.grid { display: grid; grid-template-columns: minmax(0, 1fr) 340px; gap: 18px; align-items: start; }
	.main { display: flex; flex-direction: column; gap: 18px; min-width: 0; }
	.rail { display: flex; flex-direction: column; gap: 18px; min-width: 0; }
	.two { display: grid; grid-template-columns: minmax(0, 1fr) minmax(0, 1.3fr); gap: 18px; }
	@media (max-width: 1180px) { .grid { grid-template-columns: minmax(0, 1fr); } }
	@media (max-width: 760px) { .two { grid-template-columns: minmax(0, 1fr); } }

	.card { background: var(--td-surface); border: 1px solid var(--td-border); border-radius: 22px; padding: 18px; min-width: 0; }
	.card h3 { margin: 0 0 10px; font-size: 15px; font-weight: 700; }
	.card-head { display: flex; align-items: center; justify-content: space-between; gap: 10px; margin-bottom: 10px; }
	.card-head h3 { margin: 0; }
	.note { margin: 12px 0 0; font-size: 12px; line-height: 1.5; color: var(--td-muted); }
	.count { font: 700 12px 'Figtree', sans-serif; padding: 3px 10px; border-radius: 999px; background: var(--td-surface2); color: var(--td-muted); }

	/* Pills and badges off the faceplate follow the theme. There is no warn
	   token in app.css, so warn is the accent mixed down — the same colour
	   family the design uses for it. */
	.pill, .badge { font: 700 10.5px 'Figtree', sans-serif; letter-spacing: .04em; padding: 4px 9px; border-radius: 999px; white-space: nowrap; flex: none; }
	.pill { font-size: 11.5px; font-weight: 600; letter-spacing: 0; }
	.pill.good, .badge.good { background: var(--td-good-bg); color: var(--td-good-fg); }
	.pill.warn, .badge.warn { background: color-mix(in srgb, var(--td-accent) 18%, transparent); color: var(--td-accent); }
	.pill.mute { background: transparent; color: var(--td-muted); }

	/* ── The faceplate ──
	   **Dark in both themes, on purpose**, like the lyric reader's stage: it
	   is the radio itself, not a card of the page. So its colours are literal
	   rather than tokens — a token would follow the theme and put dark text on
	   a dark panel in light mode, which is the `--td-panel` trap. The ground
	   is `--td-panel`, which is dark in both themes by definition. */
	.faceplate { background: var(--td-panel); color: #f3e9d6; border-radius: 26px; padding: 20px 22px 18px; box-shadow: inset 0 1px 0 rgba(255,255,255,.05); }
	.fp-top { display: grid; grid-template-columns: 150px minmax(0, 1fr) 150px; gap: 18px; align-items: start; }
	@media (max-width: 700px) { .fp-top { grid-template-columns: minmax(0, 1fr); } .fp-side { align-items: flex-start !important; } }
	.fp-freq { background: #1b1712; border-radius: 18px; padding: 12px 14px; }
	.fp-band { font: 700 11px 'Figtree', sans-serif; letter-spacing: .1em; color: #e07a55; }
	.fp-num { font: 600 34px ui-monospace, Menlo, monospace; letter-spacing: -.02em; margin-top: 2px; color: #f3e9d6; }
	.fp-city { font-size: 12px; color: #a8997f; margin-top: 2px; }
	.fp-name { display: flex; align-items: center; gap: 8px; font: 700 12px 'Figtree', sans-serif; letter-spacing: .08em; text-transform: uppercase; color: #cbbda4; }
	.fp-logo { width: 22px; height: 22px; border-radius: 6px; object-fit: cover; background: #f3e9d6; }
	.fp-title { font-family: 'Caprasimo', serif; font-size: 30px; line-height: 1.12; margin-top: 6px; overflow-wrap: anywhere; }
	.fp-sub { font-size: 15px; color: #cbbda4; margin-top: 4px; overflow-wrap: anywhere; }
	.fp-pills { display: flex; align-items: center; gap: 8px; flex-wrap: wrap; margin-top: 12px; }
	.fp-chain { font-size: 12px; color: #a8997f; }
	.pill.dark { font: 700 11px 'Figtree', sans-serif; letter-spacing: .06em; text-transform: uppercase; padding: 5px 11px; }
	.pill.dark.good { background: rgba(122,138,94,.3); color: #c4d3a6; }
	.pill.dark.warn, .badge.dark.warn { background: rgba(224,122,85,.24); color: #f0b48f; }
	.pill.dark.neutral { background: #3a3128; color: #cbbda4; }
	.pill.dark.bad { background: rgba(224,122,85,.35); color: #ffd2bd; }
	.fp-side { display: flex; flex-direction: column; align-items: flex-end; gap: 10px; }
	.bars { display: flex; align-items: flex-end; gap: 3px; height: 24px; }
	.bars span { width: 4px; border-radius: 2px; background: #3a3128; }
	.bars span.lit { background: #e07a55; }
	.fp-id { font: 500 11px ui-monospace, Menlo, monospace; color: #8a7d68; text-align: right; line-height: 1.6; }
	@media (max-width: 700px) { .fp-id { text-align: left; } }

	.onair { height: 3px; border-radius: 3px; background: #3a3128; margin-top: 16px; overflow: hidden; }
	.onair div { width: 30%; height: 100%; background: #e07a55; border-radius: 3px; animation: sweep 2.8s ease-in-out infinite; }
	@keyframes sweep { 0% { transform: translateX(-100%); } 100% { transform: translateX(340%); } }
	@media (prefers-reduced-motion: reduce) { .onair div { animation: none; width: 100%; opacity: .5; } }

	.dial { position: relative; height: 64px; margin: 18px 4px 6px; border-bottom: 1px solid #3a3128; }
	.tick { position: absolute; bottom: 0; width: 1.5px; height: 8px; background: #4a3f33; }
	.tick.major { height: 16px; background: #8a7d68; }
	.tick-label { position: absolute; bottom: 20px; transform: translateX(-50%); font: 500 10px ui-monospace, Menlo, monospace; color: #8a7d68; }
	.marker { position: absolute; top: 4px; transform: translateX(-50%); width: 20px; height: 20px; border-radius: 999px; border: none; cursor: pointer; font: 700 10px 'Figtree', sans-serif; background: #3a3128; color: #cbbda4; padding: 0; }
	.marker.on { background: #e07a55; color: #1b1712; }
	.needle { position: absolute; top: 0; bottom: 0; width: 2px; margin-left: -1px; background: #e07a55; box-shadow: 0 0 10px rgba(224,122,85,.7); }
	.needle.dim { opacity: .35; }

	.preset-row { display: flex; align-items: stretch; gap: 10px; margin-top: 14px; }
	.seek { flex: none; width: 38px; border-radius: 14px; border: none; background: #3a3128; color: #e9dcc4; font-size: 20px; cursor: pointer; }
	.seek:hover:not(:disabled) { background: #4a3f33; }
	.seek:disabled { opacity: .4; cursor: default; }
	.presets { flex: 1; min-width: 0; display: grid; grid-template-columns: repeat(auto-fill, minmax(92px, 1fr)); gap: 8px; }
	.preset { display: flex; flex-direction: column; align-items: center; justify-content: center; gap: 2px; padding: 9px 6px; border-radius: 16px; border: none; cursor: pointer; min-width: 0; font-family: 'Figtree', sans-serif; background: #3a3128; color: #e9dcc4; box-shadow: inset 0 -3px 0 rgba(0,0,0,.3); }
	.preset:hover:not(:disabled) { filter: brightness(1.12); }
	.preset.on { background: #c67139; color: #fbf5ea; box-shadow: inset 0 -3px 0 rgba(0,0,0,.18); }
	.pn { font-weight: 700; font-size: 15px; }
	.ps { font-size: 11px; max-width: 100%; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
	.fp-empty { color: #a8997f; font-size: 13px; align-self: center; }
	.fp-tools { display: flex; justify-content: flex-end; margin-top: 10px; }
	.fp-link { background: none; border: none; color: #e07a55; font: 600 12px 'Figtree', sans-serif; cursor: pointer; padding: 2px 4px; }
	.editor { display: flex; flex-wrap: wrap; gap: 12px; align-items: flex-end; margin-top: 8px; padding: 12px; border-radius: 16px; background: #1b1712; }
	.editor label { display: flex; flex-direction: column; gap: 4px; font-size: 11.5px; color: #a8997f; }
	.fp-input { background: #2b2721; border: 1px solid #4a3f33; color: #f3e9d6; border-radius: 999px; padding: 7px 12px; font: 14px 'Figtree', sans-serif; width: 150px; }
	.fp-input:focus-visible { outline: 2px solid #e07a55; outline-offset: 1px; }
	.editor-actions { display: flex; gap: 6px; flex-wrap: wrap; }
	.fp-btn { border: none; border-radius: 999px; padding: 7px 13px; font: 600 12.5px 'Figtree', sans-serif; background: #3a3128; color: #e9dcc4; cursor: pointer; }
	.fp-btn.primary { background: #c67139; color: #fbf5ea; }

	/* ── Tonight ── */
	.epg { display: flex; flex-direction: column; gap: 2px; }
	.epg-row { display: flex; align-items: center; gap: 10px; padding: 9px 12px; border-radius: 14px; }
	.epg-row.now { background: var(--td-surface2); }
	.epg-row.earlier { opacity: .6; }
	.epg-t { font: 500 12px ui-monospace, Menlo, monospace; color: var(--td-muted); flex: none; white-space: nowrap; }
	.epg-n { flex: 1; min-width: 0; font-size: 14px; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
	.epg-tag { font-size: 11px; color: var(--td-muted); flex: none; }
	.epg-tag.now { font: 700 10.5px 'Figtree', sans-serif; padding: 3px 9px; border-radius: 999px; background: var(--td-accent); color: var(--td-surface); }

	/* ── Air log ── */
	.log { display: flex; flex-direction: column; max-height: 460px; overflow-y: auto; }
	.log-row { display: flex; align-items: center; gap: 10px; padding: 9px 2px; border-bottom: 1px solid var(--td-border); }
	.log-row:last-child { border-bottom: none; }
	.log-t { font: 500 12px ui-monospace, Menlo, monospace; color: var(--td-muted); flex: none; min-width: 44px; white-space: nowrap; }
	.log-body { flex: 1; min-width: 0; }
	.log-body.faded { opacity: .55; }
	.log-title { font-size: 14px; font-weight: 600; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
	.log-sub { font-size: 12.5px; color: var(--td-muted); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }

	/* ── Rules ── */
	.rule { display: flex; align-items: flex-start; justify-content: space-between; gap: 12px; padding: 10px 0; border-bottom: 1px solid var(--td-border); }
	.rule:last-child { border-bottom: none; }
	.rule-text { min-width: 0; }
	.rule-label { font-size: 14px; font-weight: 600; }
	.rule-sub { font-size: 12px; color: var(--td-muted); margin-top: 2px; line-height: 1.4; }
	.switch { width: 40px; height: 24px; border-radius: 999px; border: none; padding: 3px; cursor: pointer; flex: none; display: flex; justify-content: flex-start; background: var(--td-track); }
	.switch.on { justify-content: flex-end; background: var(--td-accent-2); }
	.switch span { width: 18px; height: 18px; border-radius: 999px; background: var(--td-surface); box-shadow: 0 1px 2px rgba(0,0,0,.2); }
	.stepper { display: inline-flex; align-items: center; gap: 8px; margin-top: 8px; }
	.stepper button { width: 26px; height: 26px; border-radius: 999px; border: 1px solid var(--td-border); background: var(--td-surface2); color: var(--td-text); cursor: pointer; font-size: 14px; }
	.stepper button:hover { background: var(--td-nav-hover); }
	.stepper span { font: 600 13px 'Figtree', sans-serif; min-width: 28px; text-align: center; }
	.chain-pick { margin-top: 8px; width: 100%; }

	/* ── Review ── */
	.queue { padding: 10px 0; border-bottom: 1px solid var(--td-border); }
	.queue:last-child { border-bottom: none; }
	.queue-name { font-size: 14px; font-weight: 600; margin: 3px 0 8px; }
	.queue-actions { display: flex; gap: 8px; flex-wrap: wrap; }

	/* ── Recent & browse ── */
	.recent { display: flex; flex-direction: column; align-items: flex-start; gap: 1px; width: 100%; padding: 7px 8px; border-radius: 12px; border: none; background: transparent; color: var(--td-text); text-align: left; cursor: pointer; font: 600 13.5px 'Figtree', sans-serif; }
	.recent:hover { background: var(--td-nav-hover); }
	.browse-head { display: flex; align-items: center; justify-content: space-between; margin-top: 16px; }
	.browse-head h3 { margin: 0; }
	.seg { display: flex; gap: 4px; padding: 4px; border-radius: 999px; background: var(--td-surface2); }
	.seg button { border: none; cursor: pointer; font: 600 12px 'Figtree', sans-serif; padding: 6px 12px; border-radius: 999px; background: transparent; color: var(--td-muted); }
	.seg button.on { background: var(--td-surface); color: var(--td-text); box-shadow: 0 1px 3px rgba(46,43,37,.12); }
	.chips { display: flex; flex-wrap: wrap; gap: 6px; margin: 12px 0; }
	.chip { font: 600 12px 'Figtree', sans-serif; padding: 6px 11px; border-radius: 999px; cursor: pointer; border: 1px solid var(--td-border); background: transparent; color: var(--td-nav-fg); }
	.chip.on { background: var(--td-good-bg); color: var(--td-good-fg); border-color: transparent; }
	.chip:disabled { opacity: .4; cursor: default; }
	.search { width: 100%; margin-bottom: 8px; }
	.stations { display: flex; flex-direction: column; max-height: 420px; overflow-y: auto; }
	.station { display: flex; align-items: center; justify-content: space-between; gap: 8px; padding: 8px 2px; border-bottom: 1px solid var(--td-border); }
	.st-text { min-width: 0; }
	.st-name { font-size: 13.5px; font-weight: 600; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
	.st-add { display: flex; gap: 4px; flex: none; }
	.adder { display: flex; flex-direction: column; gap: 8px; padding: 10px; margin: 4px 0 8px; border-radius: 14px; background: var(--td-surface2); }
	.adder label { display: flex; flex-direction: column; gap: 4px; font-size: 12px; color: var(--td-muted); }
</style>
