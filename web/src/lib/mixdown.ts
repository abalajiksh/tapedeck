/**
 * Playing a report mixdown: where a layer is at a given millisecond.
 *
 * **This file has a twin**, `src/report_image/mixdown/timeline.rs`. The server
 * draws every layer and evaluates the same tracks for each frame of the
 * video; this is the same arithmetic for the browser, so the mixdown on screen
 * and the MP4 show the same thing. Change one, change the other — the Rust
 * test `the_browser_twin_follows_the_same_rules` holds the numbers both are
 * written against (ease(0.5) = 0.875, a 0→21,390 count at its midpoint =
 * 18,716).
 */
import type { MixdownCount, MixdownKey, MixdownLayer, MixdownScene } from '$lib/api';

/** The token a counting layer carries where its number goes. */
export const COUNT_TOKEN = '{{n}}';

/** Ease-out cubic. */
export function ease(p: number): number {
	const q = 1 - Math.min(1, Math.max(0, p));
	return 1 - q * q * q;
}

function at(keys: MixdownKey[] | undefined, t: number, rest: number): number {
	if (!keys || keys.length === 0) return rest;
	const first = keys[0];
	const last = keys[keys.length - 1];
	if (t <= first.t) return first.v;
	if (t >= last.t) return last.v;
	for (let i = 0; i + 1 < keys.length; i++) {
		const a = keys[i];
		const b = keys[i + 1];
		if (t >= a.t && t < b.t) {
			return a.v + (b.v - a.v) * ease((t - a.t) / Math.max(1, b.t - a.t));
		}
	}
	return last.v;
}

export interface LayerState {
	opacity: number;
	/** The SVG `transform`, or an empty string when the layer is at rest. */
	transform: string;
}

/** A layer's opacity and transform at `t` milliseconds into its scene. */
export function layerState(layer: MixdownLayer, t: number): LayerState {
	const tr = layer.tracks;
	const s = at(tr.scale, t, 1);
	const sx = s * at(tr.scale_x, t, 1);
	const sy = s * at(tr.scale_y, t, 1);
	const dx = at(tr.dx, t, 0);
	const dy = at(tr.dy, t, 0);
	const [ox, oy] = layer.origin;
	const moved = Math.abs(dx) > 0.005 || Math.abs(dy) > 0.005;
	const scaled = Math.abs(sx - 1) > 0.0005 || Math.abs(sy - 1) > 0.0005;
	let transform = '';
	if (scaled) {
		// Origin to zero, scale, back, then offset — so a scale never drifts
		// the layer and an offset is never scaled.
		transform = `translate(${dx + ox} ${dy + oy}) scale(${sx} ${sy}) translate(${-ox} ${-oy})`;
	} else if (moved) {
		transform = `translate(${dx} ${dy})`;
	}
	return { opacity: Math.min(1, Math.max(0, at(tr.opacity, t, 1))), transform };
}

/** The value a counting layer shows at `t`. Ends on the real figure. */
export function countAt(c: MixdownCount, t: number): number {
	if (t <= c.start_ms) return c.from;
	if (t >= c.end_ms) return c.to;
	const p = (t - c.start_ms) / Math.max(1, c.end_ms - c.start_ms);
	return c.from + Math.round((c.to - c.from) * ease(p));
}

/**
 * Thousands separators the way the server writes them — commas, whatever the
 * browser's locale — so a figure in the mixdown reads the same as in the video
 * and on the card.
 */
export function thousands(n: number): string {
	return String(Math.trunc(n)).replace(/\B(?=(\d{3})+(?!\d))/g, ',');
}

/** How visible the whole scene is at `t`: full, until it hands over. */
export function envelope(scene: MixdownScene, t: number): number {
	if (scene.fade_out_ms === 0 || t + scene.fade_out_ms <= scene.duration_ms) return 1;
	return Math.min(1, Math.max(0, (scene.duration_ms - t) / scene.fade_out_ms));
}
