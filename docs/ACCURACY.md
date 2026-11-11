# How accurate is Tellfold?

**We don't know yet.** This page describes the test that will tell us, and it will hold the results, including the
cases Tellfold gets wrong. Until a real pass has run, Tellfold makes no accuracy claim of any kind.

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

20 cases: 18 announced after the cutoff and 2 controls. Reactions: 15 negative, 2 mixed, 3 positive. Backdowns: 4 full, 3 partial, 13 none.

The set leans negative: well-sourced positive reactions were hard to find, because quiet approval is rarely reported. Always answering "negative" would get 75% of directions right, so that is the bar to beat, not 33%. Some outcomes may still move: the most recent cases were only a few weeks old when written, and each case's `notes` field says where the facts are thin or the dates uncertain.

| case | type | announced | reaction | backdown | |
|---|---|---|---|---|---|
| [HubSpot terms let enrichment data be shared across customers by default](../bench/cases/hubspot-contact-data-sharing-terms-2026/) | policy | 2026-07-01 | negative | full |  |
| [Sony ends physical disc production for new PlayStation games from 2028](../bench/cases/sony-playstation-ends-physical-discs-2026/) | product-change | 2026-07-01 | negative | none |  |
| [Hillsborough County schools restrict leggings and spandex in 2026-27 dress code](../bench/cases/hillsborough-leggings-dress-code-2026/) | school | 2026-07-14 | negative | full |  |
| [Garmin launches CIRQA band with live workout heart rate behind Connect+](../bench/cases/garmin-cirqa-live-heart-rate-paywall-2026/) | product-change | 2026-07-21 | negative | partial |  |
| [Barnet Council cuts Mays Lane paid parking to one weekday hour after trader complaints](../bench/cases/barnet-mays-lane-parking-2026/) | local-government | 2026-07-23 | positive | none |  |
| [MBTA launches unlimited-transfer, express-bus and car-free-senior fare pilots](../bench/cases/mbta-fare-pilots-2026/) | public-service | 2026-07-28 | positive | none |  |
| [Instagram refreshes its wordmark for the first time in a decade](../bench/cases/instagram-wordmark-refresh-2026/) | rebrand | 2026-08-13 | mixed | none |  |
| [Barclays requires UK staff in the office three days a week from October 2026](../bench/cases/barclays-three-day-office-2026/) | workplace | 2026-08-17 | negative | partial |  |
| [Iron County School District (Utah) votes 4-3 to adopt a four-day school week from 2027-28](../bench/cases/iron-county-four-day-week-2026/) | school | 2026-08-25 | mixed | none |  |
| [CD Projekt Red makes The Witcher 3 Remastered a free upgrade](../bench/cases/witcher-3-free-remaster-upgrade-2026/) | product-change | 2026-08-25 | positive | none |  |
| [Ireland's National Transport Authority raises adult bus and rail fares 15% on average from January 2027](../bench/cases/ireland-nta-fare-increase-2026/) | public-service | 2026-09-03 | negative | none |  |
| [Olathe Public Schools recommends closing five elementary schools and one middle school](../bench/cases/olathe-school-closures-2026/) | school | 2026-09-03 | negative | none |  |
| [Xbox caps Game Pass cloud gaming at 5-15 hours a month](../bench/cases/xbox-cloud-gaming-monthly-caps-2026/) | product-change | 2026-09-03 | negative | none |  |
| [UT Austin stops new enrolment in African diaspora, Mexican American/Latino and women's and gender studies majors](../bench/cases/ut-austin-ethnic-gender-majors-2026/) | school | 2026-09-16 | negative | none |  |
| [OpenAI halves the ChatGPT Pro 200 usage allowance and adds Pro 500](../bench/cases/openai-chatgpt-pro-200-usage-cut-2026/) | pricing | 2026-09-29 | negative | full |  |
| [Meta's new Commercial Terms let it crawl business-linked websites](../bench/cases/meta-commercial-terms-website-crawling-2026/) | policy | 2026-10-15 | negative | full |  |
| [NatWest ends remote-first working and requires two office days a week from February 2027](../bench/cases/natwest-office-attendance-2026/) | workplace | 2026-10-20 | negative | none |  |
| [Apple Music drops its red icon for a black-and-white identity](../bench/cases/apple-music-black-and-white-rebrand-2026/) | rebrand | 2026-10-21 | negative | none |  |
| [Dublin City Council extends its ban on street bin bags to residential Dublin 7](../bench/cases/dublin-7-bin-bag-ban-2026/) | local-government | 2026-05-07 | negative | partial | control: known to model |
| [Nintendo raises the Switch 2 price to $499.99](../bench/cases/nintendo-switch-2-price-rise-2026/) | pricing | 2026-05-08 | negative | none | control: known to model |

## Results

No real pass has run yet. The table will be filled from `bench/results/<date>-<label>.json`, every case listed,
failures included.

| | cases | direction | objection recall | backdown | groups named | direction from sentiment |
|---|---|---|---|---|---|---|
| Tellfold, after cutoff | | | | | | |
| Tellfold, known to model | | | | | | |
| No crowd (zero-shot) | | | | | | – |
| Always "negative" / "no backdown" | | | – | | – | – |

### Where it went wrong

To be written from the per-case results: every miss, and what the report said instead.

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
  the set leans towards negative reactions however hard we tried to balance it. The "always negative" baseline makes
  that lean visible.
- **The crowd is small.** Twenty simulated people over six rounds is a town meeting, not the internet. Tellfold can't
  model press coverage, virality, organised campaigns or the decision-makers' own politics, which often decide
  whether an organisation backs down.
- **The judge is an LLM.** Grading objection recall needs judgement, so a model does it. It can be lenient, strict or
  inconsistent. Its prompt is public, it sees only the report, the seed's headline and the rubric's objections and
  groups (never the true direction or backdown), and every verdict comes with a quote
  from the report so it can be checked by hand. We will spot-check a sample by hand and report the agreement.
- **Few cases.** With 20 cases, one case moves a percentage by five points. Differences of a few points
  between two passes mean nothing.
- **"Direction" is a judgement too.** Reporting rarely says "62% of people were against". The rubric's direction is
  our reading of the coverage, and mixed reactions are the hardest to call.
- **Runs vary.** The same case can come out differently with another seed. A single-seed pass is a sample, not a
  measurement; `--repeats 3` shows the spread.
- **Two weeks, one question.** The test checks the first reaction and whether there was a backdown. It doesn't
  check the long run, or anything else a report says.

## Cost of a pass

`python3 bench/run.py --estimate` prints the calls and tokens a pass needs, using the engine's own call estimate and
the average prompt and reply sizes of real runs. For the 20 cases, one seed each, with the zero-shot baseline: about
540 model calls, 3.0M input and 0.6M output tokens, roughly **$5** at API list prices (Haiku 5.5 $0.10/$0.50, Sonnet
5.5 $2/$10 per million tokens). With `--repeats 3` it is about 1,540 calls and $15. On a Claude Code subscription
nothing is billed per token; the pass counts against the plan's usage limits instead. Most of the money is the
Sonnet report and persona calls; most of the calls are Haiku agent rounds. A case takes about eight minutes, so one
seed of every case is close to three hours.

## Rerun it

```bash
cd engine-rs && cargo build --release -p tellfold-cli && cd ..
python3 bench/run.py --validate
python3 bench/run.py --estimate --zero-shot
python3 bench/run.py --zero-shot
```

See [`bench/README.md`](../bench/README.md) for the options and for adding a case.
