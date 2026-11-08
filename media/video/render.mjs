// Renders the Tellfold video cuts with the Remotion CLI.
//   node render.mjs                 every final cut into out/
//   node render.mjs --draft DIR     half-size drafts of the three main cuts into DIR
//   node render.mjs main-169 loop   only the named jobs
// Then ./encode.sh makes the small site loop, poster and README animation from out/tellfold-loop-master.mp4.
import {execFileSync} from 'node:child_process';
import path from 'node:path';
import {fileURLToPath} from 'node:url';

const here = path.dirname(fileURLToPath(import.meta.url));
const args = process.argv.slice(2);
const draftAt = args.indexOf('--draft');
const draftDir = draftAt >= 0 ? path.resolve(args[draftAt + 1]) : null;
const names = args.filter((a, i) => !a.startsWith('--') && (draftAt < 0 || i !== draftAt + 1));

const JOBS = {
	'main-169': {id: 'Main-169', file: 'tellfold-16x9.mp4', crf: 18},
	'main-11': {id: 'Main-11', file: 'tellfold-1x1.mp4', crf: 18},
	'main-45': {id: 'Main-45', file: 'tellfold-4x5.mp4', crf: 18},
	// Rendered big and clean; encode.sh makes the small web and README files from it.
	loop: {id: 'Loop', file: 'tellfold-loop-master.mp4', crf: 12, muted: true},
};

const pick = names.length ? names : draftDir ? ['main-169', 'main-11', 'main-45'] : Object.keys(JOBS);
for (const name of pick) {
	const job = JOBS[name];
	const output = draftDir ? path.join(draftDir, `draft-${name}.mp4`) : path.join(here, 'out', job.file);
	const flags = [
		'render', 'src/index.ts', job.id, output,
		'--codec=h264', '--pixel-format=yuv420p', '--concurrency=8', '--log=warn',
		`--crf=${draftDir ? 26 : job.crf}`,
		...(draftDir ? ['--scale=0.5'] : []),
		...(job.muted ? ['--muted'] : []),
	];
	const t = Date.now();
	// Chrome sometimes takes longer than Remotion's 25 s to start on a busy machine; try again before giving up.
	for (let attempt = 1; ; attempt++) {
		try {
			execFileSync(path.join(here, 'node_modules/.bin/remotion'), flags, {cwd: here, stdio: 'inherit'});
			break;
		} catch (err) {
			if (attempt >= 4) throw err;
			console.warn(`${name}: attempt ${attempt} failed, retrying`);
		}
	}
	if (!job.muted && !draftDir) {
		// Bring the mix to -16 LUFS: platforms turn loud videos down but leave quiet ones quiet.
		const tmp = output.replace(/\.mp4$/, '.loudnorm.mp4');
		execFileSync('ffmpeg', ['-loglevel', 'error', '-y', '-i', output, '-c:v', 'copy', '-af', 'loudnorm=I=-16:TP=-1.5:LRA=11', '-ar', '48000', '-c:a', 'aac', '-b:a', '192k', '-movflags', '+faststart', tmp]);
		execFileSync('mv', [tmp, output]);
	}
	console.log(`${name}: ${output} (${Math.round((Date.now() - t) / 1000)}s)`);
}
