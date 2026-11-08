# Tellfold product video

A 45-second motion-graphics film about Tellfold, built in code with [Remotion](https://www.remotion.dev/) so it can be changed and rendered again. Everything it says about a run comes from real Tellfold runs: the posts, the mood numbers round by round, the breaking news and the verdict. The people in those runs are simulated, and the film says so on screen.

## What's in the film

| Shot | Shows | Real run |
| --- | --- | --- |
| Open | Three notes someone is about to send: a café price rise, a school phone letter, the Mossbook pricing post | `examples/*.md` |
| Drop | The café note goes into a New simulation sheet, and the run's question is typed in | Linden & Rye café |
| Crowd | Eight people post, reply and like; the mood goes −0.15, −0.25, +0.13, +0.70 | Linden & Rye café |
| Breaking news | A newspaper story lands in round 3 and the mood falls from +0.43 to −0.13 | Westbrook Secondary phone ban |
| Fold | Six people sit on the six pleats of the Tellfold fan; it folds shut and the hinge opens into the report, its citations lit | Mossbook pricing |
| App | The same Mossbook run, captured from the app | Mossbook pricing |
| End | The mark, "See how people react before you tell them.", Free for Mac | |

The run data is copied into [`src/data.ts`](src/data.ts), which names the run folders it came from.

## Files

| File | Use |
| --- | --- |
| `out/tellfold-16x9.mp4` | 1920×1080, with sound. Product Hunt gallery, YouTube |
| `out/tellfold-1x1.mp4` | 1080×1080, with sound. LinkedIn feed |
| `out/tellfold-4x5.mp4` | 1080×1350, with sound. LinkedIn feed (takes more of a phone screen) |
| `out/tellfold-preview.webp` | Animated WebP of the silent loop, for the README |
| `out/producthunt/*.png` | Six 1270×760 gallery images of key moments: the notes, the crowd, the breaking news, the report, the app, the end card |
| `../../site/assets/video/tellfold-loop.{webm,mp4}` | The silent 20-second loop on the landing page, with its poster |
| `../../site/assets/video/tellfold-loop-square.*` | The same loop, square, which the page plays on phones |

## Render it again

You need Node 20 or later and ffmpeg (with libvpx-vp9), and `img2webp` from libwebp for the README animation.

```sh
cd media/video
npm install
./fetch-audio.sh     # music and sound effects (not committed, see below)
node render.mjs      # the three main cuts and both loop masters into out/
./encode.sh          # the site loops, their posters and the README animation
./stills.sh          # the Product Hunt gallery images
```

`node render.mjs main-169` renders one cut; `node render.mjs --draft /tmp/drafts` makes quick half-size drafts. `npm run studio` opens Remotion Studio to scrub through the film.

## Sound

Music is "House Vibez" by Lily J; the effects are from Mixkit. Both are under the Mixkit Free License, which allows them in a video but not passing the files on by themselves, so `fetch-audio.sh` downloads them instead of the repository carrying them. The cue sheet is in [`src/Sound.tsx`](src/Sound.tsx): each sound is pinned to the frame of the thing it belongs to. The main cuts are mixed to about −15 LUFS. The site loop and the README animation are silent.
