import type {CrowdBeat} from './scenes/Crowd';
import {cafe, school} from './data';

// The two crowd scenes, timed. Round starts are when each real mood point lands; `feed` posts are shown in full.

export const cafeBeat: CrowdBeat = {
	run: cafe,
	head: 'Simulated people talk it over.',
	sub: 'They post, reply and change their minds.',
	rounds: [44, 118, 194, 268],
	posts: [
		{n: 1, at: 50},
		{n: 2, at: 66, feed: true},
		{n: 3, at: 124},
		{n: 4, at: 140, feed: true},
		{n: 6, at: 198},
		{n: 7, at: 214, feed: true},
		{n: 8, at: 274, feed: true},
	],
	likes: [
		{from: 'a04', n: 1, at: 96},
		{from: 'a01', n: 4, at: 160},
		{from: 'a02', n: 4, at: 166},
		{from: 'a03', n: 6, at: 236},
	],
	provenance: 'Linden & Rye café · a real Tellfold run · the people are simulated',
};

export const schoolBeat: CrowdBeat = {
	run: school,
	head: 'Drop in breaking news.',
	sub: 'See whether the mood turns.',
	rounds: [22, 48, 76, 120],
	posts: [
		{n: 2, at: 28, feed: true},
		{n: 6, at: 82, feed: true},
		{n: 9, at: 160, feed: true},
	],
	likes: [],
	eventAt: 116,
	provenance: 'Westbrook Secondary · a real Tellfold run · the people are simulated',
};
