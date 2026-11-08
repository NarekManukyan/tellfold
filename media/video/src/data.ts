// Everything the video says about a run comes from a real Tellfold run, copied here so the video re-renders without
// the run folders. Mood is the round's mean sentiment from `round_stats`, posts are `posts.content` (shortened only
// with an ellipsis), verdicts are `report.json` titles. Source runs:
//   café     20261108-203108-how-will-regular-customers-react-to-lind  (examples/corner_cafe_prices.md, seed 7)
//   school   20261108-203108-how-will-parents-and-students-react-to-t  (examples/school_phone_policy.md, seed 7)
//   mossbook 20261107-025250-how-will-existing-mossbook-users-react-t  (examples/mossbook_pricing.md, seed 7)

export type Persona = {
	id: string;
	handle: string;
	name: string;
	/** Backed by an entity in the seed, so the app draws a monogram tile instead of a face. */
	org?: boolean;
};

export type Post = {
	n: number;
	round: number;
	author: string;
	/** Post number this replies to. */
	parent?: number;
	text: string;
};

export type Run = {
	key: string;
	question: string;
	people: Persona[];
	mood: number[];
	posts: Post[];
	/** Likes as [liker, post number]. */
	likes: [string, number][];
	event?: {round: number; text: string};
	verdict: string;
	confidence: 'low' | 'medium' | 'high';
};

export const cafe: Run = {
	key: 'cafe',
	question: 'How will regular customers react to Linden & Rye’s price rise and Tuesday closing?',
	people: [
		{id: 'a01', handle: 'lindenandrye', name: 'Linden & Rye', org: true},
		{id: 'a02', handle: 'tomas_lindenrye', name: 'Tomás', org: true},
		{id: 'a03', handle: 'maureen_alderst', name: 'Maureen Doyle'},
		{id: 'a04', handle: 'deanfixesvans', name: 'Dean Walcott'},
		{id: 'a05', handle: 'arthur_oldalder', name: 'Arthur Penhallow'},
		{id: 'a06', handle: 'noa_studiesarts', name: 'Noa Fenwick'},
		{id: 'a07', handle: 'priya_alderstreet', name: 'Priya Nair'},
		{id: 'a08', handle: 'jamie_otherside', name: 'Jamie Osei'},
	],
	mood: [-0.15, -0.25, 0.133, 0.7],
	posts: [
		{n: 1, round: 0, author: 'a02', text: 'We’re raising prices from 2 March and closing Tuesdays from April. It’s honest work that deserves fair pay.'},
		{n: 2, round: 0, author: 'a04', text: 'So Linden & Rye’s closing Tuesdays now? That’s my usual coffee run before work. Guess I’m hunting for a new spot…'},
		{n: 3, round: 1, author: 'a07', parent: 1, text: 'Tuesday’s my client call day at your place—this timing stings. Will the Neighbour card offset the price hike?'},
		{n: 4, round: 1, author: 'a08', parent: 1, text: 'I get why staff need fair pay and a day off—totally fair. Just worried the price jump might push out people on tighter budgets.'},
		{n: 6, round: 2, author: 'a02', parent: 3, text: 'Tuesday’s tough timing—heard you. The Neighbour card stays, and we’ll talk specifics at the 26th.'},
		{n: 7, round: 2, author: 'a04', parent: 4, text: 'Yeah, that’s fair—I get why they need a proper day off. Just annoying for the rest of us. Suppose we’ll adapt.'},
		{n: 8, round: 3, author: 'a02', parent: 7, text: 'We hear you. Working on a few ideas to soften the impact for regulars on tighter budgets.'},
	],
	likes: [
		['a04', 1],
		['a01', 4],
		['a02', 4],
		['a03', 6],
	],
	verdict: 'Regulars soften on price rise, but affordability doubts linger unresolved',
	confidence: 'medium',
};

export const school: Run = {
	key: 'school',
	question: 'How will parents and students react to the phone-free school day?',
	people: [
		{id: 'a01', handle: 'westbrooksecondary', name: 'Westbrook Secondary School', org: true},
		{id: 'a02', handle: 'drlewis_wbss', name: 'Dr Amara Lewis', org: true},
		{id: 'a03', handle: 'janet_roadsafety', name: 'Janet Pemberton'},
		{id: 'a04', handle: 'markdennison_dad', name: 'Mark Dennison'},
		{id: 'a05', handle: 'priya_sixthform', name: 'Priya Nair'},
		{id: 'a06', handle: 'deanna_k_works', name: 'Deanna Keogh'},
		{id: 'a07', handle: 'tom_tastaff', name: 'Tom Ashcroft'},
		{id: 'a08', handle: 'harold_greaves', name: 'Harold Greaves'},
	],
	mood: [0.25, -0.3, 0.433, -0.133],
	posts: [
		{n: 2, round: 0, author: 'a04', text: 'OK so the 64% pressure statistic is real and troubling, I get it. But what about kids who need to ring home?'},
		{n: 6, round: 2, author: 'a05', parent: 3, text: 'Mark, I get the logistics worry, but honestly? The focus gains are real. I was way calmer in Year 11 without the constant buzz.'},
		{n: 9, round: 3, author: 'a03', parent: 4, text: 'This is exactly what I’ve been saying. A bus breaks down and a kid can’t call home? That’s not a hypothetical.'},
	],
	likes: [['a03', 3]],
	event: {
		round: 3,
		text: 'A local newspaper reports that a student at another phone-free school could not reach her parents during a bus breakdown.',
	},
	verdict: 'Logistics, not principle, will decide parent reaction to phone ban',
	confidence: 'low',
};

export const mossbook: Run = {
	key: 'mossbook',
	question: 'How will existing Mossbook users react to the switch to subscription pricing?',
	people: [
		{id: 'a01', handle: 'mossbook_labs', name: 'Mossbook Labs', org: true},
		{id: 'a02', handle: 'ines_okafor', name: 'Ines Okafor-Brandt', org: true},
		{id: 'a03', handle: 'garden_skeptic', name: 'Maria Chen'},
		{id: 'a04', handle: 'alex_tech_casual', name: 'Alex Rodriguez'},
		{id: 'a05', handle: 'garden_collective_pm', name: 'Jamal Williams'},
		{id: 'a06', handle: 'retired_plant_dad', name: 'Robert Thompson'},
	],
	mood: [0.65, 0.2, 0.233],
	posts: [
		{n: 2, round: 0, author: 'a02', text: 'Six years ago, people trusted us with $24.99. That trust matters—so we’re honoring it.'},
		{n: 4, round: 1, author: 'a05', parent: 1, text: 'Appreciate the transparency, but I’m concerned subscriptions lock nonprofits into recurring costs we can’t always plan for.'},
		{n: 6, round: 2, author: 'a03', parent: 2, text: 'This changes things for me. If my old $24.99 licence keeps working offline, I might not need Plus at all.'},
		{n: 7, round: 2, author: 'a05', parent: 3, text: 'This offline guarantee matters… But I’m still concerned: what happens in year two if the company changes direction again?'},
	],
	likes: [],
	event: {round: 2, text: 'A popular tech reviewer posts that the old lifetime licence will keep working offline forever'},
	verdict: 'Existing Users Accept Offer but Question Long-Term Subscription Model',
	confidence: 'medium',
};

/** The opening of each seed document (examples/*.md), as the person would hand it over. */
export const seeds = [
	{
		who: 'A café',
		title: 'A note from Linden & Rye about our prices',
		body: 'We have kept our prices the same for three years. So, starting Monday 2 March, some prices will change.',
		file: 'corner_cafe_prices.md',
	},
	{
		who: 'A school',
		title: 'Letter to parents: phones at Westbrook Secondary from September',
		body: 'Westbrook will become a phone-free school day from the start of the autumn term on 3 September.',
		file: 'school_phone_policy.md',
	},
	{
		who: 'An app',
		title: 'Mossbook is changing how it’s priced. Here’s what that means for you.',
		body: 'Today we’re announcing the biggest change we have ever made to Mossbook, and we want to explain it plainly.',
		file: 'mossbook_pricing.md',
	},
];

/** The Mossbook report summary, with its citations, as the report shows it (trimmed with an ellipsis). */
export const mossbookSummary: (string | {cite: string})[] = [
	'Existing Mossbook users greeted the announcement warmly… with ',
	{cite: '@ines_okafor'},
	'’s personal framing of “honoring trust” (',
	{cite: '#2'},
	') earning the most engagement. However… sentiment dropped to +0.20 in round 1 when ',
	{cite: '@garden_collective_pm'},
	' (',
	{cite: '#4'},
	') and ',
	{cite: '@garden_skeptic'},
	' (',
	{cite: '#6'},
	') independently raised concerns…',
];
