import {Easing, interpolate} from 'remotion';

// The site's tokens (site/assets/site.css) and the brand's fan colours (brand/README.md): one indigo, a black stage,
// light grey paper for things people read.
export const C = {
	black: '#000',
	ink: '#f5f5f7',
	ink2: '#a1a1a6',
	ink3: '#86868b',
	tile: '#161617',
	tile2: '#1d1d1f',
	line: 'rgba(255,255,255,0.14)',
	paper: '#f5f5f7',
	paperInk: '#1d1d1f',
	paperInk2: '#6e6e73',
	accent: '#4f46e5',
	link: '#a5b4fc',
	fanA: '#6366F1',
	fanB: '#4338CA',
	hinge: '#312E81',
	up: '#86efac',
	down: '#fca5a5',
	event: '#fbbf24',
	eventBg: '#3a2f12',
};

export const FONT =
	'-apple-system, BlinkMacSystemFont, "SF Pro Display", "SF Pro Text", system-ui, "Helvetica Neue", Arial, sans-serif';
export const MONO = 'ui-monospace, "SF Mono", SFMono-Regular, Menlo, monospace';

// Strong curves (Emil Kowalski's set): ease-out for things arriving, ease-in-out for things moving on screen.
export const easeOut = Easing.bezier(0.23, 1, 0.32, 1);
export const easeInOut = Easing.bezier(0.77, 0, 0.175, 1);
export const easeDrawer = Easing.bezier(0.32, 0.72, 0, 1);

/** 0→1 over [start, start + dur], clamped. */
export const prog = (frame: number, start: number, dur: number, ease = easeOut) =>
	interpolate(frame, [start, start + dur], [0, 1], {
		extrapolateLeft: 'clamp',
		extrapolateRight: 'clamp',
		easing: ease,
	});

export const mix = (a: number, b: number, t: number) => a + (b - a) * t;

export const signed = (v: number) => `${v >= 0 ? '+' : '−'}${Math.abs(v).toFixed(2)}`;
