// Tellfold Studio: illustrated, deterministic avatars for simulated people, monogram tiles for organisations.
//
// The same persona always gets the same face, on the web and in the Mac app (mac/Tellfold/App/Design/Avatar.swift
// ports this file shape for shape; keep the two in step). Faces are built from plain circles, ellipses and
// rounded rectangles in a 64x64 box so both renderers draw identical pictures. They are clearly synthetic.

const SKIN = ['#F6D3B8', '#EBBE9B', '#D8A07C', '#B98059', '#8E5B3C', '#5F3C28'];
const HAIR = ['#231A14', '#4A3426', '#7A4E2D', '#B5813E', '#DDB874', '#9A9A9A', '#B4452C'];
const SHIRT = ['#6366F1', '#0EA5E9', '#14B8A6', '#F59E0B', '#EC4899', '#8B5CF6', '#64748B', '#22C55E'];
const BACK = ['#E0E7FF', '#E0F2FE', '#CCFBF1', '#FEF3C7', '#FCE7F3', '#EDE9FE', '#F1F5F9', '#DCFCE7'];
const TILE = ['#4F46E5', '#0369A1', '#0F766E', '#B45309', '#BE185D', '#6D28D9', '#334155', '#15803D'];
const INK = '#1F2937';

/** FNV-1a over the persona id and handle, then xorshift32: a small deterministic stream of choices. */
export function avatarSeed(id, handle = '') {
  let h = 0x811c9dc5;
  const s = `${id}:${handle}`;
  for (let i = 0; i < s.length; i++) {
    h ^= s.charCodeAt(i);
    h = Math.imul(h, 0x01000193) >>> 0;
  }
  return h || 1;
}

function stream(seed) {
  let x = seed >>> 0;
  return (n) => {
    x ^= x << 13; x >>>= 0;
    x ^= x >>> 17;
    x ^= x << 5; x >>>= 0;
    return x % n;
  };
}

/** The face as data: every renderer draws exactly these choices. */
export function faceSpec(id, handle = '') {
  const pick = stream(avatarSeed(id, handle));
  return {
    back: BACK[pick(BACK.length)],
    skin: SKIN[pick(SKIN.length)],
    hair: HAIR[pick(HAIR.length)],
    shirt: SHIRT[pick(SHIRT.length)],
    style: pick(8), // 0 short, 1 side part, 2 bob, 3 long, 4 bun, 5 curls, 6 buzz, 7 bald with beard
    glasses: pick(10) < 3,
    smile: pick(3), // 0 flat, 1 smile, 2 open
  };
}

/** Initials for a monogram: "App Store" -> "AS", "revenuecat" -> "RE". */
export function initials(name, handle = '') {
  const words = String(name || handle || '?').replace(/[_@]+/g, ' ').trim().split(/\s+/).filter(Boolean);
  if (words.length >= 2) return (words[0][0] + words[1][0]).toUpperCase();
  return (words[0] || '?').slice(0, 2).toUpperCase();
}

export function tileColor(id, handle = '') {
  return TILE[avatarSeed(id, handle) % TILE.length];
}

const c = (cx, cy, r, fill, extra = '') => `<circle cx="${cx}" cy="${cy}" r="${r}" fill="${fill}"${extra}/>`;
const e = (cx, cy, rx, ry, fill, extra = '') => `<ellipse cx="${cx}" cy="${cy}" rx="${rx}" ry="${ry}" fill="${fill}"${extra}/>`;
const r = (x, y, w, h, rr, fill) => `<rect x="${x}" y="${y}" width="${w}" height="${h}" rx="${rr}" fill="${fill}"/>`;

/** Inner SVG markup (64x64 user units) for a person. */
export function faceMarkup(spec) {
  const { skin, hair, shirt, style } = spec;
  const out = [r(0, 0, 64, 64, 0, spec.back)];
  out.push(e(32, 66, 23, 15, shirt)); // shoulders
  if (style === 2) out.push(r(13, 24, 9, 20, 4.5, hair), r(42, 24, 9, 20, 4.5, hair)); // bob sides
  if (style === 3) out.push(r(12, 22, 10, 32, 5, hair), r(42, 22, 10, 32, 5, hair)); // long sides
  out.push(r(28, 44, 8, 9, 3, skin)); // neck
  out.push(c(16.5, 35, 3.6, skin), c(47.5, 35, 3.6, skin)); // ears
  out.push(c(32, 34, 15.5, skin)); // face
  if (style === 7) out.push(`<path d="M18.5 37 Q32 57 45.5 37" stroke="${hair}" stroke-width="5" fill="none" stroke-linecap="round"/>`); // beard along the jaw
  switch (style) {
    case 0: out.push(e(32, 23.5, 16.5, 8.5, hair)); break;
    case 1: out.push(e(29.5, 23, 17, 8.5, hair), c(44.5, 26.5, 5, hair)); break;
    case 2: out.push(e(32, 23, 18, 9.5, hair)); break;
    case 3: out.push(e(32, 22.5, 18, 9.5, hair)); break;
    case 4: out.push(e(32, 24, 16.5, 8, hair), c(32, 12.5, 6, hair)); break;
    case 5: for (const [x, y] of [[18, 27], [20, 20], [26, 16], [32, 14.5], [38, 16], [44, 20], [46, 27]]) out.push(c(x, y, 6, hair)); break;
    case 6: out.push(e(32, 24, 15.5, 7.5, hair, ' opacity="0.55"')); break;
    default: break;
  }
  out.push(c(26.5, 34.5, 1.9, INK), c(37.5, 34.5, 1.9, INK)); // eyes
  if (spec.smile === 0) out.push(r(28.5, 41, 7, 1.8, 0.9, INK));
  else if (spec.smile === 1) out.push(`<path d="M27.5 40.2 Q32 44.6 36.5 40.2" stroke="${INK}" stroke-width="1.8" fill="none" stroke-linecap="round"/>`);
  else out.push(`<path d="M27.8 40 Q32 46.2 36.2 40 Z" fill="${INK}"/>`); // open smile
  if (spec.glasses) {
    const g = ` fill="none" stroke="${INK}" stroke-width="1.4"`;
    out.push(`<circle cx="26.5" cy="34.5" r="4.6"${g}/>`, `<circle cx="37.5" cy="34.5" r="4.6"${g}/>`,
      `<line x1="31.1" y1="34.5" x2="32.9" y2="34.5" stroke="${INK}" stroke-width="1.4"/>`);
  }
  return out.join('');
}

/** A complete inline SVG string: a round face for a person, a rounded square monogram for an organisation. */
export function avatarSVG({ id, handle = '', name = '', org = false, size = 32, title = '' }) {
  const label = title ? `<title>${escapeXml(title)}</title>` : '';
  if (org) {
    const text = initials(name, handle);
    return `<svg class="avatar org" width="${size}" height="${size}" viewBox="0 0 64 64" role="img" aria-label="${escapeXml(name || handle)}">${label}`
      + `<rect width="64" height="64" rx="14" fill="${tileColor(id, handle)}"/>`
      + `<text x="32" y="33" text-anchor="middle" dominant-baseline="central" fill="#fff" font-family="-apple-system, BlinkMacSystemFont, system-ui, sans-serif" font-weight="600" font-size="${text.length > 1 ? 24 : 30}">${escapeXml(text)}</text></svg>`;
  }
  const clip = `av${avatarSeed(id, handle).toString(36)}`;
  // People are drawn in the 64-unit box but framed on its centre 54 units (5..59), so the face fills the circle.
  return `<svg class="avatar" width="${size}" height="${size}" viewBox="5 5 54 54" role="img" aria-label="${escapeXml(name || handle)}">${label}`
    + `<defs><clipPath id="${clip}"><circle cx="32" cy="32" r="27"/></clipPath></defs>`
    + `<g clip-path="url(#${clip})">${faceMarkup(faceSpec(id, handle))}</g></svg>`;
}

/** Avatar as an <img>-ready data URI (canvas drawImage, CSS backgrounds). */
export function avatarDataURI(opts) {
  // a standalone SVG image needs its namespace; the inline markup gets it from the HTML parser
  const svg = avatarSVG({ ...opts, size: 64 }).replace('<svg ', '<svg xmlns="http://www.w3.org/2000/svg" ');
  return 'data:image/svg+xml;charset=utf-8,' + encodeURIComponent(svg);
}

function escapeXml(s) {
  return String(s).replace(/[<>&"']/g, (ch) => ({ '<': '&lt;', '>': '&gt;', '&': '&amp;', '"': '&quot;', "'": '&#39;' }[ch]));
}

const uriCache = new Map();

/** The avatar as an <img> (cached data URI): DOM lists, cards and popovers. Decorative; label it alongside. */
export function avatarImg(persona, { size = 28, org = false } = {}) {
  const p = persona || {};
  const key = `${p.id}\u0000${p.handle || ''}\u0000${org ? 1 : 0}\u0000${org ? p.name || '' : ''}`;
  let uri = uriCache.get(key);
  if (!uri) {
    uri = avatarDataURI({ id: p.id || '?', handle: p.handle || '', name: p.name || '', org });
    uriCache.set(key, uri);
  }
  const img = document.createElement('img');
  img.className = org ? 'av org' : 'av';
  img.width = size;
  img.height = size;
  img.alt = '';
  img.decoding = 'async';
  img.draggable = false;
  img.src = uri;
  return img;
}
