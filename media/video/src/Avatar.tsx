import React from 'react';
import {FONT} from './theme';

// A port of tellfold/web/static/js/avatar.js (and mac/Tellfold/App/Design/Avatar.swift): the same persona gets the
// same face here as in the app. People are illustrated faces, organisations a rounded tile with initials.

const SKIN = ['#F6D3B8', '#EBBE9B', '#D8A07C', '#B98059', '#8E5B3C', '#5F3C28'];
const HAIR = ['#231A14', '#4A3426', '#7A4E2D', '#B5813E', '#DDB874', '#9A9A9A', '#B4452C'];
const SHIRT = ['#6366F1', '#0EA5E9', '#14B8A6', '#F59E0B', '#EC4899', '#8B5CF6', '#64748B', '#22C55E'];
const BACK = ['#E0E7FF', '#E0F2FE', '#CCFBF1', '#FEF3C7', '#FCE7F3', '#EDE9FE', '#F1F5F9', '#DCFCE7'];
const TILE = ['#4F46E5', '#0369A1', '#0F766E', '#B45309', '#BE185D', '#6D28D9', '#334155', '#15803D'];
const INK = '#1F2937';

const avatarSeed = (id: string, handle = '') => {
	let h = 0x811c9dc5;
	const s = `${id}:${handle}`;
	for (let i = 0; i < s.length; i++) {
		h ^= s.charCodeAt(i);
		h = Math.imul(h, 0x01000193) >>> 0;
	}
	return h || 1;
};

const stream = (seed: number) => {
	let x = seed >>> 0;
	return (n: number) => {
		x ^= x << 13;
		x >>>= 0;
		x ^= x >>> 17;
		x ^= x << 5;
		x >>>= 0;
		return x % n;
	};
};

const faceSpec = (id: string, handle = '') => {
	const pick = stream(avatarSeed(id, handle));
	return {
		back: BACK[pick(BACK.length)],
		skin: SKIN[pick(SKIN.length)],
		hair: HAIR[pick(HAIR.length)],
		shirt: SHIRT[pick(SHIRT.length)],
		style: pick(8),
		glasses: pick(10) < 3,
		smile: pick(3),
	};
};

export const initials = (name: string, handle = '') => {
	const words = String(name || handle || '?')
		.replace(/[_@]+/g, ' ')
		.trim()
		.split(/\s+/)
		.filter(Boolean);
	if (words.length >= 2) return (words[0][0] + words[1][0]).toUpperCase();
	return (words[0] || '?').slice(0, 2).toUpperCase();
};

export const tileColor = (id: string, handle = '') => TILE[avatarSeed(id, handle) % TILE.length];

const c = (cx: number, cy: number, r: number, fill: string, extra = '') =>
	`<circle cx="${cx}" cy="${cy}" r="${r}" fill="${fill}"${extra}/>`;
const e = (cx: number, cy: number, rx: number, ry: number, fill: string, extra = '') =>
	`<ellipse cx="${cx}" cy="${cy}" rx="${rx}" ry="${ry}" fill="${fill}"${extra}/>`;
const r = (x: number, y: number, w: number, h: number, rr: number, fill: string) =>
	`<rect x="${x}" y="${y}" width="${w}" height="${h}" rx="${rr}" fill="${fill}"/>`;

const faceMarkup = (spec: ReturnType<typeof faceSpec>) => {
	const {skin, hair, shirt, style} = spec;
	const out = [r(0, 0, 64, 64, 0, spec.back)];
	out.push(e(32, 66, 23, 15, shirt));
	if (style === 2) out.push(r(13, 24, 9, 20, 4.5, hair), r(42, 24, 9, 20, 4.5, hair));
	if (style === 3) out.push(r(12, 22, 10, 32, 5, hair), r(42, 22, 10, 32, 5, hair));
	out.push(r(28, 44, 8, 9, 3, skin));
	out.push(c(16.5, 35, 3.6, skin), c(47.5, 35, 3.6, skin));
	out.push(c(32, 34, 15.5, skin));
	if (style === 7)
		out.push(`<path d="M18.5 37 Q32 57 45.5 37" stroke="${hair}" stroke-width="5" fill="none" stroke-linecap="round"/>`);
	switch (style) {
		case 0: out.push(e(32, 23.5, 16.5, 8.5, hair)); break;
		case 1: out.push(e(29.5, 23, 17, 8.5, hair), c(44.5, 26.5, 5, hair)); break;
		case 2: out.push(e(32, 23, 18, 9.5, hair)); break;
		case 3: out.push(e(32, 22.5, 18, 9.5, hair)); break;
		case 4: out.push(e(32, 24, 16.5, 8, hair), c(32, 12.5, 6, hair)); break;
		case 5:
			for (const [x, y] of [[18, 27], [20, 20], [26, 16], [32, 14.5], [38, 16], [44, 20], [46, 27]]) out.push(c(x, y, 6, hair));
			break;
		case 6: out.push(e(32, 24, 15.5, 7.5, hair, ' opacity="0.55"')); break;
		default: break;
	}
	out.push(c(26.5, 34.5, 1.9, INK), c(37.5, 34.5, 1.9, INK));
	if (spec.smile === 0) out.push(r(28.5, 41, 7, 1.8, 0.9, INK));
	else if (spec.smile === 1)
		out.push(`<path d="M27.5 40.2 Q32 44.6 36.5 40.2" stroke="${INK}" stroke-width="1.8" fill="none" stroke-linecap="round"/>`);
	else out.push(`<path d="M27.8 40 Q32 46.2 36.2 40 Z" fill="${INK}"/>`);
	if (spec.glasses) {
		const g = ` fill="none" stroke="${INK}" stroke-width="1.4"`;
		out.push(
			`<circle cx="26.5" cy="34.5" r="4.6"${g}/>`,
			`<circle cx="37.5" cy="34.5" r="4.6"${g}/>`,
			`<line x1="31.1" y1="34.5" x2="32.9" y2="34.5" stroke="${INK}" stroke-width="1.4"/>`,
		);
	}
	return out.join('');
};

export const Avatar: React.FC<{
	id: string;
	handle: string;
	name: string;
	org?: boolean;
	size: number;
	style?: React.CSSProperties;
}> = ({id, handle, name, org, size, style}) => {
	if (org) {
		const text = initials(name, handle);
		return (
			<svg width={size} height={size} viewBox="0 0 64 64" style={style}>
				<rect width="64" height="64" rx="14" fill={tileColor(id, handle)} />
				<text
					x="32"
					y="33"
					textAnchor="middle"
					dominantBaseline="central"
					fill="#fff"
					fontFamily={FONT}
					fontWeight={600}
					fontSize={text.length > 1 ? 24 : 30}
				>
					{text}
				</text>
			</svg>
		);
	}
	const clip = `av${avatarSeed(id, handle).toString(36)}`;
	return (
		<svg width={size} height={size} viewBox="5 5 54 54" style={style}>
			<defs>
				<clipPath id={clip}>
					<circle cx="32" cy="32" r="27" />
				</clipPath>
			</defs>
			<g clipPath={`url(#${clip})`} dangerouslySetInnerHTML={{__html: faceMarkup(faceSpec(id, handle))}} />
		</svg>
	);
};
