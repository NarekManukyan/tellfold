import React from 'react';
import {Composition} from 'remotion';
import {LOOP_FRAMES, Loop, MAIN_FRAMES, Main} from './Main';

// Main cut in three shapes (Product Hunt / YouTube 16:9, LinkedIn 1:1 and 4:5) and the short silent site loop.
const sizes = [
	['169', 1920, 1080],
	['11', 1080, 1080],
	['45', 1080, 1350],
] as const;

export const Root: React.FC = () => (
	<>
		{sizes.map(([k, w, h]) => (
			<Composition key={k} id={`Main-${k}`} component={Main} durationInFrames={MAIN_FRAMES} fps={30} width={w} height={h} />
		))}
		<Composition id="Loop" component={Loop} durationInFrames={LOOP_FRAMES} fps={30} width={1920} height={1080} />
		<Composition id="Loop-11" component={Loop} durationInFrames={LOOP_FRAMES} fps={30} width={1080} height={1080} />
	</>
);
