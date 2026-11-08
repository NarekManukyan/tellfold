import React from 'react';
import {Img, staticFile, useCurrentFrame} from 'remotion';
import {Fan, Stage, enter, useLayout} from '../components';
import {C, easeOut, mix, prog} from '../theme';

/** The fan opens pleat by pleat, the wordmark follows, then the line and the offer. */
export const End: React.FC = () => {
	const frame = useCurrentFrame();
	const {u, W, H, wide} = useLayout();
	const s = (v: number) => v * u;

	const open = [0, 1, 2, 3, 4, 5].map((i) => prog(frame, 8 + (5 - i) * 4, 30, easeOut));
	const fanIn = prog(frame, 0, 16);
	const hinge = mix(0.6, 1, prog(frame, 18, 20, easeOut));
	const word = prog(frame, 30, 30, easeOut);

	const fanSize = s(wide ? 190 : 200);
	const wordH = s(wide ? 128 : 112);
	const wordW = (wordH * 672) / 176;
	const gap = s(wide ? 36 : 0);
	const groupW = wide ? fanSize + gap + wordW : wordW;
	const top = wide ? H * 0.3 : H * (H > W ? 0.24 : 0.17);

	return (
		<Stage glow={[W / 2, H * 0.45]}>
			<div
				style={{
					position: 'absolute',
					left: (W - groupW) / 2,
					top,
					display: 'flex',
					flexDirection: wide ? 'row' : 'column',
					alignItems: wide ? 'flex-end' : 'center',
					gap: wide ? gap : s(34),
					width: groupW,
				}}
			>
				<Fan size={fanSize} open={open} hinge={hinge} style={{opacity: fanIn, flex: 'none'}} />
				<div
					style={{
						height: wordH,
						marginBottom: wide ? s(6) : 0,
						clipPath: `inset(-10% ${(1 - word) * 100}% -10% 0)`,
						transform: `translateX(${(1 - word) * -s(20)}px)`,
					}}
				>
					<Img src={staticFile('tellfold-wordmark-white.svg')} style={{height: wordH, width: wordW, display: 'block'}} />
				</div>
			</div>
			<div
				style={{
					position: 'absolute',
					left: s(60),
					right: s(60),
					top: wide ? H * 0.58 : H > W ? H * 0.6 : H * 0.6,
					textAlign: 'center',
				}}
			>
				<div style={{fontSize: s(wide ? 64 : 58), fontWeight: 650, letterSpacing: '-0.015em', lineHeight: 1.12, ...enter(frame, 52, 22)}}>
					See how people react{wide ? ' ' : <br />}before you tell them.
				</div>
				<div style={{display: 'flex', justifyContent: 'center', alignItems: 'center', gap: s(26), marginTop: s(44), ...enter(frame, 64, 22)}}>
					<span
						style={{
							background: C.accent,
							color: '#fff',
							fontSize: s(36),
							fontWeight: 600,
							padding: `${s(16)}px ${s(40)}px`,
							borderRadius: 999,
						}}
					>
						Free for Mac
					</span>
				</div>
				<div style={{fontSize: s(30), color: C.ink2, marginTop: s(26), ...enter(frame, 72, 22)}}>narekmanukyan.github.io/tellfold</div>
			</div>
		</Stage>
	);
};
