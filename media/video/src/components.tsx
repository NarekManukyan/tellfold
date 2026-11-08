import React from 'react';
import {AbsoluteFill, useCurrentFrame, useVideoConfig} from 'remotion';
import {Avatar} from './Avatar';
import type {Persona, Post, Run} from './data';
import {C, FONT, MONO, easeInOut, easeOut, mix, prog, signed} from './theme';

/** Frame geometry. Everything is designed on a 1080-unit short side and scaled by `u`. */
export const useLayout = () => {
	const {width: W, height: H} = useVideoConfig();
	const u = Math.min(W, H) / 1080;
	const wide = W / H > 1.4;
	const tall = H / W > 1.15;
	return {W, H, u, wide, tall};
};

export const Stage: React.FC<{children: React.ReactNode; glow?: [number, number]}> = ({children, glow}) => {
	const {W, H} = useLayout();
	const [gx, gy] = glow ?? [W / 2, H / 2];
	return (
		<AbsoluteFill style={{background: C.black, fontFamily: FONT, color: C.ink, overflow: 'hidden'}}>
			<AbsoluteFill
				style={{
					background: `radial-gradient(${Math.max(W, H) * 0.55}px ${Math.max(W, H) * 0.42}px at ${gx}px ${gy}px, rgba(79,70,229,0.16), rgba(0,0,0,0) 70%)`,
				}}
			/>
			{children}
		</AbsoluteFill>
	);
};

/** Fade + lift + un-blur in; fade out. The workhorse entrance. */
export const enter = (frame: number, at: number, dur = 18, dist = 24) => {
	const t = prog(frame, at, dur);
	return {
		opacity: t,
		transform: `translateY(${(1 - t) * dist}px)`,
		filter: t < 1 ? `blur(${(1 - t) * 8}px)` : undefined,
	} as React.CSSProperties;
};

/** The two-line headline every scene opens with: a statement, then a quieter second line. */
export const Caption: React.FC<{head: string; sub?: string; at?: number; out?: number; center?: boolean}> = ({
	head,
	sub,
	at = 0,
	out,
	center,
}) => {
	const frame = useCurrentFrame();
	const {u, wide} = useLayout();
	const centered = center ?? !wide;
	const gone = out === undefined ? 0 : prog(frame, out, 12);
	return (
		<div
			style={{
				position: 'absolute',
				left: centered ? 60 * u : 120 * u,
				right: centered ? 60 * u : undefined,
				top: wide ? 92 * u : 70 * u,
				textAlign: centered ? 'center' : 'left',
				opacity: 1 - gone,
				filter: gone > 0 ? `blur(${gone * 8}px)` : undefined,
			}}
		>
			<div
				style={{
					fontSize: (wide ? 66 : 60) * u,
					fontWeight: 650,
					letterSpacing: '-0.015em',
					lineHeight: 1.08,
					...enter(frame, at),
				}}
			>
				{head}
			</div>
			{sub ? (
				<div
					style={{
						fontSize: (wide ? 40 : 36) * u,
						fontWeight: 500,
						color: C.ink2,
						letterSpacing: '-0.01em',
						marginTop: 14 * u,
						lineHeight: 1.2,
						...enter(frame, at + 6),
					}}
				>
					{sub}
				</div>
			) : null}
		</div>
	);
};

/** A small honest note in the corner: these are real run results about simulated people. */
export const Provenance: React.FC<{text: string; at?: number}> = ({text, at = 0}) => {
	const frame = useCurrentFrame();
	const {u, wide, W} = useLayout();
	return (
		<div
			style={{
				position: 'absolute',
				left: wide ? 120 * u : 0,
				width: wide ? undefined : W,
				textAlign: wide ? 'left' : 'center',
				bottom: 34 * u,
				fontSize: 24 * u,
				color: C.ink3,
				letterSpacing: '0.01em',
				opacity: prog(frame, at, 20) * 0.95,
			}}
		>
			{text}
		</div>
	);
};

export const personOf = (run: Run, id: string) => run.people.find((p) => p.id === id) as Persona;

/** A post as the app's feed shows it: face, name, handle, number, text. */
export const PostCard: React.FC<{run: Run; post: Post; width: number; size?: number; style?: React.CSSProperties}> = ({
	run,
	post,
	width,
	size = 30,
	style,
}) => {
	const {u} = useLayout();
	const p = personOf(run, post.author);
	const parent = post.parent ? run.posts.find((q) => q.n === post.parent) : undefined;
	return (
		<div
			style={{
				width,
				boxSizing: 'border-box',
				background: C.tile,
				border: `${1.5 * u}px solid ${C.line}`,
				borderRadius: 22 * u,
				padding: `${20 * u}px ${24 * u}px`,
				display: 'flex',
				gap: 18 * u,
				...style,
			}}
		>
			<Avatar {...p} size={56 * u} style={{flex: 'none'}} />
			<div style={{minWidth: 0, flex: 1}}>
				<div style={{display: 'flex', alignItems: 'baseline', gap: 12 * u, whiteSpace: 'nowrap'}}>
					<span style={{fontSize: 27 * u, fontWeight: 650}}>{p.name}</span>
					<span style={{fontSize: 22 * u, fontFamily: MONO, color: C.link, overflow: 'hidden', textOverflow: 'ellipsis'}}>
						@{p.handle}
					</span>
					<span style={{marginLeft: 'auto', fontSize: 22 * u, fontFamily: MONO, color: C.ink3}}>#{post.n}</span>
				</div>
				{parent ? (
					<div style={{fontSize: 20 * u, fontFamily: MONO, color: C.ink3, marginTop: 4 * u}}>
						↪ replying to @{personOf(run, parent.author).handle}
					</div>
				) : null}
				<div
					style={{
						fontSize: size * u,
						lineHeight: 1.32,
						marginTop: 8 * u,
						color: C.ink,
						display: '-webkit-box',
						WebkitLineClamp: 3,
						WebkitBoxOrient: 'vertical',
						overflow: 'hidden',
					}}
				>
					{post.text}
				</div>
			</div>
		</div>
	);
};

/** Catmull-Rom through the points, as cubic Béziers. */
const smoothPath = (pts: [number, number][]) => {
	if (pts.length < 2) return '';
	let d = `M ${pts[0][0]} ${pts[0][1]}`;
	for (let i = 0; i < pts.length - 1; i++) {
		const p0 = pts[Math.max(0, i - 1)];
		const p1 = pts[i];
		const p2 = pts[i + 1];
		const p3 = pts[Math.min(pts.length - 1, i + 2)];
		const k = 1 / 6;
		d += ` C ${p1[0] + (p2[0] - p0[0]) * k} ${p1[1] + (p2[1] - p0[1]) * k}, ${p2[0] - (p3[0] - p1[0]) * k} ${
			p2[1] - (p3[1] - p1[1]) * k
		}, ${p2[0]} ${p2[1]}`;
	}
	return d;
};

/**
 * The round-by-round mood line. `times[i]` is when round i's point lands; the line draws towards it over `draw`
 * frames. Values are the run's real means, from −1 to +1.
 */
export const MoodChart: React.FC<{
	run: Run;
	times: number[];
	width: number;
	height: number;
	draw?: number;
	at?: number;
}> = ({run, times, width, height, draw = 22, at = 0}) => {
	const frame = useCurrentFrame();
	const {u} = useLayout();
	const head = 70 * u;
	const padL = 56 * u;
	const inset = 54 * u;
	// Short charts (square and portrait frames) drop the round labels to give the line room.
	const compact = height < 190 * u;
	const plotH = height - head - (compact ? 14 : 40) * u;
	const n = run.mood.length;
	const x = (i: number) => padL + inset + ((width - padL - inset * 2) * i) / Math.max(1, n - 1);
	const y = (v: number) => head + ((1 - v) / 2) * plotH;
	const pts = run.mood.map((v, i) => [x(i), y(v)] as [number, number]);

	// How far along the line has drawn, in rounds (0 … n-1).
	let reach = -1;
	times.forEach((t, i) => {
		const k = prog(frame, t, draw, i === 0 ? easeOut : easeInOut);
		if (k > 0) reach = i === 0 ? k - 1 : i - 1 + k;
	});
	// x() is linear in the round, so it takes the fractional reach directly.
	const clipW = reach < 0 ? 0 : reach >= n - 1 ? width : x(reach) + 2;

	// The headline value: the last round whose point has landed. Only real round means are ever shown.
	let landed = -1;
	times.forEach((t, i) => {
		if (frame >= t + (i === 0 ? 0 : draw - 6)) landed = i;
	});
	const now = landed < 0 ? 0 : run.mood[landed];
	const ev = run.event;
	const evShow = ev ? prog(frame, times[ev.round] - 14, 14) : 0;
	const appear = prog(frame, at, 20);

	return (
		<div style={{width, height, position: 'relative', opacity: appear}}>
			<div style={{position: 'absolute', left: 0, right: 0, top: 0, display: 'flex', alignItems: 'baseline'}}>
				<span style={{fontSize: 28 * u, fontWeight: 600, color: C.ink2}}>Mood, round by round</span>
				<span
					style={{
						marginLeft: 'auto',
						fontSize: 46 * u,
						fontWeight: 650,
						fontFamily: MONO,
						letterSpacing: '-0.02em',
						color: landed < 0 ? C.ink3 : now >= 0 ? C.up : C.down,
						fontVariantNumeric: 'tabular-nums',
					}}
				>
					{landed < 0 ? '—' : signed(now)}
				</span>
			</div>
			<svg width={width} height={height} style={{position: 'absolute', left: 0, top: 0, overflow: 'visible'}}>
				<defs>
					<clipPath id={`mood-${run.key}`}>
						<rect x={0} y={0} width={Math.max(0, clipW)} height={height} />
					</clipPath>
				</defs>
				<line x1={padL} x2={width} y1={y(0)} y2={y(0)} stroke={C.line} strokeWidth={1.5 * u} strokeDasharray={`${6 * u} ${6 * u}`} />
				{[1, -1].map((v) => (
					<text key={v} x={padL - 14 * u} y={y(v) + 8 * u} fill={C.ink3} fontSize={22 * u} fontFamily={MONO} textAnchor="end">
						{v > 0 ? '+1' : '−1'}
					</text>
				))}
				<text x={padL - 14 * u} y={y(0) + 8 * u} fill={C.ink3} fontSize={22 * u} fontFamily={MONO} textAnchor="end">
					0
				</text>
				{compact ? null : run.mood.map((_, i) => (
					<text key={i} x={x(i)} y={height - 4 * u} fill={C.ink3} fontSize={21 * u} fontFamily={MONO} textAnchor="middle">
						{`R${i}`}
					</text>
				))}
				{ev ? (
					<g opacity={evShow}>
						<line
							x1={x(ev.round)}
							x2={x(ev.round)}
							y1={head - 6 * u}
							y2={head + plotH}
							stroke={C.event}
							strokeWidth={2 * u}
							strokeDasharray={`${6 * u} ${6 * u}`}
						/>
						<path
							transform={`translate(${x(ev.round) - 11 * u} ${head - 30 * u}) scale(${1.1 * u})`}
							d="M11.25 2 4.5 11h5l-1.25 7L15.5 9h-5z"
							fill={C.event}
						/>
					</g>
				) : null}
				<g clipPath={`url(#mood-${run.key})`}>
					<path d={smoothPath(pts)} fill="none" stroke={C.ink} strokeWidth={3.5 * u} strokeLinecap="round" />
				</g>
				{pts.map(([px, py], i) => {
					const k = prog(frame, times[i] + (i === 0 ? 0 : draw - 6), 12);
					const v = run.mood[i];
					// Labels go above the point unless the line came down into it, or there is no room under the readout.
					const below = (i > 0 && run.mood[i - 1] > v) || py - head < 34 * u;
					return (
						<g key={i} opacity={k}>
							<circle cx={px} cy={py} r={(7 + (1 - k) * 6) * u} fill={v >= 0 ? C.up : C.down} />
							<text
								x={px}
								y={Math.min(height - 2 * u, Math.max(head + 18 * u, py + (below ? (compact ? 32 : 40) : -18) * u))}
								fill={C.ink2}
								fontSize={24 * u}
								fontFamily={MONO}
								textAnchor="middle"
							>
								{signed(v)}
							</text>
						</g>
					);
				})}
			</svg>
		</div>
	);
};

/** The brand fan: six pleats around a hinge. `open[i]` is 0 (folded up into the top pleat) to 1 (in place). */
export const Fan: React.FC<{size: number; open?: number[]; hinge?: number; style?: React.CSSProperties}> = ({
	size,
	open = [1, 1, 1, 1, 1, 1],
	hinge = 1,
	style,
}) => {
	const R = 160;
	const hx = 54;
	const hy = 198;
	const pt = (deg: number) => [hx + R * Math.cos((deg * Math.PI) / 180), hy - R * Math.sin((deg * Math.PI) / 180)];
	return (
		<svg width={size} height={size} viewBox="30 30 196 196" style={{overflow: 'visible', ...style}}>
			{[0, 1, 2, 3, 4, 5].map((i) => {
				const k = open[i] ?? 1;
				// A folded pleat sits on the top one (75°–90°); unfolding swings it down to its own slot.
				const rot = (1 - k) * (75 - 15 * i);
				const [ax, ay] = pt(15 * i);
				const [bx, by] = pt(15 * (i + 1));
				return (
					<path
						key={i}
						d={`M${hx} ${hy}L${ax} ${ay}L${bx} ${by}Z`}
						fill={i % 2 === 0 ? C.fanA : C.fanB}
						transform={`rotate(${-rot} ${hx} ${hy})`}
						opacity={k > 0 || i === 5 ? 1 : 0}
					/>
				);
			})}
			<circle cx={hx} cy={hy} r={16 * hinge} fill={C.hinge} />
		</svg>
	);
};

export {easeInOut, easeOut};
