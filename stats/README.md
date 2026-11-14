# Usage numbers

A small Cloudflare Worker, written in Rust, that counts two things and keeps nothing that identifies anyone. It runs at `https://tellfold-stats.tellfold.workers.dev` on the zoel.io Cloudflare account.

- **Update checks (copies in use).** The app checks for updates about once a day (Sparkle, `SUScheduledCheckInterval` 86400), and from 0.7.1 its `SUFeedURL` points here. The Worker relays the real feed from `https://narekmanukyan.github.io/tellfold/appcast.xml` (edge-cached for five minutes), so `release.sh` and the appcast stay as they are, and adds one to a count for the day and the app version. It reads the version from Sparkle's User-Agent (`Tellfold/0.7.0 Sparkle/2.10.1`). Other requests for the feed are served but not counted. A copy that's open on a given day checks about once, so a day's total is roughly the number of copies opened that day. A manual Check for Updates adds one, and a copy that stays closed adds nothing.
- **Site visits and clicks.** `site/assets/site.js` sends `/hit?n=<page or link>&s=<source>` for each page view and each click on a link marked `data-count` (Download, Sponsor). The source is a `?ref=` or `utm_source` tag if the link had one (Product Hunt adds `?ref=producthunt`), otherwise the referring site, otherwise `direct`. Only requests whose `Origin` is the site are counted, crawlers and headless browsers are skipped, and labels that aren't short plain words are dropped.

The D1 database `tellfold-stats` has two tables: `checks (day, version, n)` and `visits (day, name, source, n)`. No IP address, cookie, identifier or other header is stored.

To stop counting your own visits, open the site once with `#no-count` at the end of the address; `#count` turns it back on. This applies to that browser only.

## Reading the numbers

From this folder (uses your Cloudflare login, `npx wrangler@4 login`):

Copies in use per day:

```bash
npx wrangler@4 d1 execute tellfold-stats --remote --command "SELECT day, SUM(n) AS checks FROM checks GROUP BY day ORDER BY day DESC LIMIT 30"
```

Versions in use this week:

```bash
npx wrangler@4 d1 execute tellfold-stats --remote --command "SELECT version, SUM(n) AS checks FROM checks WHERE day >= date('now', '-7 day') GROUP BY version ORDER BY checks DESC"
```

Visits and downloads by source this week:

```bash
npx wrangler@4 d1 execute tellfold-stats --remote --command "SELECT source, SUM(CASE WHEN name LIKE '/%' THEN n END) AS views, SUM(CASE WHEN name LIKE 'download-%' THEN n END) AS downloads FROM visits WHERE day >= date('now', '-7 day') GROUP BY source ORDER BY views DESC"
```

Downloads per release are on GitHub: `gh api repos/NarekManukyan/tellfold/releases --jq '.[] | [.tag_name, .assets[0].download_count] | @tsv'`.

## Changing and deploying

Needs Rust with the `wasm32-unknown-unknown` target; the build command installs `worker-build`. The unit tests run with `cargo test`.

```bash
npx wrangler@4 d1 migrations apply tellfold-stats --remote   # after adding a file to migrations/
npx wrangler@4 deploy
```

To try it locally: `npx wrangler@4 d1 migrations apply tellfold-stats --local`, `npx wrangler@4 dev --local`, then `curl -A "Tellfold/0.7.0 Sparkle/2.10.1" localhost:8787/appcast.xml`.

If the Worker is ever taken down, copies from 0.7.1 on can't see updates until it's back, so keep it running, or ship a release that points `SUFeedURL` back at GitHub Pages first.
