# Tellfold accuracy: sonnet-haiku

Date 2026-11-11 · commit `a92638f` · 20 cases × 1 run(s) · agents haiku, planner sonnet, judge sonnet

| | graded | direction | objection recall | backdown | groups named | direction from sentiment |
|---|---|---|---|---|---|---|
| all cases | 20 | 30% | 35% | 80% | 47% | 30% |
| after the model's cutoff | 18 | 33% | 34% | 78% | 49% | 33% |
| known to the model | 2 | 0% | 40% | 100% | 33% | 0% |
| zero-shot baseline | 20 | 70% | 66% | 65% | 70% | – |

Ignoring the announcement: always "negative" gets the direction right 75% of the time; always "no backdown" gets the backdown right 65%.

## Cases

| case | actual | predicted | objections | backdown (actual / predicted) | zero-shot | error |
|---|---|---|---|---|---|---|
| apple-music-black-and-white-rebrand-2026 (seed 7) | negative | positive ✗ | 25% | none / none | mixed ✗, 100%, none |  |
| barclays-three-day-office-2026 (seed 7) | negative | negative ✓ | 50% | partial / partial | negative ✓, 80%, partial |  |
| barnet-mays-lane-parking-2026 (seed 7) | positive | mixed ✗ | 67% | none / none | positive ✓, 67%, partial |  |
| dublin-7-bin-bag-ban-2026 (seed 7) | negative | mixed ✗ | 62% | partial / partial | negative ✓, 75%, partial |  |
| garmin-cirqa-live-heart-rate-paywall-2026 (seed 7) | negative | mixed ✗ | 67% | partial / none | positive ✗, 67%, none |  |
| hillsborough-leggings-dress-code-2026 (seed 7) | negative | mixed ✗ | 12% | full / partial | mixed ✗, 50%, none |  |
| hubspot-contact-data-sharing-terms-2026 (seed 7) | negative | mixed ✗ | 38% | full / partial | mixed ✗, 62%, partial |  |
| instagram-wordmark-refresh-2026 (seed 7) | mixed | mixed ✓ | 12% | none / none | mixed ✓, 75%, none |  |
| ireland-nta-fare-increase-2026 (seed 7) | negative | mixed ✗ | 25% | none / none | negative ✓, 12%, none |  |
| iron-county-four-day-week-2026 (seed 7) | mixed | mixed ✓ | 38% | none / none | mixed ✓, 75%, none |  |
| mbta-fare-pilots-2026 (seed 7) | positive | mixed ✗ | 17% | none / none | positive ✓, 67%, none |  |
| meta-commercial-terms-website-crawling-2026 (seed 7) | negative | negative ✓ | 40% | full / partial | negative ✓, 70%, none |  |
| natwest-office-attendance-2026 (seed 7) | negative | negative ✓ | 0% | none / none | negative ✓, 88%, partial |  |
| nintendo-switch-2-price-rise-2026 (seed 7) | negative | mixed ✗ | 17% | none / none | negative ✓, 33%, none |  |
| olathe-school-closures-2026 (seed 7) | negative | mixed ✗ | 20% | none / none | negative ✓, 70%, partial |  |
| openai-chatgpt-pro-200-usage-cut-2026 (seed 7) | negative | negative ✓ | 62% | full / partial | mixed ✗, 62%, partial |  |
| sony-playstation-ends-physical-discs-2026 (seed 7) | negative | mixed ✗ | 62% | none / partial | mixed ✗, 88%, none |  |
| ut-austin-ethnic-gender-majors-2026 (seed 7) | negative | mixed ✗ | 30% | none / partial | negative ✓, 80%, partial |  |
| witcher-3-free-remaster-upgrade-2026 (seed 7) | positive | mixed ✗ | 12% | none / none | positive ✓, 50%, none |  |
| xbox-cloud-gaming-monthly-caps-2026 (seed 7) | negative | mixed ✗ | 38% | none / partial | negative ✓, 50%, none |  |

Usage: {"simulation_calls": 487, "judge": {"calls": 40, "input_tokens": 290639, "output_tokens": 90198, "cost_usd": 1.8651372}, "zero_shot": {"calls": 20, "input_tokens": 64988, "output_tokens": 44492, "cost_usd": 0.6761014000000002}}
