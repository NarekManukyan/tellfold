import React from 'react';
import {useCurrentFrame} from 'remotion';
import {Stage, useLayout} from '../components';
import {seeds} from '../data';
import {C, FONT, MONO, easeInOut, easeOut, mix, prog} from '../theme';

/** A seed document as a sheet of paper: what someone is about to send out. */
export const SeedCard: React.FC<{i: number; width: number; style?: React.CSSProperties}> = ({i, width, style}) => {
	const {u} = useLayout();
	const sd = seeds[i];
	const k = width / (500 * u);
	return (
		<div
			style={{
				width,
				height: width * 0.7,
				boxSizing: 'border-box',
				background: C.paper,
				color: C.paperInk,
				borderRadius: 24 * u * k,
				padding: `${30 * u * k}px ${32 * u * k}px`,
				boxShadow: `0 ${30 * u}px ${80 * u}px rgba(0,0,0,0.55), 0 0 0 ${1 * u}px rgba(255,255,255,0.08)`,
				fontFamily: FONT,
				display: 'flex',
				flexDirection: 'column',
				...style,
			}}
		>
			<div style={{fontSize: 24 * u * k, fontWeight: 650, color: '#4338ca'}}>{sd.who}</div>
			<div style={{fontSize: 34 * u * k, fontWeight: 700, lineHeight: 1.15, letterSpacing: '-0.015em', marginTop: 10 * u * k}}>
				{sd.title}
			</div>
			<div style={{fontSize: 23 * u * k, lineHeight: 1.4, color: C.paperInk2, marginTop: 14 * u * k}}>{sd.body}</div>
			<div style={{marginTop: 'auto', fontSize: 19 * u * k, fontFamily: MONO, color: '#8e8e93'}}>{sd.file}</div>
		</div>
	);
};

/** Where seed card i rests at the end of the opening (the drop scene starts from here). */
export const cardSlot = (i: number, {u, W, H, wide, tall}: {u: number; W: number; H: number; wide: boolean; tall: boolean}) => {
	const cw = (wide ? 500 : tall ? 440 : 400) * u;
	const spread = (wide ? 540 : tall ? 300 : 290) * u;
	const baseY = wide ? H * 0.6 : tall ? H * 0.58 : H * 0.6;
	const slot = i - 1;
	return {x: W / 2 + slot * spread - cw / 2, y: baseY - cw * 0.35 + Math.abs(slot) * 26 * u, rot: slot * 7, cw, slot};
};

/** Opening: someone has news. Three real seed documents fan out like a hand of cards. */
export const Open: React.FC = () => {
	const frame = useCurrentFrame();
	const {u, W, H, wide, tall} = useLayout();

	// The first line sits in the middle, then rises to make room for the cards.
	const rise = prog(frame, 46, 26, easeInOut);
	const line1 = prog(frame, 4, 22);
	const swap = prog(frame, 96, 16);
	const leave = prog(frame, 122, 12);
	const headSize = (wide ? 84 : 70) * u;
	const topY = mix(H / 2 - headSize * 0.6, (wide ? 120 : 110) * u, rise);

	return (
		<Stage>
			<div
				style={{
					position: 'absolute',
					left: 0,
					width: W,
					top: topY,
					textAlign: 'center',
					fontSize: headSize,
					fontWeight: 650,
					letterSpacing: '-0.02em',
					opacity: 1 - leave,
				}}
			>
				<div
					style={{
						opacity: line1 * (1 - swap),
						transform: `translateY(${(1 - line1) * 30}px)`,
						filter: `blur(${(1 - line1) * 10 + swap * 8}px)`,
					}}
				>
					You have news to share.
				</div>
				<div
					style={{
						position: 'absolute',
						left: 0,
						right: 0,
						top: 0,
						opacity: swap,
						filter: `blur(${(1 - swap) * 8}px)`,
						transform: `translateY(${(1 - swap) * 20}px)`,
					}}
				>
					How will people take it?
				</div>
			</div>
			{[0, 1, 2].map((i) => {
				// Dealt like cards: left, right, then the middle one lands on top.
				const order = [0, 2, 1][i];
				const at = 54 + order * 9;
				const k = prog(frame, at, 30, easeOut);
				const {x, y, rot, cw, slot} = cardSlot(i, {u, W, H, wide, tall});
				return (
					<div
						key={i}
						style={{
							position: 'absolute',
							left: x,
							top: y,
							zIndex: i === 1 ? 2 : 1,
							opacity: Math.min(1, k * 1.6),
							transform: `translate(${(1 - k) * -slot * 120 * u}px, ${(1 - k) * 720 * u}px) rotate(${mix(rot * 3 + (slot === 0 ? -8 : 0), rot, k)}deg)`,
						}}
					>
						<SeedCard i={i} width={cw} />
					</div>
				);
			})}
		</Stage>
	);
};
