# Tellfold accuracy: sonnet-haiku

Date 2026-11-13 · commit `5ba314c` · 23 cases × 3 run(s) · agents haiku, planner sonnet, judge sonnet

| | graded | direction | objection recall | backdown | groups named | direction from sentiment |
|---|---|---|---|---|---|---|
| all cases | 64 | 25% | 54% | 77% | 61% | 28% |
| after the model's cutoff | 64 | 25% | 54% | 77% | 61% | 28% |
| known to the model | 0 | – | – | – | – | – |
| zero-shot baseline | 23 | 74% | 62% | 70% | 72% | – |
| planner's stance mix alone | 64 | 62% | – | – | – | – |

The crowd's final direction moved away from the planner's mix in 28 of 64 runs: 0 to the right answer, 24 away from it, 4 from one wrong answer to another. Tellfold agrees with the zero-shot answer on direction in 39% of runs; the planner's mix alone agrees with it in 73%.

Ignoring the announcement: always "negative" gets the direction right 26% of the time; always "no backdown" gets the backdown right 87%.

## Cases

| case | actual | predicted | planner's mix | objections | backdown (actual / predicted) | zero-shot | error |
|---|---|---|---|---|---|---|---|
| anthropic-claude-code-weekly-limits-2026 (seed 7) | negative | mixed ✗ | negative | 62% | none / none | negative ✓, 62%, partial |  |
| anthropic-claude-code-weekly-limits-2026 (seed 8) | negative | negative ✓ | negative | 62% | none / none | negative ✓, 62%, partial |  |
| anthropic-claude-code-weekly-limits-2026 (seed 9) | negative | negative ✓ | negative | 62% | none / partial | negative ✓, 62%, partial |  |
| apple-iphone-duo-and-price-rises-2026 (seed 7) | mixed | negative ✗ | negative | 62% | none / none | mixed ✓, 88%, none |  |
| apple-iphone-duo-and-price-rises-2026 (seed 8) | mixed | negative ✗ | negative | 50% | none / none | mixed ✓, 88%, none |  |
| apple-iphone-duo-and-price-rises-2026 (seed 9) | mixed | negative ✗ | negative | 50% | none / none | mixed ✓, 88%, none |  |
| blood-of-dawnwalker-40fps-console-cap-2026 (seed 7) | negative | negative ✓ | negative | 0% | full / none | negative ✓, 50%, none |  |
| blood-of-dawnwalker-40fps-console-cap-2026 (seed 8) | negative | mixed ✗ | negative | 67% | full / none | negative ✓, 50%, none |  |
| blood-of-dawnwalker-40fps-console-cap-2026 (seed 9) | negative | mixed ✗ | negative | 67% | full / none | negative ✓, 50%, none |  |
| bungie-destiny-2-unvaulting-2026 (seed 7) | positive | mixed ✗ | mixed | 75% | none / none | mixed ✗, 75%, none |  |
| bungie-destiny-2-unvaulting-2026 (seed 8) | positive | mixed ✗ | mixed | 75% | none / none | mixed ✗, 75%, none |  |
| bungie-destiny-2-unvaulting-2026 (seed 9) | positive | mixed ✗ | mixed | 62% | none / none | mixed ✗, 75%, none |  |
| costco-churro-return-2026 (seed 7) | mixed | mixed ✓ | mixed | 0% | none / none | positive ✗, 38%, none |  |
| costco-churro-return-2026 (seed 8) | mixed | positive ✗ | positive | 50% | none / none | positive ✗, 38%, none |  |
| costco-churro-return-2026 (seed 9) | mixed | positive ✗ | positive | 25% | none / none | positive ✗, 38%, none |  |
| delta-medallion-thresholds-unchanged-2026 (seed 7) | positive | mixed ✗ | positive | 67% | none / none | positive ✓, 83%, none |  |
| google-pixel-11-launch-price-rise-2026 (seed 7) | mixed | negative ✗ | negative | 62% | none / none | mixed ✓, 50%, none |  |
| google-pixel-11-launch-price-rise-2026 (seed 8) | mixed | negative ✗ | negative | 0% | none / none | mixed ✓, 50%, none |  |
| google-pixel-11-launch-price-rise-2026 (seed 9) | mixed | negative ✗ | negative | 62% | none / none | mixed ✓, 50%, none |  |
| grossmont-librarians-reinstated-2026 (seed 7) | positive | negative ✗ | mixed | 75% | none / none | mixed ✗, 100%, none |  |
| grossmont-librarians-reinstated-2026 (seed 8) | positive | mixed ✗ | mixed | 88% | none / none | mixed ✗, 100%, none |  |
| grossmont-librarians-reinstated-2026 (seed 9) | positive | mixed ✗ | mixed | 88% | none / none | mixed ✗, 100%, none |  |
| ireland-e-scooter-under-18-ban-2026 (seed 7) | mixed | positive ✗ | mixed | 50% | none / none | mixed ✓, 50%, none |  |
| ireland-e-scooter-under-18-ban-2026 (seed 8) | mixed | negative ✗ | mixed | 40% | none / none | mixed ✓, 50%, none |  |
| ireland-e-scooter-under-18-ban-2026 (seed 9) | mixed | mixed ✓ | mixed | 30% | none / partial | mixed ✓, 50%, none |  |
| ireland-home-heating-carbon-tax-cut-2026 (seed 7) | mixed | negative ✗ | mixed | 25% | none / none | mixed ✓, 62%, none |  |
| ireland-home-heating-carbon-tax-cut-2026 (seed 8) | mixed | negative ✗ | mixed | 62% | none / none | mixed ✓, 62%, none |  |
| ireland-home-heating-carbon-tax-cut-2026 (seed 9) | mixed | negative ✗ | mixed | 75% | none / none | mixed ✓, 62%, none |  |
| jetstar-overhead-locker-fee-2026 (seed 7) | negative | negative ✓ | negative | 70% | none / none | negative ✓, 80%, partial |  |
| jetstar-overhead-locker-fee-2026 (seed 8) | negative | negative ✓ | negative | 70% | none / none | negative ✓, 80%, partial |  |
| jetstar-overhead-locker-fee-2026 (seed 9) | negative | negative ✓ | negative | 70% | none / none | negative ✓, 80%, partial |  |
| mclaren-heritage-wordmark-2026 (seed 7) | mixed | positive ✗ | mixed | 25% | none / none | mixed ✓, 38%, none |  |
| minneapolis-school-closures-pause-2026 (seed 7) | positive | mixed ✗ | positive | 62% | none / none | positive ✓, 38%, none |  |
| minneapolis-school-closures-pause-2026 (seed 8) | positive | mixed ✗ | positive | 75% | none / none | positive ✓, 38%, none |  |
| minneapolis-school-closures-pause-2026 (seed 9) | positive | mixed ✗ | positive | 50% | none / none | positive ✓, 38%, none |  |
| quake-champions-goes-paid-2026 (seed 7) | mixed | negative ✗ | negative | 38% | none / none | mixed ✓, 12%, partial |  |
| quake-champions-goes-paid-2026 (seed 8) | mixed | negative ✗ | mixed | 38% | none / none | mixed ✓, 12%, partial |  |
| quake-champions-goes-paid-2026 (seed 9) | mixed | negative ✗ | mixed | 50% | none / none | mixed ✓, 12%, partial |  |
| reddit-ends-rss-and-public-api-2026 (seed 7) | negative | negative ✓ | negative | 75% | none / none | negative ✓, 100%, partial |  |
| reddit-ends-rss-and-public-api-2026 (seed 8) | negative | negative ✓ | negative | 75% | none / none | negative ✓, 100%, partial |  |
| reddit-ends-rss-and-public-api-2026 (seed 9) | negative | negative ✓ | negative | 88% | none / partial | negative ✓, 100%, partial |  |
| runescape-reignited-fresh-start-mode-2026 (seed 7) | negative | positive ✗ | mixed | 75% | full / none | mixed ✗, 62%, partial |  |
| runescape-reignited-fresh-start-mode-2026 (seed 8) | negative | positive ✗ | mixed | 50% | full / none | mixed ✗, 62%, partial |  |
| runescape-reignited-fresh-start-mode-2026 (seed 9) | negative | mixed ✗ | negative | 0% | full / none | mixed ✗, 62%, partial |  |
| samsung-galaxy-z-fold-8-launch-2026 (seed 7) | positive | negative ✗ | negative | 38% | none / none | mixed ✗, 75%, none |  |
| samsung-galaxy-z-fold-8-launch-2026 (seed 8) | positive | negative ✗ | negative | 38% | none / none | mixed ✗, 75%, none |  |
| samsung-galaxy-z-fold-8-launch-2026 (seed 9) | positive | mixed ✗ | negative | 38% | none / none | mixed ✗, 75%, none |  |
| sfusd-neighborhood-school-assignment-2026 (seed 7) | mixed | negative ✗ | negative | 50% | none / partial | mixed ✓, 70%, none |  |
| sfusd-neighborhood-school-assignment-2026 (seed 8) | mixed | negative ✗ | negative | 50% | none / partial | mixed ✓, 70%, none |  |
| uk-electricity-vat-cut-2026 (seed 7) | positive | mixed ✗ | mixed | 50% | none / none | mixed ✗, 62%, none |  |
| uk-electricity-vat-cut-2026 (seed 8) | positive | negative ✗ | positive | 38% | none / none | mixed ✗, 62%, none |  |
| uk-electricity-vat-cut-2026 (seed 9) | positive | negative ✗ | positive | 50% | none / none | mixed ✗, 62%, none |  |
| uk-two-pound-bus-fare-cap-2026 (seed 7) | positive | mixed ✗ | positive | 75% | none / none | positive ✓, 50%, none |  |
| uk-two-pound-bus-fare-cap-2026 (seed 8) | positive | mixed ✗ | positive | 100% | none / none | positive ✓, 50%, none |  |
| uk-two-pound-bus-fare-cap-2026 (seed 9) | positive | mixed ✗ | positive | 38% | none / none | positive ✓, 50%, none |  |
| umich-first-semester-pass-fail-2026 (seed 7) | mixed | mixed ✓ | mixed | 20% | partial / none | mixed ✓, 50%, none |  |
| umich-first-semester-pass-fail-2026 (seed 8) | mixed | mixed ✓ | mixed | 40% | partial / none | mixed ✓, 50%, none |  |
| umich-first-semester-pass-fail-2026 (seed 9) | mixed | negative ✗ | mixed | 50% | partial / none | mixed ✓, 50%, none |  |
| united-mileageplus-spend-only-status-2026 (seed 7) | negative | negative ✓ | negative | 70% | none / none | negative ✓, 60%, none |  |
| united-mileageplus-spend-only-status-2026 (seed 8) | negative | negative ✓ | negative | 80% | none / none | negative ✓, 60%, none |  |
| united-mileageplus-spend-only-status-2026 (seed 9) | negative | negative ✓ | negative | 70% | none / none | negative ✓, 60%, none |  |
| wow-forever-skyborne-paid-race-2026 (seed 7) | mixed | negative ✗ | mixed | 40% | none / partial | mixed ✓, 60%, partial |  |
| wow-forever-skyborne-paid-race-2026 (seed 8) | mixed | negative ✗ | mixed | 40% | none / none | mixed ✓, 60%, partial |  |
| wow-forever-skyborne-paid-race-2026 (seed 9) | mixed | negative ✗ | negative | 50% | none / none | mixed ✓, 60%, partial |  |

Usage: {"simulation_calls": 2216, "judge": {"calls": 23, "input_tokens": 162271, "output_tokens": 45246, "cost_usd": 1.0214316}, "zero_shot": {"calls": 7, "input_tokens": 23005, "output_tokens": 15309, "cost_usd": 0.235103}}
