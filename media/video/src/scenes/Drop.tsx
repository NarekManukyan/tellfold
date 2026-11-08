import React from 'react';
import {interpolate, useCurrentFrame} from 'remotion';
import {Caption, Fan, Stage, useLayout} from '../components';
import {cafe, seeds} from '../data';
import {C, MONO, easeInOut, easeOut, mix, prog} from '../theme';
import {SeedCard, cardSlot} from './Open';

/** The café note goes into Tellfold's New simulation sheet, and the real run's question is typed in. */
export const Drop: React.FC = () => {
	const frame = useCurrentFrame();
	const {u, W, H, wide, tall} = useLayout();
	const L = {u, W, H, wide, tall};
	const s = (v: number) => v * u;

	const sheet = wide ? {w: 1120, h: 640, cy: 640} : tall ? {w: 980, h: 760, cy: 760} : {w: 980, h: 700, cy: 660};
	const sw = s(sheet.w);
	const sh = s(sheet.h);
	const sx = W / 2 - sw / 2;
	const sy = s(sheet.cy) - sh / 2;
	const pad = s(44);

	const sheetIn = prog(frame, 14, 26);
	// Where the attachment chip sits inside the sheet; the card flies into it.
	const chip = {x: sx + pad, y: sy + s(wide ? 330 : 360), w: sw - pad * 2, h: s(108)};

	const q = cafe.question;
	const typed = Math.round(interpolate(frame, [58, 104], [0, q.length], {extrapolateLeft: 'clamp', extrapolateRight: 'clamp'}));
	const caret = typed < q.length || Math.floor(frame / 8) % 2 === 0;
	const press = interpolate(frame, [110, 114, 122], [1, 0.96, 1], {extrapolateLeft: 'clamp', extrapolateRight: 'clamp'});
	const glow = interpolate(frame, [112, 118, 132], [0, 1, 0], {extrapolateLeft: 'clamp', extrapolateRight: 'clamp'});

	const fly = prog(frame, 34, 30, easeInOut);
	const center = prog(frame, 0, 26, easeInOut);
	const start = cardSlot(0, L);
	const cx = mix(start.x, W / 2 - start.cw / 2, center);
	const cy = mix(start.y, s(wide ? 330 : 320), center);
	const cardOpacity = 1 - prog(frame, 52, 12);

	return (
		<Stage>
			<Caption head="Give it to Tellfold, with your question." sub="A note, a letter, a plan or an announcement." at={4} />

			{/* The other two notes leave. */}
			{[1, 2].map((i) => {
				const sl = cardSlot(i, L);
				const k = prog(frame, i === 1 ? 0 : 4, 26, easeInOut);
				return (
					<div
						key={i}
						style={{
							position: 'absolute',
							left: sl.x,
							top: sl.y,
							opacity: 1 - k,
							transform: `translate(${sl.slot * 160 * u * k}px, ${k * 700 * u}px) rotate(${sl.rot + sl.slot * 10 * k}deg)`,
						}}
					>
						<SeedCard i={i} width={sl.cw} />
					</div>
				);
			})}

			{/* The New simulation sheet. */}
			<div
				style={{
					position: 'absolute',
					left: sx,
					top: sy,
					width: sw,
					height: sh,
					boxSizing: 'border-box',
					padding: pad,
					background: C.tile,
					border: `${s(1.5)}px solid ${C.line}`,
					borderRadius: s(30),
					boxShadow: `0 ${s(40)}px ${s(120)}px rgba(0,0,0,0.6)`,
					opacity: sheetIn,
					transform: `translateY(${(1 - sheetIn) * 40}px) scale(${mix(0.96, 1, sheetIn)})`,
				}}
			>
				<div style={{display: 'flex', alignItems: 'center', gap: s(14)}}>
					<Fan size={s(44)} />
					<span style={{fontSize: s(36), fontWeight: 700, letterSpacing: '-0.02em'}}>New simulation</span>
				</div>
				<div style={{fontSize: s(26), fontWeight: 600, color: C.ink2, marginTop: s(30)}}>What do you want to find out?</div>
				<div
					style={{
						marginTop: s(12),
						background: C.tile2,
						border: `${s(1.5)}px solid ${typed > 0 ? 'rgba(165,180,252,0.6)' : C.line}`,
						borderRadius: s(16),
						padding: `${s(16)}px ${s(20)}px`,
						fontSize: s(32),
						lineHeight: 1.3,
						minHeight: s(wide ? 60 : 100),
					}}
				>
					{q.slice(0, typed)}
					<span style={{opacity: caret && frame > 50 ? 1 : 0, color: C.link, marginLeft: s(2)}}>|</span>
				</div>
				<div style={{fontSize: s(26), fontWeight: 600, color: C.ink2, marginTop: s(wide ? 30 : 26)}}>What is it about?</div>

				<div style={{position: 'absolute', left: pad, right: pad, bottom: pad, display: 'flex', alignItems: 'center'}}>
					<span style={{fontSize: s(26), color: C.ink2}}>8 people · 3 rounds</span>
					<span
						style={{
							marginLeft: 'auto',
							background: C.accent,
							color: '#fff',
							fontSize: s(28),
							fontWeight: 600,
							padding: `${s(14)}px ${s(30)}px`,
							borderRadius: s(16),
							transform: `scale(${press})`,
							boxShadow: `0 0 0 ${s(8) * glow}px rgba(99,102,241,${0.45 * glow})`,
						}}
					>
						Start simulation
					</span>
				</div>
			</div>

			{/* The attachment, once the card has landed. */}
			<div
				style={{
					position: 'absolute',
					left: chip.x,
					top: chip.y,
					width: chip.w,
					height: chip.h,
					boxSizing: 'border-box',
					display: 'flex',
					alignItems: 'center',
					gap: s(20),
					padding: `0 ${s(22)}px`,
					background: C.tile2,
					border: `${s(1.5)}px solid ${C.line}`,
					borderRadius: s(16),
					opacity: prog(frame, 54, 12),
				}}
			>
				<div style={{width: s(54), height: s(68), background: C.paper, borderRadius: s(8), flex: 'none'}} />
				<div style={{minWidth: 0}}>
					<div style={{fontSize: s(28), fontWeight: 600, whiteSpace: 'nowrap', overflow: 'hidden', textOverflow: 'ellipsis'}}>
						{seeds[0].title}
					</div>
					<div style={{fontSize: s(22), fontFamily: MONO, color: C.ink3, marginTop: s(4)}}>{seeds[0].file}</div>
				</div>
			</div>

			{/* The café note itself: centres, then shrinks into the attachment slot. */}
			<div
				style={{
					position: 'absolute',
					left: mix(cx, chip.x, fly),
					top: mix(cy, chip.y, fly),
					opacity: cardOpacity,
					transformOrigin: 'top left',
					transform: `rotate(${mix(start.rot, 0, center)}deg) scale(${mix(1, chip.w / start.cw / 3, fly)})`,
				}}
			>
				<SeedCard i={0} width={start.cw} />
			</div>
		</Stage>
	);
};

export {easeOut};
