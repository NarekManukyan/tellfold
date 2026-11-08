import React from 'react';
import {AbsoluteFill, Sequence, interpolate, useCurrentFrame} from 'remotion';
import {cafeBeat, schoolBeat} from './beats';
import {AppShot} from './scenes/AppShot';
import {Crowd} from './scenes/Crowd';
import {Drop} from './scenes/Drop';
import {End} from './scenes/End';
import {Fold} from './scenes/Fold';
import {Open} from './scenes/Open';
import {Sound} from './Sound';

/** Cross-dissolve with a little blur, so the two scenes read as one changing picture rather than two layers. */
const Fade: React.FC<{dur: number; fadeIn: number; fadeOut: number; children: React.ReactNode}> = ({dur, fadeIn, fadeOut, children}) => {
	const f = useCurrentFrame();
	const a = fadeIn ? interpolate(f, [0, fadeIn], [0, 1], {extrapolateRight: 'clamp'}) : 1;
	const b = fadeOut ? interpolate(f, [dur - fadeOut, dur], [1, 0], {extrapolateLeft: 'clamp'}) : 1;
	const o = Math.min(a, b);
	return <AbsoluteFill style={{opacity: o, filter: o < 1 ? `blur(${(1 - o) * 6}px)` : undefined}}>{children}</AbsoluteFill>;
};

type Shot = {id: string; dur: number; overlap: number; el: React.ReactNode};

/** Shots in order; `overlap` is how many frames the shot dissolves over the one before it. */
const MAIN: Shot[] = [
	{id: 'open', dur: 135, overlap: 0, el: <Open />},
	{id: 'drop', dur: 140, overlap: 0, el: <Drop />},
	{id: 'cafe', dur: 330, overlap: 14, el: <Crowd {...cafeBeat} />},
	{id: 'school', dur: 250, overlap: 14, el: <Crowd {...schoolBeat} />},
	{id: 'fold', dur: 290, overlap: 14, el: <Fold />},
	{id: 'app', dur: 140, overlap: 14, el: <AppShot />},
	{id: 'end', dur: 150, overlap: 16, el: <End />},
];

/** The site loop: the crowd talks, the fan folds into the answer, then black, then round again. */
const LOOP: Shot[] = [
	{id: 'cafe', dur: 330, overlap: 0, el: <Crowd {...cafeBeat} />},
	{id: 'fold', dur: 290, overlap: 14, el: <Fold />},
];

const place = (shots: Shot[]) => {
	let t = 0;
	return shots.map((s, i) => {
		const from = i === 0 ? 0 : t - s.overlap;
		t = from + s.dur;
		return {...s, from, fadeOut: shots[i + 1]?.overlap ?? 0};
	});
};

export const total = (shots: Shot[]) => {
	const p = place(shots);
	return p[p.length - 1].from + p[p.length - 1].dur;
};
export const MAIN_FRAMES = total(MAIN);
export const LOOP_FRAMES = total(LOOP) + 16;

const Cut: React.FC<{shots: Shot[]; tailFade?: number; sound?: boolean}> = ({shots, tailFade = 0, sound}) => {
	const placed = place(shots);
	const starts = Object.fromEntries(placed.map((s) => [s.id, s.from]));
	return (
	<AbsoluteFill style={{background: '#000'}}>
		{sound ? <Sound starts={starts} total={total(shots)} eventFrame={starts.school + (schoolBeat.eventAt ?? 0)} /> : null}
		{placed.map((s, i, all) => (
			<Sequence key={s.id} from={s.from} durationInFrames={s.dur} name={s.id}>
				<Fade dur={s.dur} fadeIn={s.overlap} fadeOut={i === all.length - 1 ? tailFade : s.fadeOut}>
					{s.el}
				</Fade>
			</Sequence>
		))}
	</AbsoluteFill>
	);
};

export const Main: React.FC = () => <Cut shots={MAIN} sound />;
export const Loop: React.FC = () => <Cut shots={LOOP} tailFade={24} />;
