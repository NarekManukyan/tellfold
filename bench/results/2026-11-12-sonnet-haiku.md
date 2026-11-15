# Tellfold accuracy: sonnet-haiku

Date 2026-11-12 · commit `a963978` · 20 cases × 1 run(s) · agents haiku, planner sonnet, judge sonnet

| | graded | direction | objection recall | backdown | groups named | direction from sentiment |
|---|---|---|---|---|---|---|
| all cases | 19 | 37% | 41% | 68% | 36% | 37% |
| after the model's cutoff | 17 | 29% | 43% | 65% | 36% | 35% |
| known to the model | 2 | 100% | 19% | 100% | 33% | 50% |
| zero-shot baseline | 20 | 55% | 68% | 55% | 69% | – |

Ignoring the announcement: always "negative" gets the direction right 75% of the time; always "no backdown" gets the backdown right 65%.

**Failures:** 1 simulation run(s) and 0 zero-shot answer(s) failed; the case table shows each error. Failed entries are left out of the averages.

## Cases

| case | actual | predicted | objections | backdown (actual / predicted) | zero-shot | error |
|---|---|---|---|---|---|---|
| apple-music-black-and-white-rebrand-2026 (seed 7) | negative | mixed ✗ | 50% | none / none | mixed ✗, 88%, none |  |
| barclays-three-day-office-2026 (seed 7) | negative | negative ✓ | 60% | partial / partial | negative ✓, 90%, partial |  |
| barnet-mays-lane-parking-2026 (seed 7) | positive | negative ✗ | 50% | none / none | positive ✓, 83%, none |  |
| dublin-7-bin-bag-ban-2026 (seed 7) | negative | negative ✓ | 38% | partial / partial | mixed ✗, 75%, partial |  |
| garmin-cirqa-live-heart-rate-paywall-2026 (seed 7) | negative | mixed ✗ | 83% | partial / partial | mixed ✗, 67%, none |  |
| hillsborough-leggings-dress-code-2026 (seed 7) | negative | mixed ✗ | 38% | full / none | mixed ✗, 38%, none |  |
| hubspot-contact-data-sharing-terms-2026 (seed 7) | negative | mixed ✗ | 38% | full / partial | mixed ✗, 62%, partial |  |
| instagram-wordmark-refresh-2026 (seed 7) | mixed | mixed ✓ | 12% | none / none | mixed ✓, 50%, none |  |
| ireland-nta-fare-increase-2026 (seed 7) | negative | mixed ✗ | 25% | none / none | negative ✓, 62%, partial |  |
| iron-county-four-day-week-2026 (seed 7) | mixed | negative ✗ | 62% | none / partial | mixed ✓, 88%, partial |  |
| mbta-fare-pilots-2026 (seed 7) | positive | mixed ✗ | 67% | none / none | positive ✓, 83%, none |  |
| meta-commercial-terms-website-crawling-2026 (seed 7) | negative | mixed ✗ | 40% | full / none | mixed ✗, 90%, none |  |
| natwest-office-attendance-2026 (seed 7) | negative | negative ✓ | 0% | none / partial | negative ✓, 88%, partial |  |
| nintendo-switch-2-price-rise-2026 (seed 7) | negative | negative ✓ | 0% | none / none | negative ✓, 17%, none |  |
| olathe-school-closures-2026 (seed 7) | negative | mixed ✗ | 50% | none / none | mixed ✗, 40%, partial |  |
| openai-chatgpt-pro-200-usage-cut-2026 (seed 7) | negative | mixed ✗ | 25% | full / none | mixed ✗, 88%, partial |  |
| sony-playstation-ends-physical-discs-2026 (seed 7) | negative | negative ✓ | 75% | none / partial | mixed ✗, 75%, none |  |
| ut-austin-ethnic-gender-majors-2026 (seed 7) | negative | – | – | none / – | negative ✓, 70%, partial | tellfold exited 1 with 1 run folder(s):        │       │       │
│     6 │       |
| witcher-3-free-remaster-upgrade-2026 (seed 7) | positive | positive ✓ | 50% | none / none | positive ✓, 50%, none |  |
| xbox-cloud-gaming-monthly-caps-2026 (seed 7) | negative | mixed ✗ | 12% | none / none | negative ✓, 62%, partial |  |

Usage: {"simulation_calls": 491, "judge": {"calls": 39, "input_tokens": 270571, "output_tokens": 84794, "cost_usd": 1.7451442000000001}, "zero_shot": {"calls": 20, "input_tokens": 110759, "output_tokens": 65070, "cost_usd": 0.9558847999999998}}
