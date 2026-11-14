# Update feed

The Mac app checks for updates about once a day by fetching `appcast.xml` (Sparkle, `SUScheduledCheckInterval` 86400). Those checks are the only signal of how many copies are in use, and GitHub Pages keeps no logs. So the app fetches the feed from this Cloudflare Worker instead, which:

- passes the request on to the real feed at `https://narekmanukyan.github.io/tellfold/appcast.xml` (edge-cached for five minutes), so `release.sh` and the appcast stay exactly as they are;
- adds one to a counter for the day and the app version, read from Sparkle's User-Agent (`Tellfold/0.7.0 Sparkle/2.10.1`).

It stores one row per day and version (`day`, `version`, `n`) in a D1 database. No IP addresses, identifiers or other headers are kept. Requests that don't come from the app are served but not counted.

A copy that is open on a given day checks roughly once that day, so `n` is about the number of copies opened that day. It's a rough number: a manual Check for Updates adds one, and a copy that stays closed adds nothing.

## Reading the numbers

From this folder (uses your Cloudflare login):

```bash
npx wrangler@4 d1 execute tellfold-feed --remote --command "SELECT day, SUM(n) AS checks FROM checks GROUP BY day ORDER BY day DESC LIMIT 30"
```

```bash
npx wrangler@4 d1 execute tellfold-feed --remote --command "SELECT version, SUM(n) AS checks FROM checks WHERE day >= date('now', '-7 day') GROUP BY version ORDER BY checks DESC"
```

## Deploying

Needs Rust with the `wasm32-unknown-unknown` target; `worker-build` is installed by the build command.

```bash
npx wrangler@4 login
npx wrangler@4 d1 create tellfold-feed          # once; put the printed database_id in wrangler.toml
npx wrangler@4 d1 migrations apply tellfold-feed --remote
npx wrangler@4 deploy
```

To try it locally: `npx wrangler@4 d1 migrations apply tellfold-feed --local`, `npx wrangler@4 dev --local`, then `curl -A "Tellfold/0.7.0 Sparkle/2.10.1" localhost:8787/appcast.xml`. The unit tests run with `cargo test`.

The app's `SUFeedURL` (in `mac/project.yml`) points at this Worker from 0.7.1 on. Copies older than that still read GitHub Pages directly and aren't counted until they update.
