import React from 'react';
import {interpolateColors, useCurrentFrame} from 'remotion';
import {Avatar} from '../Avatar';
import {Caption, PostCard, Provenance, Stage, enter, personOf, useLayout} from '../components';
import {mossbook, mossbookSummary} from '../data';
import type {Post} from '../data';
import {C, MONO, easeInOut, easeOut, mix, prog} from '../theme';

const CITED: {cite: string[]; post: Post; at: number}[] = [
	{cite: ['@ines_okafor', '#2'], post: mossbook.posts.find((p) => p.n === 2)!, at: 196},
	{cite: ['@garden_collective_pm', '#4'], post: mossbook.posts.find((p) => p.n === 4)!, at: 220},
	{cite: ['@garden_skeptic', '#6'], post: mossbook.posts.find((p) => p.n === 6)!, at: 244},
];

/**
 * Mossbook's six simulated people sit on the six pleats of the Tellfold fan. The fan folds shut, and its hinge opens
 * into the report: one verdict, every claim citing the posts behind it.
 */
export const Fold: React.FC = () => {
	const frame = useCurrentFrame();
	const {u, W, H, wide, tall} = useLayout();
	const s = (v: number) => v * u;

	const hinge = wide ? [s(720), s(900)] : tall ? [s(330), s(1040)] : [s(330), s(900)];
	const R = s(wide ? 500 : tall ? 520 : 470);
	const av = s(wide ? 112 : 100);

	const fold = prog(frame, 78, 44, easeInOut);
	const shrink = prog(frame, 118, 22, easeInOut);
	const morph = prog(frame, 132, 30, easeOut);

	const card = wide
		? {x: s(120), y: s(262), w: s(1060), h: s(690)}
		: tall
			? {x: s(60), y: s(250), w: s(960), h: s(560)}
			: {x: s(60), y: s(232), w: s(960), h: s(520)};
	const content = frame >= 150;
	const rad = (d: number) => (d * Math.PI) / 180;

	return (
		<Stage glow={[card.x + card.w / 2, card.y + card.h / 2]}>
			<Caption head="Then it folds into one answer." sub="Six simulated people, two rounds, one report." at={2} out={140} />
			<Caption head="Every claim cites the posts behind it." sub="Open any one to see who said it, and when." at={156} />

			{/* The pleats, then the people on their edges. */}
			{morph < 1 ? (
				<svg style={{position: 'absolute', left: 0, top: 0, width: W, height: H, overflow: 'visible'}}>
					{[0, 1, 2, 3, 4, 5].map((i) => {
						const grow = prog(frame, 26 + i * 5, 28, easeOut);
						const r = R * grow * (1 - shrink) * 0.86;
						const off = fold * (75 - 15 * i);
						const a0 = rad(15 * i + off);
						const a1 = rad(15 * (i + 1) + off);
						const [hx, hy] = hinge;
						return (
							<path
								key={i}
								d={`M${hx} ${hy}L${hx + r * Math.cos(a0)} ${hy - r * Math.sin(a0)}L${hx + r * Math.cos(a1)} ${hy - r * Math.sin(a1)}Z`}
								fill={i % 2 === 0 ? C.fanA : C.fanB}
								opacity={grow * (1 - morph)}
							/>
						);
					})}
					<circle cx={hinge[0]} cy={hinge[1]} r={s(30) * prog(frame, 30, 20)} fill={C.hinge} opacity={1 - morph} />
				</svg>
			) : null}

			{mossbook.people.map((p, i) => {
				const k = prog(frame, 4 + i * 5, 26);
				const ang = rad(7.5 + 15 * i + fold * (75 - 15 * i));
				const r = mix(R * 0.55, R, k) * (1 - shrink);
				const x = hinge[0] + r * Math.cos(ang);
				const y = hinge[1] - r * Math.sin(ang);
				const gone = prog(frame, 100, 22);
				return (
					<div
						key={p.id}
						style={{
							position: 'absolute',
							left: x - av / 2,
							top: y - av / 2,
							opacity: k * (1 - gone),
							transform: `scale(${mix(0.86, 1, k) * mix(1, 0.6, gone)})`,
						}}
					>
						<div style={{borderRadius: p.org ? av * 0.24 : '50%', boxShadow: `0 0 0 ${s(5)}px #000`}}>
							<Avatar {...p} size={av} />
						</div>
					</div>
				);
			})}

			{/* The hinge opens into the report card. */}
			{morph > 0 ? (
				<div
					style={{
						position: 'absolute',
						left: mix(hinge[0] - s(30), card.x, morph),
						top: mix(hinge[1] - s(30), card.y, morph),
						width: mix(s(60), card.w, morph),
						height: mix(s(60), card.h, morph),
						borderRadius: mix(s(30), s(30), morph),
						background: interpolateColors(morph, [0, 0.6, 1], [C.hinge, C.tile, C.tile]),
						border: `${s(1.5)}px solid ${C.line}`,
						boxShadow: `0 ${s(40)}px ${s(120)}px rgba(0,0,0,0.6)`,
						overflow: 'hidden',
					}}
				>
					{content ? <ReportBody width={card.w} /> : null}
				</div>
			) : null}

			{/* The cited posts. */}
			{wide ? (
				<div style={{position: 'absolute', left: s(1250), top: s(262), width: s(560)}}>
					{CITED.map((c, i) => (
						<div key={i} style={{marginBottom: s(15), ...enter(frame, c.at, 22, s(24))}}>
							<PostCard
								run={mossbook}
								post={c.post}
								width={s(560)}
								size={26}
								style={{
									height: s(220),
									borderColor: `rgba(165,180,252,${0.9 * hot(frame, c.at)})`,
									boxShadow: `0 0 0 ${s(4) * hot(frame, c.at)}px rgba(99,102,241,0.35)`,
								}}
							/>
						</div>
					))}
				</div>
			) : (
				<div style={{position: 'absolute', left: s(60), top: card.y + card.h + s(24), width: s(960)}}>
					{CITED.map((c, i) => {
						const next = CITED[i + 1];
						// Square frames show one at a time; tall frames stack all three.
						const away = tall ? 0 : next ? prog(frame, next.at, 14) : 0;
						return (
							<div
								key={i}
								style={{
									position: tall ? 'relative' : 'absolute',
									top: 0,
									left: 0,
									marginBottom: tall ? s(14) : 0,
									...enter(frame, c.at, 20, s(20)),
									opacity: prog(frame, c.at, 20) * (1 - away),
								}}
							>
								<CompactPost post={c.post} lit={hot(frame, c.at)} />
							</div>
						);
					})}
				</div>
			)}
			<Provenance text="Mossbook pricing example · a real Tellfold run · the people are simulated" at={10} />
		</Stage>
	);
};

/** How lit a citation is: on when it is first pointed at, settling to a quiet outline. */
const hot = (frame: number, at: number) => (frame < at ? 0 : mix(1, 0.35, prog(frame, at + 16, 30)));

const CompactPost: React.FC<{post: Post; lit: number}> = ({post, lit}) => {
	const {u} = useLayout();
	const p = personOf(mossbook, post.author);
	return (
		<div
			style={{
				display: 'flex',
				alignItems: 'center',
				gap: 16 * u,
				width: 960 * u,
				boxSizing: 'border-box',
				padding: `${14 * u}px ${20 * u}px`,
				background: C.tile,
				borderRadius: 20 * u,
				border: `${1.5 * u}px solid rgba(165,180,252,${0.15 + 0.75 * lit})`,
			}}
		>
			<Avatar {...p} size={52 * u} style={{flex: 'none'}} />
			<div style={{minWidth: 0}}>
				<div style={{fontSize: 22 * u, fontFamily: MONO, color: C.link}}>
					@{p.handle} <span style={{color: C.ink3}}>#{post.n}</span>
				</div>
				<div style={{fontSize: 27 * u, whiteSpace: 'nowrap', overflow: 'hidden', textOverflow: 'ellipsis', marginTop: 2 * u}}>
					{post.text}
				</div>
			</div>
		</div>
	);
};

const ReportBody: React.FC<{width: number}> = ({width}) => {
	const frame = useCurrentFrame();
	const {u, wide} = useLayout();
	const s = (v: number) => v * u;
	const pad = s(wide ? 52 : 40);
	const litFor = (cite: string) => {
		const c = CITED.find((x) => x.cite.includes(cite));
		return c ? hot(frame, c.at) : 0;
	};
	return (
		<div style={{padding: pad, width, boxSizing: 'border-box'}}>
			<div style={{display: 'flex', gap: s(12), ...enter(frame, 150, 18, s(16))}}>
				<Pill>
					<span style={{width: s(14), height: s(14), borderRadius: '50%', background: '#34d399', display: 'inline-block'}} /> Done
				</Pill>
				<Pill>Medium confidence</Pill>
			</div>
			<div style={{fontSize: s(25), color: C.ink2, marginTop: s(22), ...enter(frame, 154, 18, s(16))}}>{mossbook.question}</div>
			<div
				style={{
					fontSize: s(wide ? 54 : 44),
					fontWeight: 700,
					letterSpacing: '-0.025em',
					lineHeight: 1.1,
					marginTop: s(12),
					...enter(frame, 158, 20, s(18)),
				}}
			>
				{mossbook.verdict}
			</div>
			<div style={{fontSize: s(wide ? 30 : 26), lineHeight: 1.45, color: C.ink, marginTop: s(24), ...enter(frame, 166, 20, s(18))}}>
				{mossbookSummary.map((part, i) =>
					typeof part === 'string' ? (
						<span key={i}>{part}</span>
					) : (
						<span
							key={i}
							style={{
								fontFamily: MONO,
								fontSize: '0.9em',
								color: C.link,
								background: `rgba(99,102,241,${0.55 * litFor(part.cite)})`,
								borderRadius: s(8),
								padding: `0 ${s(4)}px`,
							}}
						>
							{part.cite}
						</span>
					),
				)}
			</div>
			<div style={{fontSize: s(22), color: C.ink3, marginTop: s(22), ...enter(frame, 176, 20, s(12))}}>
				A scenario exploration of a simulated crowd, not a poll of real people.
			</div>
		</div>
	);
};

const Pill: React.FC<{children: React.ReactNode}> = ({children}) => {
	const {u} = useLayout();
	return (
		<span
			style={{
				display: 'inline-flex',
				alignItems: 'center',
				gap: 10 * u,
				fontSize: 24 * u,
				fontWeight: 600,
				padding: `${8 * u}px ${18 * u}px`,
				borderRadius: 999,
				border: `${1.5 * u}px solid ${C.line}`,
				background: C.tile2,
			}}
		>
			{children}
		</span>
	);
};
