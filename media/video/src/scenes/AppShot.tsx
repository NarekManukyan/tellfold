import React from 'react';
import {Img, staticFile, useCurrentFrame} from 'remotion';
import {Caption, Provenance, Stage, useLayout} from '../components';
import {C, easeInOut, easeOut, mix, prog} from '../theme';

/** The same Mossbook run in the real app: a capture of the Overview, rising into place. */
export const AppShot: React.FC = () => {
	const frame = useCurrentFrame();
	const {u, W, H, wide} = useLayout();
	const s = (v: number) => v * u;
	const rise = prog(frame, 0, 54, easeOut);
	const push = prog(frame, 40, 100, easeInOut);

	const box = wide ? {x: (W - s(1500)) / 2, y: s(270), w: s(1500)} : {x: s(60), y: s(290), w: s(960)};
	const imgW = wide ? box.w : s(1200);
	const shift = wide ? 0 : -s(225);
	return (
		<Stage glow={[W / 2, H * 0.75]}>
			<Caption head="Runs on your Mac." sub="With the Claude Code you already have. No account, no server." at={6} />
			<div style={{position: 'absolute', inset: 0, perspective: s(2200)}}>
				<div
					style={{
						position: 'absolute',
						left: box.x,
						top: box.y,
						width: box.w,
						height: H - box.y + s(40),
						borderRadius: s(22),
						overflow: 'hidden',
						boxShadow: `0 ${s(50)}px ${s(140)}px rgba(0,0,0,0.7), 0 0 0 ${s(1.5)}px ${C.line}`,
						opacity: Math.min(1, rise * 1.4),
						transformOrigin: '50% 0%',
						transform: `translateY(${(1 - rise) * s(260)}px) rotateX(${(1 - rise) * 26}deg) scale(${mix(0.92, 1, rise) * mix(1, 1.045, push)})`,
					}}
				>
					<Img src={staticFile('app-window-2400.webp')} style={{width: imgW, display: 'block', transform: `translateX(${shift}px)`}} />
				</div>
			</div>
			<Provenance text="The Mossbook run, captured from the Tellfold app" at={30} />
		</Stage>
	);
};
