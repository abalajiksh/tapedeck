<script lang="ts">
	import { onMount, onDestroy } from 'svelte';
	import { syncUrl } from '$lib/url';
	import type {
		McpConnection,
		McpAuditCall,
		OpsStatus,
		LogTail,
		SourceHealth
	} from '$lib/api';
	import { api } from '$lib/api';
	import type { TokenInfo } from '$lib/api';
	import { formatDateTime, formatDuration } from '$lib/utils';
	import { theme, setTheme, type ThemeMode } from '$lib/theme';
	import { showArtwork, setShowArtwork } from '$lib/prefs';
	import type {
		Connection,
		SignalChain,
		UserProfile,
		PublishSwitch,
		FederationSwitch
	} from '$lib/api';
	import SourcesPanel from '$lib/components/SourcesPanel.svelte';

	const uiVersion = __UI_VERSION__;
	let backendVersion = '…';
	let origin = typeof window !== 'undefined' ? window.location.origin : 'http://your-server:8080';

	// ── Sub-sections ──
	// The page had grown to nine stacked panels; splitting it means the thing you
	// came for is one click away instead of a scroll hunt. The section lives in the
	// hash so a link like /settings#sources lands where it says it will.
	const SECTIONS = [
		{ id: 'account', label: 'Account', hint: 'Password, picture, bio' },
		{ id: 'sharing', label: 'Sharing', hint: 'Now-playing, and being findable' },
		{ id: 'appearance', label: 'Appearance', hint: 'Theme and artwork' },
		{ id: 'time', label: 'Time', hint: 'Timezone and week start' },
		{ id: 'scrobbling', label: 'Scrobbling', hint: 'When a listen counts' },
		{ id: 'sources', label: 'Sources', hint: 'Plex, Navidrome, Jellyfin' },
		{ id: 'connections', label: 'Connections', hint: 'Where listens are forwarded' },
		{ id: 'tokens', label: 'API Tokens', hint: 'Scrobble clients' },
		{ id: 'ai', label: 'AI Connections', hint: 'Chatbots that can read your listening' },
		{ id: 'data', label: 'Data', hint: 'Export, backup, wipe' },
		{ id: 'sanitiser', label: 'Sanitiser', hint: 'Fix split albums, artists and credits' },
		{ id: 'maintenance', label: 'Maintenance', hint: 'Enrichment and health' },
		{ id: 'genremap', label: 'Genre Map', hint: 'Every Noise at Once' },
		{ id: 'ops', label: 'Operations', hint: 'Engine, services, logs', admin: true },
		{ id: 'about', label: 'About', hint: 'Version, sign out' }
	];
	let section = 'appearance';
	function go(id: string) {
		section = id;
		syncUrl(`#${id}`);
	}

	// ── The shelf, in and out ──
	let shelfImporting = false;
	let shelfMsg = '';
	let shelfErr = '';

	async function onShelfImport(ev: Event) {
		const input = ev.target as HTMLInputElement;
		const file = input.files?.[0];
		if (!file) return;
		shelfImporting = true;
		shelfMsg = '';
		shelfErr = '';
		try {
			const r = await api.importShelf(file);
			// Every count is reported, including the ones that are zero on a
			// second import — "nothing happened" is the answer that tells you it
			// matched rather than that it failed.
			const bits = [
				`${r.releases} added`,
				r.already_on_the_shelf ? `${r.already_on_the_shelf} already on the shelf` : '',
				r.plays ? `${r.plays} side${r.plays === 1 ? '' : 's'} played` : '',
				r.images ? `${r.images} scan${r.images === 1 ? '' : 's'}` : '',
				r.gear ? `${r.gear} piece${r.gear === 1 ? '' : 's'} of gear` : '',
				r.missing_scans ? `${r.missing_scans} scan${r.missing_scans === 1 ? '' : 's'} missing from the archive` : '',
				r.skipped ? `${r.skipped} entr${r.skipped === 1 ? 'y' : 'ies'} unreadable` : ''
			].filter(Boolean);
			shelfMsg = `${bits.join(' · ')}. ${r.note}`;
		} catch (e) {
			shelfErr = String(e).replace(/^API \d+: /, '');
		}
		shelfImporting = false;
		input.value = '';
	}

	// ── Account ──
	let profile: UserProfile | null = null;
	let pDisplayName = '';
	let pBio = '';
	let profileMsg = '';
	let profileErr = '';
	let savingProfile = false;
	// Bumped after an upload so the <img> re-requests. The endpoint is a fixed
	// URL cached `immutable`, so without this a new picture would not appear
	// until the tab was reloaded.
	let avatarBust = 0;
	let avatarBusy = false;

	let curPw = '';
	let newPw = '';
	let confirmPw = '';
	let pwMsg = '';
	let pwErr = '';
	let savingPw = false;

	async function loadProfile() {
		try {
			profile = await api.getProfile();
			pDisplayName = profile.display_name ?? '';
			pBio = profile.bio ?? '';
		} catch { /* the section shows its own empty state */ }
	}

	async function saveProfile() {
		savingProfile = true;
		profileMsg = ''; profileErr = '';
		try {
			// Empty means "clear", which the server also treats as a clear rather
			// than storing a blank nobody can see to delete.
			await api.updateProfile({
				display_name: pDisplayName.trim() || null,
				bio: pBio.trim() || null
			});
			await loadProfile();
			profileMsg = 'Saved';
			setTimeout(() => (profileMsg = ''), 2500);
		} catch (e) {
			profileErr = e instanceof Error ? e.message : 'Could not save';
		}
		savingProfile = false;
	}

	async function onAvatarPicked(ev: Event) {
		const input = ev.target as HTMLInputElement;
		const file = input.files?.[0];
		if (!file) return;
		avatarBusy = true;
		profileErr = '';
		try {
			await api.uploadAvatar(file);
			await loadProfile();
			avatarBust = Date.now();
		} catch (e) {
			profileErr = e instanceof Error ? e.message : 'Could not upload that picture';
		}
		avatarBusy = false;
		input.value = '';
	}

	async function removeAvatar() {
		avatarBusy = true;
		try {
			await api.deleteAvatar();
			await loadProfile();
			avatarBust = Date.now();
		} catch (e) {
			profileErr = e instanceof Error ? e.message : 'Could not remove that picture';
		}
		avatarBusy = false;
	}

	async function savePassword() {
		pwMsg = ''; pwErr = '';
		if (newPw.length < 8) { pwErr = 'The new password must be at least 8 characters.'; return; }
		// Checked here as well as on the server, because the server only ever
		// sees one of the two and cannot catch a typo in the confirmation.
		if (newPw !== confirmPw) { pwErr = 'The two new passwords do not match.'; return; }
		savingPw = true;
		try {
			await api.changePassword(curPw, newPw);
			curPw = ''; newPw = ''; confirmPw = '';
			pwMsg = 'Password changed. Any other browser signed in as you has been signed out.';
		} catch (e) {
			const msg = e instanceof Error ? e.message : '';
			pwErr = msg.includes('403') ? 'That is not your current password.' : (msg || 'Could not change the password');
		}
		savingPw = false;
	}

	/** The lettered tile, same fallback the rest of the app uses for missing art. */
	$: avatarLetter = (profile?.display_name || profile?.username || '?').trim().charAt(0).toUpperCase();

	let tokens: TokenInfo[] = [];
	let chains: SignalChain[] = [];
	let loading = true;
	let listenCount = 0;
	let noteCount = 0;

	let newTokenName = '';
	let newTokenChain = ''; // '' = no chain; otherwise the chain id as a string
	// Off by default, and deliberately: `read` is an explicit grant, never
	// implied by `submit`. A scrobble client that only posts listens has no
	// business enumerating your gear.
	let newTokenRead = false;
	let newTokenWrite = false;
	let newTokenLiveImports = false;

	// ── Pairing a device ──
	// For clients with no browser: they show a code, you type it here.
	let pairCode = '';
	let pairInfo: { client_name: string; scopes: string; can_read: boolean; approved: boolean } | null = null;
	let pairChain = '';
	let pairMsg = '';
	let pairBusy = false;

	async function lookupPairing() {
		pairInfo = null;
		pairMsg = '';
		const code = pairCode.trim().toUpperCase();
		if (!code) return;
		pairBusy = true;
		try {
			pairInfo = await api.describePairing(code);
			if (pairInfo.approved) pairMsg = 'That code has already been approved.';
		} catch (e) {
			pairMsg = String(e).replace(/^API \d+: /, '');
		}
		pairBusy = false;
	}

	async function approvePairing() {
		pairBusy = true;
		try {
			await api.approvePairing(pairCode.trim().toUpperCase(), pairChain ? Number(pairChain) : null);
			pairMsg = 'Approved — the device should pick up its token within a few seconds.';
			pairInfo = null;
			pairCode = '';
			pairChain = '';
			tokens = (await api.getTokens()).tokens;
		} catch (e) {
			pairMsg = String(e).replace(/^API \d+: /, '');
		}
		pairBusy = false;
	}
	let createdToken = '';
	let tokenCopied = false;
	let creating = false;

	let healthStatus = '';
	let checking = false;

	// ── Metadata sanitiser ──
	// Preview first, always. Scrobble metadata that came from a real MusicBrainz
	// mapping is still data; rewriting it without showing what changes would be
	// a worse failure than the mismatch being fixed.
	let sanitize: import('$lib/api').SanitizePreview | null = null;
	let sanitizing = false;
	let sanitizeMsg = '';
	// Per-edit review: every proposed change is its own row with its own
	// checkbox, because a majority vote of 4-of-29 is a guess and only the
	// person who did the listening can tell. Keyed by identity, not index, so a
	// refreshed preview doesn't silently re-point a tick at something else.
	let pickedAlbums = new Set<string>();
	let pickedCredits = new Set<string>();
	let pickedVariants = new Set<string>();
	/** Which release each album should unify onto — defaults to the plurality. */
	let albumChoice: Record<string, string> = {};
	const albumKey = (a: { artist: string; album: string }) => `${a.artist}\u0001${a.album}`;
	const variantKey = (from: string, to: string) => `${from}\u0001${to}`;
	let pickedSplits = new Set<string>();
	const splitKey = (t: string, al: string, from: string) => `${t}\u0001${al}\u0001${from}`;

	/** Everything ticked, which is genuinely the common case. */
	function selectAll(on: boolean) {
		if (!sanitize) return;
		pickedAlbums = new Set(on ? sanitize.mixed_albums.map(albumKey) : []);
		pickedCredits = new Set(on ? sanitize.featured_artists.map((a) => a.artist) : []);
		pickedVariants = new Set(
			on
				? sanitize.artist_variants.flatMap((v) =>
						v.variants.map((x) => variantKey(x[0], v.primary))
					)
				: []
		);
		pickedSplits = new Set(
			on
				? sanitize.split_attributions.flatMap((sp) =>
						sp.folding.map((f) => splitKey(sp.title, sp.album, f[0]))
					)
				: []
		);
	}

	$: pickedCount =
		pickedAlbums.size + pickedCredits.size + pickedVariants.size + pickedSplits.size;

	// ── Rebuilding sessions ──
	// Needed on any existing install: `assign_session` kept the session rows but
	// nothing ever wrote `scrobbles.session_id`, so the sessions had no contents
	// until this runs once. Also the only way an imported history gets grouped.
	let rebuildingSessions = false;
	let sessionMsg = '';
	let sessionGapMinutes = 30;
	async function rebuildSessions() {
		if (!confirm('Regroup every listen into sessions?\n\nThis discards any sessions you have merged or split by hand.')) return;
		rebuildingSessions = true;
		sessionMsg = 'Regrouping your listens…';
		try {
			const { job_id } = await api.rebuildSessions(Math.max(1, sessionGapMinutes) * 60);
			await followRebuild(job_id);
		} catch (e) {
			// A 409 carries the id of the rebuild already running, so attach to
			// that one rather than reporting a failure for work in progress.
			const running = String(e).match(/"job_id"\s*:\s*"([0-9a-f]+)"/);
			if (running) {
				sessionMsg = 'A rebuild was already running — following it.';
				await followRebuild(running[1]);
			} else {
				sessionMsg = 'Failed: ' + String(e).replace(/^API \d+: /, '');
				rebuildingSessions = false;
			}
		}
	}

	/** Poll until it finishes.
	 *
	 *  Every two seconds: the whole point is that this can run for minutes, and
	 *  a tighter poll would be one more writer competing with the rebuild for
	 *  the same database. */
	async function followRebuild(jobId: string) {
		for (;;) {
			await new Promise((r) => setTimeout(r, 2000));
			try {
				const s = await api.rebuildStatus(jobId);
				if (s.state === 'running') {
					const mins = Math.floor((Date.now() / 1000 - s.started_at) / 60);
					sessionMsg = mins
						? `Regrouping your listens… (${mins} min so far)`
						: 'Regrouping your listens…';
					continue;
				}
				sessionMsg = s.state === 'done' ? s.status : 'Failed: ' + s.status;
				break;
			} catch (e) {
				// The job is gone — the process restarted mid-rebuild, which
				// rolls the whole transaction back rather than leaving half a
				// regrouping behind.
				sessionMsg = 'Lost track of the rebuild: ' + String(e).replace(/^API \d+: /, '');
				break;
			}
		}
		rebuildingSessions = false;
	}

	// ── Pulling loves from connected services ──
	let pullingLoves = false;
	let lovesMsg = '';
	async function pullLoves() {
		pullingLoves = true;
		lovesMsg = '';
		try {
			const r = await api.pullLoves();
			const parts = r.services.map((s) =>
				s.error
					? `${s.service}: ${s.error}`
					: `${s.service}: ${s.loved} new${s.skipped ? `, ${s.skipped} not in your history` : ''}`
			);
			lovesMsg = parts.length ? parts.join(' · ') : 'No connected services to pull from.';
		} catch (e) {
			lovesMsg = 'Failed: ' + String(e).replace(/^API \d+: /, '');
		}
		pullingLoves = false;
	}

	async function loadSanitize() {
		try {
			sanitize = await api.sanitizePreview();
			// Default each album to the plurality; the operator overrules it per row.
			albumChoice = {};
			for (const a of sanitize.mixed_albums) {
				if (a.winner) albumChoice[albumKey(a)] = a.winner;
			}
			selectAll(true);
		} catch { sanitize = null; }
	}
	async function runSanitize() {
		if (!sanitize || !pickedCount) return;
		sanitizing = true;
		sanitizeMsg = '';
		try {
			const r = await api.sanitizeApply({
				album_edits: sanitize.mixed_albums
					.filter((a) => pickedAlbums.has(albumKey(a)) && albumChoice[albumKey(a)])
					.map((a) => ({
						artist: a.artist,
						album: a.album,
						mbid_release: albumChoice[albumKey(a)]
					})),
				credit_edits: sanitize.featured_artists
					.filter((a) => pickedCredits.has(a.artist))
					.map((a) => ({ artist: a.artist })),
				variant_edits: sanitize.artist_variants.flatMap((v) =>
					v.variants
						.filter((x) => pickedVariants.has(variantKey(x[0], v.primary)))
						.map((x) => ({ from: x[0], to: v.primary }))
				),
				split_edits: sanitize.split_attributions.flatMap((sp) =>
					sp.folding
						.filter((f) => pickedSplits.has(splitKey(sp.title, sp.album, f[0])))
						.map((f) => ({ title: sp.title, album: sp.album, from: f[0], to: sp.primary }))
				)
			});
			sanitizeMsg =
				`Fixed ${r.albums_fixed} album${r.albums_fixed === 1 ? '' : 's'} (${r.album_listens_changed} listens), ` +
				`folded ${r.artists_folded} featured credit${r.artists_folded === 1 ? '' : 's'} (${r.artist_listens_changed} listens) ` +
				`merged ${r.variants_merged} spelling${r.variants_merged === 1 ? '' : 's'} (${r.variant_listens_changed} listens) ` +
				`and put ${r.splits_merged ?? 0} track${(r.splits_merged ?? 0) === 1 ? '' : 's'} back on their record's artist (${r.split_listens_changed ?? 0} listens). ` +
				`Artwork will re-resolve in the background.`;
			await loadSanitize();
		} catch (e) {
			sanitizeMsg = 'Failed: ' + String(e).replace(/^API \d+: /, '');
		}
		sanitizing = false;
	}

	/** Set-in-a-let needs the reassignment to publish. */
	function toggle(set: Set<string>, key: string): Set<string> {
		const next = new Set(set);
		if (next.has(key)) next.delete(key);
		else next.add(key);
		return next;
	}

	let confirmClear = false;
	let clearing = false;
	let exportMsg = '';

	let timezone = '';
	let tzSaved = false;
	// ── Discogs (admin, server-wide) ──
	// Server-wide rather than per-user because it is one API credential for the
	// instance, like the metrics token — not something each household member
	// would hold separately.
	let discogsConfigured = false;
	let discogsFromEnv = false;
	let discogsToken = '';
	let discogsKey = '';
	let discogsSecret = '';
	let discogsKind = 'none';
	let discogsMsg = '';
	/** 0=Sun..6=Sat. Monday is ISO-8601 and the default; Last.fm's chart week
	 *  runs Friday to Friday, which is why this is settable at all. */
	let weekStart = 1;
	let weekSaved = false;

	// ── Scrobble threshold ──
	// The defaults every source used to hardcode. Held as the numbers the
	// sliders show; `null` would mean "not yet loaded" and the sliders have to
	// start somewhere, so the defaults double as the initial value and as what
	// Reset returns to.
	const THRESHOLD_DEFAULT_PERCENT = 50;
	const THRESHOLD_DEFAULT_SECS = 240;
	let thresholdPercent = THRESHOLD_DEFAULT_PERCENT;
	let thresholdSecs = THRESHOLD_DEFAULT_SECS;
	let thresholdSaved = false;
	let thresholdSaving = false;
	/** True while the sliders sit exactly on what every source did before this
	 *  setting existed — which is worth saying, so nobody has to remember it. */
	$: thresholdIsDefault =
		thresholdPercent === THRESHOLD_DEFAULT_PERCENT && thresholdSecs === THRESHOLD_DEFAULT_SECS;

	/** What the current pair means for one real track length, which is far
	 *  easier to judge than two abstract numbers. */
	function thresholdFor(trackSecs: number): number {
		return Math.min(Math.round(trackSecs * (thresholdPercent / 100)), thresholdSecs);
	}

	async function saveThreshold() {
		thresholdSaving = true;
		try {
			await api.putPreferences({
				scrobble_threshold_percent: thresholdPercent,
				scrobble_threshold_secs: thresholdSecs
			});
			thresholdSaved = true;
			setTimeout(() => (thresholdSaved = false), 2500);
		} catch { /* the sliders keep what was typed; nothing was lost */ }
		thresholdSaving = false;
	}

	function resetThreshold() {
		thresholdPercent = THRESHOLD_DEFAULT_PERCENT;
		thresholdSecs = THRESHOLD_DEFAULT_SECS;
		saveThreshold();
	}

	// language enrichment
	let isAdmin = false;

	// ── Operations ──
	//
	// Built after 2026-08-24, when one unreachable upstream stalled the engine
	// tick from five seconds to ninety minutes: Plex stopped being polled, the
	// deck went dark, and none of that was visible from inside the application.
	// The diagnosis needed SSH and 3 MB of log; the only tool to hand was a
	// force restart, three times.
	let ops: OpsStatus | null = null;
	let opsErr = '';
	let logTail: LogTail | null = null;
	let logLevel = 'info';
	let logQuery = '';
	let logFile = '';
	let logLines = 300;
	let logBusy = false;
	let restarting = false;
	let opsTimer: ReturnType<typeof setInterval> | null = null;

	async function loadOps() {
		try {
			ops = await api.getOps();
			opsErr = '';
		} catch (e) {
			opsErr = e instanceof Error ? e.message : String(e);
		}
	}

	async function loadLog() {
		if (!ops?.logs_enabled) return;
		logBusy = true;
		try {
			logTail = await api.getLogTail({
				lines: logLines,
				level: logLevel,
				q: logQuery || undefined,
				file: logFile || undefined
			});
			if (!logFile) logFile = logTail.file;
		} catch (e) {
			opsErr = e instanceof Error ? e.message : String(e);
		} finally {
			logBusy = false;
		}
	}

	// Polled only while the section is open, and stopped when it is not: this
	// page is the one an admin opens *because* the instance is struggling, so it
	// must not be another thing asking it for work every few seconds.
	$: if (section === 'ops' && isAdmin) {
		if (!opsTimer) {
			loadOps().then(loadLog);
			opsTimer = setInterval(loadOps, 5000);
		}
	} else if (opsTimer) {
		clearInterval(opsTimer);
		opsTimer = null;
	}
	onDestroy(() => {
		if (opsTimer) clearInterval(opsTimer);
	});

	async function doRestart() {
		if (
			!confirm(
				'Restart Tapedeck?\n\nIn-flight requests finish and the engine completes its ' +
					'current tick first, so nothing is cut in half. It comes back only if something ' +
					'is supervising the process — under the supplied systemd unit it does.'
			)
		)
			return;
		restarting = true;
		try {
			await api.restartServer();
		} catch (e) {
			// A dropped connection here is the restart working, not failing.
			console.warn('restart', e);
		}
	}

	/** Seconds, as something readable. Ages here run from ms to hours. */
	function ago(ts: number | null | undefined, now: number): string {
		if (!ts) return 'never';
		const d = Math.max(0, now - ts);
		if (d < 60) return `${d}s ago`;
		if (d < 3600) return `${Math.round(d / 60)}m ago`;
		if (d < 86400) return `${Math.round(d / 3600)}h ago`;
		return `${Math.round(d / 86400)}d ago`;
	}

	function dur(ms: number): string {
		if (ms < 1000) return `${ms}ms`;
		if (ms < 60000) return `${(ms / 1000).toFixed(1)}s`;
		return `${Math.round(ms / 60000)}m`;
	}

	/**
	 * Is a source overdue relative to *its own* interval?
	 *
	 * The comparison that matters, and the one nobody could make on 2026-08-24:
	 * "last polled 78 minutes ago" is unremarkable for a 30-minute source and is
	 * the whole incident for a 5-second one. Three intervals of slack, so an
	 * ordinary jitter is not drawn as a fault.
	 */
	function sourceLate(src: SourceHealth, now: number): boolean {
		if (!src.enabled) return false;
		const iv = src.poll_interval_secs || 30;
		if (!src.last_polled_at) return true;
		return now - src.last_polled_at > Math.max(iv * 3, 60);
	}

	// Ticks so the ages above re-render. Named in the template that uses it,
	// because a reactive expression does not trace a variable through a call.
	let nowSec = Math.floor(Date.now() / 1000);
	setInterval(() => (nowSec = Math.floor(Date.now() / 1000)), 1000);

	let langStatus: { total: number; resolved: number; pending: number } | null = null;
	let enriching = false;
	let langPoll: ReturnType<typeof setInterval> | null = null;
	async function loadLangStatus() {
		try { langStatus = await api.getLanguageStatus(); } catch { /* optional */ }
	}
	async function runEnrichment() {
		enriching = true;
		try {
			await api.backfillLanguages();
			// poll progress until it stops moving
			langPoll = setInterval(async () => {
				const prev = langStatus?.pending ?? -1;
				await loadLangStatus();
				if (langStatus && (langStatus.pending === 0 || langStatus.pending === prev)) {
					if (langPoll) clearInterval(langPoll);
					langPoll = null;
					enriching = false;
				}
			}, 2000);
		} catch (e) { enriching = false; alert(String(e).replace(/^API \d+: /, '')); }
	}
	// MusicBrainz recordings — the pass the others key off. An imported listen
	// never got a recording MBID, because only `flush_pending` ever wrote one.
	let mbidStatus: {
		total: number; resolved: number; pending: number; unmatched_tracks: number;
	} | null = null;
	let fetchingMbids = false;
	let mbidPoll: ReturnType<typeof setInterval> | null = null;
	async function loadMbidStatus() {
		try { mbidStatus = await api.getMbidStatus(); } catch { /* optional */ }
	}
	async function runMbidBackfill() {
		fetchingMbids = true;
		try {
			await api.backfillMbids();
			mbidPoll = setInterval(async () => {
				const prev = mbidStatus?.pending ?? -1;
				await loadMbidStatus();
				// Stops on a pass that resolved nothing, which on a history whose
				// remainder MusicBrainz genuinely does not have is the end rather
				// than a stall — `unmatched_tracks` is what says so on screen.
				if (mbidStatus && (mbidStatus.pending === 0 || mbidStatus.pending === prev)) {
					if (mbidPoll) clearInterval(mbidPoll);
					mbidPoll = null;
					fetchingMbids = false;
				}
			}, 2500);
		} catch (e) { fetchingMbids = false; alert(String(e).replace(/^API \d+: /, '')); }
	}

	// Lyrics — a readout, with no button beside it on purpose. The pass runs
	// only on the engine tick, at the pace LRCLIB asks for, so a "fetch now"
	// control here would let a page refresh drive an unbounded number of
	// requests at a free service. That pacing is the whole of our side of the
	// bargain with them.
	let lyricStatus: {
		available: boolean; why?: string;
		asked?: number; with_lyrics?: number; instrumental?: number;
		unresolved: number;
	} | null = null;
	async function loadLyricStatus() {
		try { lyricStatus = await api.getLyricsStatus(); } catch { /* optional */ }
	}

	// artwork
	let artStatus: { total: number; resolved: number; pending: number } | null = null;
	let fetchingArt = false;
	let artPoll: ReturnType<typeof setInterval> | null = null;
	async function loadArtStatus() {
		try { artStatus = await api.getArtworkStatus(); } catch { /* optional */ }
	}
	async function runArtworkBackfill() {
		fetchingArt = true;
		try {
			await api.backfillArtwork();
			artPoll = setInterval(async () => {
				const prev = artStatus?.pending ?? -1;
				await loadArtStatus();
				// Stop when it's done, or when a pass made no progress at all.
				if (artStatus && (artStatus.pending === 0 || artStatus.pending === prev)) {
					if (artPoll) clearInterval(artPoll);
					artPoll = null;
					fetchingArt = false;
				}
			}, 2500);
		} catch (e) { fetchingArt = false; alert(String(e).replace(/^API \d+: /, '')); }
	}

	// ── Every Noise genre map (admin) ──
	// The site has been frozen since December 2023, so this is a one-time
	// acquisition rather than anything that needs syncing.
	let enStatus: import('$lib/api').EveryNoiseStatus | null = null;
	let enContact = '';
	let enMsg = '';
	let enBusy = false;
	let enPoll: ReturnType<typeof setInterval> | null = null;

	async function loadEveryNoise() {
		try {
			enStatus = await api.getEveryNoise();
			if (enStatus.contact && !enContact) enContact = enStatus.contact;
			// A walk started before this page was opened should still show a bar.
			if (enStatus.progress?.state === 'running' && !enPoll) startEnPoll();
		} catch { /* non-admins simply don't see it */ }
	}

	function startEnPoll() {
		if (enPoll) return;
		enPoll = setInterval(async () => {
			await loadEveryNoise();
			if (enStatus?.progress?.state !== 'running') {
				if (enPoll) clearInterval(enPoll);
				enPoll = null;
				enBusy = false;
			}
		}, 2000);
	}

	async function saveEnContact() {
		try {
			await api.setEveryNoiseContact(enContact.trim());
			enMsg = enContact.trim() ? 'Saved.' : 'Cleared.';
			setTimeout(() => (enMsg = ''), 3000);
			await loadEveryNoise();
		} catch (e) { enMsg = String(e).replace(/^API \d+: /, ''); }
	}

	async function startEnScrape(tier: 'map' | 'pages') {
		// Two hours of requests at someone else's frozen site is worth one
		// deliberate confirmation.
		if (tier === 'pages' && !confirm(
			`This walks every genre page one request per second — about ${Math.ceil((enStatus?.summary.genres || 6300) / 3600)} hours for ${enStatus?.summary.genres || 6300} genres.\n\nIt resumes where it stops, so it's safe to interrupt. Start?`
		)) return;
		enBusy = true;
		enMsg = '';
		try {
			await api.startEveryNoiseScrape(tier);
			await loadEveryNoise();
			startEnPoll();
		} catch (e) {
			enBusy = false;
			enMsg = String(e).replace(/^API \d+: /, '');
		}
	}

	async function cancelEnScrape() {
		try { await api.cancelEveryNoiseScrape(); await loadEveryNoise(); }
		catch (e) { enMsg = String(e).replace(/^API \d+: /, ''); }
	}

	async function clearEveryNoise() {
		if (!confirm('Delete the local genre map? Re-fetching the map is one request; re-walking every genre page is a couple of hours.')) return;
		try { await api.clearEveryNoise(); enMsg = 'Cleared.'; await loadEveryNoise(); }
		catch (e) { enMsg = String(e).replace(/^API \d+: /, ''); }
	}

	// Discogs answers a bad credential with a normal 200, serving the request as
	// anonymous — so "saved" says nothing about whether it works. This asks.
	let discogsTesting = false;
	let discogsTestOk: boolean | null = null;
	async function testDiscogs() {
		discogsTesting = true;
		discogsTestOk = null;
		try {
			const r = await api.testDiscogsSetting();
			discogsTestOk = r.ok;
			discogsMsg = r.message;
		} catch (e) {
			discogsTestOk = false;
			discogsMsg = String(e).replace(/^API \d+: /, '');
		}
		discogsTesting = false;
	}

	async function loadDiscogs() {
		try {
			const d = await api.getDiscogsSetting();
			discogsConfigured = d.configured;
			discogsFromEnv = d.from_env;
			discogsKind = d.kind;
		} catch { /* non-admins simply don't see it */ }
	}

	async function saveDiscogs() {
		try {
			// Only the fields actually filled in are sent, so saving a key/secret
			// pair doesn't wipe a token that was already working.
			const patch: { token?: string; key?: string; secret?: string } = {};
			if (discogsToken.trim()) patch.token = discogsToken.trim();
			if (discogsKey.trim() || discogsSecret.trim()) {
				patch.key = discogsKey.trim();
				patch.secret = discogsSecret.trim();
			}
			// Nothing typed at all means "clear everything".
			if (!Object.keys(patch).length) { patch.token = ''; patch.key = ''; patch.secret = ''; }
			const r = await api.setDiscogsSetting(patch);
			discogsConfigured = r.configured;
			discogsToken = discogsKey = discogsSecret = '';
			discogsMsg = r.configured ? 'Saved — barcode lookups will use Discogs.' : 'Cleared.';
			setTimeout(() => (discogsMsg = ''), 3000);
			await loadDiscogs();
		} catch (e) {
			discogsMsg = String(e).replace(/^API \d+: /, '');
		}
	}

	async function saveWeekStart() {
		try { await api.putPreferences({ week_start: weekStart }); weekSaved = true; setTimeout(() => (weekSaved = false), 2000); }
		catch { /* ignore */ }
	}

	async function saveTimezone() {
		try { await api.putPreferences({ timezone: timezone.trim() }); tzSaved = true; setTimeout(() => (tzSaved = false), 2000); }
		catch { /* ignore */ }
	}

	// Typeahead over the browser's own IANA zone list — no bundled table to go
	// stale. Older engines lack supportedValuesOf; there the field stays free text.
	const allZones: string[] = (() => {
		try { return (Intl as any).supportedValuesOf?.('timeZone') ?? []; }
		catch { return []; }
	})();
	let tzFocused = false;
	let tzIndex = 0;

	function matchZones(query: string, focused: boolean): string[] {
		if (!focused || !allZones.length) return [];
		const q = query.trim().toLowerCase().replace(/\s+/g, '_');
		const pool = q ? allZones.filter((z) => z.toLowerCase().includes(q)) : allZones;
		// An exact hit means they've already chosen — don't hover a list over it.
		if (pool.length === 1 && pool[0].toLowerCase() === q) return [];
		return pool.slice(0, 60);
	}

	/** What a zone actually does, read out of the browser's own tz database
	 *  rather than a table we would have to keep current.
	 *
	 *  Two things this gets right that the obvious version does not.
	 *
	 *  **No single locale names every zone.** `en-GB` gives CET/CEST for Berlin
	 *  but `GMT-5` for New York; `en-US` gives EST/EDT for New York but `GMT+1`
	 *  for Berlin. Those named forms are exactly what people go hunting for in
	 *  the picker, so both are asked and whichever returns a real name wins.
	 *
	 *  **Standard vs daylight is decided by the offset, not by the month.**
	 *  Probing January and July and calling them winter and summer is wrong for
	 *  the entire southern hemisphere — Sydney's January is its summer. The
	 *  larger UTC offset is the shifted one wherever the zone sits. */
	function zoneFacts(name: string) {
		const abbr = (d: Date) => {
			for (const loc of ['en-GB', 'en-US']) {
				const v = new Intl.DateTimeFormat(loc, { timeZone: name, timeZoneName: 'short' })
					.formatToParts(d)
					.find((p) => p.type === 'timeZoneName')?.value;
				// A bare `GMT+2` is a fallback, not a name — keep looking.
				if (v && !/^(GMT|UTC)[+-]/.test(v)) return v;
			}
			return new Intl.DateTimeFormat('en-GB', { timeZone: name, timeZoneName: 'short' })
				.formatToParts(d)
				.find((p) => p.type === 'timeZoneName')?.value ?? '';
		};
		/** Minutes east of UTC, from the offset the browser reports. */
		const offsetMins = (d: Date) => {
			const v = new Intl.DateTimeFormat('en-GB', { timeZone: name, timeZoneName: 'longOffset' })
				.formatToParts(d)
				.find((p) => p.type === 'timeZoneName')?.value ?? '';
			const m = /GMT([+-])(\d{2}):(\d{2})/.exec(v);
			return m ? (m[1] === '-' ? -1 : 1) * (+m[2] * 60 + +m[3]) : 0;
		};
		const offsetText = (d: Date) =>
			new Intl.DateTimeFormat('en-GB', { timeZone: name, timeZoneName: 'longOffset' })
				.formatToParts(d)
				.find((p) => p.type === 'timeZoneName')?.value ?? '';
		try {
			const now = new Date();
			const jan = new Date(Date.UTC(now.getUTCFullYear(), 0, 15));
			const jul = new Date(Date.UTC(now.getUTCFullYear(), 6, 15));
			const janAbbr = abbr(jan);
			if (!janAbbr) return null;
			const julAbbr = abbr(jul);
			// Whichever probe sits further east is the shifted one.
			const janIsDst = offsetMins(jan) > offsetMins(jul);
			return {
				now: abbr(now),
				offset: offsetText(now) || 'GMT+00:00',
				standard: janIsDst ? julAbbr : janAbbr,
				daylight: janIsDst ? janAbbr : julAbbr,
				observesDst: offsetMins(jan) !== offsetMins(jul)
			};
		} catch {
			// An unrecognised zone throws rather than returning anything — that
			// is the signal the field holds half-typed text, not a zone.
			return null;
		}
	}

	$: tzFacts = timezone.trim() ? zoneFacts(timezone.trim()) : null;

	$: tzMatches = matchZones(timezone, tzFocused);
	$: if (tzIndex >= tzMatches.length) tzIndex = 0;

	function pickZone(z: string) {
		timezone = z;
		tzFocused = false;
		saveTimezone();
	}

	function tzKeydown(e: KeyboardEvent) {
		if (!tzMatches.length) return;
		if (e.key === 'ArrowDown') { e.preventDefault(); tzIndex = (tzIndex + 1) % tzMatches.length; }
		else if (e.key === 'ArrowUp') { e.preventDefault(); tzIndex = (tzIndex - 1 + tzMatches.length) % tzMatches.length; }
		else if (e.key === 'Enter') { e.preventDefault(); pickZone(tzMatches[tzIndex]); }
		else if (e.key === 'Escape') { tzFocused = false; }
	}

	// connections
	let connections: Connection[] = [];
	// Whether a play accepted out of a Shared Spool reaches the services above.
	// A *state*, so it is drawn as a segmented pair rather than a checkbox — a
	// checkbox label has to say what ticking does, and "Forward spool listens"
	// beside a box that is already the answer is the ambiguity the
	// Private | Shared pair was built to remove.
	let spoolForward = false;
	let spoolWindowDays = 14;
	let spoolForwardBusy = false;
	let spoolForwardErr = '';
	let lbToken = '';
	let lbBusy = false;
	// Last.fm and Libre.fm share the same approve-then-finish flow, so their
	// in-flight state is keyed by service rather than duplicated.
	type Scrobbler = 'lastfm' | 'librefm';
	let fmBusy: Scrobbler | null = null;
	let fmPendingToken: Record<Scrobbler, string> = { lastfm: '', librefm: '' };
	let connError = '';
	$: conn = (svc: string) => connections.find((c) => c.service === svc) ?? null;

	async function setSpoolForward(on: boolean) {
		// Clicking the lit segment is a no-op rather than a PUT setting the
		// state it is already in — the rule the Private | Shared pair follows.
		if (on === spoolForward || spoolForwardBusy) return;
		spoolForwardBusy = true;
		spoolForwardErr = '';
		try {
			const r = await api.setSpoolForwarding(on);
			spoolForward = r.enabled;
		} catch (e) {
			spoolForwardErr = e instanceof Error ? e.message : 'Could not save that';
		} finally {
			spoolForwardBusy = false;
		}
	}

	async function loadConnections() {
		try {
			const r = await api.getConnections();
			connections = r.connections;
		} catch { /* optional */ }
		try {
			const f = await api.getSpoolForwarding();
			spoolForward = f.enabled;
			spoolWindowDays = f.window_days;
		} catch { /* optional */ }
		try {
			const t = await api.tidalStatus();
			tidalConfigured = t.configured;
			tidalConnected = t.connected;
		} catch { /* optional */ }
		await loadTidalSettings();
	}

	// TIDAL. `configured` is the instance's app registration, `connected` this
	// user's account — an instance with no client id cannot offer the button at
	// all, which is a different thing from nobody having linked an account yet.
	let tidalConfigured = false;
	let tidalConnected = false;
	let tidalBusy = false;
	let tidalError = '';
	// The server-wide registration (admin). The credential is never sent back,
	// so these start empty and a placeholder says whether one is stored.
	let tidalStored = false;
	let tidalHasSecret = false;
	let tidalFromEnv = false;
	let tidalRedirect = '';
	let tidalClientId = '';
	let tidalClientSecret = '';
	let tidalSaving = false;
	let tidalSaved = '';

	async function loadTidalSettings() {
		if (!isAdmin) return;
		try {
			const t = await api.tidalSettings();
			tidalStored = t.stored;
			tidalHasSecret = t.has_secret;
			tidalFromEnv = t.from_env;
			tidalRedirect = t.redirect_uri;
		} catch { /* optional */ }
	}

	async function saveTidalApp() {
		tidalError = '';
		tidalSaved = '';
		tidalSaving = true;
		try {
			// Only send what was typed: an untouched field must not clear the
			// stored value it is standing in for.
			const body: { client_id?: string; client_secret?: string } = {};
			if (tidalClientId.trim() || !tidalStored) body.client_id = tidalClientId.trim();
			if (tidalClientSecret.trim()) body.client_secret = tidalClientSecret.trim();
			const r = await api.setTidalSettings(body);
			tidalConfigured = r.configured;
			tidalClientId = '';
			tidalClientSecret = '';
			tidalSaved = 'Saved.';
			await loadTidalSettings();
			await loadConnections();
		} catch (e) {
			tidalError = e instanceof Error ? e.message : String(e);
		} finally {
			tidalSaving = false;
		}
	}

	async function clearTidalApp() {
		tidalError = '';
		tidalSaving = true;
		try {
			await api.setTidalSettings({ client_id: '', client_secret: '' });
			tidalClientId = '';
			tidalClientSecret = '';
			tidalSaved = 'Cleared.';
			await loadTidalSettings();
			await loadConnections();
		} catch (e) {
			tidalError = e instanceof Error ? e.message : String(e);
		} finally {
			tidalSaving = false;
		}
	}

	async function connectTidal() {
		tidalError = '';
		tidalBusy = true;
		try {
			const { authorize_url } = await api.tidalConnect();
			// A new tab rather than a redirect, so the settings page keeps its
			// state — the same shape the Last.fm connect flow uses.
			window.open(authorize_url, '_blank', 'noopener');
		} catch (e) {
			tidalError = e instanceof Error ? e.message : String(e);
		} finally {
			tidalBusy = false;
		}
	}

	async function disconnectTidal() {
		tidalError = '';
		tidalBusy = true;
		try {
			await api.tidalDisconnect();
			tidalConnected = false;
		} catch (e) {
			tidalError = e instanceof Error ? e.message : String(e);
		} finally {
			tidalBusy = false;
		}
	}
	async function connectLB() {
		connError = '';
		if (!lbToken.trim()) return;
		lbBusy = true;
		try {
			await api.connectListenBrainz(lbToken.trim());
			lbToken = '';
			await loadConnections();
		} catch (e) { connError = String(e).replace(/^API \d+: /, ''); }
		lbBusy = false;
	}
	async function scrobblerConnect(svc: Scrobbler) {
		connError = '';
		fmBusy = svc;
		try {
			if (!fmPendingToken[svc]) {
				const { token, auth_url } = await api.scrobblerStart(svc);
				fmPendingToken[svc] = token;
				window.open(auth_url, '_blank', 'noopener');
			} else {
				await api.scrobblerComplete(svc, fmPendingToken[svc]);
				fmPendingToken[svc] = '';
				await loadConnections();
			}
		} catch (e) { connError = String(e).replace(/^API \d+: /, ''); }
		fmBusy = null;
	}
	async function disconnectSvc(svc: string) {
		try { await api.disconnect(svc); await loadConnections(); } catch (e) { connError = String(e).replace(/^API \d+: /, ''); }
		if (svc === 'lastfm' || svc === 'librefm') fmPendingToken[svc as Scrobbler] = '';
	}

	const themes: { v: ThemeMode; label: string }[] = [
		{ v: 'light', label: 'Light' },
		{ v: 'dark', label: 'Dark' },
		{ v: 'system', label: 'System' }
	];

	function sectionFromHash() {
		const h = window.location.hash.slice(1);
		if (SECTIONS.some((s) => s.id === h)) section = h;
	}

	// ── Sharing: the one thing here that leaves the instance ──
	//
	// Every other read path in Tapedeck is self-only. This screen is where that
	// stops being true, so it is written to make the disclosure legible rather
	// than to make it convenient: the master switch and the four field toggles
	// are separate answers, and the page states in words what each one puts on
	// the internet.
	let publish: PublishSwitch | null = null;
	let publishLoading = false;
	let publishSaving = false;
	let publishSaved = false;
	let publishCopied = '';

	// The vocabulary is the server's (`known_fields`); this is only the prose
	// for it. A field the server grows without a line here still renders — with
	// its bare name — rather than silently vanishing from the screen while being
	// published, which is the failure that would matter.
	const FIELD_COPY: Record<string, { label: string; sub: string }> = {
		artwork: {
			label: 'Cover art',
			sub: 'The sleeve of whatever is on, resolved from your own listens. Nothing is uploaded — it is a link to the same art the album page shows.'
		},
		position: {
			label: 'How far in',
			sub: 'Lets a widget draw a progress bar. It also shows the moment you pause, and how far into a track you are.'
		},
		fidelity: {
			label: 'Format',
			sub: 'Codec, bit depth, sample rate, lossless. Most listens carry none of this; the ones that do say something about your library.'
		},
		gear: {
			label: 'Signal chain',
			sub: 'What the listen ran through. Only ever set for a scrobble client or a side off the shelf — a polled listen omits it rather than naming a fallback.'
		}
	};

	// ── Patch: whether this deck can be found at all ──
	//
	// Deliberately a **second** switch on this page rather than a field of the
	// one above, because it answers a different question: that one decides
	// whether a stranger can read what is on the deck, this one decides whether
	// the person can be found and patched into. In this design following happens
	// *while* private — a private deck holds patches in the inbox for approval —
	// so somebody can be discoverable and unreadable at the same time, and
	// collapsing the two would make that state unreachable.
	let federation: FederationSwitch | null = null;
	let federationLoading = false;
	let federationSaving = false;
	let federationCopied = '';

	// ── What a patched deck sees ──
	//
	// The third sharing switch, and it answers a third question. The other two
	// are about strangers: whether the deck can be *read* without a password,
	// and whether the account can be *found*. This one is about somebody you
	// have already let in.
	let shareAll: { enabled: boolean } | null = null;
	let shareAllLoading = false;
	let shareAllSaving = false;

	async function loadShareAll() {
		shareAllLoading = true;
		try {
			shareAll = await api.getShareAll();
		} catch {
			/* the section shows its own empty state */
		} finally {
			shareAllLoading = false;
		}
	}

	async function saveShareAll(enabled: boolean) {
		shareAllSaving = true;
		try {
			shareAll = await api.setShareAll(enabled);
		} catch (e) {
			alert(e instanceof Error ? e.message : 'Could not save that');
			await loadShareAll();
		} finally {
			shareAllSaving = false;
		}
	}

	async function loadFederation() {
		federationLoading = true;
		try {
			federation = await api.getFederation();
		} catch {
			/* the section shows its own empty state */
		} finally {
			federationLoading = false;
		}
	}

	async function saveFederation(enabled: boolean) {
		federationSaving = true;
		try {
			federation = await api.setFederation(enabled);
		} catch (e) {
			alert(e instanceof Error ? e.message : 'Could not save');
			// Re-read rather than keeping the optimistic value, for the same reason
			// the switch above does: what is published must not be guessed at.
			await loadFederation();
		} finally {
			federationSaving = false;
		}
	}

	async function copyPatch(text: string, which: string) {
		try {
			await navigator.clipboard.writeText(text);
			federationCopied = which;
			setTimeout(() => (federationCopied = ''), 1600);
		} catch {
			/* a browser that refuses the clipboard is not an error worth a dialog */
		}
	}

	async function loadPublish() {
		publishLoading = true;
		try {
			publish = await api.getPublicNowPlaying();
		} catch {
			/* the section shows its own empty state */
		} finally {
			publishLoading = false;
		}
	}

	async function savePublish(enabled: boolean, fields: string[]) {
		if (!publish) return;
		publishSaving = true;
		publishSaved = false;
		try {
			publish = await api.setPublicNowPlaying({ enabled, fields });
			publishSaved = true;
			setTimeout(() => (publishSaved = false), 2200);
		} catch (e) {
			alert(e instanceof Error ? e.message : 'Could not save');
			// Re-read rather than keeping the optimistic value: what is public
			// is the one setting on this page that must not be guessed at.
			await loadPublish();
		} finally {
			publishSaving = false;
		}
	}

	function toggleField(name: string, on: boolean) {
		if (!publish) return;
		const next = on
			? [...publish.fields, name]
			: publish.fields.filter((f) => f !== name);
		savePublish(publish.enabled, next);
	}

	$: publicUrl = publish ? origin + publish.path : '';

	// The Hugo half. Handed over as text rather than described, because the one
	// thing a shortcode has to get exactly right is the URL, and retyping it is
	// where that goes wrong.
	$: hugoShortcode = `{{< tapedeck-now-playing src="${publicUrl}" >}}`;

	async function copyPublish(what: string, text: string) {
		try {
			await navigator.clipboard.writeText(text);
			publishCopied = what;
			setTimeout(() => (publishCopied = ''), 1600);
		} catch {
			/* clipboard refused — the value is on screen and selectable */
		}
	}

	// ── AI connections (MCP) ──
	//
	// Loaded when the section is first opened rather than on mount: this is one
	// of a dozen sections and most visits never come here.
	$: if (section === 'ai' && !mcpEndpoint && !mcpLoading) loadMcp();
	$: if (section === 'sharing' && !publish && !publishLoading) loadPublish();
	$: if (section === 'sharing' && !federation && !federationLoading) loadFederation();
	$: if (section === 'sharing' && !shareAll && !shareAllLoading) loadShareAll();

	let mcpConnections: McpConnection[] = [];
	let mcpEndpoint = '';
	let mcpScopeCatalogue: { scope: string; description: string }[] = [];
	let mcpAudit: McpAuditCall[] = [];
	let mcpAuditOpen = false;
	let mcpLoading = false;
	let newMcpName = '';
	// Off by default, every one of them. A grant has to be asked for — the same
	// discipline the token scopes follow.
	let newMcpScopes = new Set<string>(['listening:read']);
	let createdMcpToken = '';
	let mcpCopied = false;

	async function loadMcp() {
		mcpLoading = true;
		try {
			const r = await api.getMcpConnections();
			mcpConnections = r.connections;
			mcpEndpoint = r.endpoint;
			mcpScopeCatalogue = r.scopes;
		} catch (e) { console.error(e); }
		mcpLoading = false;
	}

	async function loadMcpAudit() {
		mcpAuditOpen = !mcpAuditOpen;
		if (!mcpAuditOpen) return;
		try { mcpAudit = (await api.getMcpAudit()).calls; } catch (e) { console.error(e); }
	}

	function toggleMcpScope(scope: string) {
		if (newMcpScopes.has(scope)) newMcpScopes.delete(scope);
		else newMcpScopes.add(scope);
		newMcpScopes = newMcpScopes;
	}

	async function createMcp() {
		if (!newMcpName.trim() || !newMcpScopes.size) return;
		try {
			const r = await api.createMcpConnection(newMcpName.trim(), [...newMcpScopes].join(' '));
			createdMcpToken = r.token;
			mcpCopied = false;
			newMcpName = '';
			await loadMcp();
		} catch (e) { alert(e instanceof Error ? e.message : 'Could not create the connection'); }
	}

	async function revokeMcp(c: McpConnection) {
		if (!confirm(`Revoke \u201c${c.name}\u201d? It stops working immediately.`)) return;
		try { await api.revokeMcpConnection(c.id); await loadMcp(); } catch (e) { console.error(e); }
	}

	function copyMcpToken() {
		navigator.clipboard.writeText(createdMcpToken);
		mcpCopied = true;
	}

	onMount(async () => {
		sectionFromHash();
		// A hash-only navigation (a link from elsewhere in the SPA, or Back after
		// clicking through sections) doesn't remount this page, so follow it.
		window.addEventListener('hashchange', sectionFromHash);

		try {
			const [health, tokensRes] = await Promise.all([api.health(), api.getTokens()]);
			backendVersion = health.version;
			tokens = tokensRes.tokens;
		} catch { /* fallback */ }
		// Chains drive the per-token dropdown; an instance with none simply
		// doesn't show it, so this failing is not worth surfacing.
		try { chains = (await api.getChains()).chains; } catch { /* optional */ }
		try { listenCount = (await api.getDashboardStats()).total; } catch { /* optional */ }
		try { noteCount = (await api.getNotes()).total; } catch { /* optional */ }
		try {
			const prefs = await api.getPreferences();
			timezone = (prefs.timezone as string) || Intl.DateTimeFormat().resolvedOptions().timeZone || 'UTC';
			if (typeof prefs.week_start === 'number') weekStart = ((prefs.week_start % 7) + 7) % 7;
			if (typeof prefs.scrobble_threshold_percent === 'number')
				thresholdPercent = Math.min(100, Math.max(5, Math.round(prefs.scrobble_threshold_percent)));
			if (typeof prefs.scrobble_threshold_secs === 'number')
				thresholdSecs = Math.min(3600, Math.max(10, Math.round(prefs.scrobble_threshold_secs)));
		} catch { timezone = 'UTC'; }
		try { isAdmin = (await api.me()).is_admin; } catch { /* optional */ }
		loadProfile();
		loadMbidStatus();
		loadArtStatus();
		loadLyricStatus();
		loadSanitize();
		loadDiscogs();
		loadEveryNoise();
		loadConnections();
		loadLangStatus();
		loading = false;
	});

	onDestroy(() => {
		if (langPoll) clearInterval(langPoll);
		if (artPoll) clearInterval(artPoll);
		if (enPoll) clearInterval(enPoll);
		if (typeof window !== 'undefined') window.removeEventListener('hashchange', sectionFromHash);
	});

	async function createToken() {
		if (!newTokenName.trim()) return;
		creating = true;
		createdToken = '';
		try {
			const result = await api.createToken(newTokenName.trim(), {
				scopes: ['submit', newTokenRead && 'read', newTokenWrite && 'write'].filter(Boolean).join(' '),
				default_chain_id: newTokenChain ? Number(newTokenChain) : null,
				imports_are_live: newTokenLiveImports
			});
			createdToken = result.token;
			newTokenName = '';
			newTokenChain = '';
			newTokenRead = false;
			newTokenWrite = false;
			newTokenLiveImports = false;
			tokens = (await api.getTokens()).tokens;
		} catch (e) { console.error('Create token failed:', e); }
		creating = false;
	}

	/** Repoint a token at a different chain. Optimistic — the row is the only
	 *  place this value is shown, so a failure just puts it back. */
	async function setTokenChain(t: TokenInfo, value: string) {
		const chain_id = value ? Number(value) : null;
		const before = t.default_chain_id;
		tokens = tokens.map((x) => (x.id === t.id ? { ...x, default_chain_id: chain_id } : x));
		try {
			await api.updateToken(t.id, { default_chain_id: chain_id });
		} catch (e) {
			tokens = tokens.map((x) => (x.id === t.id ? { ...x, default_chain_id: before } : x));
			alert('Failed to set chain: ' + e);
		}
	}

	/** Whether this client's `import` submissions are relayed onward. Optimistic
	 *  like the chain picker beside it — the row is the only place it is shown. */
	async function setTokenLiveImports(t: TokenInfo, value: boolean) {
		const before = t.imports_are_live;
		tokens = tokens.map((x) => (x.id === t.id ? { ...x, imports_are_live: value } : x));
		try {
			await api.updateToken(t.id, { imports_are_live: value });
		} catch (e) {
			tokens = tokens.map((x) => (x.id === t.id ? { ...x, imports_are_live: before } : x));
			alert('Failed to change forwarding: ' + e);
		}
	}

	/** Lost the token? Only its hash is stored, so there is nothing to show —
	 *  re-issuing in place is the recovery path. The row keeps its chain. */
	async function resetToken(t: TokenInfo) {
		if (!confirm(`Re-issue "${t.name}"? The old token stops working immediately and you'll need to paste the new one into that client.`)) return;
		try {
			createdToken = (await api.rotateToken(t.id)).token;
			tokens = (await api.getTokens()).tokens;
		} catch (e) { alert('Reset failed: ' + e); }
	}

	async function revokeToken(id: number) {
		if (!confirm('Revoke this token? Any clients using it will stop working.')) return;
		try {
			await api.revokeToken(id);
			tokens = tokens.filter((t) => t.id !== id);
		} catch (e) { console.error('Revoke failed:', e); }
	}

	function copyToken() {
		navigator.clipboard.writeText(createdToken);
		tokenCopied = true;
		setTimeout(() => (tokenCopied = false), 2000);
	}

	async function checkConnection() {
		checking = true;
		healthStatus = '';
		try {
			const h = await api.health();
			healthStatus = h.status === 'ok' ? '✓ Connected' : `⚠ Status: ${h.status}`;
		} catch (e) { healthStatus = `✗ ${e instanceof Error ? e.message : e}`; }
		checking = false;
	}

	async function doExport(format: 'json' | 'csv') {
		try {
			const blob = await api.exportBlob(format);
			const url = URL.createObjectURL(blob);
			const a = document.createElement('a');
			a.href = url;
			a.download = `tapedeck-export.${format}`;
			a.click();
			URL.revokeObjectURL(url);
			exportMsg = `Exported ${format.toUpperCase()}`;
			setTimeout(() => (exportMsg = ''), 3000);
		} catch (e) { exportMsg = 'Export failed'; }
	}

	/** Every liner note as Markdown.
	 *
	 *  Its own control rather than a third format on the listens export: those
	 *  two are the same rows in two shapes, and this is a different thing
	 *  entirely — the writing rather than the record of what played. */
	async function doNotesExport(format: 'zip' | 'md') {
		try {
			const blob = await api.notesExportBlob(format);
			const url = URL.createObjectURL(blob);
			const a = document.createElement('a');
			a.href = url;
			a.download = `tapedeck-liner-notes.${format}`;
			a.click();
			URL.revokeObjectURL(url);
			exportMsg = format === 'zip' ? 'Notes exported as a folder' : 'Notes exported as one file';
			setTimeout(() => (exportMsg = ''), 3000);
		} catch (e) { exportMsg = 'Notes export failed'; }
	}

	async function doBackup() {
		try {
			const blob = await api.backupBlob();
			const url = URL.createObjectURL(blob);
			const a = document.createElement('a');
			a.href = url;
			a.download = 'tapedeck-backup.db';
			a.click();
			URL.revokeObjectURL(url);
			exportMsg = 'Backup downloaded';
			setTimeout(() => (exportMsg = ''), 3000);
		} catch (e) { exportMsg = 'Backup failed (admin only)'; }
	}

	async function doClear() {
		clearing = true;
		try {
			await api.clearScrobbles();
			listenCount = 0;
			confirmClear = false;
		} catch (e) { alert('Clear failed: ' + e); }
		clearing = false;
	}

	async function handleLogout() {
		await api.logout();
		window.location.href = '/login';
	}
</script>

<svelte:head><title>Settings — Tapedeck</title></svelte:head>

<h1 class="h-display" style="font-size:38px;margin-bottom:6px;">Settings</h1>
<p style="margin:0 0 24px;color:var(--td-muted);font-size:15px;">Tokens, connections, and the machinery behind the deck.</p>

<div class="settings-layout">
	<nav class="settings-nav" aria-label="Settings sections">
		{#each SECTIONS.filter((s) => !s.admin || isAdmin) as s}
			<button class="nav-item" class:on={section === s.id} on:click={() => go(s.id)}>
				<span class="nav-label">{s.label}</span>
				<span class="nav-hint">{s.hint}</span>
			</button>
		{/each}
	</nav>

	<div class="settings-body">
		{#if section === 'account'}
			<div class="panel" style="border-radius:22px;padding:24px;margin-bottom:18px;">
				<h3 class="h-display" style="font-size:19px;margin-bottom:4px;">Your account</h3>
				<p style="font-size:13px;color:var(--td-muted);margin:0 0 18px;">
					{#if profile}
						Signed in as <strong>{profile.username}</strong>{profile.role === 'admin' ? ' · admin' : ''}.
					{:else}
						Your username, picture and password.
					{/if}
				</p>

				<div style="display:flex;gap:20px;align-items:flex-start;flex-wrap:wrap;">
					<div style="display:flex;flex-direction:column;align-items:center;gap:10px;">
						{#if profile?.avatar}
							<img
								src={profile.avatar.startsWith('http') ? profile.avatar : `${profile.avatar}?v=${avatarBust}`}
								alt="You"
								style="width:92px;height:92px;border-radius:50%;object-fit:cover;border:1px solid var(--td-border);"
							/>
						{:else}
							<div
								style="width:92px;height:92px;border-radius:50%;display:flex;align-items:center;justify-content:center;
								       background:var(--td-sidebar);border:1px solid var(--td-border);font-size:34px;"
								class="h-display"
								aria-hidden="true"
							>{avatarLetter}</div>
						{/if}
						<div style="display:flex;gap:8px;">
							<label class="btn btn-secondary" style="cursor:pointer;font-size:12px;padding:5px 12px;">
								{avatarBusy ? '…' : 'Change'}
								<input type="file" accept="image/*" on:change={onAvatarPicked} style="display:none;" disabled={avatarBusy} />
							</label>
							{#if profile?.avatar}
								<button class="btn btn-ghost" style="font-size:12px;padding:5px 12px;" on:click={removeAvatar} disabled={avatarBusy}>Remove</button>
							{/if}
						</div>
					</div>

					<div style="flex:1;min-width:260px;display:flex;flex-direction:column;gap:14px;">
						<div>
							<label for="pdn" style="display:block;font-size:13px;margin-bottom:5px;">Display name</label>
							<input id="pdn" class="input" bind:value={pDisplayName} maxlength="80" placeholder={profile?.username ?? ''} />
							<p style="font-size:12px;color:var(--td-muted-2);margin:5px 0 0;">Shown instead of your username. Your username itself can only be changed by an admin.</p>
						</div>
						<div>
							<label for="pbio" style="display:block;font-size:13px;margin-bottom:5px;">Bio</label>
							<textarea id="pbio" class="input" bind:value={pBio} maxlength="500" rows="3"
								placeholder="What you listen to, what you listen on."
								style="resize:vertical;font-family:inherit;"></textarea>
							<p style="font-size:12px;color:var(--td-muted-2);margin:5px 0 0;">{pBio.length}/500</p>
						</div>
						<div style="display:flex;gap:10px;align-items:center;">
							<button class="btn btn-primary" on:click={saveProfile} disabled={savingProfile}>
								{savingProfile ? 'Saving…' : 'Save'}
							</button>
							{#if profileMsg}
								<span style="font-size:13px;color:var(--td-good-fg);background:var(--td-good-bg);padding:6px 13px;border-radius:999px;">✓ {profileMsg}</span>
							{/if}
						</div>
						{#if profileErr}
							<p style="font-size:13px;color:#b2622d;margin:0;">{profileErr}</p>
						{/if}
					</div>
				</div>
			</div>

			<div class="panel" style="border-radius:22px;padding:24px;margin-bottom:18px;">
				<h3 class="h-display" style="font-size:19px;margin-bottom:4px;">Password</h3>
				<p style="font-size:13px;color:var(--td-muted);margin:0 0 16px;">
					Changing this signs out every other browser you are signed in on — but not this one.
				</p>
				<div style="display:flex;flex-direction:column;gap:12px;max-width:360px;">
					<div>
						<label for="cpw" style="display:block;font-size:13px;margin-bottom:5px;">Current password</label>
						<input id="cpw" class="input" type="password" autocomplete="current-password" bind:value={curPw} />
					</div>
					<div>
						<label for="npw" style="display:block;font-size:13px;margin-bottom:5px;">New password</label>
						<input id="npw" class="input" type="password" autocomplete="new-password" bind:value={newPw} />
					</div>
					<div>
						<label for="npw2" style="display:block;font-size:13px;margin-bottom:5px;">Confirm new password</label>
						<input id="npw2" class="input" type="password" autocomplete="new-password" bind:value={confirmPw} />
					</div>
					<div style="display:flex;gap:10px;align-items:center;">
						<button class="btn btn-primary" on:click={savePassword} disabled={savingPw || !curPw || !newPw}>
							{savingPw ? 'Changing…' : 'Change password'}
						</button>
					</div>
					{#if pwMsg}
						<p style="font-size:13px;color:var(--td-good-fg);background:var(--td-good-bg);padding:8px 13px;border-radius:12px;margin:0;">✓ {pwMsg}</p>
					{/if}
					{#if pwErr}
						<p style="font-size:13px;color:#b2622d;margin:0;">{pwErr}</p>
					{/if}
				</div>
			</div>
		{/if}

		{#if section === 'sharing'}
			<!-- Three switches, three different questions, and the copy has to
			     carry the difference or they read as one feature built thrice.
			     This one first, because it is the one about people you know. -->
			<div class="panel" style="border-radius:22px;padding:24px;margin-bottom:18px;">
				<h3 class="h-display" style="font-size:19px;margin-bottom:4px;">What patched decks see</h3>
				<p style="font-size:13px;color:var(--td-muted);margin:0 0 6px;max-width:64ch;">
					Somebody you have patched with has already been let in — you approved them. This
					decides how much of your listening they see once they are.
				</p>
				<p style="font-size:12.5px;color:var(--td-muted-3);margin:0 0 18px;max-width:64ch;">
					It changes nothing for anyone else. The instance-wide figures still count only
					listens through chains you share, and a Shared Spool still captures only those —
					a capture lands in somebody's history, which is a good deal more than being seen.
				</p>

				{#if shareAllLoading && !shareAll}
					<p style="font-size:13px;color:var(--td-muted-3);">Loading…</p>
				{:else if !shareAll}
					<p style="font-size:13px;color:var(--td-muted-3);">Couldn't load this setting.</p>
				{:else}
					<div class="row">
						<div class="row-main">
							<div class="row-title">Show them everything I play</div>
							<div class="row-sub">
								Off, they see only what runs through chains you have shared — which
								leaves out anything played on gear you keep private, and everything
								with no chain recorded at all. On, they see every listen, and a chain
								you keep private simply reads <em>Private signal chain</em> instead of
								its name.
							</div>
						</div>
						<label style="display:flex;align-items:center;gap:8px;cursor:pointer;font-size:14px;white-space:nowrap;">
							<input type="checkbox" checked={shareAll.enabled} disabled={shareAllSaving}
								on:change={(e) => saveShareAll(e.currentTarget.checked)}
								style="width:16px;height:16px;accent-color:var(--td-accent);cursor:pointer;">
							{shareAllSaving ? 'Saving…' : shareAll.enabled ? 'On' : 'Off'}
						</label>
					</div>
					<p style="font-size:12px;color:var(--td-muted-3);margin:12px 0 0;max-width:64ch;">
						Nothing is copied anywhere, so turning this off takes it all back — unlike a
						spool capture, which somebody has already accepted into their own history.
					</p>
				{/if}
			</div>

			<div class="panel" style="border-radius:22px;padding:24px;margin-bottom:18px;">
				<h3 class="h-display" style="font-size:19px;margin-bottom:4px;">Public now-playing</h3>
				<p style="font-size:13px;color:var(--td-muted);margin:0 0 6px;max-width:64ch;">
					Everything else in Tapedeck is yours alone — every page, every API route, every
					number. This is the one address that answers without a password, so that a widget
					on your own website can show what is on the deck right now.
				</p>
				<p style="font-size:12.5px;color:var(--td-muted-3);margin:0 0 18px;max-width:64ch;">
					It publishes <strong>one track</strong> and never your history. There is no route
					from here to a listen, a count, a note or a love.
				</p>

				{#if publishLoading && !publish}
					<p style="font-size:13px;color:var(--td-muted-3);">Loading…</p>
				{:else if !publish}
					<p style="font-size:13px;color:var(--td-muted-3);">Couldn't load this setting.</p>
				{:else}
					<div class="row">
						<div class="row-main">
							<div class="row-title">Publish my now-playing</div>
							<div class="row-sub">
								Anyone who knows your username can read it. The address is
								<code>/public/np/{publish.path.split('/').pop()}</code>, so treat it as
								genuinely public rather than merely unlisted.
							</div>
						</div>
						<label style="display:flex;align-items:center;gap:8px;cursor:pointer;font-size:14px;white-space:nowrap;">
							<input type="checkbox" checked={publish.enabled} disabled={publishSaving}
								on:change={(e) => savePublish(e.currentTarget.checked, publish?.fields ?? [])}
								style="width:16px;height:16px;accent-color:var(--td-accent);cursor:pointer;">
							{publish.enabled ? 'On' : 'Off'}
						</label>
					</div>

					{#if publish.enabled}
						<div style="margin-top:18px;padding-top:18px;border-top:1px solid var(--td-border);">
							<div style="font-size:14px;font-weight:600;margin-bottom:2px;">What it carries</div>
							<p style="font-size:12.5px;color:var(--td-muted);margin:0 0 12px;max-width:64ch;">
								The track, the artist and the record are always included — that is what
								publishing means. Everything below is a separate answer, and each one is
								off until you say otherwise.
							</p>

							{#each publish.known_fields as f}
								<div class="row">
									<div class="row-main">
										<div class="row-title">{FIELD_COPY[f]?.label ?? f}</div>
										<div class="row-sub">{FIELD_COPY[f]?.sub ?? ''}</div>
									</div>
									<label style="display:flex;align-items:center;gap:8px;cursor:pointer;font-size:13px;white-space:nowrap;">
										<input type="checkbox" checked={publish.fields.includes(f)}
											disabled={publishSaving}
											on:change={(e) => toggleField(f, e.currentTarget.checked)}
											style="width:16px;height:16px;accent-color:var(--td-accent);cursor:pointer;">
										{publish.fields.includes(f) ? 'Public' : 'Private'}
									</label>
								</div>
							{/each}
						</div>

						<div style="margin-top:18px;padding-top:18px;border-top:1px solid var(--td-border);">
							<div style="font-size:14px;font-weight:600;margin-bottom:8px;">Your address</div>
							<div class="copyrow">
								<code>{publicUrl}</code>
								<button class="btn btn-secondary" style="font-size:12.5px;"
									on:click={() => copyPublish('url', publicUrl)}>
									{publishCopied === 'url' ? '✓ Copied' : 'Copy'}
								</button>
							</div>
							<p style="font-size:12px;color:var(--td-muted-3);margin:8px 0 0;max-width:64ch;">
								Sends <code>Access-Control-Allow-Origin: *</code>, so a page on any domain
								can read it. A username that doesn't exist and one that doesn't publish
								both answer 404 — nobody can use this to find out who has an account here.
							</p>

							<div style="font-size:14px;font-weight:600;margin:18px 0 8px;">On a Hugo site</div>
							<div class="copyrow">
								<code>{hugoShortcode}</code>
								<button class="btn btn-secondary" style="font-size:12.5px;"
									on:click={() => copyPublish('hugo', hugoShortcode)}>
									{publishCopied === 'hugo' ? '✓ Copied' : 'Copy'}
								</button>
							</div>
							<p style="font-size:12px;color:var(--td-muted-3);margin:8px 0 0;max-width:64ch;">
								Needs <code>layouts/shortcodes/tapedeck-now-playing.html</code> in your
								site — it ships with Tapedeck under <code>integrations/hugo/</code>.
							</p>
						</div>
					{/if}

					<div style="display:flex;gap:10px;align-items:center;margin-top:16px;">
						{#if publishSaved}<span style="color:var(--td-good-fg);font-size:13px;">✓ Saved</span>
						{:else if !publish.enabled}<span style="color:var(--td-muted-3);font-size:13px;">Nothing is public.</span>{/if}
					</div>
				{/if}
			</div>

			<!--
				A second panel, not a field of the one above. The two answer
				different questions and the copy has to carry that, or this page
				reads as one feature built twice: the switch above decides
				whether a stranger can *read* the deck, this one decides whether
				the person can be *found*. Following happens while private here,
				so being discoverable and being readable are genuinely separate
				states and folding them would make one of them unreachable.
			-->
			<div class="panel" style="border-radius:22px;padding:24px;margin-bottom:18px;">
				<h3 class="h-display" style="font-size:19px;margin-bottom:4px;">Patch — being findable</h3>
				<p style="font-size:13px;color:var(--td-muted);margin:0 0 6px;max-width:64ch;">
					A different question from the switch above. That one decides whether anyone can
					<em>read</em> what is on your deck. This one decides whether you can be
					<em>found</em> — whether someone on Mastodon or another Tapedeck can look up your
					handle and ask to patch into your deck.
				</p>
				<p style="font-size:12.5px;color:var(--td-muted-3);margin:0 0 18px;max-width:64ch;">
					Being findable does not publish your listening. A patch is a
					<strong>request</strong> that waits for you to say yes, so your deck stays private
					whether or not anyone is following it. Nothing is sent out to other servers yet —
					this is the half that answers when they ask.
				</p>

				{#if federationLoading && !federation}
					<p style="font-size:13px;color:var(--td-muted-3);">Loading…</p>
				{:else if !federation}
					<p style="font-size:13px;color:var(--td-muted-3);">Couldn't load this setting.</p>
				{:else}
					<div class="row">
						<div class="row-main">
							<div class="row-title">Let other servers find me</div>
							<div class="row-sub">
								Publishes your handle, display name, bio and profile picture to anyone who
								asks for them, and gives this account a signing key. Off until you say
								otherwise — a username that hasn't switched this on answers exactly like
								one that doesn't exist.
							</div>
						</div>
						<label style="display:flex;align-items:center;gap:8px;cursor:pointer;font-size:14px;white-space:nowrap;">
							<input type="checkbox" checked={federation.enabled} disabled={federationSaving}
								on:change={(e) => saveFederation(e.currentTarget.checked)}
								style="width:16px;height:16px;accent-color:var(--td-accent);cursor:pointer;">
							{federationSaving ? 'Saving…' : federation.enabled ? 'On' : 'Off'}
						</label>
					</div>

					{#if federation.enabled}
						<div style="margin-top:18px;padding-top:18px;border-top:1px solid var(--td-border);">
							<div style="font-size:14px;font-weight:600;margin-bottom:8px;">Your handle</div>
							<div class="copyrow">
								<code>{federation.handle}</code>
								<button class="btn btn-secondary" style="font-size:12.5px;"
									on:click={() => copyPatch(federation?.handle ?? '', 'handle')}>
									{federationCopied === 'handle' ? '✓ Copied' : 'Copy'}
								</button>
							</div>
							<p style="font-size:12px;color:var(--td-muted-3);margin:8px 0 0;max-width:64ch;">
								This is what someone types into another server's search box.
							</p>

							{#if federation.actor}
								<div style="font-size:14px;font-weight:600;margin:18px 0 8px;">Your actor</div>
								<div class="copyrow">
									<code>{federation.actor}</code>
									<button class="btn btn-secondary" style="font-size:12.5px;"
										on:click={() => copyPatch(federation?.actor ?? '', 'actor')}>
										{federationCopied === 'actor' ? '✓ Copied' : 'Copy'}
									</button>
								</div>
								<p style="font-size:12px;color:var(--td-muted-3);margin:8px 0 0;max-width:64ch;">
									The address other servers store for you. It is fixed at this value now,
									even if the instance's public URL changes later — to anyone holding it,
									a change wouldn't be a rename, it would be you disappearing and a
									stranger arriving.
								</p>
							{/if}

							{#if federation.frozen_at}
								<p style="font-size:12px;color:var(--td-muted-3);margin:14px 0 0;max-width:64ch;">
									Your username is now fixed: another server is holding this identity, so
									renaming would break it for them.
								</p>
							{/if}
						</div>
					{:else}
						<p style="font-size:12.5px;color:var(--td-muted-3);margin-top:14px;max-width:64ch;">
							Nobody can find you. Turning this on creates a signing key for this account —
							switching it off later stops answering but keeps the key, so anyone who
							already patched in still recognises you if you come back.
						</p>
					{/if}
				{/if}
			</div>
		{/if}

		{#if section === 'appearance'}
			<div class="panel" style="border-radius:22px;padding:24px;margin-bottom:18px;">
				<h3 class="h-display" style="font-size:19px;margin-bottom:4px;">Appearance</h3>
				<p style="font-size:13px;color:var(--td-muted);margin:0 0 16px;">Choose a theme, or follow your system. Saved to your profile so every device matches.</p>
				<div style="display:flex;background:var(--td-sidebar);border-radius:999px;padding:4px;gap:2px;max-width:360px;">
					{#each themes as o}
						<button class="seg-btn" class:on={$theme === o.v} on:click={() => setTheme(o.v)} style="flex:1;">{o.label}</button>
					{/each}
				</div>

				<div style="margin-top:18px;padding-top:18px;border-top:1px solid var(--td-border);">
					<label style="display:flex;align-items:center;gap:10px;cursor:pointer;font-size:14px;">
						<input type="checkbox" checked={$showArtwork} on:change={(e) => setShowArtwork(e.currentTarget.checked)}
							style="width:16px;height:16px;accent-color:var(--td-accent);cursor:pointer;">
						Show album art
					</label>
					<p style="font-size:12.5px;color:var(--td-muted);margin:6px 0 0;">
						Covers come from your own library where the listen came from one, then Deezer, then the
						Cover Art Archive. Listens with no match anywhere keep the lettered tile.
						{#if artStatus}
							<a href="#maintenance" on:click|preventDefault={() => go('maintenance')}>Fetch missing art →</a>
						{/if}
					</p>
				</div>

			</div>
		{/if}

		{#if section === 'time'}
			<div class="panel" style="border-radius:22px;padding:24px;margin-bottom:18px;">
				<h3 class="h-display" style="font-size:19px;margin-bottom:4px;">Time</h3>
				<p style="font-size:13px;color:var(--td-muted);margin:0 0 16px;max-width:62ch;">
					Every date in Tapedeck is stored in UTC and displayed in your zone — which is what
					makes “today”, the listening clock and the yearly trajectory line up with the day
					you actually had.
				</p>
				<div>
					<div class="field" style="max-width:360px;"><label for="tz">Timezone <span style="opacity:.6">(for local-time analytics)</span></label>
						<div style="display:flex;gap:8px;position:relative;">
							<div style="flex:1;position:relative;">
								<input
									id="tz" class="input" bind:value={timezone} placeholder="Type to search — e.g. Berlin"
									autocomplete="off" role="combobox" aria-expanded={tzMatches.length > 0} aria-controls="tz-list"
									on:focus={() => (tzFocused = true)}
									on:blur={() => setTimeout(() => (tzFocused = false), 120)}
									on:keydown={tzKeydown}
								>
								{#if tzMatches.length}
									<ul id="tz-list" class="tz-list">
										{#each tzMatches as z, i}
											<li>
												<button
													type="button"
													class="tz-option"
													class:active={i === tzIndex}
													on:mouseenter={() => (tzIndex = i)}
													on:click={() => pickZone(z)}
												>{z.replace(/_/g, ' ')}</button>
											</li>
										{/each}
									</ul>
								{/if}
							</div>
							<button class="btn btn-secondary" on:click={saveTimezone}>{tzSaved ? '✓' : 'Save'}</button>
						</div>
					</div>

					{#if tzFacts}
						<!-- The question this answers is "why can't I pick CET or CEST?".
						     Because those are the two halves of one zone, and picking
						     either would freeze the offset and shift half the year's
						     listening. Naming the region and showing it switching is a
						     shorter answer than a paragraph about it. -->
						<p class="tz-note">
							Currently <strong>{tzFacts.now}</strong> ({tzFacts.offset}).
							{#if tzFacts.observesDst}
								This region switches between {tzFacts.standard} and {tzFacts.daylight}, and
								Tapedeck follows that on the right dates by itself. It is why the list holds
								regions like <code>Europe/Berlin</code> rather than CET and CEST — those are
								the two halves of one zone, and pinning either would put half your year an
								hour out.
							{:else}
								This region does not observe daylight saving, so the offset never moves.
							{/if}
						</p>
					{:else if timezone}
						<p class="tz-note" style="color:var(--td-muted-3);">
							Not a zone this browser recognises — pick one from the list.
						</p>
					{/if}

					<div class="row">
						<div class="row-main">
							<div class="row-title">Week starts on</div>
							<div class="row-sub">
								Sets the “this week” counter on the dashboard and the row order of the
								listening heatmap.
							</div>
						</div>
						<div style="display:flex;gap:8px;align-items:center;">
							<select class="input" style="width:auto;" bind:value={weekStart} on:change={saveWeekStart}>
								<option value={1}>Monday</option>
								<option value={0}>Sunday</option>
								<option value={5}>Friday</option>
								<option value={6}>Saturday</option>
							</select>
							{#if weekSaved}<span style="color:var(--td-good-fg);">✓</span>{/if}
						</div>
					</div>
				</div>
			</div>
		{/if}

		{#if section === 'scrobbling'}
			<div class="panel" style="border-radius:22px;padding:24px;margin-bottom:18px;">
				<h3 class="h-display" style="font-size:19px;margin-bottom:4px;">When a listen counts</h3>
				<p style="font-size:13px;color:var(--td-muted);margin:0 0 6px;max-width:64ch;">
					A track is banked once <strong>either</strong> of these is reached — whichever comes
					first. The defaults are Last.fm's convention, which suits a three-minute song and
					suits a forty-minute raga rather less.
				</p>
				<p style="font-size:12.5px;color:var(--td-muted-3);margin:0 0 18px;max-width:64ch;">
					Applies to sources that watch playback as it happens — Plex, Jellyfin, Emby and
					Roon. A scrobble client decides for itself and tells Tapedeck the answer, and an
					import is reading listens somebody else already counted.
				</p>

				<div class="row">
					<div class="row-main">
						<div class="row-title">Share of the track — {thresholdPercent}%</div>
						<div class="row-sub">
							What makes a short track count at all. At 100% only a track played to the end
							is banked.
						</div>
					</div>
					<input type="range" class="slider" min="5" max="100" step="5"
						bind:value={thresholdPercent} on:change={saveThreshold}
						aria-label="Share of the track that must play">
				</div>

				<div class="row">
					<div class="row-main">
						<div class="row-title">Or after — {formatDuration(thresholdSecs)}</div>
						<div class="row-sub">
							What stops a long piece needing half of itself first. Also the only rule
							available when a source reports no track length at all.
						</div>
					</div>
					<input type="range" class="slider" min="30" max="1200" step="30"
						bind:value={thresholdSecs} on:change={saveThreshold}
						aria-label="Seconds after which a track counts regardless">
				</div>

				<!-- Two abstract numbers are hard to judge; three real records are not. -->
				<div class="examples">
					<div class="ex"><span>A 3:00 song</span><strong>{formatDuration(thresholdFor(180))}</strong></div>
					<div class="ex"><span>A 9:04 track</span><strong>{formatDuration(thresholdFor(544))}</strong></div>
					<div class="ex"><span>A 40:00 raga</span><strong>{formatDuration(thresholdFor(2400))}</strong></div>
				</div>

				<div style="display:flex;gap:10px;align-items:center;margin-top:16px;flex-wrap:wrap;">
					<button class="btn btn-secondary" on:click={resetThreshold} disabled={thresholdIsDefault || thresholdSaving}>
						Back to the defaults
					</button>
					{#if thresholdSaved}<span style="color:var(--td-good-fg);font-size:13px;">✓ Saved</span>
					{:else if thresholdIsDefault}<span style="color:var(--td-muted-3);font-size:13px;">Currently the standard 50% or 4:00.</span>{/if}
				</div>

				<p style="font-size:12px;color:var(--td-muted-3);margin:14px 0 0;max-width:64ch;">
					A change reaches the running sources within a minute. Anything already past the
					old threshold has been banked and stays banked.
				</p>
			</div>
		{/if}

		{#if section === 'sources'}
			<SourcesPanel />
		{/if}

		{#if section === 'connections'}
			<div class="panel" style="border-radius:22px;padding:24px;margin-bottom:18px;">
				<h3 class="h-display" style="font-size:19px;margin-bottom:4px;">TIDAL</h3>
				<p style="font-size:13px;color:var(--td-muted);margin:0 0 14px;">
					Somewhere to send a playlist. Tapedeck matches each track against
					TIDAL's catalogue and creates the playlist on your account —
					<strong>unlisted</strong>, never public. It does not read your TIDAL
					listening, and nothing is forwarded here.
				</p>
				{#if isAdmin}
					<!-- The registration is the server's, not this user's: everyone on
					     the instance connects their own account through the one app. -->
					<div style="border:1px solid var(--td-border);border-radius:14px;padding:14px;margin-bottom:14px;">
						<p style="font-size:12.5px;color:var(--td-muted);margin:0 0 10px;">
							<strong>Server setup.</strong> Register an app at
							<code>developer.tidal.com</code> and add this exact redirect URI:
						</p>
						<p style="font-size:12px;margin:0 0 12px;">
							<code>{tidalRedirect || origin + '/api/v1/tidal/callback'}</code>
						</p>
						<div style="display:flex;gap:10px;align-items:center;flex-wrap:wrap;">
							<input class="input" type="text" bind:value={tidalClientId} style="max-width:220px;"
								placeholder={tidalStored ? '••••••  (client id stored)' : 'Client ID'}>
							<input class="input" type="password" bind:value={tidalClientSecret} style="max-width:220px;"
								placeholder={tidalHasSecret ? '••••••  (secret stored)' : 'Client secret'}>
							<button class="btn btn-secondary" on:click={saveTidalApp} disabled={tidalSaving}>
								{tidalSaving ? 'Saving…' : 'Save'}
							</button>
							{#if tidalStored}
								<button class="btn btn-secondary" on:click={clearTidalApp} disabled={tidalSaving}>
									Clear
								</button>
							{/if}
						</div>
						{#if tidalSaved}
							<p style="font-size:12.5px;color:var(--td-accent);margin:10px 0 0;">{tidalSaved}</p>
						{/if}
						{#if tidalFromEnv && !tidalStored}
							<p style="font-size:12.5px;color:var(--td-muted-2);margin:10px 0 0;">
								Currently using the client id from the environment. Saving one here
								takes precedence over it.
							</p>
						{/if}
						{#if tidalStored}
							<p style="font-size:12.5px;color:var(--td-muted-2);margin:10px 0 0;">
								Changing the client id disconnects every account on this instance —
								tokens issued by the old app do not work with a new one.
							</p>
						{/if}
					</div>
				{/if}

				{#if !tidalConfigured}
					<p style="font-size:12.5px;color:var(--td-muted-2);margin:0;">
						{isAdmin
							? 'Add a client ID above to switch this on.'
							: 'Not configured on this server yet — an admin needs to register an app first.'}
					</p>
				{:else}
					<div style="display:flex;gap:10px;align-items:center;flex-wrap:wrap;">
						{#if tidalConnected}
							<span style="font-size:13px;color:var(--td-accent);">Connected</span>
							<button class="btn btn-secondary" on:click={disconnectTidal} disabled={tidalBusy}>
								Disconnect
							</button>
						{:else}
							<button class="btn btn-secondary" on:click={connectTidal} disabled={tidalBusy}>
								{tidalBusy ? 'Opening…' : 'Connect TIDAL'}
							</button>
							<span style="font-size:12.5px;color:var(--td-muted-2);">
								Opens TIDAL in a new tab. Come back here when it sends you back.
							</span>
						{/if}
					</div>
					{#if tidalError}
						<p style="font-size:12.5px;color:var(--td-love);margin:10px 0 0;">{tidalError}</p>
					{/if}
				{/if}
			</div>

			{#if isAdmin}
				<div class="panel" style="border-radius:22px;padding:24px;margin-bottom:18px;">
					<h3 class="h-display" style="font-size:19px;margin-bottom:4px;">Discogs</h3>
					<p style="font-size:13px;color:var(--td-muted);margin:0 0 14px;">
						Used to identify a pressing from its barcode on the Vinyl &amp; Tape
						screen. Optional — MusicBrainz works without it, but Discogs is a
						collector's database and usually lands on the exact edition in your
						hands. One credential for the whole server.
					</p>
					<p style="font-size:12.5px;color:var(--td-muted-2);margin:0 0 14px;">
						Discogs issues two kinds and <strong>either works</strong>. A personal access
						token is one string; registering an application gives a consumer key and
						secret. Fill in whichever you have — the OAuth flow isn't needed, since
						Tapedeck only reads the public database.
					</p>

					<div style="display:flex;gap:10px;align-items:center;flex-wrap:wrap;margin-bottom:10px;">
						<input class="input" type="password" bind:value={discogsToken} style="max-width:320px;"
							placeholder={discogsKind === 'token' ? '••••••••  (token stored)' : 'Personal access token'}>
					</div>
					<div style="display:flex;gap:10px;align-items:center;flex-wrap:wrap;">
						<input class="input" type="password" bind:value={discogsKey} style="max-width:190px;"
							placeholder={discogsKind === 'key_secret' ? '••••••  (key stored)' : 'Consumer key'}>
						<input class="input" type="password" bind:value={discogsSecret} style="max-width:190px;"
							placeholder={discogsKind === 'key_secret' ? '••••••  (secret stored)' : 'Consumer secret'}>
						<button class="btn btn-secondary" on:click={saveDiscogs}>
							{discogsToken.trim() || discogsKey.trim() || discogsSecret.trim() ? 'Save' : 'Clear'}
						</button>
						{#if discogsConfigured}
							<button class="btn btn-secondary" on:click={testDiscogs} disabled={discogsTesting}>
								{discogsTesting ? 'Testing…' : 'Test'}
							</button>
						{/if}
					</div>
					{#if discogsMsg}
						<p style="font-size:13px;margin:10px 0 0;color:{discogsTestOk === false ? 'var(--td-bad-fg)' : 'var(--td-good-fg)'};">
							{discogsMsg}
						</p>
					{/if}
					<p style="font-size:12.5px;color:var(--td-muted-2);margin:10px 0 0;">
						{#if discogsFromEnv && discogsKind === 'key_secret'}
							Currently supplied by the environment as a consumer key and secret. Saving
							one here overrides it.
						{:else if discogsFromEnv}
							Currently supplied by TAPEDECK_DISCOGS_TOKEN. Saving one here overrides it.
						{:else if discogsConfigured}
							Configured ({discogsKind === 'key_secret' ? 'consumer key and secret' : 'personal access token'}).
							Barcode lookups ask Discogs first, at 60 requests a minute.
						{:else}
							Not configured — lookups use MusicBrainz only, which has no barcode
							coverage for many pressings.
						{/if}
					</p>
				</div>
			{/if}

			<div class="panel" style="border-radius:22px;padding:24px;margin-bottom:18px;">
				<h3 class="h-display" style="font-size:19px;margin-bottom:4px;">Scrobble Connections</h3>
				<p style="font-size:13px;color:var(--td-muted);margin:0 0 16px;">Forward <em>your</em> listens to your own accounts. Nothing is shared across users, and a service you don't connect simply isn't forwarded to — listens are always kept locally either way.</p>

				{#if connError}<div style="font-size:13px;color:#b2622d;margin-bottom:12px;">{connError}</div>{/if}

				{#if connections.length}
					<!--
						Shown only when there is somewhere to forward *to*. With no
						connections it is a control with no effect, which reads as a
						setting that does not work — the same call the Pull loves row
						below already makes.
					-->
					<div style="display:flex;align-items:center;gap:12px;flex-wrap:wrap;padding:12px 14px;margin-bottom:14px;background:var(--td-surface2);border:1px solid var(--td-border);border-radius:14px;">
						<div style="flex:1;min-width:220px;">
							<div style="font-weight:600;font-size:14px;">Plays from a Shared Spool</div>
							<div style="font-size:12.5px;color:var(--td-muted-2);margin-top:2px;">
								When you accept a play someone else's deck captured for you, this
								decides whether it reaches the services above. You were in the room
								and you accepted it one play at a time — but it is still someone
								else's turntable writing to your public record, so it is off until
								you say otherwise. Anything older than {spoolWindowDays} days is kept
								here whatever this says, so handing over an old evening can't rewrite
								your history.
							</div>
							{#if spoolForwardErr}
								<div style="font-size:12.5px;color:#b2622d;margin-top:6px;">{spoolForwardErr}</div>
							{/if}
						</div>
						<div style="display:flex;background:var(--td-sidebar);border-radius:999px;padding:4px;gap:2px;">
							<button class="seg-btn" class:on={!spoolForward} disabled={spoolForwardBusy}
								on:click={() => setSpoolForward(false)}>Kept here</button>
							<button class="seg-btn" class:on={spoolForward} disabled={spoolForwardBusy}
								on:click={() => setSpoolForward(true)}>Forwarded on</button>
						</div>
					</div>

					<div style="display:flex;align-items:center;gap:12px;flex-wrap:wrap;padding:12px 14px;margin-bottom:14px;background:var(--td-surface2);border:1px solid var(--td-border);border-radius:14px;">
						<div style="flex:1;min-width:180px;">
							<div style="font-weight:600;font-size:14px;">Pull loved tracks</div>
							<div style="font-size:12.5px;color:var(--td-muted-2);margin-top:2px;">
								Brings your existing loves across from the services above. A track you've never
								played here is skipped rather than invented.
							</div>
						</div>
						<button class="btn btn-secondary" on:click={pullLoves} disabled={pullingLoves}>
							{pullingLoves ? 'Pulling…' : 'Pull loves'}
						</button>
					</div>
					{#if lovesMsg}
						<p style="font-size:12.5px;color:var(--td-good-fg);margin:-6px 0 14px;">{lovesMsg}</p>
					{/if}
				{/if}

				<!-- ListenBrainz -->
				<div style="display:flex;align-items:center;gap:12px;flex-wrap:wrap;padding:12px 0;border-bottom:1px solid var(--td-border);">
					<div style="flex:1;min-width:160px;">
						<div style="font-weight:600;font-size:14px;">ListenBrainz</div>
						{#if conn('listenbrainz')}
							<div style="font-size:12.5px;color:var(--td-good-fg);">Connected{conn('listenbrainz')?.username ? ` as ${conn('listenbrainz')?.username}` : ''}</div>
						{:else}
							<div style="font-size:12.5px;color:var(--td-muted-2);">Paste your user token from listenbrainz.org → Settings.</div>
						{/if}
					</div>
					{#if conn('listenbrainz')}
						<button class="btn btn-secondary" style="color:#b2622d;" on:click={() => disconnectSvc('listenbrainz')}>Disconnect</button>
					{:else}
						<input class="input" style="width:220px;" type="password" bind:value={lbToken} placeholder="ListenBrainz token">
						<button class="btn btn-primary" on:click={connectLB} disabled={lbBusy || !lbToken.trim()}>{lbBusy ? '…' : 'Connect'}</button>
					{/if}
				</div>

				<!-- Last.fm and Libre.fm: same approve-then-finish flow -->
				{#each [{ id: 'lastfm', label: 'Last.fm' }, { id: 'librefm', label: 'Libre.fm' }] as svc, i}
					{@const key = svc.id as 'lastfm' | 'librefm'}
					<div style="display:flex;align-items:center;gap:12px;flex-wrap:wrap;padding:12px 0;{i === 0 ? 'border-bottom:1px solid var(--td-border);' : ''}">
						<div style="flex:1;min-width:160px;">
							<div style="font-weight:600;font-size:14px;">{svc.label}</div>
							{#if conn(key)}
								<div style="font-size:12.5px;color:var(--td-good-fg);">Connected{conn(key)?.username ? ` as ${conn(key)?.username}` : ''}</div>
							{:else if fmPendingToken[key]}
								<div style="font-size:12.5px;color:var(--td-accent);">Approve Tapedeck in the {svc.label} tab, then click “Finish”.</div>
							{:else}
								<div style="font-size:12.5px;color:var(--td-muted-2);">Click Connect, approve on {svc.label}, then finish.</div>
							{/if}
						</div>
						{#if conn(key)}
							<button class="btn btn-secondary" style="color:#b2622d;" on:click={() => disconnectSvc(key)}>Disconnect</button>
						{:else}
							<button class="btn btn-primary" on:click={() => scrobblerConnect(key)} disabled={fmBusy !== null}>
								{fmBusy === key ? '…' : fmPendingToken[key] ? 'Finish connecting' : `Connect ${svc.label}`}
							</button>
							{#if fmPendingToken[key]}<button class="btn btn-secondary" on:click={() => (fmPendingToken[key] = '')}>Cancel</button>{/if}
						{/if}
					</div>
				{/each}
			</div>
		{/if}

		{#if section === 'tokens'}
			<div class="panel" style="border-radius:22px;padding:24px;margin-bottom:18px;">
				<h3 class="h-display" style="font-size:19px;margin-bottom:4px;">API Tokens</h3>
				<p style="font-size:13px;color:var(--td-muted);margin:0 0 16px;">Tokens let Pano Scrobbler and other clients submit listens via <code class="chip">Authorization: Token td_…</code></p>

				{#if loading}
					<div style="display:flex;justify-content:center;padding:16px;"><div class="spin" style="width:18px;height:18px;border:2px solid var(--td-track);border-top-color:var(--td-accent);border-radius:50%;"></div></div>
				{:else}
					{#if tokens.length}
						<div style="display:flex;flex-direction:column;gap:8px;margin-bottom:16px;">
							{#each tokens as t (t.id)}
								<div style="display:flex;align-items:center;justify-content:space-between;gap:12px;flex-wrap:wrap;background:var(--td-surface2);border:1px solid var(--td-border);border-radius:14px;padding:12px 16px;">
									<div style="min-width:0;flex:1;">
										<div><span style="font-weight:600;font-size:14px;">{t.name}</span> <span style="font-size:10px;letter-spacing:.08em;text-transform:uppercase;color:var(--td-muted-3);margin-left:6px;">{t.scopes}</span></div>
										<div style="font-size:12px;color:var(--td-muted-2);margin-top:2px;">Created {formatDateTime(t.created_at)} · {t.last_used_at ? `Last used ${formatDateTime(t.last_used_at)}` : 'Never used'}</div>
									</div>
									<select class="input" style="width:auto;min-width:170px;font-size:12.5px;" value={t.default_chain_id ?? ''}
										on:change={(e) => setTokenChain(t, e.currentTarget.value)} aria-label="Default signal chain">
										<option value="">— no chain —</option>
										{#each chains as c}<option value={c.id}>{c.name}</option>{/each}
									</select>
									<label style="display:flex;align-items:center;gap:7px;font-size:12.5px;color:var(--td-muted);white-space:nowrap;" title="Some players send every scrobble as listen_type: import. Tick this if this client's listens should be forwarded on to Last.fm and ListenBrainz.">
										<input type="checkbox" checked={t.imports_are_live} on:change={(e) => setTokenLiveImports(t, e.currentTarget.checked)} style="width:16px;height:16px;accent-color:var(--td-accent);cursor:pointer;">
										Relay listens
									</label>
									<button class="btn btn-secondary" style="font-size:12.5px;" on:click={() => resetToken(t)}>Reset</button>
									<button class="btn btn-danger" style="font-size:12.5px;" on:click={() => revokeToken(t.id)}>Revoke</button>
								</div>
							{/each}
						</div>
						<p style="font-size:12px;color:var(--td-muted-2);margin:-6px 0 16px;">
							Changing a chain here takes effect immediately — the token itself doesn't change, so there's nothing to re-paste into the app.
							<strong>Reset</strong> is for a token you've lost: only its hash is stored, so it can't be shown again — it's re-issued instead, and the old one stops working.
							{#if !chains.length}<a href="/chains">Create a chain first →</a>{/if}
						</p>
					{/if}

					{#if createdToken}
						<div style="background:var(--td-panel);border-radius:16px;padding:16px;margin-bottom:16px;">
							<div style="font-size:10.5px;letter-spacing:.1em;text-transform:uppercase;color:#e0a06a;margin-bottom:8px;">Copy it now — it won't be shown again</div>
							<div style="display:flex;gap:8px;align-items:center;">
								<code style="flex:1;background:var(--td-panel2);color:#a9c48a;border-radius:10px;padding:10px 12px;font-size:13px;word-break:break-all;">{createdToken}</code>
								<button class="btn btn-primary" on:click={copyToken}>{tokenCopied ? '✓ Copied' : 'Copy'}</button>
							</div>
						</div>
					{/if}

					<div style="display:flex;gap:8px;flex-wrap:wrap;">
						<input class="input" style="flex:1;min-width:180px;" bind:value={newTokenName} placeholder="Token name (e.g. fooyin, Pano on Android)">
						{#if chains.length}
							<select class="input" style="width:auto;min-width:170px;" bind:value={newTokenChain} aria-label="Default signal chain for the new token">
								<option value="">— no chain —</option>
								{#each chains as c}<option value={String(c.id)}>{c.name}</option>{/each}
							</select>
						{/if}
						<label style="display:flex;align-items:center;gap:7px;font-size:13px;color:var(--td-muted);white-space:nowrap;">
							<input type="checkbox" bind:checked={newTokenRead} style="width:16px;height:16px;accent-color:var(--td-accent);cursor:pointer;">
							Allow reading
						</label>
						<label style="display:flex;align-items:center;gap:7px;font-size:13px;color:var(--td-muted);white-space:nowrap;">
							<input type="checkbox" bind:checked={newTokenWrite} style="width:16px;height:16px;accent-color:var(--td-accent);cursor:pointer;">
							Allow writing
						</label>
						<label style="display:flex;align-items:center;gap:7px;font-size:13px;color:var(--td-muted);white-space:nowrap;">
							<input type="checkbox" bind:checked={newTokenLiveImports} style="width:16px;height:16px;accent-color:var(--td-accent);cursor:pointer;">
							Relay listens
						</label>
						<button class="btn btn-primary" on:click={createToken} disabled={creating || !newTokenName.trim()}>{creating ? '…' : 'Create'}</button>
					</div>
					<p style="font-size:12px;color:var(--td-muted-2);margin:10px 0 0;">
						One token per app is the idea: give it a chain here and every listen it submits is attributed to that setup, with nothing to configure in the app itself.
						A listen that carries its own chain, or arrives through a mapped output, still wins.
						<br>
						<strong>Allow reading</strong> additionally lets the app list your chains, devices, gear and what's playing — so it can offer a chain picker instead of asking you to type a name. It can still only read your own data, and can't change anything.
						<br>
						<strong>Relay listens</strong> is for players that label every scrobble as an import — fooyin does, and so do others. Tapedeck stores those but doesn't pass them on, because an import normally means a chunk of old history and forwarding that would rewrite your Last.fm. Tick this and the client's listens are forwarded like any other, as long as they happened in the last day; anything older is still stored only, so a genuine backfill from the same app can't overwrite your history. Leave it off for anything importing your past.
						<br>
						<strong>Allow writing</strong> lets it edit and delete your listens, and save loves and liner notes. The two are separate grants and neither implies the other, so give an app only what it needs. Neither one reaches your sources, your forwarding connections, an export of your data, or anything under user management — those stay signed-in-browser only. If a device is lost, revoke its token here.
					</p>
					<p style="font-size:12px;color:var(--td-muted-2);margin:8px 0 0;">Writes and admin actions always require signing in here — a token can never do either.</p>
				{/if}
			</div>

			<div class="panel" style="border-radius:22px;padding:24px;margin-bottom:18px;">
				<h3 class="h-display" style="font-size:19px;margin-bottom:4px;">Pair a device</h3>
				<p style="font-size:13px;color:var(--td-muted);margin:0 0 14px;">
					For players with no browser — a DAP, a TV app, a headless box. It shows a code;
					type it here and it collects its own token. Codes last five minutes.
				</p>
				<div style="display:flex;gap:10px;align-items:center;flex-wrap:wrap;">
					<input class="input" bind:value={pairCode} placeholder="XXXX-XXXX"
						style="max-width:180px;text-transform:uppercase;letter-spacing:.1em;font-family:ui-monospace,monospace;"
						on:keydown={(e) => e.key === 'Enter' && lookupPairing()}>
					<button class="btn btn-secondary" on:click={lookupPairing} disabled={pairBusy || !pairCode.trim()}>
						Look up
					</button>
				</div>

				{#if pairInfo && !pairInfo.approved}
					<div style="margin-top:16px;padding:14px;border-radius:14px;background:var(--td-track);">
						<div style="font-size:14px;font-weight:600;margin-bottom:6px;">{pairInfo.client_name}</div>
						<div style="font-size:12.5px;color:var(--td-muted-2);margin-bottom:12px;">
							Will be able to submit listens{pairInfo.can_read ? ' and read your chains, devices, gear and what\'s playing' : ''}.
							It can never write anything else or reach admin actions.
						</div>
						<div style="display:flex;gap:10px;align-items:center;flex-wrap:wrap;">
							{#if chains.length}
								<select class="input" style="width:auto;min-width:170px;" bind:value={pairChain} aria-label="Default signal chain for this device">
									<option value="">No default chain</option>
									{#each chains as c}<option value={String(c.id)}>{c.name}</option>{/each}
								</select>
							{/if}
							<button class="btn btn-primary" on:click={approvePairing} disabled={pairBusy}>
								{pairBusy ? '…' : 'Approve'}
							</button>
						</div>
					</div>
				{/if}
				{#if pairMsg}
					<p style="font-size:13px;color:var(--td-good-fg);margin:12px 0 0;">{pairMsg}</p>
				{/if}
			</div>

			<div class="panel" style="border-radius:22px;padding:24px;margin-bottom:18px;">
				<h3 class="h-display" style="font-size:19px;margin-bottom:4px;">Pano Scrobbler Setup</h3>
				<p style="font-size:13px;color:var(--td-muted);margin:0 0 16px;">Point every device at your instance in one step.</p>
				<ol style="margin:0;padding-left:22px;font-size:14px;color:var(--td-nav-fg);display:flex;flex-direction:column;gap:8px;">
					<li>Open Pano Scrobbler → Settings → Scrobble services</li>
					<li>Add a custom ListenBrainz server</li>
					<li>URL: <code class="chip">{origin}</code></li>
					<li>Token: one of your <code class="chip">td_…</code> tokens above</li>
				</ol>
			</div>
		{/if}

		{#if section === 'ai'}
			<div class="panel" style="border-radius:22px;padding:24px;margin-bottom:18px;">
				<h3 class="h-display" style="font-size:19px;margin-bottom:4px;">AI Connections</h3>
				<p style="font-size:13px;color:var(--td-muted);margin:0 0 4px;">
					Tapedeck speaks the Model Context Protocol, so a chatbot can read your listening and hand playlists back.
					Nothing reaches your data until you authorise it here.
				</p>
				<p style="font-size:12.5px;color:var(--td-muted-2);margin:0 0 16px;">
					Endpoint: <code class="chip">{mcpEndpoint}</code>
				</p>

				{#if mcpLoading}
					<div style="display:flex;justify-content:center;padding:16px;"><div class="spin" style="width:18px;height:18px;border:2px solid var(--td-track);border-top-color:var(--td-accent);border-radius:50%;"></div></div>
				{:else}
					{#if mcpConnections.length}
						<div style="display:flex;flex-direction:column;gap:8px;margin-bottom:16px;">
							{#each mcpConnections as c (c.id)}
								<div style="display:flex;align-items:center;justify-content:space-between;gap:12px;flex-wrap:wrap;background:var(--td-surface2);border:1px solid var(--td-border);border-radius:14px;padding:12px 16px;">
									<div style="min-width:0;flex:1;">
										<div>
											<span style="font-weight:600;font-size:14px;">{c.name}</span>
											{#if c.client_id}<span style="font-size:10px;letter-spacing:.08em;text-transform:uppercase;color:var(--td-muted-3);margin-left:6px;">via OAuth</span>{/if}
										</div>
										<div style="display:flex;gap:5px;flex-wrap:wrap;margin-top:5px;">
											{#each c.scopes.split(' ').filter(Boolean) as sc}
												<span class="chip" style="font-size:10.5px;">{sc}</span>
											{/each}
										</div>
										<div style="font-size:12px;color:var(--td-muted-2);margin-top:6px;">
											Added {formatDateTime(c.created_at)} ·
											{c.last_used_at ? `last used ${formatDateTime(c.last_used_at)}` : 'never used'} ·
											{c.calls} call{c.calls === 1 ? '' : 's'}
											{#if c.expires_at} · expires {formatDateTime(c.expires_at)}{/if}
										</div>
									</div>
									<button class="btn btn-secondary" style="font-size:12.5px;color:#b2622d;" on:click={() => revokeMcp(c)}>Revoke</button>
								</div>
							{/each}
						</div>
					{:else}
						<p style="font-size:13px;color:var(--td-muted-2);margin:0 0 16px;">No assistant is connected.</p>
					{/if}

					{#if createdMcpToken}
						<div style="background:var(--td-panel);border-radius:16px;padding:16px;margin-bottom:16px;">
							<div style="font-size:10.5px;letter-spacing:.1em;text-transform:uppercase;color:#e0a06a;margin-bottom:8px;">Copy it now — it won't be shown again</div>
							<div style="display:flex;gap:8px;align-items:center;">
								<code style="flex:1;background:var(--td-panel2);color:#a9c48a;border-radius:10px;padding:10px 12px;font-size:13px;word-break:break-all;">{createdMcpToken}</code>
								<button class="btn btn-primary" on:click={copyMcpToken}>{mcpCopied ? '✓ Copied' : 'Copy'}</button>
							</div>
							<p style="font-size:12px;color:var(--td-muted-2);margin:10px 0 0;">
								Point the client at <code class="chip">{mcpEndpoint}</code> and send it as
								<code class="chip">Authorization: Bearer …</code>
							</p>
						</div>
					{/if}

					<div style="border-top:1px solid var(--td-border);padding-top:16px;">
						<div style="font-size:10.5px;letter-spacing:.1em;text-transform:uppercase;color:var(--td-muted-3);margin-bottom:10px;">Connect a local client</div>
						<input class="input" style="width:100%;margin-bottom:12px;" bind:value={newMcpName} placeholder="What is it? (e.g. Claude Desktop, my agent)">
						<div style="display:flex;flex-direction:column;gap:7px;margin-bottom:14px;">
							{#each mcpScopeCatalogue as sc}
								<label style="display:flex;align-items:flex-start;gap:9px;font-size:13px;color:var(--td-nav-fg);cursor:pointer;">
									<input type="checkbox" checked={newMcpScopes.has(sc.scope)} on:change={() => toggleMcpScope(sc.scope)} style="width:16px;height:16px;margin-top:1px;accent-color:var(--td-accent);cursor:pointer;">
									<span>{sc.description}</span>
								</label>
							{/each}
						</div>
						<button class="btn btn-primary" disabled={!newMcpName.trim() || !newMcpScopes.size} on:click={createMcp}>Create connection</button>
						<p style="font-size:12px;color:var(--td-muted-2);margin:12px 0 0;">
							A hosted assistant (claude.ai and the like) connects through OAuth instead — give it the endpoint above and
							approve the request when it appears. Either way, every call is recorded below.
						</p>
					</div>
				{/if}
			</div>

			<div class="panel" style="border-radius:22px;padding:24px;margin-bottom:18px;">
				<div style="display:flex;align-items:center;justify-content:space-between;gap:12px;">
					<div>
						<h3 class="h-display" style="font-size:19px;margin-bottom:4px;">Activity</h3>
						<p style="font-size:13px;color:var(--td-muted);margin:0;">Every call an assistant has made, and how much came back.</p>
					</div>
					<button class="btn btn-secondary" on:click={loadMcpAudit}>{mcpAuditOpen ? 'Hide' : 'Show'}</button>
				</div>
				{#if mcpAuditOpen}
					{#if mcpAudit.length}
						<div class="tablewrap" style="margin-top:16px;overflow-x:auto;">
							<table style="width:100%;border-collapse:collapse;font-size:12.5px;">
								<thead>
									<tr style="text-align:left;color:var(--td-muted-3);font-size:10.5px;letter-spacing:.08em;text-transform:uppercase;">
										<th style="padding:6px 10px 6px 0;">When</th>
										<th style="padding:6px 10px;">Connection</th>
										<th style="padding:6px 10px;">Tool</th>
										<th style="padding:6px 10px;">Rows</th>
										<th style="padding:6px 0 6px 10px;">Time</th>
									</tr>
								</thead>
								<tbody>
									{#each mcpAudit as a (a.id)}
										<tr style="border-top:1px solid var(--td-border);">
											<td style="padding:7px 10px 7px 0;color:var(--td-muted-2);white-space:nowrap;">{formatDateTime(a.at)}</td>
											<td style="padding:7px 10px;">{a.connection_name ?? '—'}</td>
											<td style="padding:7px 10px;">
												<code class="chip" style="font-size:10.5px;">{a.tool ?? a.method}</code>
												{#if !a.ok}<span style="color:#b2622d;margin-left:6px;">failed</span>{/if}
											</td>
											<td style="padding:7px 10px;color:var(--td-muted-2);">{a.rows_returned ?? '—'}</td>
											<td style="padding:7px 0 7px 10px;color:var(--td-muted-2);white-space:nowrap;">{a.ms ?? '—'}ms</td>
										</tr>
									{/each}
								</tbody>
							</table>
						</div>
					{:else}
						<p style="font-size:13px;color:var(--td-muted-2);margin:16px 0 0;">Nothing yet.</p>
					{/if}
				{/if}
			</div>
		{/if}

		{#if section === 'data'}
			<div class="panel" style="border-radius:22px;padding:24px;margin-bottom:18px;">
				<h3 class="h-display" style="font-size:19px;margin-bottom:4px;">Data &amp; Backups</h3>
				<p style="font-size:13px;color:var(--td-muted);margin:0 0 16px;">{listenCount} listens recorded. Export a full backup or wipe the deck clean.</p>
				<div style="display:flex;gap:10px;flex-wrap:wrap;align-items:center;">
					<button class="btn btn-secondary" on:click={() => doExport('json')}>Export JSON</button>
					<button class="btn btn-secondary" on:click={() => doExport('csv')}>Export CSV</button>
					<button class="btn btn-secondary" on:click={doBackup}>Download DB backup</button>
					<span style="flex:1;"></span>
					<button class="btn btn-ghost" style="color:#b2622d;" on:click={() => (confirmClear = true)}>Clear history</button>
				</div>
				{#if exportMsg}
					<div style="margin-top:14px;display:inline-flex;align-items:center;gap:8px;font-size:13px;color:var(--td-good-fg);background:var(--td-good-bg);padding:7px 14px;border-radius:999px;">✓ {exportMsg}</div>
				{/if}
			<p style="font-size:12px;color:var(--td-muted-2);margin:14px 0 0;">
					The DB backup contains every user's listens and their encrypted credentials — treat it
					like a password file. The encryption key is kept outside the database and is
					<em>not</em> in the backup, so a leaked copy alone won't hand over your tokens.
				</p>
			</div>

			<div class="panel" style="border-radius:22px;padding:24px;margin-bottom:18px;">
				<h3 class="h-display" style="font-size:19px;margin-bottom:4px;">Liner notes</h3>
				<p style="font-size:13px;color:var(--td-muted);margin:0 0 16px;">
					{noteCount} note{noteCount === 1 ? '' : 's'} written. Take them out as Markdown —
					for Obsidian, Logseq, or just a folder that still opens in ten years.
				</p>
				<div style="display:flex;gap:10px;flex-wrap:wrap;align-items:center;">
					<button class="btn btn-secondary" disabled={!noteCount} on:click={() => doNotesExport('zip')}>
						Export as a folder (.zip)
					</button>
					<button class="btn btn-secondary" disabled={!noteCount} on:click={() => doNotesExport('md')}>
						Export as one file (.md)
					</button>
				</div>
				<p style="font-size:12px;color:var(--td-muted-2);margin:14px 0 0;">
					One file per note, with YAML frontmatter carrying the artist, kind, MBID and dates.
					Your <code>[[track:…]]</code> references become ordinary
					<code>[[wikilinks]]</code> pointing at the matching file — Tapedeck's shortcode
					syntax and Obsidian's link syntax are the same brackets, so they have to be
					rewritten or every one of them would dangle. A reference that never resolved is
					left exactly as you wrote it.
				</p>
			</div>

			<!-- Listens keep their own page rather than being inlined here: an
			     import is a long-running job with progress and a history, which
			     is a screen rather than a button. The pointer is what makes this
			     section the one index of everything in and out. -->
			<div class="panel" style="border-radius:22px;padding:24px;margin-bottom:18px;">
				<h3 class="h-display" style="font-size:19px;margin-bottom:4px;">Listens</h3>
				<p style="font-size:13px;color:var(--td-muted);margin:0 0 16px;">
					Bring in a history from ListenBrainz, Last.fm, Spotify or a Rockbox
					player. Large imports run as a job you can watch.
				</p>
				<a class="btn btn-secondary" href="/import">Import listens →</a>
			</div>

			<!-- The shelf. Its export used to live only on the shelf page and its
			     import did not exist at all — so "how do I get my collection out
			     and back" had two different answers in two different places. Every
			     way in and out of Tapedeck is here now. -->
			<div class="panel" style="border-radius:22px;padding:24px;margin-bottom:18px;">
				<h3 class="h-display" style="font-size:19px;margin-bottom:4px;">The shelf</h3>
				<p style="font-size:13px;color:var(--td-muted);margin:0 0 16px;">
					Your pressings, the sides you have played, the gear, and the scans you
					uploaded — as a zip.
				</p>
				<div style="display:flex;gap:10px;flex-wrap:wrap;align-items:center;">
					<a class="btn btn-secondary" href="/api/v1/physical/export">Export the shelf</a>
					<label class="btn btn-secondary" style="cursor:pointer;">
						{shelfImporting ? 'Importing…' : 'Import a shelf'}
						<input type="file" accept=".zip,application/zip" style="display:none;"
							disabled={shelfImporting} on:change={onShelfImport} />
					</label>
				</div>
				{#if shelfMsg}
					<p style="font-size:13px;color:var(--td-good-fg);background:var(--td-good-bg);padding:9px 14px;border-radius:12px;margin:14px 0 0;">
						{shelfMsg}
					</p>
				{/if}
				{#if shelfErr}
					<p style="font-size:13px;color:#b2622d;margin:14px 0 0;">{shelfErr}</p>
				{/if}
				<p style="font-size:12px;color:var(--td-muted-2);margin:14px 0 0;">
					An import brings back the <em>objects</em>. The listens those sides
					generated are not in a shelf export — they live in the database, so
					restore a backup for the history and the chain hours derived from it.
					Everything is matched before it is added, so importing the same archive
					twice changes nothing the second time.
				</p>
			</div>
		{/if}

		{#if section === 'sanitiser'}
			<div class="panel" style="border-radius:22px;padding:24px;margin-bottom:18px;">
				<h3 class="h-display" style="font-size:19px;margin-bottom:4px;">Metadata Sanitiser</h3>
				<p style="font-size:13px;color:var(--td-muted);margin:0 0 16px;">
					Scrobble clients tag each track on its own, so one song from an album routinely arrives
					carrying a compilation's release — wrong cover, and it drops out of the album's stats.
					This finds those and puts them back on the release the rest of the album agrees on.
				</p>

				{#if sanitize}
					{@const total = sanitize.mixed_albums.length + sanitize.featured_artists.length + sanitize.artist_variants.length + sanitize.split_attributions.reduce((n, sp) => n + sp.folding.length, 0)}
					{#if total}
						<div class="san-bar">
							<span style="font-weight:600;font-size:14px;">{pickedCount} of {total} selected</span>
							<span style="flex:1;"></span>
							<button class="btn btn-ghost" style="font-size:12.5px;" on:click={() => selectAll(true)}>Select all</button>
							<button class="btn btn-ghost" style="font-size:12.5px;" on:click={() => selectAll(false)}>None</button>
						</div>

						{#if sanitize.mixed_albums.length}
							<div class="san-group">Albums on more than one release</div>
							{#each sanitize.mixed_albums as a (albumKey(a))}
								{@const key = albumKey(a)}
								{@const chosen = albumChoice[key]}
								{@const chosenPlays = a.candidates.find((c) => c.mbid === chosen)?.plays ?? 0}
								{@const weak = chosenPlays * 2 <= a.plays}
								<label class="san-row" class:on={pickedAlbums.has(key)}>
									<input type="checkbox" checked={pickedAlbums.has(key)}
										on:change={() => (pickedAlbums = toggle(pickedAlbums, key))}>
									<span style="flex:1;min-width:0;">
										<span class="san-name">{a.album}</span>
										<span class="san-sub">
											{a.artist} · {a.plays} plays across {a.variants} releases{#if a.untagged}, {a.untagged} with none{/if}
										</span>
										<span class="san-controls">
											<select class="input san-select" value={chosen ?? ''}
												on:change={(e) => (albumChoice = { ...albumChoice, [key]: e.currentTarget.value })}
												aria-label="Release to unify onto">
												{#each a.candidates as c}
													<option value={c.mbid}>{c.mbid.slice(0, 8)}… — {c.plays} play{c.plays === 1 ? '' : 's'}</option>
												{/each}
											</select>
											{#if weak}
												<span class="san-warn" title="No release has a clear majority — worth checking on MusicBrainz first">
													weak majority
												</span>
											{/if}
											{#if chosen}
												<a class="san-link" href={`https://musicbrainz.org/release/${chosen}`} target="_blank" rel="noopener">check →</a>
											{/if}
										</span>
									</span>
								</label>
							{/each}
						{/if}

						{#if sanitize.featured_artists.length}
							<div class="san-group">Featured credits splitting an artist</div>
							{#each sanitize.featured_artists as a (a.artist)}
								<label class="san-row" class:on={pickedCredits.has(a.artist)}>
									<input type="checkbox" checked={pickedCredits.has(a.artist)}
										on:change={() => (pickedCredits = toggle(pickedCredits, a.artist))}>
									<span style="flex:1;min-width:0;">
										<span class="san-name">{a.artist}</span>
										<span class="san-sub">→ {a.primary} · {a.plays} play{a.plays === 1 ? '' : 's'} · full credit is kept</span>
									</span>
								</label>
							{/each}
						{/if}

						{#if sanitize.artist_variants.length}
							<div class="san-group">One artist spelled several ways</div>
							<p style="font-size:12.5px;color:var(--td-muted);margin:0 0 10px;">
								Merging spellings is <strong>not reversible</strong> — unlike the
								featured-credit fold, nothing is kept, because both strings name the
								same artist. The ones marked <em>counts split</em> are the ones that
								matter: those spellings are separate artists to every chart. The
								rest only display inconsistently, since every counting query already
								lowercases.
							</p>
							{#each sanitize.artist_variants as v}
								{#each v.variants as x}
									{@const key = variantKey(x[0], v.primary)}
									<label class="san-row" class:on={pickedVariants.has(key)}>
										<input type="checkbox" checked={pickedVariants.has(key)}
											on:change={() => (pickedVariants = toggle(pickedVariants, key))}>
										<span style="flex:1;min-width:0;">
											<span class="san-name">{x[0]}</span>
											<span class="san-sub">
												→ {v.primary} · {x[1]} play{x[1] === 1 ? '' : 's'} moved to the spelling with {v.primary_plays}
												{#if v.case_only}
													· <span style="color:var(--td-muted-3);">capitalisation only — counts are already correct</span>
												{:else}
													· <span style="color:#b2622d;font-weight:600;">counts split</span>
												{/if}
											</span>
										</span>
									</label>
								{/each}
							{/each}
						{/if}

						{#if sanitize.split_attributions.length}
							<div class="san-group">One track credited two ways on the same record</div>
							<p style="font-size:12.5px;color:var(--td-muted);margin:0 0 10px;">
								A film soundtrack is filed under its music director while each track is
								tagged with its singers, so the same song ends up split between them —
								halving its plays and resolving to two different MusicBrainz recordings.
								The record's own artist wins; every folded name is kept in the credit.
							</p>
							{#each sanitize.split_attributions as sp}
								{#each sp.folding as f}
									{@const key = splitKey(sp.title, sp.album, f[0])}
									<label class="san-row" class:on={pickedSplits.has(key)}>
										<input type="checkbox" checked={pickedSplits.has(key)}
											on:change={() => (pickedSplits = toggle(pickedSplits, key))}>
										<span style="flex:1;min-width:0;">
											<span class="san-name">{f[0]}</span>
											<span class="san-sub">
												→ {sp.primary} · {f[1]} play{f[1] === 1 ? '' : 's'} on “{sp.title}”
												({sp.album})
												{#if sp.primary_album_tracks >= 3}
													· credited on {sp.primary_album_tracks} tracks of this record
												{:else}
													· <span style="color:#b2622d;">weak — only on {sp.primary_album_tracks} track{sp.primary_album_tracks === 1 ? '' : 's'} of this record</span>
												{/if}
											</span>
										</span>
									</label>
								{/each}
							{/each}
						{/if}

						<button class="btn btn-primary" style="margin-top:16px;" on:click={runSanitize}
							disabled={sanitizing || !pickedCount}>
							{sanitizing ? 'Sanitising…' : `Apply ${pickedCount} change${pickedCount === 1 ? '' : 's'}`}
						</button>
					{:else}
						<p style="font-size:14px;color:var(--td-muted);margin:0;">Nothing to fix — every album agrees with itself.</p>
					{/if}
				{:else}
					<p style="font-size:14px;color:var(--td-muted);margin:0;">Checking…</p>
				{/if}
				{#if sanitizeMsg}
					<p style="font-size:13px;color:var(--td-good-fg);margin:12px 0 0;">{sanitizeMsg}</p>
				{/if}
			</div>
		{/if}

		{#if section === 'maintenance'}
			<div class="panel" style="border-radius:22px;padding:24px;margin-bottom:18px;">
				<h3 class="h-display" style="font-size:19px;margin-bottom:4px;">Listening Sessions</h3>
				<p style="font-size:13px;color:var(--td-muted);margin:0 0 16px;">
					Listens are grouped into sessions as they arrive — contiguous listening, split by a
					long gap or a change of app. Rebuilding regroups your whole history from scratch,
					which is what an imported backlog needs, and lets you reconsider the gap.
				</p>
				<div style="display:flex;align-items:center;gap:12px;flex-wrap:wrap;">
					<label style="font-size:13px;color:var(--td-muted);display:flex;align-items:center;gap:8px;">
						New session after
						<input class="input" type="number" min="1" max="1440" bind:value={sessionGapMinutes} style="width:80px;">
						minutes
					</label>
					<button class="btn btn-secondary" on:click={rebuildSessions} disabled={rebuildingSessions}>
						{rebuildingSessions ? 'Regrouping…' : 'Rebuild sessions'}
					</button>
				</div>
				<p style="font-size:12px;color:var(--td-muted-3);margin:10px 0 0;">
					Discards sessions you've merged or split by hand — those choices live in the same
					place this rewrites.
				</p>
				{#if sessionMsg}
					<p style="font-size:13px;color:var(--td-good-fg);margin:12px 0 0;">{sessionMsg}</p>
				{/if}
			</div>

			<!-- Above Album Art on purpose: this is the pass the others key off.
			     The ISRC, work-language and release-year lookups all need a
			     recording MBID, and an imported listen never had one. -->
			<div class="panel" style="border-radius:22px;padding:24px;margin-bottom:18px;">
				<h3 class="h-display" style="font-size:19px;margin-bottom:4px;">MusicBrainz Recordings</h3>
				<p style="font-size:13px;color:var(--td-muted);margin:0 0 16px;max-width:66ch;">
					Identifying a listen's <em>recording</em> is what unlocks the rest — its ISRC, the
					language it is sung in, its release year. Listens that arrived from a scrobble
					client got one on the way out; <strong>imported listens never did</strong>, so on a
					history that came from an export most of it is unidentified until this runs.
				</p>
				{#if mbidStatus}
					<div style="display:flex;align-items:center;gap:12px;flex-wrap:wrap;">
						<div style="flex:1;min-width:180px;">
							<div style="height:8px;border-radius:999px;background:var(--td-track);overflow:hidden;">
								<div style="height:100%;width:{mbidStatus.total ? (mbidStatus.resolved / mbidStatus.total) * 100 : 0}%;background:linear-gradient(90deg,var(--td-accent-2),var(--td-accent));border-radius:999px;transition:width .4s;"></div>
							</div>
							<div style="font-size:12.5px;color:var(--td-muted-2);margin-top:6px;">
								{mbidStatus.resolved} / {mbidStatus.total} listens identified
								<!-- Said plainly, because otherwise a bar that stops short
								     looks like a stall rather than an answer. -->
								{#if mbidStatus.unmatched_tracks > 0}
									· {mbidStatus.unmatched_tracks} track{mbidStatus.unmatched_tracks === 1 ? '' : 's'}
									MusicBrainz doesn't have
								{/if}
							</div>
						</div>
						{#if isAdmin}
							<button class="btn btn-secondary" on:click={runMbidBackfill} disabled={fetchingMbids || mbidStatus.pending === 0}>
								{fetchingMbids ? 'Identifying…' : mbidStatus.pending === 0 ? 'All identified' : 'Identify the rest'}
							</button>
						{/if}
					</div>
					{#if fetchingMbids}
						<p style="font-size:12px;color:var(--td-muted-3);margin:10px 0 0;">
							One rate-limited lookup per track, so a large history takes hours rather than
							minutes. You can leave this page — it keeps going.
						</p>
					{/if}
				{:else}
					<p style="font-size:13px;color:var(--td-muted-3);margin:0;">No status available.</p>
				{/if}
			</div>

			<div class="panel" style="border-radius:22px;padding:24px;margin-bottom:18px;">
				<h3 class="h-display" style="font-size:19px;margin-bottom:4px;">Album Art</h3>
				<p style="font-size:13px;color:var(--td-muted);margin:0 0 16px;">
					Art is resolved in order: your own library (free and exact), then Deezer, then the Cover
					Art Archive. It runs in the background — this fills the backlog now.
				</p>
				{#if artStatus}
					<div style="display:flex;align-items:center;gap:12px;flex-wrap:wrap;">
						<div style="flex:1;min-width:180px;">
							<div style="height:8px;border-radius:999px;background:var(--td-track);overflow:hidden;">
								<div style="height:100%;width:{artStatus.total ? (artStatus.resolved / artStatus.total) * 100 : 0}%;background:linear-gradient(90deg,var(--td-accent-2),var(--td-accent));border-radius:999px;transition:width .4s;"></div>
							</div>
							<div style="font-size:12.5px;color:var(--td-muted-2);margin-top:6px;">
								{artStatus.resolved} / {artStatus.total} listens have artwork
							</div>
						</div>
						{#if isAdmin}
							<button class="btn btn-secondary" on:click={runArtworkBackfill} disabled={fetchingArt || artStatus.pending === 0}>
								{fetchingArt ? 'Fetching…' : artStatus.pending === 0 ? 'All fetched' : 'Fetch missing art'}
							</button>
						{/if}
					</div>
					{#if fetchingArt}
						<p style="font-size:12px;color:var(--td-muted-3);margin:10px 0 0;">
							MusicBrainz is rate-limited, so a large history takes a while. You can leave this page.
						</p>
					{/if}
				{:else}
					<p style="font-size:13px;color:var(--td-muted-3);margin:0;">No status available.</p>
				{/if}
			</div>

			<!-- Immediately above Language Enrichment because it feeds it: a
			     romanised title tells `script_language` nothing, and the lyric is
			     where that title's own script went.

			     No button, unlike every other card in this section. Lyrics are
			     fetched on the engine tick alone, at the pace LRCLIB asks for; a
			     control here would let a page refresh drive an unbounded number of
			     requests at a free service that charges nothing and asks only that.

			     And no progress bar, which is the tempting thing to draw. `asked`
			     and `unresolved` do not share a population — a track asked about
			     whose lyric came back in Latin script is counted in both — so a bar
			     dividing one into the other could never reach the end, and would
			     report an answer as a stall. Two counts, each saying what it is of. -->
			<div class="panel" style="border-radius:22px;padding:24px;margin-bottom:18px;">
				<h3 class="h-display" style="font-size:19px;margin-bottom:4px;">Lyrics</h3>
				<p style="font-size:13px;color:var(--td-muted);margin:0 0 16px;max-width:66ch;">
					Words come from <a href="https://lrclib.net" target="_blank" rel="noreferrer noopener">LRCLIB</a>,
					and they are what settles the language of a <strong>romanised</strong> title — a Korean
					track written in Latin letters gives the script test nothing to read, and its lyric
					gives it everything. They are also what the reader and <code>[[line:…]]</code> quotes are
					made of. Fetched in the background, a few tracks a minute, most-played first.
				</p>
				{#if lyricStatus?.available}
					<div style="font-size:12.5px;color:var(--td-muted-2);">
						{lyricStatus.asked ?? 0} track{(lyricStatus.asked ?? 0) === 1 ? '' : 's'} asked about —
						{lyricStatus.with_lyrics ?? 0} with words
						· {lyricStatus.instrumental ?? 0} instrumental
						<!-- Named rather than left as the gap between two numbers: a track
						     LRCLIB has never heard of is a finished lookup, not a pending one. -->
						· {Math.max(0, (lyricStatus.asked ?? 0) - (lyricStatus.with_lyrics ?? 0) - (lyricStatus.instrumental ?? 0))}
						LRCLIB doesn't have
					</div>
					<div style="font-size:12.5px;color:var(--td-muted-2);margin-top:6px;">
						{lyricStatus.unresolved} track{lyricStatus.unresolved === 1 ? '' : 's'} still filed under
						English/Other
					</div>
					<p style="font-size:12px;color:var(--td-muted-3);margin:10px 0 0;max-width:66ch;">
						Those two counts overlap and are not a fraction of one another: some of what is
						still English/Other has already been asked about and came back in Latin script,
						which this pass cannot settle either way. Telling French from English needs a
						detector Tapedeck doesn't have yet.
					</p>
				{:else if lyricStatus}
					<p style="font-size:13px;color:var(--td-muted-3);margin:0;">
						{lyricStatus.why ?? 'The lyrics cache is unavailable, so the language pass is skipped.'}
					</p>
				{:else}
					<p style="font-size:13px;color:var(--td-muted-3);margin:0;">No status available.</p>
				{/if}
			</div>

			{#if isAdmin}
				<div class="panel" style="border-radius:22px;padding:24px;margin-bottom:18px;">
					<h3 class="h-display" style="font-size:19px;margin-bottom:4px;">Language Enrichment</h3>
					<p style="font-size:13px;color:var(--td-muted);margin:0 0 16px;">Classify listens by language for the Analytics breakdown — instant for non-Latin scripts, MusicBrainz (rate-limited) for the rest. Runs automatically in the background; use this to fill the backlog now.</p>
					<div style="display:flex;align-items:center;gap:14px;flex-wrap:wrap;">
						<button class="btn btn-primary" on:click={runEnrichment} disabled={enriching}>{enriching ? 'Enriching…' : 'Enrich languages'}</button>
						{#if langStatus}
							<span style="font-size:13.5px;color:var(--td-muted);">{langStatus.resolved} / {langStatus.total} classified{langStatus.pending ? ` · ${langStatus.pending} pending` : ''}</span>
						{/if}
					</div>
				</div>
			{/if}

			<div class="panel" style="border-radius:22px;padding:24px;margin-bottom:18px;">
				<h3 class="h-display" style="font-size:19px;margin-bottom:4px;">Connection</h3>
				<p style="font-size:13px;color:var(--td-muted);margin:0 0 16px;">Test connectivity to the Tapedeck backend.</p>
				<div style="display:flex;align-items:center;gap:14px;">
					<button class="btn btn-secondary" on:click={checkConnection} disabled={checking}>{checking ? 'Checking…' : 'Test connection'}</button>
					{#if healthStatus}<span style="font-size:14px;font-weight:600;color:{healthStatus.startsWith('✓') ? 'var(--td-good-fg)' : '#b2622d'};">{healthStatus}</span>{/if}
				</div>
			</div>
		{/if}

		{#if section === 'ops'}
			{#if !isAdmin}
				<div class="panel" style="border-radius:22px;padding:24px;">
					<p style="font-size:13px;color:var(--td-muted);margin:0;">Admin only.</p>
				</div>
			{:else}
				{#if opsErr}
					<div class="panel" style="border-radius:22px;padding:16px 20px;margin-bottom:18px;">
						<p style="font-size:13px;color:var(--td-bad-fg);margin:0;">{opsErr}</p>
					</div>
				{/if}

				<!-- The engine, first. A tick that cannot keep its own schedule is the
				     one number that says this instance is in trouble whatever the
				     cause, and on 2026-08-24 it was true all day and visible nowhere. -->
				<div class="panel" style="border-radius:22px;padding:24px;margin-bottom:18px;">
					<div style="display:flex;align-items:baseline;justify-content:space-between;gap:12px;flex-wrap:wrap;">
						<h3 class="h-display" style="font-size:19px;margin:0 0 4px;">The engine</h3>
						{#if ops}
							<span class="opspill" class:bad={ops.tick.behind} class:good={!ops.tick.behind}>
								{ops.tick.behind ? 'Behind schedule' : 'Keeping up'}
							</span>
						{/if}
					</div>
					<p style="font-size:13px;color:var(--td-muted);margin:0 0 16px;">
						One poll tick reads every due source, updates the deck, and forwards whatever is
						waiting. A tick longer than its own interval means sources and the deck are that
						late — which is what every &ldquo;Plex isn't syncing&rdquo; report has turned out to be.
					</p>
					{#if ops}
						<div class="opsgrid">
							{#each [
								['Interval', `${ops.tick.interval_secs}s`, 'how often it should run'],
								['Last tick', dur(ops.tick.last_duration_ms), ago(ops.tick.last_started, nowSec)],
								['Worst tick', dur(ops.tick.longest_ms), `${ops.tick.overruns} overrun${ops.tick.overruns === 1 ? '' : 's'}`],
								['Uptime', dur(ops.uptime_secs * 1000), `v${ops.version}`]
							] as [label, value, sub]}
								<div class="opstile">
									<div class="opslabel">{label}</div>
									<div class="opsvalue">{value}</div>
									<div class="opssub">{sub}</div>
								</div>
							{/each}
						</div>
						{#if ops.tick.deferred_listens > 0}
							<p style="font-size:12.5px;color:var(--td-muted-2);margin:14px 0 0;">
								{ops.tick.deferred_listens} listen{ops.tick.deferred_listens === 1 ? '' : 's'} have been
								held over by the flush budget since this process started. Nothing is lost — they
								stay pending and go out on a later tick — but it means forwarding is running behind,
								which usually means a service below is unhappy.
							</p>
						{/if}
					{:else}
						<p style="font-size:13px;color:var(--td-muted-3);margin:0;">Loading…</p>
					{/if}
				</div>

				<!-- Sources: last poll against each one's own interval. -->
				<div class="panel" style="border-radius:22px;padding:24px;margin-bottom:18px;">
					<h3 class="h-display" style="font-size:19px;margin-bottom:4px;">Sources</h3>
					<p style="font-size:13px;color:var(--td-muted);margin:0 0 16px;">
						Every source on this instance, and when it was last read. Late is measured against
						each source's own interval, since thirty minutes is normal for one and an outage
						for another.
					</p>
					{#if ops && ops.sources.length}
						<div class="opsrows">
							{#each ops.sources as src (src.id)}
								<div class="opsrow">
									<div style="min-width:0;">
										<div class="opsname">
											{src.label || src.kind}
											<span class="opsdim">· {src.username}</span>
										</div>
										<div class="opssub">
											{#if !src.enabled}
												Switched off
											{:else}
												polled {ago(src.last_polled_at, nowSec)} · every {src.poll_interval_secs ?? 30}s
											{/if}
											{#if src.last_error}<span class="opserr">· {src.last_error}</span>{/if}
										</div>
									</div>
									<span
										class="opspill"
										class:bad={sourceLate(src, nowSec)}
										class:good={src.enabled && !sourceLate(src, nowSec)}
									>
										{!src.enabled ? 'Off' : sourceLate(src, nowSec) ? 'Overdue' : 'OK'}
									</span>
								</div>
							{/each}
						</div>
					{:else if ops}
						<p style="font-size:13px;color:var(--td-muted-3);margin:0;">No sources configured.</p>
					{/if}
				</div>

				<!-- Sinks, with the breaker's state. -->
				<div class="panel" style="border-radius:22px;padding:24px;margin-bottom:18px;">
					<h3 class="h-display" style="font-size:19px;margin-bottom:4px;">Outbound services</h3>
					<p style="font-size:13px;color:var(--td-muted);margin:0 0 16px;">
						Everything this instance has spoken to since it started — the services your
						listens are forwarded to, and the ones it looks metadata up in. When one fails
						repeatedly it is skipped for a while rather than retried per listen, which is
						what stops a single service's outage from holding up the engine.
					</p>
					{#if ops && ops.sinks.length}
						<div class="opsrows">
							{#each ops.sinks as sink (sink.name)}
								<div class="opsrow">
									<div style="min-width:0;">
										<div class="opsname">{sink.name}</div>
										<div class="opssub">
											last answered {ago(sink.last_ok, nowSec)}
											· {sink.total_successes} ok, {sink.total_failures} failed
										</div>
										{#if sink.last_error}
											<!-- Once, verbatim. The alternative is what the log did on
											     2026-08-24: 6,096 copies of one sentence. -->
											<div class="opserr">{sink.last_error}</div>
										{/if}
									</div>
									<span
										class="opspill"
										class:bad={sink.skipping_for != null}
										class:warn={sink.skipping_for == null && sink.failures > 0}
										class:good={sink.skipping_for == null && sink.failures === 0}
									>
										{sink.skipping_for != null
											? `Skipping ${sink.skipping_for}s`
											: sink.failures > 0
												? `${sink.failures} failing`
												: 'OK'}
									</span>
								</div>
							{/each}
						</div>
					{:else if ops}
						<p style="font-size:13px;color:var(--td-muted-3);margin:0;">
							Nothing has been called yet this run. A service appears here the first time
							this instance speaks to it.
						</p>
					{/if}
				</div>

				<!-- The log. -->
				<div class="panel" style="border-radius:22px;padding:24px;margin-bottom:18px;">
					<h3 class="h-display" style="font-size:19px;margin-bottom:4px;">Log</h3>
					<p style="font-size:13px;color:var(--td-muted);margin:0 0 16px;">
						Seven days are kept. A level means that and worse, so <strong>Warnings</strong>
						includes errors.
					</p>
					{#if ops && !ops.logs_enabled}
						<p style="font-size:13px;color:var(--td-muted-3);margin:0;">
							File logging is switched off on this instance, so there is nothing to show here.
						</p>
					{:else}
						<div class="logbar">
							<select class="input" bind:value={logLevel} on:change={loadLog} aria-label="Level">
								<option value="">Everything</option>
								<option value="info">Info and worse</option>
								<option value="warn">Warnings and errors</option>
								<option value="error">Errors only</option>
							</select>
							{#if logTail && logTail.files.length > 1}
								<select class="input" bind:value={logFile} on:change={loadLog} aria-label="Day">
									{#each logTail.files as f}<option value={f}>{f}</option>{/each}
								</select>
							{/if}
							<input
								class="input"
								placeholder="Filter…"
								bind:value={logQuery}
								on:keydown={(e) => e.key === 'Enter' && loadLog()}
								style="flex:1;min-width:140px;"
							/>
							<button class="btn btn-secondary" on:click={loadLog} disabled={logBusy}>
								{logBusy ? '…' : 'Refresh'}
							</button>
							<a class="btn btn-secondary" href={api.logDownloadUrl(logFile || undefined)} download>
								Download
							</a>
						</div>
						{#if logTail}
							<div class="logview">
								{#each logTail.lines as line}
									<div
										class="logline"
										class:err={line.includes(' ERROR ')}
										class:warn={line.includes(' WARN ')}
									>{line}</div>
								{:else}
									<div class="logline" style="opacity:.6;">Nothing matched.</div>
								{/each}
							</div>
							<p style="font-size:12px;color:var(--td-muted-3);margin:8px 0 0;">
								<!-- What the filter found against what is shown, so a capped tail
								     cannot be mistaken for the whole answer. -->
								Showing the last {logTail.shown} of {logTail.matched} matching line{logTail.matched === 1 ? '' : 's'}
								in {logTail.file}.{logTail.truncated ? ' Download the file for the rest.' : ''}
							</p>
						{/if}
					{/if}
				</div>

				<!-- Restart, last and visually apart: it is the destructive one. -->
				<div class="panel" style="border-radius:22px;padding:24px;margin-bottom:18px;">
					<h3 class="h-display" style="font-size:19px;margin-bottom:4px;">Restart</h3>
					<p style="font-size:13px;color:var(--td-muted);margin:0 0 16px;">
						In-flight requests finish and the engine completes its current tick before the
						process stops, so nothing is cut in half — gentler than killing it from a shell.
						It comes back only if something is supervising the process; under the systemd unit
						in <code>deploy/</code> it does.
					</p>
					<button class="btn btn-danger" on:click={doRestart} disabled={restarting}>
						{restarting ? 'Draining…' : 'Restart Tapedeck'}
					</button>
					{#if restarting}
						<p style="font-size:12.5px;color:var(--td-muted-2);margin:12px 0 0;">
							Draining. This page will fail to reach the server for a few seconds — reload once
							it is back.
						</p>
					{/if}
				</div>
			{/if}
		{/if}

		{#if section === 'genremap'}
			<div class="panel" style="border-radius:22px;padding:24px;margin-bottom:18px;">
				<h3 class="h-display" style="font-size:19px;margin-bottom:4px;">Every Noise at Once</h3>
				<p style="font-size:13px;color:var(--td-muted);margin:0 0 16px;">
					Glenn McDonald's map of ~6,300 genres, where each one has a position, a colour and a
					prominence. It's been frozen since December 2023, so this is a one-time fetch — there's
					nothing to keep in sync. Your instance reads it directly rather than being shipped a
					copy.
				</p>

				<label for="en-contact" style="display:block;font-size:13px;color:var(--td-muted);margin-bottom:6px;">
					Contact email
				</label>
				<div style="display:flex;gap:10px;flex-wrap:wrap;align-items:center;">
					<input id="en-contact" class="input" type="email" bind:value={enContact}
						placeholder="you@example.com" style="flex:1;min-width:220px;">
					<button class="btn btn-secondary" on:click={saveEnContact}>Save</button>
				</div>
				<p style="font-size:12px;color:var(--td-muted-3);margin:8px 0 0;">
					Sent in the request header so the site's owner can see who is asking. Required — a
					personal site being read a few thousand times deserves to know by whom.
				</p>
				{#if enMsg}
					<p style="font-size:13px;color:var(--td-good-fg);margin:10px 0 0;">{enMsg}</p>
				{/if}
			</div>

			<div class="panel" style="border-radius:22px;padding:24px;margin-bottom:18px;">
				<h3 class="h-display" style="font-size:19px;margin-bottom:12px;">What's stored</h3>
				{#if enStatus}
					<div style="display:grid;grid-template-columns:repeat(auto-fit,minmax(120px,1fr));gap:14px;margin-bottom:16px;">
						<div>
							<div style="font-size:22px;font-weight:600;">{enStatus.summary.genres.toLocaleString()}</div>
							<div style="font-size:12px;color:var(--td-muted-2);">genres</div>
						</div>
						<div>
							<div style="font-size:22px;font-weight:600;">{enStatus.summary.artists.toLocaleString()}</div>
							<div style="font-size:12px;color:var(--td-muted-2);">artist placements</div>
						</div>
						<div>
							<div style="font-size:22px;font-weight:600;">{enStatus.summary.edges.toLocaleString()}</div>
							<div style="font-size:12px;color:var(--td-muted-2);">genre links</div>
						</div>
						<div>
							<div style="font-size:22px;font-weight:600;">
								{enStatus.summary.pages_scraped.toLocaleString()}<span style="font-size:14px;color:var(--td-muted-3);">/{enStatus.summary.genres.toLocaleString()}</span>
							</div>
							<div style="font-size:12px;color:var(--td-muted-2);">pages walked</div>
						</div>
					</div>

					{#if enStatus.progress && enStatus.progress.state === 'running'}
						<div style="margin-bottom:14px;">
							<div style="height:8px;border-radius:999px;background:var(--td-track);overflow:hidden;">
								<div style="height:100%;width:{enStatus.progress.total ? (enStatus.progress.done / enStatus.progress.total) * 100 : 0}%;background:linear-gradient(90deg,var(--td-accent-2),var(--td-accent));border-radius:999px;transition:width .4s;"></div>
							</div>
							<div style="font-size:12.5px;color:var(--td-muted-2);margin-top:6px;">
								{enStatus.progress.done.toLocaleString()} / {enStatus.progress.total.toLocaleString()}
								{#if enStatus.progress.current}· {enStatus.progress.current}{/if}
							</div>
						</div>
					{:else if enStatus.progress?.message}
						<p style="font-size:13px;color:{enStatus.progress.state === 'failed' ? 'var(--td-bad-fg)' : 'var(--td-good-fg)'};margin:0 0 14px;">
							{enStatus.progress.message}
						</p>
					{/if}

					<div style="display:flex;gap:10px;flex-wrap:wrap;">
						<button class="btn btn-secondary" on:click={() => startEnScrape('map')}
							disabled={enBusy || enStatus.progress?.state === 'running'}>
							Fetch genre map — 1 request
						</button>
						<button class="btn btn-secondary" on:click={() => startEnScrape('pages')}
							disabled={enBusy || enStatus.progress?.state === 'running' || !enStatus.summary.genres}>
							Walk every genre page — ~2 hours
						</button>
						{#if enStatus.progress?.state === 'running'}
							<button class="btn btn-secondary" on:click={cancelEnScrape}>Stop</button>
						{/if}
						{#if enStatus.summary.genres > 0}
							<button class="btn btn-secondary" on:click={clearEveryNoise}
								disabled={enStatus.progress?.state === 'running'}>Delete local copy</button>
						{/if}
					</div>

					<p style="font-size:12px;color:var(--td-muted-3);margin:12px 0 0;">
						The map is a single page load and gives every genre a position. Walking the
						per-genre pages adds the artists in each one and the nearby/opposite links, at one
						request per second — it resumes where it stops, so interrupting it costs nothing.
					</p>
					{#if enStatus.summary.last_scraped_at}
						<p style="font-size:12px;color:var(--td-muted-3);margin:6px 0 0;">
							Last updated {formatDateTime(enStatus.summary.last_scraped_at)}.
						</p>
					{/if}
				{:else}
					<p style="font-size:13px;color:var(--td-muted);margin:0;">
						{loading ? 'Loading…' : 'Only an admin can manage the genre map.'}
					</p>
				{/if}
			</div>
		{/if}

		{#if section === 'about'}
			<div class="panel" style="border-radius:22px;padding:24px;display:flex;align-items:flex-start;justify-content:space-between;gap:16px;">
				<div>
					<h3 class="h-display" style="font-size:19px;margin-bottom:6px;">About</h3>
					<div style="font-size:13px;color:var(--td-muted);display:flex;flex-direction:column;gap:3px;">
						<!-- See the sidebar footer: one artifact, so one version,
						     and the UI's is only named when it disagrees. -->
						<span>Tapedeck v{backendVersion}{#if backendVersion !== '…' && backendVersion !== uiVersion}{` · UI built from v${uiVersion}`}{/if}</span>
						<span>A self-hosted music listening journal.</span>
						<a href="https://codeberg.org/abksh/tapedeck" target="_blank" rel="noopener">View on Codeberg →</a>
					</div>
				</div>
				<button class="btn btn-secondary" style="color:#b2622d;" on:click={handleLogout}>Sign out</button>
			</div>
		{/if}
	</div>
</div>

{#if confirmClear}
	<div class="dialog-backdrop" on:click={() => (confirmClear = false)} on:keydown={() => {}} role="presentation">
		<div class="dialog" on:click|stopPropagation on:keydown|stopPropagation role="presentation">
			<div class="dialog-title">Clear listen history?</div>
			<div class="dialog-body" style="color:var(--td-nav-fg);">This permanently deletes all {listenCount} recorded listens from this instance. Export a backup first if you might want them back — this can't be undone.</div>
			<div class="dialog-actions">
				<button class="btn btn-secondary" on:click={() => (confirmClear = false)}>Cancel</button>
				<button class="btn btn-primary" style="background:#b2622d;" on:click={doClear} disabled={clearing}>{clearing ? 'Clearing…' : 'Clear everything'}</button>
			</div>
		</div>
	</div>
{/if}

<style>
	.san-bar {
		display: flex; align-items: center; gap: 8px; flex-wrap: wrap;
		padding: 10px 0 12px; border-bottom: 1px solid var(--td-border);
	}
	.san-group {
		font-size: 11px; letter-spacing: .1em; text-transform: uppercase;
		color: var(--td-muted-3); margin: 16px 0 6px;
	}
	.san-row {
		display: flex; align-items: flex-start; gap: 10px;
		padding: 9px 10px; border-radius: 12px; cursor: pointer;
	}
	.san-row:hover { background: var(--td-nav-hover); }
	.san-row.on { background: color-mix(in srgb, var(--td-accent) 7%, transparent); }
	.san-row input { width: 16px; height: 16px; margin-top: 2px; accent-color: var(--td-accent); cursor: pointer; flex: none; }
	.san-name { display: block; font-size: 14px; font-weight: 600; overflow-wrap: anywhere; }
	.san-sub { display: block; font-size: 12.5px; color: var(--td-muted-2); margin-top: 2px; }
	.san-controls { display: flex; align-items: center; gap: 8px; flex-wrap: wrap; margin-top: 7px; }
	.san-select { width: auto; font-size: 12px; padding: 4px 10px; }
	/* Named rather than coloured-only: "the plurality is a minority" is the one
	   thing the operator most needs to notice before applying. */
	.san-warn {
		font-size: 11px; font-weight: 600; color: #b2622d;
		background: color-mix(in srgb, #b2622d 12%, transparent);
		padding: 3px 8px; border-radius: 999px; white-space: nowrap;
	}
	.san-link { font-size: 11.5px; }

	.settings-layout {
		display: grid;
		grid-template-columns: 208px minmax(0, 1fr);
		gap: 24px;
		align-items: start;
	}
	.settings-nav {
		display: flex;
		flex-direction: column;
		gap: 2px;
		position: sticky;
		top: 24px;
	}
	.nav-item {
		display: flex;
		flex-direction: column;
		gap: 2px;
		text-align: left;
		border: none;
		background: transparent;
		font: inherit;
		cursor: pointer;
		padding: 9px 14px;
		border-radius: 12px;
		color: var(--td-nav-fg);
	}
	.nav-item:hover { background: var(--td-nav-hover); }
	.nav-item.on { background: var(--td-sidebar); color: var(--td-text); }
	.nav-label { font-size: 14px; font-weight: 600; }
	.nav-hint { font-size: 11.5px; color: var(--td-muted-3); }
	.nav-item.on .nav-hint { color: var(--td-muted-2); }

	/* Narrow screens get the same list as a scrolling strip above the content —
	   a 208px rail would eat half a phone. */
	@media (max-width: 720px) {
		.settings-layout { grid-template-columns: minmax(0, 1fr); gap: 16px; }
		.settings-nav {
			position: static;
			flex-direction: row;
			gap: 6px;
			overflow-x: auto;
			padding-bottom: 4px;
		}
		.nav-item { flex: none; padding: 7px 14px; border-radius: 999px; background: var(--td-surface2); }
		/* The rail's subtle tint isn't enough to read as "selected" on a row of
		   pills, so the active one takes the accent outright. */
		.nav-item.on { background: var(--td-accent); color: #fbf5ea; }
		.nav-hint { display: none; }
	}

	/* The Time section used `.row` / `.row-main` / `.row-title` / `.row-sub`,
	   which were defined *nowhere* — this is the only place in the app that uses
	   them. Svelte scopes styles, so they rendered as bare divs: the heading came
	   out at body size, the explanation ran the full width at full weight, and
	   the select dropped underneath instead of sitting beside it. Nothing errors
	   when a class doesn't exist, which is why it survived. */
	.row {
		display: flex;
		align-items: center;
		justify-content: space-between;
		gap: 16px;
		flex-wrap: wrap;
		background: var(--td-surface2);
		border: 1px solid var(--td-border);
		border-radius: 14px;
		padding: 14px 16px;
		margin-top: 18px;
	}
	.row-main { min-width: 0; flex: 1 1 260px; }
	.row-title { font-size: 14px; font-weight: 600; margin-bottom: 3px; }
	.row-sub { font-size: 12.5px; color: var(--td-muted); line-height: 1.45; max-width: 52ch; }

	/* An address you are meant to copy, not read. `min-width: 0` on the code is
	   load-bearing: a flex item defaults to `min-width: auto`, so a long URL
	   would refuse to shrink and push the Copy button off the panel instead of
	   scrolling inside its own box. */
	.copyrow {
		display: flex;
		align-items: center;
		gap: 10px;
		background: var(--td-surface2);
		border: 1px solid var(--td-border);
		border-radius: 12px;
		padding: 10px 12px;
	}
	.copyrow code {
		flex: 1 1 auto;
		min-width: 0;
		overflow-x: auto;
		white-space: nowrap;
		font-size: 12.5px;
	}

	/* Sized to sit in the same slot the `.row` selects and buttons use, so the
	   Scrobbling rows line up with the Time ones above them. */
	.slider { width: 200px; max-width: 100%; accent-color: var(--td-accent); cursor: pointer; }

	.examples {
		display: flex; gap: 10px; flex-wrap: wrap; margin-top: 18px;
		padding-top: 16px; border-top: 1px solid var(--td-border);
	}
	.ex {
		display: flex; flex-direction: column; gap: 3px;
		padding: 10px 14px; border-radius: 12px; background: var(--td-surface2); min-width: 118px;
	}
	.ex span { font-size: 11.5px; color: var(--td-muted); }
	.ex strong { font-size: 16px; font-variant-numeric: tabular-nums; }

	/* Sat directly under the input on a negative margin, which put it against
	   the field's bottom edge with no room to breathe. */
	.tz-note {
		font-size: 12.5px;
		color: var(--td-muted);
		line-height: 1.5;
		margin: 10px 0 0;
		max-width: 62ch;
	}

	.tz-list {
		position: absolute;
		z-index: 30;
		top: calc(100% + 4px);
		left: 0;
		right: 0;
		max-height: 240px;
		overflow-y: auto;
		margin: 0;
		padding: 5px;
		list-style: none;
		background: var(--td-surface);
		border: 1px solid var(--td-border);
		border-radius: 16px;
		box-shadow: 0 12px 28px rgba(46, 43, 37, 0.18);
	}
	.tz-option {
		display: block;
		width: 100%;
		text-align: left;
		border: none;
		background: transparent;
		cursor: pointer;
		font: inherit;
		font-size: 13.5px;
		color: var(--td-text);
		padding: 7px 12px;
		border-radius: 10px;
	}
	.tz-option.active {
		background: var(--td-nav-hover);
	}
	.spin { animation: td-spin 0.7s linear infinite; }
	.seg-btn { border: none; background: transparent; cursor: pointer; color: var(--td-nav-fg); font-size: 13px; padding: 7px 14px; border-radius: 999px; font-family: inherit; }
	.seg-btn.on { background: var(--td-accent); color: #fbf5ea; }
	.chip { background: var(--td-sidebar); padding: 2px 6px; border-radius: 6px; font-size: 12px; }
	/* — Operations —
	   `--td-surface`, never `--td-panel`: the latter is the *dark card* in both
	   themes, and using it without also restating the text colours reads
	   correctly in exactly one of them (the v0.70.8 trap). */
	.opsgrid {
		display: grid;
		grid-template-columns: repeat(auto-fit, minmax(150px, 1fr));
		gap: 12px;
	}
	.opstile {
		background: var(--td-surface);
		border: 1px solid var(--td-border);
		border-radius: 14px;
		padding: 14px 16px;
	}
	.opslabel {
		font-size: 11px;
		letter-spacing: .09em;
		text-transform: uppercase;
		color: var(--td-muted-2);
	}
	.opsvalue {
		font-size: 22px;
		font-weight: 700;
		font-variant-numeric: tabular-nums;
		color: var(--td-text);
		margin-top: 2px;
	}
	.opssub {
		font-size: 12px;
		color: var(--td-muted-2);
		margin-top: 2px;
	}
	.opsrows { display: flex; flex-direction: column; gap: 10px; }
	.opsrow {
		display: flex;
		align-items: center;
		justify-content: space-between;
		gap: 14px;
		background: var(--td-surface);
		border: 1px solid var(--td-border);
		border-radius: 14px;
		padding: 12px 16px;
	}
	.opsname { font-size: 14px; font-weight: 600; color: var(--td-text); }
	.opsdim { font-weight: 400; color: var(--td-muted-2); }
	.opserr {
		font-size: 12px;
		color: var(--td-bad-fg);
		margin-top: 3px;
		overflow-wrap: anywhere;
	}
	.opspill {
		flex: none;
		font-size: 11.5px;
		font-weight: 600;
		letter-spacing: .03em;
		border-radius: 999px;
		padding: 3px 10px;
		white-space: nowrap;
		background: var(--td-surface2);
		color: var(--td-muted-2);
	}
	.opspill.good { background: var(--td-good-bg); color: var(--td-good-fg); }
	.opspill.bad { background: var(--td-bad-bg); color: var(--td-bad-fg); }
	.opspill.warn {
		background: color-mix(in srgb, var(--td-accent) 16%, transparent);
		color: var(--td-accent);
	}

	.logbar {
		display: flex;
		gap: 10px;
		flex-wrap: wrap;
		align-items: center;
		margin-bottom: 12px;
	}
	.logview {
		background: var(--td-surface2);
		border: 1px solid var(--td-border);
		border-radius: 14px;
		padding: 10px 12px;
		max-height: 460px;
		/* Both axes scroll *inside* the box: a log line is arbitrarily long and
		   must not make the settings page scroll sideways. */
		overflow: auto;
	}
	.logline {
		font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
		font-size: 11.5px;
		line-height: 1.55;
		white-space: pre;
		color: var(--td-muted);
	}
	.logline.warn { color: var(--td-accent); }
	.logline.err { color: var(--td-bad-fg); }
</style>
