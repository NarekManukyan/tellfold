# Tellfold accuracy: sonnet-haiku

Date 2026-11-13 · commit `62357bc` · 43 cases × 3 run(s) · agents haiku, planner sonnet, judge sonnet

| | graded | direction | objection recall | backdown | groups named | direction from sentiment |
|---|---|---|---|---|---|---|
| all cases | 129 | 55% | 52% | 70% | 56% | 49% |
| after the model's cutoff | 123 | 55% | 52% | 70% | 57% | 50% |
| known to the model | 6 | 67% | 38% | 83% | 33% | 20% |
| zero-shot baseline | 42 | 62% | 67% | 64% | 76% | – |
| planner's stance mix alone | 129 | 66% | – | – | – | – |

The crowd's final direction moved away from the planner's mix in 51 of 129 runs: 16 to the right answer, 30 away from it, 5 from one wrong answer to another. Tellfold agrees with the zero-shot answer on direction in 50% of runs; the planner's mix alone agrees with it in 63%.

Ignoring the announcement: always "negative" gets the direction right 49% of the time; always "no backdown" gets the backdown right 77%.

**Failures:** 0 simulation run(s) and 1 zero-shot answer(s) failed; the case table shows each error. Failed entries are left out of the averages.

## Cases

| case | actual | predicted | planner's mix | objections | backdown (actual / predicted) | zero-shot | error |
|---|---|---|---|---|---|---|---|
| anthropic-claude-code-weekly-limits-2026 (seed 7) | negative | mixed ✗ | negative | 50% | none / none | negative ✓, 62%, partial |  |
| anthropic-claude-code-weekly-limits-2026 (seed 8) | negative | mixed ✗ | negative | 62% | none / none | negative ✓, 62%, partial |  |
| anthropic-claude-code-weekly-limits-2026 (seed 9) | negative | negative ✓ | negative | 62% | none / none | negative ✓, 62%, partial |  |
| apple-iphone-duo-and-price-rises-2026 (seed 7) | mixed | negative ✗ | negative | 38% | none / none | mixed ✓, 62%, none |  |
| apple-iphone-duo-and-price-rises-2026 (seed 8) | mixed | negative ✗ | negative | 62% | none / none | mixed ✓, 62%, none |  |
| apple-iphone-duo-and-price-rises-2026 (seed 9) | mixed | negative ✗ | negative | 88% | none / none | mixed ✓, 62%, none |  |
| apple-music-black-and-white-rebrand-2026 (seed 7) | negative | mixed ✗ | negative | 75% | none / none | mixed ✗, 100%, none |  |
| apple-music-black-and-white-rebrand-2026 (seed 8) | negative | mixed ✗ | negative | 62% | none / none | mixed ✗, 100%, none |  |
| apple-music-black-and-white-rebrand-2026 (seed 9) | negative | mixed ✗ | negative | 75% | none / none | mixed ✗, 100%, none |  |
| barclays-three-day-office-2026 (seed 7) | negative | negative ✓ | negative | 70% | partial / none | mixed ✗, 50%, none |  |
| barclays-three-day-office-2026 (seed 8) | negative | negative ✓ | negative | 70% | partial / none | mixed ✗, 50%, none |  |
| barclays-three-day-office-2026 (seed 9) | negative | negative ✓ | negative | 70% | partial / partial | mixed ✗, 50%, none |  |
| barnet-mays-lane-parking-2026 (seed 7) | positive | mixed ✗ | mixed | 83% | none / none | positive ✓, 67%, none |  |
| barnet-mays-lane-parking-2026 (seed 8) | positive | positive ✓ | positive | 50% | none / none | positive ✓, 67%, none |  |
| barnet-mays-lane-parking-2026 (seed 9) | positive | mixed ✗ | positive | 50% | none / none | positive ✓, 67%, none |  |
| blood-of-dawnwalker-40fps-console-cap-2026 (seed 7) | negative | negative ✓ | negative | 50% | full / none | negative ✓, 67%, none |  |
| blood-of-dawnwalker-40fps-console-cap-2026 (seed 8) | negative | negative ✓ | negative | 83% | full / none | negative ✓, 67%, none |  |
| blood-of-dawnwalker-40fps-console-cap-2026 (seed 9) | negative | negative ✓ | negative | 50% | full / none | negative ✓, 67%, none |  |
| bungie-destiny-2-unvaulting-2026 (seed 7) | positive | mixed ✗ | mixed | 75% | none / none | mixed ✗, 75%, none |  |
| bungie-destiny-2-unvaulting-2026 (seed 8) | positive | mixed ✗ | negative | 75% | none / none | mixed ✗, 75%, none |  |
| bungie-destiny-2-unvaulting-2026 (seed 9) | positive | mixed ✗ | mixed | 38% | none / none | mixed ✗, 75%, none |  |
| costco-churro-return-2026 (seed 7) | mixed | mixed ✓ | positive | 38% | none / none | positive ✗, 62%, none |  |
| costco-churro-return-2026 (seed 8) | mixed | mixed ✓ | positive | 25% | none / none | positive ✗, 62%, none |  |
| costco-churro-return-2026 (seed 9) | mixed | positive ✗ | positive | 38% | none / none | positive ✗, 62%, none |  |
| delta-medallion-thresholds-unchanged-2026 (seed 7) | positive | mixed ✗ | mixed | 67% | none / none | positive ✓, 83%, none |  |
| delta-medallion-thresholds-unchanged-2026 (seed 8) | positive | mixed ✗ | mixed | 50% | none / none | positive ✓, 83%, none |  |
| delta-medallion-thresholds-unchanged-2026 (seed 9) | positive | positive ✓ | positive | 33% | none / none | positive ✓, 83%, none |  |
| dublin-7-bin-bag-ban-2026 (seed 7) | negative | negative ✓ | negative | 50% | partial / partial | mixed ✗, 50%, partial |  |
| dublin-7-bin-bag-ban-2026 (seed 8) | negative | negative ✓ | negative | 75% | partial / none | mixed ✗, 50%, partial |  |
| dublin-7-bin-bag-ban-2026 (seed 9) | negative | mixed ✗ | negative | 50% | partial / partial | mixed ✗, 50%, partial |  |
| garmin-cirqa-live-heart-rate-paywall-2026 (seed 7) | negative | negative ✓ | mixed | 50% | partial / none | mixed ✗, 83%, none |  |
| garmin-cirqa-live-heart-rate-paywall-2026 (seed 8) | negative | negative ✓ | mixed | 83% | partial / none | mixed ✗, 83%, none |  |
| garmin-cirqa-live-heart-rate-paywall-2026 (seed 9) | negative | negative ✓ | mixed | 50% | partial / none | mixed ✗, 83%, none |  |
| google-pixel-11-launch-price-rise-2026 (seed 7) | mixed | negative ✗ | negative | 25% | none / none | mixed ✓, 50%, partial |  |
| google-pixel-11-launch-price-rise-2026 (seed 8) | mixed | negative ✗ | negative | 62% | none / none | mixed ✓, 50%, partial |  |
| google-pixel-11-launch-price-rise-2026 (seed 9) | mixed | negative ✗ | negative | 62% | none / none | mixed ✓, 50%, partial |  |
| grossmont-librarians-reinstated-2026 (seed 7) | positive | mixed ✗ | mixed | 75% | none / none | mixed ✗, 88%, none |  |
| grossmont-librarians-reinstated-2026 (seed 8) | positive | negative ✗ | mixed | 62% | none / none | mixed ✗, 88%, none |  |
| grossmont-librarians-reinstated-2026 (seed 9) | positive | mixed ✗ | mixed | 88% | none / none | mixed ✗, 88%, none |  |
| hillsborough-leggings-dress-code-2026 (seed 7) | negative | positive ✗ | positive | 0% | full / none | mixed ✗, 38%, partial |  |
| hillsborough-leggings-dress-code-2026 (seed 8) | negative | negative ✓ | mixed | 25% | full / none | mixed ✗, 38%, partial |  |
| hillsborough-leggings-dress-code-2026 (seed 9) | negative | mixed ✗ | mixed | 50% | full / none | mixed ✗, 38%, partial |  |
| hubspot-contact-data-sharing-terms-2026 (seed 7) | negative | mixed ✗ | negative | 38% | full / none | mixed ✗, 62%, none |  |
| hubspot-contact-data-sharing-terms-2026 (seed 8) | negative | negative ✓ | negative | 38% | full / none | mixed ✗, 62%, none |  |
| hubspot-contact-data-sharing-terms-2026 (seed 9) | negative | negative ✓ | negative | 62% | full / none | mixed ✗, 62%, none |  |
| instagram-wordmark-refresh-2026 (seed 7) | mixed | mixed ✓ | mixed | 25% | none / none | mixed ✓, 62%, none |  |
| instagram-wordmark-refresh-2026 (seed 8) | mixed | mixed ✓ | mixed | 38% | none / none | mixed ✓, 62%, none |  |
| instagram-wordmark-refresh-2026 (seed 9) | mixed | mixed ✓ | mixed | 12% | none / none | mixed ✓, 62%, none |  |
| ireland-e-scooter-under-18-ban-2026 (seed 7) | mixed | negative ✗ | mixed | 50% | none / none | positive ✗, 60%, none |  |
| ireland-e-scooter-under-18-ban-2026 (seed 8) | mixed | mixed ✓ | mixed | 50% | none / none | positive ✗, 60%, none |  |
| ireland-e-scooter-under-18-ban-2026 (seed 9) | mixed | negative ✗ | mixed | 40% | none / none | positive ✗, 60%, none |  |
| ireland-home-heating-carbon-tax-cut-2026 (seed 7) | mixed | negative ✗ | mixed | 62% | none / none | mixed ✓, 75%, none |  |
| ireland-home-heating-carbon-tax-cut-2026 (seed 8) | mixed | mixed ✓ | positive | 62% | none / none | mixed ✓, 75%, none |  |
| ireland-home-heating-carbon-tax-cut-2026 (seed 9) | mixed | negative ✗ | positive | 75% | none / none | mixed ✓, 75%, none |  |
| ireland-nta-fare-increase-2026 (seed 7) | negative | negative ✓ | negative | 25% | none / partial | negative ✓, 38%, partial |  |
| ireland-nta-fare-increase-2026 (seed 8) | negative | negative ✓ | negative | 12% | none / none | negative ✓, 38%, partial |  |
| ireland-nta-fare-increase-2026 (seed 9) | negative | negative ✓ | negative | 25% | none / none | negative ✓, 38%, partial |  |
| iron-county-four-day-week-2026 (seed 7) | mixed | mixed ✓ | positive | 75% | none / partial | positive ✗, 75%, none |  |
| iron-county-four-day-week-2026 (seed 8) | mixed | mixed ✓ | positive | 75% | none / none | positive ✗, 75%, none |  |
| iron-county-four-day-week-2026 (seed 9) | mixed | negative ✗ | mixed | 12% | none / partial | positive ✗, 75%, none |  |
| jetstar-overhead-locker-fee-2026 (seed 7) | negative | negative ✓ | negative | 60% | none / none | negative ✓, 80%, none |  |
| jetstar-overhead-locker-fee-2026 (seed 8) | negative | mixed ✗ | negative | 70% | none / none | negative ✓, 80%, none |  |
| jetstar-overhead-locker-fee-2026 (seed 9) | negative | negative ✓ | negative | 70% | none / none | negative ✓, 80%, none |  |
| mbta-fare-pilots-2026 (seed 7) | positive | mixed ✗ | positive | 50% | none / partial | positive ✓, 83%, none |  |
| mbta-fare-pilots-2026 (seed 8) | positive | mixed ✗ | positive | 100% | none / partial | positive ✓, 83%, none |  |
| mbta-fare-pilots-2026 (seed 9) | positive | mixed ✗ | positive | 67% | none / none | positive ✓, 83%, none |  |
| mclaren-heritage-wordmark-2026 (seed 7) | mixed | positive ✗ | mixed | 0% | none / none | mixed ✓, 25%, none |  |
| mclaren-heritage-wordmark-2026 (seed 8) | mixed | mixed ✓ | mixed | 25% | none / none | mixed ✓, 25%, none |  |
| mclaren-heritage-wordmark-2026 (seed 9) | mixed | mixed ✓ | negative | 12% | none / none | mixed ✓, 25%, none |  |
| meta-commercial-terms-website-crawling-2026 (seed 7) | negative | negative ✓ | negative | 50% | full / none | mixed ✗, 80%, partial |  |
| meta-commercial-terms-website-crawling-2026 (seed 8) | negative | negative ✓ | mixed | 40% | full / none | mixed ✗, 80%, partial |  |
| meta-commercial-terms-website-crawling-2026 (seed 9) | negative | negative ✓ | mixed | 30% | full / none | mixed ✗, 80%, partial |  |
| minneapolis-school-closures-pause-2026 (seed 7) | positive | mixed ✗ | positive | 75% | none / none | mixed ✗, 50%, none |  |
| minneapolis-school-closures-pause-2026 (seed 8) | positive | mixed ✗ | positive | 62% | none / none | mixed ✗, 50%, none |  |
| minneapolis-school-closures-pause-2026 (seed 9) | positive | mixed ✗ | positive | 62% | none / none | mixed ✗, 50%, none |  |
| natwest-office-attendance-2026 (seed 7) | negative | negative ✓ | negative | 100% | none / partial | negative ✓, 88%, partial |  |
| natwest-office-attendance-2026 (seed 8) | negative | negative ✓ | negative | 88% | none / partial | negative ✓, 88%, partial |  |
| natwest-office-attendance-2026 (seed 9) | negative | negative ✓ | negative | 88% | none / none | negative ✓, 88%, partial |  |
| nintendo-switch-2-price-rise-2026 (seed 7) | negative | negative ✓ | negative | 17% | none / none | negative ✓, 33%, none |  |
| nintendo-switch-2-price-rise-2026 (seed 8) | negative | mixed ✗ | negative | 17% | none / none | negative ✓, 33%, none |  |
| nintendo-switch-2-price-rise-2026 (seed 9) | negative | negative ✓ | negative | 17% | none / none | negative ✓, 33%, none |  |
| olathe-school-closures-2026 (seed 7) | negative | negative ✓ | negative | 30% | none / partial | negative ✓, 50%, partial |  |
| olathe-school-closures-2026 (seed 8) | negative | negative ✓ | negative | 30% | none / none | negative ✓, 50%, partial |  |
| olathe-school-closures-2026 (seed 9) | negative | negative ✓ | negative | 60% | none / partial | negative ✓, 50%, partial |  |
| openai-chatgpt-pro-200-usage-cut-2026 (seed 7) | negative | negative ✓ | negative | 50% | full / none | negative ✓, 75%, partial |  |
| openai-chatgpt-pro-200-usage-cut-2026 (seed 8) | negative | negative ✓ | negative | 50% | full / none | negative ✓, 75%, partial |  |
| openai-chatgpt-pro-200-usage-cut-2026 (seed 9) | negative | negative ✓ | negative | 75% | full / partial | negative ✓, 75%, partial |  |
| quake-champions-goes-paid-2026 (seed 7) | mixed | mixed ✓ | mixed | 62% | none / none | mixed ✓, 25%, none |  |
| quake-champions-goes-paid-2026 (seed 8) | mixed | negative ✗ | mixed | 38% | none / none | mixed ✓, 25%, none |  |
| quake-champions-goes-paid-2026 (seed 9) | mixed | mixed ✓ | negative | 12% | none / none | mixed ✓, 25%, none |  |
| reddit-ends-rss-and-public-api-2026 (seed 7) | negative | negative ✓ | negative | 88% | none / none | negative ✓, 100%, partial |  |
| reddit-ends-rss-and-public-api-2026 (seed 8) | negative | negative ✓ | negative | 88% | none / none | negative ✓, 100%, partial |  |
| reddit-ends-rss-and-public-api-2026 (seed 9) | negative | negative ✓ | negative | 12% | none / none | negative ✓, 100%, partial |  |
| runescape-reignited-fresh-start-mode-2026 (seed 7) | negative | negative ✓ | negative | 12% | full / none | mixed ✗, 50%, partial |  |
| runescape-reignited-fresh-start-mode-2026 (seed 8) | negative | positive ✗ | mixed | 50% | full / partial | mixed ✗, 50%, partial |  |
| runescape-reignited-fresh-start-mode-2026 (seed 9) | negative | mixed ✗ | mixed | 75% | full / none | mixed ✗, 50%, partial |  |
| samsung-galaxy-z-fold-8-launch-2026 (seed 7) | positive | negative ✗ | negative | 50% | none / none | mixed ✗, 100%, none |  |
| samsung-galaxy-z-fold-8-launch-2026 (seed 8) | positive | negative ✗ | negative | 50% | none / none | mixed ✗, 100%, none |  |
| samsung-galaxy-z-fold-8-launch-2026 (seed 9) | positive | negative ✗ | negative | 38% | none / none | mixed ✗, 100%, none |  |
| sfusd-neighborhood-school-assignment-2026 (seed 7) | mixed | mixed ✓ | negative | 80% | none / none | mixed ✓, 80%, partial |  |
| sfusd-neighborhood-school-assignment-2026 (seed 8) | mixed | negative ✗ | negative | 40% | none / partial | mixed ✓, 80%, partial |  |
| sfusd-neighborhood-school-assignment-2026 (seed 9) | mixed | negative ✗ | negative | 70% | none / partial | mixed ✓, 80%, partial |  |
| sony-playstation-ends-physical-discs-2026 (seed 7) | negative | negative ✓ | negative | 62% | none / none | mixed ✗, 75%, none |  |
| sony-playstation-ends-physical-discs-2026 (seed 8) | negative | negative ✓ | negative | 75% | none / none | mixed ✗, 75%, none |  |
| sony-playstation-ends-physical-discs-2026 (seed 9) | negative | negative ✓ | mixed | 62% | none / none | mixed ✗, 75%, none |  |
| uk-electricity-vat-cut-2026 (seed 7) | positive | negative ✗ | mixed | 50% | none / none | positive ✓, 62%, none |  |
| uk-electricity-vat-cut-2026 (seed 8) | positive | negative ✗ | negative | 0% | none / none | positive ✓, 62%, none |  |
| uk-electricity-vat-cut-2026 (seed 9) | positive | mixed ✗ | positive | 50% | none / none | positive ✓, 62%, none |  |
| uk-two-pound-bus-fare-cap-2026 (seed 7) | positive | mixed ✗ | positive | 62% | none / none | positive ✓, 88%, none |  |
| uk-two-pound-bus-fare-cap-2026 (seed 8) | positive | mixed ✗ | positive | 88% | none / none | positive ✓, 88%, none |  |
| uk-two-pound-bus-fare-cap-2026 (seed 9) | positive | mixed ✗ | positive | 75% | none / partial | positive ✓, 88%, none |  |
| umich-first-semester-pass-fail-2026 (seed 7) | mixed | positive ✗ | mixed | 60% | partial / partial | mixed ✓, 60%, none |  |
| umich-first-semester-pass-fail-2026 (seed 8) | mixed | negative ✗ | mixed | 0% | partial / none | mixed ✓, 60%, none |  |
| umich-first-semester-pass-fail-2026 (seed 9) | mixed | mixed ✓ | mixed | 70% | partial / partial | mixed ✓, 60%, none |  |
| united-mileageplus-spend-only-status-2026 (seed 7) | negative | negative ✓ | negative | 80% | none / none | negative ✓, 80%, none |  |
| united-mileageplus-spend-only-status-2026 (seed 8) | negative | negative ✓ | negative | 90% | none / none | negative ✓, 80%, none |  |
| united-mileageplus-spend-only-status-2026 (seed 9) | negative | negative ✓ | negative | 80% | none / none | negative ✓, 80%, none |  |
| ut-austin-ethnic-gender-majors-2026 (seed 7) | negative | negative ✓ | negative | 60% | none / partial | negative ✓, 70%, partial |  |
| ut-austin-ethnic-gender-majors-2026 (seed 8) | negative | negative ✓ | negative | 40% | none / none | negative ✓, 70%, partial |  |
| ut-austin-ethnic-gender-majors-2026 (seed 9) | negative | negative ✓ | negative | 30% | none / none | negative ✓, 70%, partial |  |
| witcher-3-free-remaster-upgrade-2026 (seed 7) | positive | positive ✓ | positive | 12% | none / none | failed: claude call failed (sonnet): None; stderr:  |  |
| witcher-3-free-remaster-upgrade-2026 (seed 8) | positive | mixed ✗ | positive | 12% | none / none | failed: claude call failed (sonnet): None; stderr:  |  |
| witcher-3-free-remaster-upgrade-2026 (seed 9) | positive | mixed ✗ | positive | 25% | none / none | failed: claude call failed (sonnet): None; stderr:  |  |
| wow-forever-skyborne-paid-race-2026 (seed 7) | mixed | mixed ✓ | mixed | 30% | none / none | mixed ✓, 70%, partial |  |
| wow-forever-skyborne-paid-race-2026 (seed 8) | mixed | mixed ✓ | negative | 50% | none / partial | mixed ✓, 70%, partial |  |
| wow-forever-skyborne-paid-race-2026 (seed 9) | mixed | mixed ✓ | mixed | 30% | none / none | mixed ✓, 70%, partial |  |
| xbox-cloud-gaming-monthly-caps-2026 (seed 7) | negative | negative ✓ | negative | 50% | none / none | negative ✓, 62%, partial |  |
| xbox-cloud-gaming-monthly-caps-2026 (seed 8) | negative | negative ✓ | negative | 50% | none / none | negative ✓, 62%, partial |  |
| xbox-cloud-gaming-monthly-caps-2026 (seed 9) | negative | negative ✓ | negative | 38% | none / partial | negative ✓, 62%, partial |  |

Usage: {"simulation_calls": 4136, "judge": {"calls": 171, "input_tokens": 1486658, "output_tokens": 396449, "cost_usd": 8.631000799999999}, "zero_shot": {"calls": 42, "input_tokens": 205874, "output_tokens": 114575, "cost_usd": 1.7388928000000001}}
