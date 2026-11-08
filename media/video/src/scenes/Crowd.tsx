import React from 'react';
import {interpolate, useCurrentFrame} from 'remotion';
import {Avatar} from '../Avatar';
import {Caption, MoodChart, PostCard, Provenance, Stage, enter, useLayout} from '../components';
import type {Run} from '../data';
import {C, MONO, easeInOut, easeOut, mix, prog} from '../theme';

export type CrowdBeat = {
	run: Run;
	head: string;
	sub: string;
	/** When each round starts (and its mood point lands). */
	rounds: number[];
	/** Every post's moment, by post number; `feed` marks the ones shown in full. */
	posts: {n: number; at: number; feed?: boolean}[];
	likes?: {from: string; n: number; at: number}[];
	eventAt?: number;
	provenance: string;
	headAt?: number;
};

type FeedItem = {kind: 'post'; n: number; at: number} | {kind: 'event'; at: number};

export const Crowd: React.FC<CrowdBeat> = ({run, head, sub, rounds, posts, likes = [], eventAt, provenance, headAt = 0}) => {
	const frame = useCurrentFrame();
	const {u, wide, tall} = useLayout();

	const L = wide
		? {cx: 520, cy: 620, r: 250, av: 124, feed: {x: 1000, y: 276, w: 800, h: 520}, chart: {x: 1000, y: 818, w: 800, h: 212}, num: 118}
		: tall
			? {cx: 540, cy: 500, r: 200, av: 104, feed: {x: 60, y: 772, w: 960, h: 318}, chart: {x: 60, y: 1110, w: 960, h: 170}, num: 92}
			: {cx: 540, cy: 418, r: 160, av: 88, feed: {x: 60, y: 622, w: 960, h: 222}, chart: {x: 60, y: 856, w: 960, h: 156}, num: 76};
	const s = (v: number) => v * u;
	const cardH = s(wide ? 250 : 218);
	const eventH = s(wide ? 200 : 170);
	const gap = s(18);

	const n = run.people.length;
	const pos = run.people.map((_, i) => {
		const a = -Math.PI / 2 + (i * 2 * Math.PI) / n;
		return [s(L.cx + L.r * Math.cos(a)), s(L.cy + L.r * Math.sin(a))] as [number, number];
	});
	const idx = (id: string) => run.people.findIndex((p) => p.id === id);
	const byN = (num: number) => run.posts.find((p) => p.n === num);

	// The ring: who posted, who answered whom.
	const authorOf = (num: number) => {
		const p = byN(num);
		if (p) return p.author;
		return null;
	};

	const evK = eventAt === undefined ? 0 : prog(frame, eventAt, 30, easeOut);
	const evPulse = eventAt === undefined ? 0 : interpolate(frame, [eventAt, eventAt + 6, eventAt + 22], [0, 1, 0], {extrapolateLeft: 'clamp', extrapolateRight: 'clamp'});
	const bump = 1 + evPulse * 0.012;

	// Current round, for the big readout in the ring.
	let round = -1;
	rounds.forEach((t, i) => {
		if (frame >= t) round = i;
	});

	const feedItems: FeedItem[] = [
		...posts.filter((p) => p.feed).map((p) => ({kind: 'post' as const, n: p.n, at: p.at})),
		...(eventAt !== undefined ? [{kind: 'event' as const, at: eventAt}] : []),
	].sort((a, b) => a.at - b.at);
	const height = (it: FeedItem) => (it.kind === 'event' ? eventH : cardH);

	return (
		<Stage glow={[s(L.cx), s(L.cy)]}>
			<div style={{position: 'absolute', inset: 0, transform: `scale(${bump})`}}>
				<Caption head={head} sub={sub} at={headAt} />

				{/* Reply threads and likes, under the faces. */}
				<svg style={{position: 'absolute', left: 0, top: 0, width: '100%', height: '100%', overflow: 'visible'}}>
					{posts.map((p) => {
						const post = byN(p.n);
						if (!post?.parent) return null;
						const to = authorOf(post.parent);
						if (!to) return null;
						const a = pos[idx(post.author)];
						const b = pos[idx(to)];
						const mx = mix((a[0] + b[0]) / 2, s(L.cx), 0.55);
						const my = mix((a[1] + b[1]) / 2, s(L.cy), 0.55);
						const k = prog(frame, p.at, 20, easeInOut);
						const len = Math.hypot(b[0] - a[0], b[1] - a[1]) * 1.25;
						const settle = interpolate(frame, [p.at + 20, p.at + 60], [1, 0.38], {extrapolateLeft: 'clamp', extrapolateRight: 'clamp'});
						return (
							<path
								key={`r${p.n}`}
								d={`M${a[0]} ${a[1]} Q${mx} ${my} ${b[0]} ${b[1]}`}
								fill="none"
								stroke={C.link}
								strokeWidth={s(3)}
								strokeLinecap="round"
								strokeDasharray={len}
								strokeDashoffset={len * (1 - k)}
								opacity={k > 0 ? settle : 0}
							/>
						);
					})}
					{likes.map((l, i) => {
						const target = authorOf(l.n);
						if (!target) return null;
						const a = pos[idx(l.from)];
						const b = pos[idx(target)];
						const k = prog(frame, l.at, 22, easeInOut);
						if (k <= 0 || frame > l.at + 34) return null;
						const mx = mix((a[0] + b[0]) / 2, s(L.cx), 0.35);
						const my = mix((a[1] + b[1]) / 2, s(L.cy), 0.35);
						const x = (1 - k) * (1 - k) * a[0] + 2 * (1 - k) * k * mx + k * k * b[0];
						const y = (1 - k) * (1 - k) * a[1] + 2 * (1 - k) * k * my + k * k * b[1];
						const pop = interpolate(frame, [l.at + 22, l.at + 28, l.at + 34], [1, 1.35, 0], {extrapolateLeft: 'clamp', extrapolateRight: 'clamp'});
						const sc = s(1.5) * pop * (0.9 + 0.1 * k);
						return (
							<path
								key={`l${i}`}
								transform={`translate(${x} ${y}) scale(${sc}) translate(-10 -10)`}
								d="M10 17.5s-6.5-4-6.5-8.6A3.6 3.6 0 0 1 10 6.6a3.6 3.6 0 0 1 6.5 2.3c0 4.6-6.5 8.6-6.5 8.6z"
								fill="#EC4899"
							/>
						);
					})}
					{/* Breaking news ripples out from the middle of the crowd. */}
					{eventAt !== undefined && evK > 0 && evK < 1 ? (
						<circle cx={s(L.cx)} cy={s(L.cy)} r={s(L.r * 1.7) * evK} fill="none" stroke={C.event} strokeWidth={s(5) * (1 - evK)} opacity={1 - evK} />
					) : null}
				</svg>

				{/* The round readout in the middle of the ring. */}
				<div
					style={{
						position: 'absolute',
						left: s(L.cx) - s(160),
						top: s(L.cy) - s(L.num * 0.95),
						width: s(320),
						textAlign: 'center',
						opacity: prog(frame, rounds[0] - 8, 16),
					}}
				>
					<div
						style={{
							position: 'absolute',
							left: s(160) - s(L.num * 1.25),
							top: s(L.num * 0.95) - s(L.num * 1.25),
							width: s(L.num * 2.5),
							height: s(L.num * 2.5),
							borderRadius: '50%',
							background: 'radial-gradient(closest-side, rgba(0,0,0,0.92) 72%, rgba(0,0,0,0) 100%)',
						}}
					/>
					<div style={{position: 'relative', fontSize: s(26), fontWeight: 600, color: C.ink3, letterSpacing: '0.06em', textTransform: 'uppercase'}}>Round</div>
					<div style={{position: 'relative', height: s(L.num * 1.1), overflow: 'hidden'}}>
						{rounds.map((t, i) => {
							const inK = prog(frame, t, 16);
							const outK = i + 1 < rounds.length ? prog(frame, rounds[i + 1], 16) : 0;
							if (inK <= 0 || outK >= 1) return null;
							return (
								<div
									key={i}
									style={{
										position: 'absolute',
										left: 0,
										right: 0,
										fontSize: s(L.num),
										fontWeight: 650,
										lineHeight: 1.1,
										fontVariantNumeric: 'tabular-nums',
										letterSpacing: '-0.03em',
										transform: `translateY(${(1 - inK) * 60 - outK * 60}%)`,
										opacity: inK * (1 - outK),
									}}
								>
									{i}
								</div>
							);
						})}
					</div>
					<div style={{position: 'relative', fontSize: s(22), fontFamily: MONO, color: C.ink3, marginTop: s(2)}}>
						{n} people · {round < 0 ? 0 : round} / {rounds.length - 1}
					</div>
				</div>

				{/* The faces. */}
				{run.people.map((p, i) => {
					const t = 4 + i * 4;
					const k = prog(frame, t, 22);
					const mine = posts.filter((q) => byN(q.n)?.author === p.id);
					const pulse = Math.max(0, ...mine.map((q) => interpolate(frame, [q.at, q.at + 26], [1, 0], {extrapolateLeft: 'clamp', extrapolateRight: 'clamp'}) * (frame >= q.at ? 1 : 0)));
					const lastPost = mine.filter((q) => frame >= q.at).at(-1);
					const chip = lastPost ? interpolate(frame, [lastPost.at, lastPost.at + 10, lastPost.at + 50, lastPost.at + 62], [0, 1, 1, 0], {extrapolateLeft: 'clamp', extrapolateRight: 'clamp'}) : 0;
					const amber = eventAt === undefined ? 0 : interpolate(frame, [eventAt + 4, eventAt + 14, eventAt + 50], [0, 1, 0], {extrapolateLeft: 'clamp', extrapolateRight: 'clamp'});
					const [x, y] = pos[i];
					const size = s(L.av);
					return (
						<div
							key={p.id}
							style={{
								position: 'absolute',
								left: x - size / 2,
								top: y - size / 2,
								width: size,
								height: size,
								opacity: k,
								transform: `scale(${mix(0.86, 1, k) * (1 + pulse * 0.08)})`,
								filter: k < 1 ? `blur(${(1 - k) * 6}px)` : undefined,
							}}
						>
							<div
								style={{
									position: 'absolute',
									inset: -s(7),
									borderRadius: p.org ? s(L.av * 0.26) : '50%',
									boxShadow: `0 0 0 ${s(3 + pulse * 4)}px rgba(165,180,252,${pulse * 0.9}), 0 0 0 ${s(3)}px rgba(251,191,36,${amber})`,
								}}
							/>
							<Avatar {...p} size={size} />
							{lastPost ? (
								<div
									style={{
										position: 'absolute',
										left: size * 0.62,
										top: -s(18),
										background: C.accent,
										color: '#fff',
										fontFamily: MONO,
										fontSize: s(22),
										fontWeight: 600,
										padding: `${s(4)}px ${s(12)}px`,
										borderRadius: s(14),
										opacity: chip,
										transform: `translateY(${(1 - chip) * 10}px) scale(${mix(0.9, 1, chip)})`,
										transformOrigin: 'left bottom',
										whiteSpace: 'nowrap',
									}}
								>
									#{lastPost.n}
								</div>
							) : null}
						</div>
					);
				})}

				{/* The feed: newest on top, older posts slide down and out. */}
				<div style={{position: 'absolute', left: s(L.feed.x), top: s(L.feed.y), width: s(L.feed.w), height: s(L.feed.h), overflow: 'hidden'}}>
					{feedItems.map((it, i) => {
						const newer = feedItems.slice(i + 1);
						const y = newer.reduce((acc, nw) => acc + (height(nw) + gap) * prog(frame, nw.at, 22, easeInOut), 0);
						const h = height(it);
						const overflow = y + h - s(L.feed.h);
						const keep = overflow > 0 ? Math.max(0, 1 - overflow / (h * 0.25)) : 1;
						const st = enter(frame, it.at + 10, 20, -s(20));
						if (frame < it.at) return null;
						return (
							<div key={i} style={{position: 'absolute', left: 0, top: y, width: '100%', ...st, opacity: (st.opacity as number) * keep}}>
								{it.kind === 'post' ? (
									<PostCard run={run} post={byN(it.n)!} width={s(L.feed.w)} style={{height: cardH}} />
								) : (
									<EventCard run={run} height={eventH} />
								)}
							</div>
						);
					})}
				</div>

				<div style={{position: 'absolute', left: s(L.chart.x), top: s(L.chart.y)}}>
					<MoodChart run={run} times={rounds} width={s(L.chart.w)} height={s(L.chart.h)} at={rounds[0] - 10} />
				</div>
				<Provenance text={provenance} at={20} />
			</div>
		</Stage>
	);
};

const EventCard: React.FC<{run: Run; height: number}> = ({run, height}) => {
	const {u} = useLayout();
	return (
		<div
			style={{
				height,
				boxSizing: 'border-box',
				background: C.eventBg,
				border: `${1.5 * u}px solid rgba(251,191,36,0.45)`,
				borderRadius: 22 * u,
				padding: `${18 * u}px ${24 * u}px`,
			}}
		>
			<div style={{display: 'flex', alignItems: 'center', gap: 10 * u, color: C.event, fontSize: 26 * u, fontWeight: 650}}>
				<svg width={26 * u} height={26 * u} viewBox="0 0 20 20">
					<path d="M11.25 2 4.5 11h5l-1.25 7L15.5 9h-5z" fill={C.event} />
				</svg>
				Breaking news, round {run.event?.round}
			</div>
			<div style={{fontSize: 29 * u, lineHeight: 1.3, marginTop: 8 * u, color: '#fde68a'}}>{run.event?.text}</div>
		</div>
	);
};
