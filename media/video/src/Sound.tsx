import React from 'react';
import {Audio, Sequence, interpolate, staticFile} from 'remotion';
import {cafeBeat, schoolBeat} from './beats';

// Sound for the main cut. Files live in public/audio (fetch-audio.sh downloads them; they are not committed).
// Each cue is pinned to the frame of the picture it belongs to: `at` is the frame of the hit inside its shot, `lag`
// is how far into the file its peak sits, so the peak lands on the hit.

type Cue = {shot: string; at: number; file: string; lag: number; volume: number; dur: number};

const FILES = {
	slide: {file: 'paper-slide.mp3', lag: 11, dur: 33},
	page: {file: 'paper-page-turn.mp3', lag: 6, dur: 14},
	tap: {file: 'switch-tap.mp3', lag: 0, dur: 33},
	click: {file: 'switch-click-quick.mp3', lag: 0, dur: 31},
	impact: {file: 'impact-deep-whoosh.mp3', lag: 16, dur: 122},
	swirl: {file: 'whoosh-swirl.mp3', lag: 59, dur: 75},
	swish: {file: 'stardust-swish.mp3', lag: 29, dur: 144},
	woosh: {file: 'air-woosh-deep.mp3', lag: 15, dur: 89},
	shimmer: {file: 'shimmer-sparkle-sweep.mp3', lag: 28, dur: 90},
};

const cue = (shot: string, at: number, kind: keyof typeof FILES, volume: number): Cue => ({shot, at, volume, ...FILES[kind]});

// Repeated sounds step down in volume so a run of them reads as a rhythm, not a machine gun.
const postCues = (shot: string, beat: typeof cafeBeat) =>
	beat.posts.map((p, i) => cue(shot, p.at, 'click', (p.feed ? 0.24 : 0.13) * (1 - i * 0.05)));

export const CUES: Cue[] = [
	// Cards dealt: left, middle, right; each lands about ten frames after it starts.
	cue('open', 64, 'slide', 0.42),
	cue('open', 73, 'slide', 0.36),
	cue('open', 82, 'slide', 0.3),
	// The other two notes leave; the café note goes into the sheet; Start simulation.
	cue('drop', 10, 'slide', 0.26),
	cue('drop', 56, 'page', 0.7),
	cue('drop', 110, 'tap', 0.45),
	...postCues('cafe', cafeBeat),
	...postCues('school', schoolBeat),
	// The one big hit in the film: breaking news lands.
	cue('school', schoolBeat.eventAt!, 'impact', 0.55),
	// The fan folds, then the hinge opens into the report; each citation lights with a tick.
	cue('fold', 104, 'swirl', 0.45),
	cue('fold', 146, 'swish', 0.4),
	cue('fold', 196, 'click', 0.16),
	cue('fold', 220, 'click', 0.14),
	cue('fold', 244, 'click', 0.12),
	cue('app', 8, 'woosh', 0.3),
	cue('end', 40, 'shimmer', 0.42),
];

export const Sound: React.FC<{starts: Record<string, number>; total: number; eventFrame: number}> = ({starts, total, eventFrame}) => (
	<>
		{/* House Vibez by Lily J (Mixkit). In under a second, a dip as the breaking news hits, out over the end card. */}
		<Audio
			src={staticFile('audio/house-vibez.mp3')}
			volume={(f) =>
				interpolate(f, [0, 24, eventFrame - 6, eventFrame + 4, eventFrame + 40, total - 75, total], [0, 0.3, 0.3, 0.16, 0.3, 0.3, 0], {
					extrapolateLeft: 'clamp',
					extrapolateRight: 'clamp',
				})
			}
		/>
		{CUES.map((c, i) => {
			const from = starts[c.shot] + c.at - c.lag;
			return (
				<Sequence key={i} from={from} durationInFrames={c.dur} name={`sfx ${c.file}`} layout="none">
					<Audio src={staticFile(`audio/${c.file}`)} volume={c.volume} />
				</Sequence>
			);
		})}
	</>
);
