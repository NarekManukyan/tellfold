# How accurate is Tellfold?

**Not accurate enough yet.** There have been four passes. In the latest (13 November 2026: 43 cases, three
seeds each), Tellfold called the overall reaction right in 55% of runs. Asking the same model once, with no simulated
crowd, got 62%. The stance mix Tellfold's planner picks before anyone in the crowd posts got 66% on its own, and
answering "negative" every time would get 49%. The simulated crowd beats a fixed guess by a few points, but it does
worse than the model's own answer and worse than the mix it started from. Tellfold makes no accuracy claim. This
page describes the test, holds every result and lists where it goes wrong.

Tellfold calls itself a rehearsal, not a survey. A rehearsal is still only worth doing if it points the right way
more often than a guess. So we took real announcements whose public reaction is well documented, gave Tellfold only
what was known on the day, and compared its report with what actually happened. Everything is in this repository
([`bench/`](../bench/)): the cases, their sources, the grading prompt, the harness and every result file. Anyone can
rerun it.

## Method

### Cases

Each case is a real public announcement: a price change, a product change, a policy, a council or school decision.
For each one there are three files:

- `input.md`: a neutral summary, in our own words, of the announcement as it stood on the day. It is the only seed
  the simulation gets. It says nothing about the reaction, the coverage or anything that happened later.
- `outcome.md`: what actually happened, as a dated timeline with links to the reporting.
- `rubric.json`: the overall direction of the reaction in the first two weeks (negative, mixed or positive), the
  three to five objections people actually raised, whether the organisation backed down (none, partial or full), and
  which groups pushed back.

Every fact in `outcome.md` and `rubric.json` links to its source, and we dropped any case whose announcement and
reaction we couldn't confirm in at least two independent reports. Some details rest on less, and the case says so
in `outcome.md` or its `notes`: Barnet's coverage is thin (a civic society and the council's own notice) and one of
its objections is inferred from the traders' earlier campaign; Instagram cites a headline and a social-listening
analysis we could not open, marked unverified; Garmin's backdown is scored "partial" on our judgement, because only
the live heart-rate part was reversed.

`input.md` holds only the announcement. It says nothing about the benchmark, the model's cutoff or whether the case
is a control, so the simulation is not nudged to recall; `--validate` rejects a seed that does.

We chose cases announced **from 1 July 2026**, after the training cutoff of the models under test (Claude Sonnet 5.5
and Claude Haiku 5.5, both June 2026), so the model can't simply remember how the story ended. Each rubric records
the cutoff it was chosen against. A few earlier cases, marked `known_to_model`, are kept as a control: if Tellfold
does much better on those, it is recalling rather than simulating.

We also tried not to pick only disasters. A set made of backlash-then-U-turn stories would reward a tool that always
predicts backlash. The case list below shows the mix.

### Running

Every case runs with the same fixed settings, the app's defaults for a new simulation: 20 agents, 6 rounds, batch
size 6, Claude Haiku 5.5 for the agents and Claude Sonnet 5.5 for the planner and the report, through the Claude Code
CLI, random seed 7. The question follows one template for every case:

> How will *the affected public* react to this over the first two weeks, which groups will push back and why, and
> is *the organisation* likely to keep, change or reverse the decision?

A pass with `--repeats 3` runs each case with seeds 7, 8 and 9, which shows how much the answer moves between runs.

### Crowd rules

The first pass (11 November) found four faults in the engine, listed under [Where it went wrong](#where-it-went-wrong).
Since commit `a963978` every new run follows a set of crowd rules (`crowd_rules: 1` in its `config.json`). Older runs
keep the original behaviour and still reproduce exactly. The rules were written from what each fault was, and none
of them was tuned against the 20 cases:

- **No invented facts.** The round and opening prompts say that the source material is everything that is known. An
  organisation's account may restate, explain or defend what it says, but never announces anything it doesn't: no
  new concessions, exemptions, figures, data, dates, promises or changes of plan. No one else may report such things
  as fact either. A case with no organisation account gets none added.
- **Stay on topic.** Every round prompt now shows the digest of the source material, as the opening prompt always
  did, and tells users to stay on the announcement and on what others have actually said, not bring in other plans
  or controversies.
- **Everyone speaks early.** In rounds 1 and 2, anyone who hasn't posted yet is told this is their turn to give a
  first reaction, and in round 2 everyone still silent joins the round. The rule builds on the fair turns from
  [PR #16](https://github.com/NarekManukyan/tellfold/pull/16), which only cover camps and strong views and are off for command-line runs.
- **Direction from the first reaction.** The report no longer reads the direction off where sentiment drifted to. The
  engine takes each person's first scored post (organisation accounts are left out, since they speak for a side) and
  counts it as against (below −0.2), in favour (above 0.2) or neither. The reaction is **negative** or **positive**
  when at least half the people took a side and at least 60% of those (3 in 5) took the same one. Otherwise it is
  **mixed**: the sides are close to even, or most people didn't take one. The report is told to state that
  direction first and to describe later movement as a shift. The counts are saved in `report.json` as
  `first_reaction`.

Why the first reaction: the question asks about the first two weeks, the people in a run are cast to represent the
public on the day, and the later rounds are where the faults above showed up, with the crowd calming down in
response to things that never happened. The 60% bar is the smallest majority that is clearly not an even split; the
half bar matches the judge's definition of mixed, which includes "mostly indifferent".

The second pass (12 November) showed what version 1 didn't fix, listed under [Where it still goes
wrong](#where-it-still-goes-wrong). Since commit `599845a`, new runs follow version 2 (`crowd_rules: 2`), which
keeps every rule above and adds these:

- **A cast of the people it affects, leaning the way they would.** Before writing the cast, the planner lists the
  groups of ordinary people the announcement touches and what it changes for each of them (their money, time, work,
  routine or rights), using only the source material, with roughly how much of the conversation each group makes up.
  It then estimates how those people lean when they first hear the news (supportive, critical, undecided or
  indifferent) and gives a short reason. The public is spread over the groups, and each person's starting stance is
  drawn from that mix, after the bounds below, instead of from one fixed mix of about a third supportive and a third
  critical. With the fixed mix, the first reaction came out mixed in 12 of the 19 runs of the second pass (in 11 the
  sides were close to even, in one most people took no side), whatever the announcement was. The groups, the planner's mix, its reason and the bounded mix are saved in the run's
  `config.json` as `stakes`, so a reader can see why the crowd leaned the way it did.
- **Organisation posts are checked.** Before a post by an organisation account goes into the feed, the planner model
  checks it against the seed text, and the post is dropped if it states, offers or announces anything the seed
  doesn't contain. The prompt rule alone didn't stop Barclays, HubSpot, Meta and the MBTA from inventing concessions
  late in the second pass. Dropped posts are listed in the run's `dropped_posts.jsonl`.
- **Every objection reaches the report.** The report's digest favours the most engaged posts, so objections raised
  by quieter people never reached it. The report now also gets each person's first post and must include a section
  called "What people objected to" that lists every distinct objection and who raised it.
- **One more try at the report.** When the report reply fails validation even after its JSON repair, the report gets
  one more full attempt before the run fails. UT Austin's report failed this way in the second pass, and the same
  prompt worked when run again.

The bounds on the planner's mix: at least 15% of the public starts on each side, and at least 15% starts undecided
or indifferent. Whatever the planner expects, some people defend the decision, some oppose it and some haven't made
up their mind, so the crowd has a minority that can win the argument and a middle that can move. With 14 members of
the public (20 agents less the organisation accounts), 15% is two people, the fewest who can reply to each other.
The bounds still let one side hold up to 70%, enough for a clear lean under the first-reaction rule. They were set
from that reasoning before the third pass and not tuned on the 20 cases.

The catch: with a planned mix, the crowd's direction can simply be the planner's guess under another name, since
people mostly post the stance they were cast with (in the second pass 78 of 106 supportive people and 87 of 102
critical ones posted that way first). So the harness also scores the direction the planner's own mix implies, by the
same first-reaction rule, as a row of its own, counts the runs where the crowd's final direction moved away from it
and whether each move was right, and counts how often Tellfold agrees with the zero-shot answer. If Tellfold only
matches the planner, the crowd adds nothing to the planner's guess, and the results say so.

The third pass and the 23 balanced cases (both 13 November, [below](#third-and-fourth-passes-13-november)) showed
that the catch was real, and that the crowd moved the wrong way. On the balanced cases its direction moved away from
the planner's in 28 of 64 runs and never once to the right answer, and in the 28 runs of either set whose real
reaction was positive it called 2 positive. In the balanced runs the mean sentiment of public posts (organisation
accounts left out) fell from −0.15 in the opening round to −0.40 by round 6. Since commit `62357bc`, new runs follow version 3 (`crowd_rules: 3`),
which keeps every rule above and adds these:

- **The middle of the crowd behaves like a middle.** The round and opening prompts say that an undecided user stays
  undecided until something specific in their feed persuades them, and weighs what they like against what worries
  them or asks a question rather than only complaining. An indifferent user mostly scrolls past, and posts about the
  practical effect on them without taking a side. A supporter can ask for more, sooner or in writing and still be a
  supporter.
- **People who have made their point go quiet.** A user who got what they wanted, or whose question was answered,
  rarely keeps posting about it, and nobody grows angrier round after round unless something new in their feed gives
  them a reason.
- **Sentiment is the attitude to the announcement.** The sentiment prompt scores what a post says about the
  announcement itself. A post that welcomes it but asks for more or for a guarantee is positive; one that weighs both
  sides about evenly, or mainly asks a question, is close to 0.

What version 3 answers, measured on the version 2 balanced runs with organisation accounts left out: about half of
the first posts of people written as undecided (matched by their written stance) scored against and 8% in favour, so
the first reaction leaned negative whatever the planner expected; and people whose first post was in favour posted on
average 0.55 lower afterwards, those in the middle 0.27 lower, while critics posted 0.10 higher.

Unlike versions 1 and 2, version 3 was written after looking at the cases it was then measured on (the balanced
cases), so its scores on those cases are not an independent test.

### Scoring

One judge call (Claude Sonnet 5.5, prompt in [`bench/judge_prompt.md`](../bench/judge_prompt.md)) reads Tellfold's
report and a checklist: the seed's own headline (not the rubric title, which was written with hindsight), the
question, the objections and the pushback groups. It is never shown the true direction or backdown. A case with no
pushback groups leaves them out of the checklist, the prompt and the reply format altogether, because an empty list
would tell the judge that nobody pushed back. It classifies the direction and backdown the report predicts blind, marks each
objection and group, and is told not to use anything it may know about the event. The harness then compares its
answer with the rubric:

| score | what it measures |
|---|---|
| **direction** | the report's predicted overall reaction equals the rubric's (negative, mixed, positive) |
| **objection recall** | share of the rubric's objections the report raises: covered 1, partial 0.5, missed 0 |
| **backdown** | the report predicts a change (partial or full) exactly when one happened. A report that doesn't say gets no credit |
| **groups named** | share of the rubric's pushback groups the report names |
| **direction from sentiment** | a judge-free check: the last round's mean sentiment read as negative (below −0.2), mixed or positive (above 0.2). A run whose last round has no sentiment score gets none; an earlier round is never used instead |

Two baselines sit next to the scores, because a number means little on its own:

- **Ignoring the announcement.** Always answering "negative" and always answering "no backdown". If Tellfold doesn't
  beat these, the simulation adds nothing.
- **No crowd** (`--zero-shot`). One direct call to the same planner model with the same seed and question, graded by
  the same judge. If Tellfold doesn't beat this, the crowd adds nothing over asking the model once.

## Cases

43 cases: 41 announced after the cutoff and 2 controls. Reactions: 21 negative, 12 mixed, 10 positive. Backdowns: 6 full, 4 partial, 33 none.

The first 20 cases leaned negative (15 of 20): well-sourced positive reactions were hard to find, because quiet approval is rarely reported. On 13 November we added 23 cases chosen to balance the set ([PR #22](https://github.com/NarekManukyan/tellfold/pull/22)): 6 negative, 10 mixed and 7 positive. Always answering "negative" now gets 49% of directions right, against 75% on the first 20 alone, so that is the bar to beat, not 33%. Some outcomes may still move: the most recent cases were only a few weeks old when written, and each case's `notes` field says where the facts are thin or the dates uncertain.

| case | type | announced | reaction | backdown | |
|---|---|---|---|---|---|
| [HubSpot terms let enrichment data be shared across customers by default](../bench/cases/hubspot-contact-data-sharing-terms-2026/) | policy | 2026-07-01 | negative | full |  |
| [Sony ends physical disc production for new PlayStation games from 2028](../bench/cases/sony-playstation-ends-physical-discs-2026/) | product-change | 2026-07-01 | negative | none |  |
| [Hillsborough County schools restrict leggings and spandex in 2026-27 dress code](../bench/cases/hillsborough-leggings-dress-code-2026/) | school | 2026-07-14 | negative | full |  |
| [Irish government raises the e-scooter age limit to 18 and makes helmets and hi-vis mandatory, stopping short of a full ban](../bench/cases/ireland-e-scooter-under-18-ban-2026/) | policy | 2026-07-20 | mixed | none |  |
| [Garmin launches CIRQA band with live workout heart rate behind Connect+](../bench/cases/garmin-cirqa-live-heart-rate-paywall-2026/) | product-change | 2026-07-21 | negative | partial |  |
| [UK government removes VAT from household electricity bills from October 2026](../bench/cases/uk-electricity-vat-cut-2026/) | pricing | 2026-07-21 | positive | none |  |
| [Samsung launches a passport-shaped Galaxy Z Fold 8, a Fold 8 Ultra and Z Flip 8, each $100 dearer](../bench/cases/samsung-galaxy-z-fold-8-launch-2026/) | product-change | 2026-07-22 | positive | none |  |
| [UK government cuts the England bus fare cap from £3 to £2 for 2027, funded from climate finance](../bench/cases/uk-two-pound-bus-fare-cap-2026/) | public-service | 2026-07-22 | positive | none |  |
| [Barnet Council cuts Mays Lane paid parking to one weekday hour after trader complaints](../bench/cases/barnet-mays-lane-parking-2026/) | local-government | 2026-07-23 | positive | none |  |
| [MBTA launches unlimited-transfer, express-bus and car-free-senior fare pilots](../bench/cases/mbta-fare-pilots-2026/) | public-service | 2026-07-28 | positive | none |  |
| [Jetstar charges for overhead locker cabin bags from February 2027](../bench/cases/jetstar-overhead-locker-fee-2026/) | pricing | 2026-08-05 | negative | none |  |
| [University of Michigan's LSA will show freshmen's first-semester grades as Pass/No Credit from fall 2027](../bench/cases/umich-first-semester-pass-fail-2026/) | school | 2026-08-06 | mixed | partial |  |
| [Google launches the Pixel 11 range at $100 more per model, with 256 GB base storage](../bench/cases/google-pixel-11-launch-price-rise-2026/) | product-change | 2026-08-12 | mixed | none |  |
| [Instagram refreshes its wordmark for the first time in a decade](../bench/cases/instagram-wordmark-refresh-2026/) | rebrand | 2026-08-13 | mixed | none |  |
| [Barclays requires UK staff in the office three days a week from October 2026](../bench/cases/barclays-three-day-office-2026/) | workplace | 2026-08-17 | negative | partial |  |
| [Rebel Wolves confirms The Blood of Dawnwalker tops out at 40fps on consoles, with no 60fps mode](../bench/cases/blood-of-dawnwalker-40fps-console-cap-2026/) | product-change | 2026-08-21 | negative | full |  |
| [Iron County School District (Utah) votes 4-3 to adopt a four-day school week from 2027-28](../bench/cases/iron-county-four-day-week-2026/) | school | 2026-08-25 | mixed | none |  |
| [CD Projekt Red makes The Witcher 3 Remastered a free upgrade](../bench/cases/witcher-3-free-remaster-upgrade-2026/) | product-change | 2026-08-25 | positive | none |  |
| [Anthropic ends a temporary 50% Claude Code boost and sets permanent weekly limits 25% above the old standard](../bench/cases/anthropic-claude-code-weekly-limits-2026/) | pricing | 2026-08-29 | negative | none |  |
| [Ireland's National Transport Authority raises adult bus and rail fares 15% on average from January 2027](../bench/cases/ireland-nta-fare-increase-2026/) | public-service | 2026-09-03 | negative | none |  |
| [Olathe Public Schools recommends closing five elementary schools and one middle school](../bench/cases/olathe-school-closures-2026/) | school | 2026-09-03 | negative | none |  |
| [Xbox caps Game Pass cloud gaming at 5-15 hours a month](../bench/cases/xbox-cloud-gaming-monthly-caps-2026/) | product-change | 2026-09-03 | negative | none |  |
| [Apple unveils the $1,999 iPhone Duo foldable and iPhone 18 Pro, and raises prices of existing iPhones by $100](../bench/cases/apple-iphone-duo-and-price-rises-2026/) | pricing | 2026-09-09 | mixed | none |  |
| [Blizzard announces World of Warcraft: Forever, with its new Skyborne race sold in $30+ packs](../bench/cases/wow-forever-skyborne-paid-race-2026/) | product-change | 2026-09-12 | mixed | none |  |
| [UT Austin stops new enrolment in African diaspora, Mexican American/Latino and women's and gender studies majors](../bench/cases/ut-austin-ethnic-gender-majors-2026/) | school | 2026-09-16 | negative | none |  |
| [Costco brings the Twisted Churro back to US food courts for a limited time](../bench/cases/costco-churro-return-2026/) | product-change | 2026-09-17 | mixed | none |  |
| [Bungie says Destiny 2 will continue and vaulted campaigns, destinations and raids will return](../bench/cases/bungie-destiny-2-unvaulting-2026/) | product-change | 2026-09-21 | positive | none |  |
| [McLaren replaces its wordmark with a design based on Bruce McLaren's family service station sign](../bench/cases/mclaren-heritage-wordmark-2026/) | rebrand | 2026-09-22 | mixed | none |  |
| [Quake Champions drops free-to-play for a paid Steam release and ends content updates](../bench/cases/quake-champions-goes-paid-2026/) | pricing | 2026-09-23 | mixed | none |  |
| [OpenAI halves the ChatGPT Pro 200 usage allowance and adds Pro 500](../bench/cases/openai-chatgpt-pro-200-usage-cut-2026/) | pricing | 2026-09-29 | negative | full |  |
| [Reddit ends RSS feeds on 13 November and closes its public API by March 2027](../bench/cases/reddit-ends-rss-and-public-api-2026/) | product-change | 2026-09-30 | negative | none |  |
| [Jagex announces RuneScape Reignited, a fresh-start mode with a separate economy](../bench/cases/runescape-reignited-fresh-start-mode-2026/) | product-change | 2026-10-03 | negative | full |  |
| [Ireland's Budget 2027 cuts carbon tax on home heating oil and gas and freezes it for the government's term](../bench/cases/ireland-home-heating-carbon-tax-cut-2026/) | pricing | 2026-10-06 | mixed | none |  |
| [Minneapolis Public Schools drops plans to close up to 16 schools and bars closures for 2027-28](../bench/cases/minneapolis-school-closures-pause-2026/) | school | 2026-10-13 | positive | none |  |
| [San Francisco school board votes 5-2 to replace the elementary school lottery with neighbourhood assignment](../bench/cases/sfusd-neighborhood-school-assignment-2026/) | school | 2026-10-13 | mixed | none |  |
| [United drops flight-based MileagePlus status and raises Premier 1K to 32,000 PQPs for 2027](../bench/cases/united-mileageplus-spend-only-status-2026/) | policy | 2026-10-13 | negative | none |  |
| [Meta's new Commercial Terms let it crawl business-linked websites](../bench/cases/meta-commercial-terms-website-crawling-2026/) | policy | 2026-10-15 | negative | full |  |
| [NatWest ends remote-first working and requires two office days a week from February 2027](../bench/cases/natwest-office-attendance-2026/) | workplace | 2026-10-20 | negative | none |  |
| [Apple Music drops its red icon for a black-and-white identity](../bench/cases/apple-music-black-and-white-rebrand-2026/) | rebrand | 2026-10-21 | negative | none |  |
| [Grossmont Union High School District settles with its union and brings back teacher-librarians from 2027-28](../bench/cases/grossmont-librarians-reinstated-2026/) | school | 2026-10-21 | positive | none |  |
| [Delta keeps SkyMiles Medallion spending thresholds unchanged for a third year](../bench/cases/delta-medallion-thresholds-unchanged-2026/) | policy | 2026-10-22 | positive | none |  |
| [Dublin City Council extends its ban on street bin bags to residential Dublin 7](../bench/cases/dublin-7-bin-bag-ban-2026/) | local-government | 2026-05-07 | negative | partial | control: known to model |
| [Nintendo raises the Switch 2 price to $499.99](../bench/cases/nintendo-switch-2-price-rise-2026/) | pricing | 2026-05-08 | negative | none | control: known to model |

## Results

Four passes so far, with the same models, settings and judge, each with the zero-shot baseline. The first two
ran the original 20 cases with one seed (7), the third the same 20 with three seeds (7, 8 and 9), and the fourth all
43 cases with three seeds. A separate run put the 23 new cases through the third pass's engine, so version 2 can be
compared on all 43; it is listed with the passes but not counted as one:

- **11 November 2026, commit `a92638f`:** the engine as released. All 20 runs and all 20 zero-shot answers were
  graded. Results: [`2026-11-11-sonnet-haiku.json`](../bench/results/2026-11-11-sonnet-haiku.json), per-case table in
  [`2026-11-11-sonnet-haiku.md`](../bench/results/2026-11-11-sonnet-haiku.md).
- **12 November 2026, commit `a963978`:** the same engine with the [crowd rules](#crowd-rules). 19 runs were graded.
  The UT Austin simulation finished, but its report failed: the report call returned JSON without the required
  `summary` field twice, and the harness leaves a failed run out of the averages. All 20 zero-shot answers were
  graded. Results: [`2026-11-12-sonnet-haiku.json`](../bench/results/2026-11-12-sonnet-haiku.json), per-case table in
  [`2026-11-12-sonnet-haiku.md`](../bench/results/2026-11-12-sonnet-haiku.md).
- **13 November 2026, third pass, commit `5ba314c`:** crowd rules version 2 on the 20 cases, three seeds each. All
  60 runs and all 20 zero-shot answers were graded. A network outage stopped the pass overnight; the harness resumed
  it, kept the finished runs and reran only the rest. Results:
  [`2026-11-13-sonnet-haiku.json`](../bench/results/2026-11-13-sonnet-haiku.json), per-case table in
  [`2026-11-13-sonnet-haiku.md`](../bench/results/2026-11-13-sonnet-haiku.md).
- **13 November 2026, the 23 balanced cases, same engine:** version 2 on the cases added that day, three seeds
  each. 64 of the 69 runs are graded. The API refused sign-in for part of the afternoon, and by the time the harness
  resumed, the engine in the working copy had moved to version 3, so five resumed runs (Delta seeds 8 and 9, McLaren
  seeds 8 and 9, SFUSD seed 9) ran version 3 and are left out. All 23 zero-shot answers were graded. Results:
  [`2026-11-13-sonnet-haiku-balanced.json`](../bench/results/2026-11-13-sonnet-haiku-balanced.json), per-case table
  in [`2026-11-13-sonnet-haiku-balanced.md`](../bench/results/2026-11-13-sonnet-haiku-balanced.md).
- **13 November 2026, fourth pass, commit `62357bc`:** crowd rules version 3 on all 43 cases, three seeds each. All
  129 runs and 42 of the 43 zero-shot answers were graded (one zero-shot call failed). Results:
  [`2026-11-13-sonnet-haiku-rules3.json`](../bench/results/2026-11-13-sonnet-haiku-rules3.json), per-case table in
  [`2026-11-13-sonnet-haiku-rules3.md`](../bench/results/2026-11-13-sonnet-haiku-rules3.md).

Every judge verdict and the quote behind it is in the JSON files.

### Third and fourth passes (13 November)

All 43 cases. Version 2 is the third pass and the balanced cases together; version 3 is the fourth pass. "Planner's
mix" is the direction the mix the planner gave implies, by the same first-reaction rule, before anyone in the crowd
posts. It is scored as the planner gave it, before the bounds that keep 15% on each side; the crowd is drawn from the
bounded mix. The two zero-shot columns are separate samples of the same prompt.

| | Tellfold v2 | Planner's mix v2 | No crowd (v2 pass) | Tellfold v3 | Planner's mix v3 | No crowd (v3 pass) | Ignoring the announcement |
|---|---|---|---|---|---|---|---|
| graded | 124 runs | 124 runs | 43 cases | 129 runs | 129 runs | 42 cases | 43 cases |
| direction | 51% | 71% | 58% | 55% | 66% | 62% | 49% |
| objection recall | 52% | – | 65% | 52% | – | 67% | – |
| backdown | 72% | – | 65% | 70% | – | 64% | 77% |
| groups named | 57% | – | 71% | 56% | – | 76% | – |
| direction from sentiment | 50% | – | – | 49% | – | – | – |

Direction by set:

| | Tellfold v2 | Planner's mix v2 | No crowd (v2 pass) | Tellfold v3 | Planner's mix v3 | No crowd (v3 pass) | Always "negative" |
|---|---|---|---|---|---|---|---|
| original 20 cases | 47 of 60 | 48 of 60 | 8 of 20 | 44 of 60 | 48 of 60 | 10 of 19 | 15 of 20 |
| 23 balanced cases | 16 of 64 | 40 of 64 | 17 of 23 | 27 of 69 | 37 of 69 | 16 of 23 | 6 of 23 |

Direction by the real reaction, in runs:

| real reaction | runs v2 | Tellfold v2 | Planner's mix v2 | runs v3 | Tellfold v3 | Planner's mix v3 |
|---|---|---|---|---|---|---|
| negative | 63 | 53 | 53 | 63 | 50 | 52 |
| mixed | 33 | 8 | 18 | 36 | 18 | 17 |
| positive | 28 | 2 | 17 | 30 | 3 | 16 |

What this says:

- **The crowd makes the planner's guess worse.** In both versions the planner's starting mix called more directions
  right than the crowd that started from it: 71% against 51% with version 2, 66% against 55% with version 3. With
  version 2 the crowd's final direction moved away from the planner's in 48 of 124 runs: 9 times to the right answer,
  34 times away from it and 5 times from one wrong answer to another. On the balanced cases alone it moved 28 times
  and never to the right answer. With version 3 it moved in 51 of 129 runs: 16 right, 30 away, 5 from wrong to wrong.
- **The crowd almost never says positive.** It predicted a positive reaction in 8 of 124 runs with version 2 and 8
  of 129 with version 3, in a set where 10 of the 43 reactions were positive. In the runs whose real reaction was
  positive it was right 2 times of 28 and 3 of 30; the planner's mix was right 17 and 16 times. In the MBTA fare
  pilots, the Minneapolis closures pause and the UK £2 bus fare cap, the planner's mix was positive in all six runs
  and the crowd came out mixed in all six. The UK electricity VAT cut came out negative in four of six.
- **The third pass looked good because the first 20 cases lean negative.** On those cases version 2 got 47 of 60,
  above always "negative" (15 of 20). On the balanced cases it got 16 of 64, level with always "negative" (6 of 23),
  while asking the model once got 17 of 23.
- **Version 3 helped on mixed reactions and cost a little on negative ones.** Mixed reactions went from 8 of 33 right
  to 18 of 36, negative ones from 53 to 50 of 63, and positive ones didn't move. Overall that is 51% to 55%, a small
  change on 43 cases, and version 3 was written after seeing the balanced cases. Backdown on the original 20 cases
  fell from 40 to 32 of 60 runs right.
- **The model alone still names more of what people said.** Its objection recall (65% and 67%) and groups named
  (71% and 76%) beat Tellfold's in every pass. Tellfold's backdown (72% and 70%) beats the model's (65% and 64%) but
  not always "no backdown" (77%).
- **The controls don't show recall.** On the two cases known to the model, version 2 got all 6 directions and
  version 3 got 4 of 6. That is too few runs to say more than that the controls don't stand out.

### What still goes wrong on 13 November

Every case whose real reaction was not negative, with the direction of each run (seeds 7, 8, 9; "–" is a run left
out):

| case | actual | Tellfold v2 | Planner's mix v2 | Tellfold v3 | Planner's mix v3 | No crowd (v2 pass) |
|---|---|---|---|---|---|---|
| Apple iPhone Duo and price rises | mixed | neg neg neg | neg neg neg | neg neg neg | neg neg neg | mixed |
| Barnet parking | positive | mixed mixed mixed | pos pos pos | mixed pos mixed | mixed pos pos | pos |
| Bungie Destiny 2 unvaulting | positive | mixed mixed mixed | mixed mixed mixed | mixed mixed mixed | mixed neg mixed | mixed |
| Costco churro return | mixed | mixed pos pos | mixed pos pos | mixed mixed pos | pos pos pos | pos |
| Delta Medallion thresholds unchanged | positive | mixed – – | pos – – | mixed mixed pos | mixed mixed pos | pos |
| Google Pixel 11 price rise | mixed | neg neg neg | neg neg neg | neg neg neg | neg neg neg | mixed |
| Grossmont librarians reinstated | positive | neg mixed mixed | mixed mixed mixed | mixed neg mixed | mixed mixed mixed | mixed |
| Instagram wordmark | mixed | mixed mixed neg | mixed mixed mixed | mixed mixed mixed | mixed mixed mixed | mixed |
| Ireland e-scooter under-18 ban | mixed | pos neg mixed | mixed mixed mixed | neg mixed neg | mixed mixed mixed | mixed |
| Ireland home heating carbon tax cut | mixed | neg neg neg | mixed mixed mixed | neg mixed neg | mixed pos pos | mixed |
| Iron County four-day week | mixed | neg mixed mixed | pos pos pos | mixed mixed neg | pos pos mixed | pos |
| MBTA fare pilots | positive | mixed mixed mixed | pos pos pos | mixed mixed mixed | pos pos pos | pos |
| McLaren heritage wordmark | mixed | pos – – | mixed – – | pos mixed mixed | mixed mixed neg | mixed |
| Minneapolis school closures pause | positive | mixed mixed mixed | pos pos pos | mixed mixed mixed | pos pos pos | pos |
| Quake Champions goes paid | mixed | neg neg neg | neg mixed mixed | mixed neg mixed | mixed mixed neg | mixed |
| Samsung Galaxy Z Fold 8 launch | positive | neg neg mixed | neg neg neg | neg neg neg | neg neg neg | mixed |
| SFUSD school assignment | mixed | neg neg – | neg neg – | mixed neg neg | neg neg neg | mixed |
| UK electricity VAT cut | positive | mixed neg neg | mixed pos pos | neg neg mixed | mixed neg pos | mixed |
| UK £2 bus fare cap | positive | mixed mixed mixed | pos pos pos | mixed mixed mixed | pos pos pos | pos |
| UMich first-semester pass/fail | mixed | mixed mixed neg | mixed mixed mixed | pos neg mixed | mixed mixed mixed | mixed |
| Witcher 3 free remaster upgrade | positive | neg pos pos | neg pos pos | pos mixed mixed | pos pos pos | pos |
| WoW Forever paid race | mixed | neg neg neg | mixed mixed neg | mixed mixed mixed | mixed neg mixed | mixed |

Two patterns stand out. Where the news is good for the people in the cast (a fare cap, a tax cut, closures paused,
librarians reinstated), the crowd still lands on mixed or negative. A likely reason, not yet tested: the 15% floor
puts critics in every cast, the others answer them with worries and requests for more, and the conversation reads as
divided. Where the news is a price rise with something new attached (the iPhone Duo, the Pixel 11), the
planner and the crowd both call it negative, while the model alone, and the real reaction, were mixed.

One other cause was tested and ruled out. Every case's question asks "which groups will push back and why, and is X
likely to keep, change or reverse the decision", and the crowd saw that question in every prompt. A trial version
(commit `e54bdd5`, not kept) showed the crowd only the situation and kept the question for the planner, the sentiment
scorer and the report. On the 10 positive cases, 3 seeds each (`bench/results/2026-11-13-sonnet-haiku-rules4-positive.json`),
it called 3 of 30 runs right, the same as version 3, while the planner's mix in those runs called 14. Objection
recall didn't change (57% against 56%). The crowd's lean against good news doesn't come from the wording of the
question.

What we take from this, as hypotheses for the next pass: on this benchmark the simulated crowd does not add accuracy
to one call of the same model, and on the direction it takes some away. A Tellfold report is better read as a
rehearsal of the arguments people may raise than as a forecast of how the reaction will go. The next things to
test are whether the 15% floor on each side should apply when the planner expects a clear lean, and whether the
direction should come from the planner's mix with the crowd used only for the arguments. Both need a pass on cases
that weren't used to design them.

### The first two passes

All cases:

| | Tellfold 11 Nov | Tellfold 12 Nov | No crowd 11 Nov | No crowd 12 Nov | Ignoring the announcement |
|---|---|---|---|---|---|
| cases graded | 20 | 19 | 20 | 20 | 20 |
| direction | 30% | 37% | 70% | 55% | 75% |
| objection recall | 35% | 41% | 66% | 68% | – |
| backdown | 80% | 68% | 65% | 55% | 65% |
| groups named | 47% | 36% | 70% | 69% | – |
| direction from sentiment | 30% | 37% | – | – | – |

After the models' cutoff:

| | Tellfold 11 Nov | Tellfold 12 Nov | No crowd 11 Nov | No crowd 12 Nov | Ignoring the announcement |
|---|---|---|---|---|---|
| cases graded | 18 | 17 | 18 | 18 | 18 |
| direction | 33% | 29% | 67% | 56% | 72% |
| objection recall | 34% | 43% | 67% | 71% | – |
| backdown | 78% | 65% | 61% | 50% | 67% |
| groups named | 49% | 36% | 72% | 74% | – |
| direction from sentiment | 33% | 35% | – | – | – |

"Ignoring the announcement" is always answering "negative" for the direction and "no backdown" for the backdown.
The two `known_to_model` controls went from 0 to 2 directions right, with backdown right in both passes. On the 19
cases both passes graded, Tellfold went from 6 to 7 directions right, from 35% to 41% objection recall, from 16 to 13
backdowns right and from 47% to 36% of groups named.

### Second pass (12 November)

What this says:

- **Direction: still worse than both baselines.** 7 of 19 right, against 11 of 20 for the model alone and 15 of 20
  for always "negative".
- **The difference between the passes is within the noise.** Nothing changed in the zero-shot baseline between the
  two passes (same model, same prompt, same seed text), yet it went from 14 to 11 directions right and from 13 to 11
  backdowns right. That is how much one sample per case moves on 20 cases. Tellfold's changes (one more direction
  right, three fewer backdowns, six points more objections) are no bigger, so this pass doesn't show that the rules
  helped or hurt any score. It needs `--repeats 3` to tell.
- **Everyone now speaks.** On 11 November 164 of the 400 simulated people (41%) never posted. On 12 November 12 of 400
  (3%) never posted, and everyone else had posted by round 2. Nine of the 12 were cast as indifferent. Olathe still
  had three silent, one of them cast as strongly opposed.
- **The direction rule did what it says, and showed where the hedging comes from.** The report followed the
  first-reaction count in 16 of 19 runs. But the count itself came out mixed in 12 of the 19: in Instagram most
  people took no side, and in the other 11 the sides were close to even (Apple Music 5 against and 6 in favour,
  Garmin 5 and 6, Hillsborough 6 and 6, Olathe 4 and 5). The rule calls those mixed, as it should. The reason is
  upstream. Without audience material, as in every bench run, each cast is drawn from the same fixed stance mix
  (about a third supportive, a third critical, a third undecided or indifferent; `STANCE_MIX` in
  [`personas.rs`](../engine-rs/crates/tellfold-engine/src/personas.rs)), whatever the announcement says. People's
  first posts follow the stance they were cast with: of 106 people cast as supportive, 78 posted in favour first and
  10 against; of 102 cast as critical, 87 posted against first and 2 in favour (reading each stance by its wording).
  So the crowd starts near even by construction, and a report that reads the crowd honestly says mixed.
- **Backdown fell, for a reason worth knowing.** Three full reversals (Hillsborough, Meta, OpenAI) that the first
  pass called "partial" were called "none". In the first pass the reports often predicted a change because the
  simulated organisation had already conceded something inside the run. This time the three reports saw the
  organisation hold its line with monitoring and talks, and predicted no change. Either way the report reads a
  backdown off the organisation's simulated account, which is not a model of how the real organisation decides.

### Where it still goes wrong

Every direction miss of the 12 November pass, with the first-reaction count the engine saved in `report.json`
(against, in favour, neither; organisation accounts left out):

| case | actual | Tellfold | first reaction |
|---|---|---|---|
| Apple Music rebrand | negative | mixed | 5, 6, 4 |
| Barnet parking | positive | negative | 7, 4, 6 |
| Garmin CIRQA paywall | negative | mixed | 5, 6, 4 |
| Hillsborough dress code | negative | mixed | 6, 6, 2 |
| HubSpot data sharing | negative | mixed | 5, 4, 5 |
| Ireland NTA fares | negative | mixed | 7, 5, 1 |
| Iron County four-day week | mixed | negative | 8, 3, 2 |
| MBTA fare pilots | positive | mixed | 4, 4, 6 |
| Meta crawling terms | negative | mixed | 12, 3, 3 (the report called it mixed against the count) |
| Olathe school closures | negative | mixed | 4, 5, 2 |
| OpenAI Pro 200 cut | negative | mixed | 7, 5, 5 |
| Xbox cloud caps | negative | mixed | 8, 6, 6 |

Nintendo (7, 6, 7) and Sony (7, 5, 4) were called negative, correctly, by reports that went against a mixed count.
Barclays, Dublin 7, Instagram, NatWest and the Witcher 3 upgrade were right and matched the count.

Two of the four fixes didn't fully hold:

- **Organisations still invent things, late in a run.** The rule is in every prompt, but under pressure from users
  the organisation's account still announced things the input doesn't have. Barclays' account said in round 6 that
  it had "shared consultation framework and flexibility guidance" and that implementation would include "day-choice
  options for most roles and carers' case-by-case review". HubSpot's said in round 5 that it was "prioritizing
  client-level opt-out controls". Meta's said in round 6 that licensing talks with publishers were "constructive".
  MBTA's promised in round 3 to publish budget figures. We found these with a keyword scan of the 232 organisation
  posts, not a full count, so there may be more. A rule in the prompt is not enough on its own.
- **NatWest still went off topic, because of its cast.** The announcement is about more than 30,000 staff and how
  many days they spend in the office. The cast was mostly customers, students and one union rep, and their profiles
  are about bank branches and digital banking. 15 of the 56 posts were about redundancies or restructuring, against
  2 of 37 in the first pass, and objection recall stayed at 0%. The topic rule shapes the rounds; it can't fix a cast
  that is about something else.

Other misses: Iron County, NatWest and Sony were called a partial change where none came, and Meta, OpenAI and
Hillsborough no change where the decision was reversed in full. Nintendo's report raised none of the objections.
UT Austin's report failed, as described above; the first pass had no report failures, and we don't know yet whether
the longer report prompt made it more likely.

What we take from this, again as hypotheses for the next pass: the direction is mostly set by the cast's fixed
stance mix, so the next fix is to let the cast's mix follow the announcement. That has a cost to watch: if the
planner model chooses the mix, its own expectation goes into the cast, and the crowd's direction becomes partly the
zero-shot answer by another route. A pass would need to show the crowd adds something beyond that. Organisation
posts need a check against the source material rather than a rule in the prompt, and casts need a check that they
are made of the people the announcement affects.

### First pass (11 November)

The first pass's own table:

| | cases | direction | objection recall | backdown | groups named | direction from sentiment |
|---|---|---|---|---|---|---|
| Tellfold, after cutoff | 18 | 33% | 34% | 78% | 49% | 33% |
| Tellfold, known to model | 2 | 0% | 40% | 100% | 33% | 0% |
| Tellfold, all cases | 20 | 30% | 35% | 80% | 47% | 30% |
| No crowd (zero-shot), after cutoff | 18 | 67% | 67% | 61% | 72% | – |
| No crowd (zero-shot), all cases | 20 | 70% | 66% | 65% | 70% | – |
| Always "negative" / "no backdown", after cutoff | 18 | 72% | – | 67% | – | – |
| Always "negative" / "no backdown", all cases | 20 | 75% | – | 65% | – | – |

"Groups named" covers the 18 cases that have pushback groups. With 20 cases, one case is five points.

What this says:

- **Direction: worse than both baselines.** Tellfold called 6 of 20 right. Asking the model once called 14, and
  always answering "negative" would have called 15. This is the main result of the pass.
- **Objections: about half of what the model finds on its own.** The reports raised 35% of the objections people
  actually made; the zero-shot answers raised 66%.
- **Backdown: the one score above the baselines, and only just.** Tellfold said correctly whether the organisation
  would change course in 16 of 20 cases, against 13 for "never backs down" and 13 for zero-shot. That is three cases,
  on one seed. It caught 6 of the 7 real backdowns but called a change in 3 cases where none came. It never
  predicted a full reversal: all 4 full reversals were called "partial". Counting the exact kind (none, partial or
  full), it got 12 of 20, and zero-shot 11.
- **The control says nothing yet.** Two `known_to_model` cases are too few to show recall either way. Tellfold got
  the direction of both wrong; zero-shot got both right.
- **Sentiment agrees with the judge.** The judge-free reading of the last round's sentiment also gets 6 of 20, so
  the low direction score is not the judge misreading the reports.

#### Where it went wrong

Every miss from the 11 November pass. "Said" is what the judge quoted from Tellfold's report; anyone can check the
quote against the report in the results file.

**It hedged to "mixed".** The reports called 15 of the 20 cases mixed. Two were. All three positive cases and 10 of
the 15 negative ones were called mixed. In most of them the simulated crowd started negative and then recovered,
often after an agent playing the organisation posted reassurance, data or a concession inside the simulation. In
the real cases the reaction stayed negative, or, for the three positive ones, stayed positive.

| case | actual | Tellfold | what the report said instead |
|---|---|---|---|
| Apple Music rebrand | negative | positive | Sentiment "opens positive at +0.37" and "climbs sharply" to +0.77. Missed that people found the new icon dull and lost the red's identity |
| Barnet parking | positive | mixed | Traders turned sentiment from +0.35 to −0.57, while residents "stayed broadly satisfied" |
| Dublin 7 bin bags (control) | negative | mixed | Sentiment fell to −0.60, then recovered to +0.12 "once" the council and waste firm "commit to specific, dated deliverables" |
| Garmin CIRQA paywall | negative | mixed | Casual and training buyers "warm up"; Garmin wins sceptics "through documentation promises". Also called no backdown; Garmin reversed the live heart-rate part |
| Hillsborough dress code | negative | mixed | Sentiment rose to +0.18 after the simulated district "admitted enforcement gaps". Called a partial change; the board dropped the restriction. Raised 1 of 4 objections, partly |
| HubSpot data sharing | negative | mixed | An opt-in concession "flipped sentiment to +0.38". Called partial; HubSpot withdrew the change |
| Ireland NTA fares | negative | mixed | Crowd settled on "conditional acceptance pending Q1-Q2 evidence". Named none of the groups that pushed back |
| MBTA fare pilots | positive | mixed | Pushback from riders who "distrust pilots as political theater" and riders left out, though sentiment ended at +0.30 |
| Nintendo Switch 2 price (control) | negative | mixed | Anger "cooled" after invented preorder data showed demand "+18% above Switch 1 pace" |
| Olathe school closures | negative | mixed | The crowd "never turned hostile"; five personas with critical stances "never posted a single message" |
| Sony ends discs | negative | mixed | The crowd split "between pragmatic acceptance" and demands for guarantees. Called a partial change from a promised FAQ; Sony changed nothing |
| UT Austin majors | negative | mixed | Sentiment "swung wildly" (−0.73, +0.60, −0.31). Called a partial change; there was none |
| Witcher 3 free upgrade | positive | mixed | No one questioned the free upgrade, but an invented "labor-transparency thread" pulled sentiment to −0.35 |
| Xbox cloud caps | negative | mixed | Sentiment ended at +0.10 after the simulated Xbox account announced "a grandfathering concession". Xbox kept the caps and only added paid top-ups |

**Right direction, wrong backdown.** Meta's crawling terms and OpenAI's Pro 200 cut were called negative, correctly,
but both reports predicted a partial change where the companies reversed in full.

**Right for the wrong reason.** NatWest was called negative, correctly and with no backdown, correctly, but the
simulation drifted to an invented Ulster Bank branch restructuring and argued about capital spending and rural
branches. It raised none of the four objections staff actually made (the union consultation process, travel and
childcare costs, caring and health needs, a broken understanding about remote work) and named none of the groups.
Its objection recall was 0%.

**Thin objection coverage even when the direction was right.** Instagram's wordmark (mixed, correct) raised 1 of 4
objections, partly. Barclays (negative, correct) raised 50%.

What we take from this, as hypotheses for the next pass rather than findings: the simulated crowd is pulled towards
the middle, in-simulation responses from the organisation calm it in a way the real world didn't, and some runs drift
away from the announcement. The [crowd rules](#crowd-rules) were written to test these; the
[second pass](#second-pass-12-november) shows how far they got.

### Judge check

The judge's verdicts were reread for six reports of the 11 November pass: Apple Music, Barclays, Hillsborough,
NatWest, Olathe and Xbox. That covers 6 directions, 6 backdowns and 26 objection verdicts. The second reading was
done by the Claude session that ran the benchmark, reading each report against the checklist; it is not a check by a
person.

- **Direction: 5 of 6 agreed.** Olathe was called "mixed" by the judge, but the report says the crowd "never turned
  hostile" and leans "yes with conditions", which reads as positive. Both are wrong against the real reaction
  (negative), so the score doesn't change. Hillsborough and Xbox were called mixed: both reports describe an early
  negative turn that the simulated organisation then calmed down, so mixed is a fair reading of what they say.
- **Backdown: 6 of 6 agreed.** Hillsborough is the closest call: the report keeps the dress code and predicts changes
  to how it is enforced, which the judge counted as partial under its rules.
- **Objections: 25 of 26 agreed.** The one disagreement is Xbox's "the 4% figure understates the impact on cloud-only
  players", marked partial because the report says cloud-only players are the most affected; the report never
  questions the figure, so it reads as missed. A few other partials (Apple Music's "looks like a different app",
  Barclays' "delay until the union consultation ends") are generous but defensible.

On these six the judge looks slightly lenient on objections and accurate on direction and backdown. Six reports is
a small sample, and they all come from the first pass; the second pass's verdicts have not been reread.

## Known limits

Read the results with these in mind. Some of them we can measure; some we can only name.

- **The model may already know.** The cases come after the models' stated training cutoff, but cutoffs are fuzzy and
  similar stories from earlier years are in the training data ("price rises on a streaming service usually go
  badly"). The `known_to_model` control and the zero-shot baseline show how much of the score that explains; they
  don't remove it.
- **We wrote the inputs.** `input.md` is our summary, not the original announcement, so our choice of what to
  include can steer the result. Whoever wrote a summary had already read how the story ended, so hindsight can leak
  into what it stresses. We kept each one to the facts of the day, and the harness warns about words that describe a
  reaction, but a reader should check a few against the sources.
- **We chose the cases.** Well-documented reactions are the loud ones. Quiet acceptance rarely makes the news, so
  the first 20 cases leaned towards negative reactions however hard we tried to balance them. The 23 cases added on 13
  November were chosen to balance the set, and the "always negative" baseline makes any lean visible.
- **The crowd is small.** Twenty simulated people over six rounds is a town meeting, not the internet. Tellfold can't
  model press coverage, virality, organised campaigns or the decision-makers' own politics, which often decide
  whether an organisation backs down.
- **The judge is an LLM.** Grading objection recall needs judgement, so a model does it. It can be lenient, strict or
  inconsistent. Its prompt is public, it sees only the report, the seed's headline and the rubric's objections and
  groups (never the true direction or backdown), and every verdict comes with a quote
  from the report so it can be checked by hand. The [judge check](#judge-check) above compares six of its gradings
  with a second reading; that second reading was also done by a Claude model, so a check by a person is still owed.
- **Few cases.** With 20 cases, one case moves a percentage by five points; with 43, by more than two. Three seeds
  per case show the spread of one case, but they are not three more cases. Differences of a few points between two
  passes mean little. The zero-shot baseline, which didn't change at all between the first two passes, still moved
  by three cases on direction and two on backdown, and it got 8 of the 20 original cases in the third pass.
- **"Direction" is a judgement too.** Reporting rarely says "62% of people were against". The rubric's direction is
  our reading of the coverage, and mixed reactions are the hardest to call.
- **The judge was sometimes asked twice.** Until the review of this page's pull request, the harness took any reply
  containing the word "network" for a dropped connection and asked again, so a judge or zero-shot answer that
  mentioned a bus, rail or social network was replaced by a later one that didn't. That changed which of several
  equally valid answers was graded in a few transport and social media cases. It is fixed for the next pass; the
  numbers on this page were graded before the fix.
- **Runs vary.** The same case can come out differently with another seed. A single-seed pass is a sample, not a
  measurement; `--repeats 3` shows the spread.
- **Two weeks, one question.** The test checks the first reaction and whether there was a backdown. It doesn't
  check the long run, or anything else a report says.

## Cost of a pass

`python3 bench/run.py --estimate` prints the calls and tokens a pass needs, using the engine's own call estimate and
the average prompt and reply sizes of real runs, including the stakes plan and fact checks of the current crowd rules.
For the 43 cases, one seed each, with the zero-shot baseline: about 1,460 model calls, 7.1M input and 1.3M output
tokens, roughly **$13** at API list prices (Haiku 5.5 $0.10/$0.50, Sonnet 5.5 $2/$10 per million tokens). With
`--repeats 3` it is about 4,210 calls and $37. The fourth pass really cost about $66, so read the estimate as a floor. On a Claude Code subscription
nothing is billed per token; the pass counts against the plan's usage limits instead. Most of the money is the
Sonnet report and persona calls; most of the calls are Haiku agent rounds.

The 11 November pass ran on a Claude Code plan, so nothing was billed per token. It made 547 calls: 487 for the
simulations, 40 for the judge and 20 for zero-shot. Priced at API list rates it would have cost about **$10**,
nearly twice the estimate. The judge alone used 0.29M input and 0.09M output tokens ($1.87), and zero-shot $0.68.
The simulations used about 0.9M output tokens, mostly Sonnet ($6.00). Their input was about 2.7M tokens ($1.20), an
estimate from prompt size, since the CLI logs output tokens only. The estimator undercounts Sonnet output. The pass
took about four hours and fifteen minutes, close to 13 minutes a case including grading.

The 12 November pass, counted the same way, made 550 calls: 491 for the simulations, 39 for the judge (none for the
failed UT Austin report) and 20 for zero-shot. It would have cost about **$10.40**: judge $1.75, zero-shot $0.96,
and simulations about 0.98M output tokens ($6.70) and an estimated 2.5M input tokens ($1.00). It took about four
hours and fifty-five minutes, close to 15 minutes a case.

The 13 November passes, counted the same way, ran with three seeds and cost three to six times as much:

- **Third pass** (20 cases): 2,075 calls, 1,975 of them for the simulations, 80 for the judge and 20 for zero-shot.
  About **$31**: simulations $25.80 (3.1M output tokens and an estimated 7.9M input), judge $4.10, zero-shot $0.93.
- **Balanced cases** (23 cases, version 2): 2,331 calls, 2,216 of them for the simulations (including the attempts
  that failed when sign-in was refused and the five version 3 runs), 92 for the judge and 23 for zero-shot. About
  **$34**: simulations $28.70 (3.4M output, an estimated 8.8M input), judge $4.60, zero-shot $0.80.
- **Fourth pass** (43 cases, version 3): 4,411 calls, 4,198 of them for the simulations, 171 for the judge and 42
  for zero-shot. About **$66**: simulations $56 (6.8M output, an estimated 17.2M input), judge $8.60, zero-shot
  $1.70.

These ran on a Claude Code plan too, so none of it was billed per token. They ran in four to six parallel parts, so
their wall-clock time says little about a single run.

## Rerun it

```bash
cd engine-rs && cargo build --release -p tellfold-cli && cd ..
python3 bench/run.py --validate
python3 bench/run.py --estimate --zero-shot
python3 bench/run.py --zero-shot
```

See [`bench/README.md`](../bench/README.md) for the options and for adding a case.
