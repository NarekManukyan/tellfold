# Tellfold accuracy: sonnet-haiku-rules4-hedge-fix

Date 2026-11-17 · commit `055cbe2` · 43 cases × 3 run(s) · agents haiku, planner sonnet, judge sonnet

**Direction from the planner.** Each report's direction comes from the planner's mix, and the crowd gives the reasons, objections and quotes.

| | graded | direction | objection recall | backdown | groups named | direction from sentiment |
|---|---|---|---|---|---|---|
| all cases | 129 | 69% | 49% | 74% | 54% | 51% |
| after the model's cutoff | 123 | 68% | 49% | 74% | 54% | 52% |
| known to the model | 6 | 100% | 44% | 83% | 44% | 40% |
| planner's stance mix alone | 129 | 68% | – | – | – | – |

The report's direction moved away from the planner's mix in 2 of 129 runs: 1 to the right answer, 0 away from it, 1 from one wrong answer to another. Tellfold agrees with the zero-shot answer on direction in – of runs; the planner's mix alone agrees with it in –.

Ignoring the announcement: always "negative" gets the direction right 49% of the time; always "no backdown" gets the backdown right 77%.

## Cases

| case | actual | predicted | planner's mix | objections | backdown (actual / predicted) | zero-shot | error |
|---|---|---|---|---|---|---|---|
| anthropic-claude-code-weekly-limits-2026 (seed 7) | negative | negative ✓ | negative | 38% | none / none |  |  |
| anthropic-claude-code-weekly-limits-2026 (seed 8) | negative | negative ✓ | negative | 50% | none / none |  |  |
| anthropic-claude-code-weekly-limits-2026 (seed 9) | negative | negative ✓ | negative | 25% | none / partial |  |  |
| apple-iphone-duo-and-price-rises-2026 (seed 7) | mixed | negative ✗ | negative | 50% | none / none |  |  |
| apple-iphone-duo-and-price-rises-2026 (seed 8) | mixed | negative ✗ | negative | 62% | none / none |  |  |
| apple-iphone-duo-and-price-rises-2026 (seed 9) | mixed | negative ✗ | negative | 25% | none / none |  |  |
| apple-music-black-and-white-rebrand-2026 (seed 7) | negative | negative ✓ | negative | 62% | none / none |  |  |
| apple-music-black-and-white-rebrand-2026 (seed 8) | negative | mixed ✗ | mixed | 62% | none / none |  |  |
| apple-music-black-and-white-rebrand-2026 (seed 9) | negative | negative ✓ | negative | 88% | none / none |  |  |
| barclays-three-day-office-2026 (seed 7) | negative | negative ✓ | negative | 60% | partial / none |  |  |
| barclays-three-day-office-2026 (seed 8) | negative | negative ✓ | negative | 80% | partial / partial |  |  |
| barclays-three-day-office-2026 (seed 9) | negative | negative ✓ | negative | 90% | partial / partial |  |  |
| barnet-mays-lane-parking-2026 (seed 7) | positive | positive ✓ | positive | 67% | none / none |  |  |
| barnet-mays-lane-parking-2026 (seed 8) | positive | positive ✓ | positive | 33% | none / none |  |  |
| barnet-mays-lane-parking-2026 (seed 9) | positive | positive ✓ | positive | 50% | none / none |  |  |
| blood-of-dawnwalker-40fps-console-cap-2026 (seed 7) | negative | negative ✓ | negative | 67% | full / none |  |  |
| blood-of-dawnwalker-40fps-console-cap-2026 (seed 8) | negative | negative ✓ | negative | 67% | full / none |  |  |
| blood-of-dawnwalker-40fps-console-cap-2026 (seed 9) | negative | negative ✓ | negative | 67% | full / partial |  |  |
| bungie-destiny-2-unvaulting-2026 (seed 7) | positive | mixed ✗ | mixed | 75% | none / none |  |  |
| bungie-destiny-2-unvaulting-2026 (seed 8) | positive | mixed ✗ | mixed | 75% | none / none |  |  |
| bungie-destiny-2-unvaulting-2026 (seed 9) | positive | mixed ✗ | mixed | 62% | none / none |  |  |
| costco-churro-return-2026 (seed 7) | mixed | negative ✗ | negative | 0% | none / none |  |  |
| costco-churro-return-2026 (seed 8) | mixed | positive ✗ | positive | 50% | none / none |  |  |
| costco-churro-return-2026 (seed 9) | mixed | positive ✗ | positive | 62% | none / none |  |  |
| delta-medallion-thresholds-unchanged-2026 (seed 7) | positive | mixed ✗ | mixed | 50% | none / none |  |  |
| delta-medallion-thresholds-unchanged-2026 (seed 8) | positive | positive ✓ | positive | 50% | none / none |  |  |
| delta-medallion-thresholds-unchanged-2026 (seed 9) | positive | mixed ✗ | mixed | 17% | none / none |  |  |
| dublin-7-bin-bag-ban-2026 (seed 7) | negative | negative ✓ | negative | 50% | partial / partial |  |  |
| dublin-7-bin-bag-ban-2026 (seed 8) | negative | negative ✓ | negative | 75% | partial / partial |  |  |
| dublin-7-bin-bag-ban-2026 (seed 9) | negative | negative ✓ | negative | 38% | partial / none |  |  |
| garmin-cirqa-live-heart-rate-paywall-2026 (seed 7) | negative | negative ✓ | negative | 67% | partial / none |  |  |
| garmin-cirqa-live-heart-rate-paywall-2026 (seed 8) | negative | positive ✗ | mixed | 50% | partial / none |  |  |
| garmin-cirqa-live-heart-rate-paywall-2026 (seed 9) | negative | positive ✗ | positive | 50% | partial / none |  |  |
| google-pixel-11-launch-price-rise-2026 (seed 7) | mixed | negative ✗ | negative | 62% | none / none |  |  |
| google-pixel-11-launch-price-rise-2026 (seed 8) | mixed | negative ✗ | negative | 50% | none / partial |  |  |
| google-pixel-11-launch-price-rise-2026 (seed 9) | mixed | negative ✗ | negative | 62% | none / none |  |  |
| grossmont-librarians-reinstated-2026 (seed 7) | positive | positive ✓ | positive | 88% | none / none |  |  |
| grossmont-librarians-reinstated-2026 (seed 8) | positive | mixed ✗ | mixed | 88% | none / none |  |  |
| grossmont-librarians-reinstated-2026 (seed 9) | positive | negative ✗ | negative | 0% | none / none |  |  |
| hillsborough-leggings-dress-code-2026 (seed 7) | negative | mixed ✗ | mixed | 25% | full / partial |  |  |
| hillsborough-leggings-dress-code-2026 (seed 8) | negative | mixed ✗ | mixed | 12% | full / none |  |  |
| hillsborough-leggings-dress-code-2026 (seed 9) | negative | negative ✓ | negative | 25% | full / partial |  |  |
| hubspot-contact-data-sharing-terms-2026 (seed 7) | negative | negative ✓ | negative | 62% | full / partial |  |  |
| hubspot-contact-data-sharing-terms-2026 (seed 8) | negative | negative ✓ | negative | 62% | full / partial |  |  |
| hubspot-contact-data-sharing-terms-2026 (seed 9) | negative | negative ✓ | negative | 38% | full / none |  |  |
| instagram-wordmark-refresh-2026 (seed 7) | mixed | mixed ✓ | mixed | 0% | none / none |  |  |
| instagram-wordmark-refresh-2026 (seed 8) | mixed | mixed ✓ | mixed | 25% | none / none |  |  |
| instagram-wordmark-refresh-2026 (seed 9) | mixed | mixed ✓ | mixed | 50% | none / none |  |  |
| ireland-e-scooter-under-18-ban-2026 (seed 7) | mixed | mixed ✓ | mixed | 30% | none / none |  |  |
| ireland-e-scooter-under-18-ban-2026 (seed 8) | mixed | mixed ✓ | mixed | 20% | none / partial |  |  |
| ireland-e-scooter-under-18-ban-2026 (seed 9) | mixed | mixed ✓ | mixed | 40% | none / none |  |  |
| ireland-home-heating-carbon-tax-cut-2026 (seed 7) | mixed | positive ✗ | positive | 38% | none / none |  |  |
| ireland-home-heating-carbon-tax-cut-2026 (seed 8) | mixed | positive ✗ | positive | 50% | none / none |  |  |
| ireland-home-heating-carbon-tax-cut-2026 (seed 9) | mixed | mixed ✓ | mixed | 62% | none / none |  |  |
| ireland-nta-fare-increase-2026 (seed 7) | negative | negative ✓ | negative | 25% | none / partial |  |  |
| ireland-nta-fare-increase-2026 (seed 8) | negative | negative ✓ | negative | 25% | none / none |  |  |
| ireland-nta-fare-increase-2026 (seed 9) | negative | negative ✓ | negative | 12% | none / none |  |  |
| iron-county-four-day-week-2026 (seed 7) | mixed | mixed ✓ | positive | 62% | none / partial |  |  |
| iron-county-four-day-week-2026 (seed 8) | mixed | positive ✗ | positive | 50% | none / partial |  |  |
| iron-county-four-day-week-2026 (seed 9) | mixed | positive ✗ | positive | 62% | none / none |  |  |
| jetstar-overhead-locker-fee-2026 (seed 7) | negative | negative ✓ | negative | 70% | none / none |  |  |
| jetstar-overhead-locker-fee-2026 (seed 8) | negative | negative ✓ | negative | 80% | none / none |  |  |
| jetstar-overhead-locker-fee-2026 (seed 9) | negative | negative ✓ | negative | 60% | none / none |  |  |
| mbta-fare-pilots-2026 (seed 7) | positive | positive ✓ | positive | 83% | none / partial |  |  |
| mbta-fare-pilots-2026 (seed 8) | positive | positive ✓ | positive | 67% | none / partial |  |  |
| mbta-fare-pilots-2026 (seed 9) | positive | positive ✓ | positive | 67% | none / none |  |  |
| mclaren-heritage-wordmark-2026 (seed 7) | mixed | mixed ✓ | mixed | 25% | none / none |  |  |
| mclaren-heritage-wordmark-2026 (seed 8) | mixed | mixed ✓ | mixed | 38% | none / none |  |  |
| mclaren-heritage-wordmark-2026 (seed 9) | mixed | mixed ✓ | mixed | 38% | none / none |  |  |
| meta-commercial-terms-website-crawling-2026 (seed 7) | negative | negative ✓ | negative | 0% | full / none |  |  |
| meta-commercial-terms-website-crawling-2026 (seed 8) | negative | negative ✓ | negative | 40% | full / none |  |  |
| meta-commercial-terms-website-crawling-2026 (seed 9) | negative | mixed ✗ | mixed | 60% | full / none |  |  |
| minneapolis-school-closures-pause-2026 (seed 7) | positive | positive ✓ | positive | 75% | none / none |  |  |
| minneapolis-school-closures-pause-2026 (seed 8) | positive | positive ✓ | positive | 50% | none / none |  |  |
| minneapolis-school-closures-pause-2026 (seed 9) | positive | positive ✓ | positive | 62% | none / none |  |  |
| natwest-office-attendance-2026 (seed 7) | negative | negative ✓ | negative | 75% | none / partial |  |  |
| natwest-office-attendance-2026 (seed 8) | negative | negative ✓ | negative | 75% | none / none |  |  |
| natwest-office-attendance-2026 (seed 9) | negative | negative ✓ | negative | 75% | none / partial |  |  |
| nintendo-switch-2-price-rise-2026 (seed 7) | negative | negative ✓ | negative | 17% | none / none |  |  |
| nintendo-switch-2-price-rise-2026 (seed 8) | negative | negative ✓ | negative | 33% | none / none |  |  |
| nintendo-switch-2-price-rise-2026 (seed 9) | negative | negative ✓ | negative | 50% | none / none |  |  |
| olathe-school-closures-2026 (seed 7) | negative | negative ✓ | negative | 40% | none / none |  |  |
| olathe-school-closures-2026 (seed 8) | negative | negative ✓ | negative | 30% | none / none |  |  |
| olathe-school-closures-2026 (seed 9) | negative | negative ✓ | negative | 30% | none / none |  |  |
| openai-chatgpt-pro-200-usage-cut-2026 (seed 7) | negative | negative ✓ | negative | 62% | full / none |  |  |
| openai-chatgpt-pro-200-usage-cut-2026 (seed 8) | negative | negative ✓ | negative | 62% | full / none |  |  |
| openai-chatgpt-pro-200-usage-cut-2026 (seed 9) | negative | negative ✓ | negative | 62% | full / none |  |  |
| quake-champions-goes-paid-2026 (seed 7) | mixed | mixed ✓ | mixed | 38% | none / none |  |  |
| quake-champions-goes-paid-2026 (seed 8) | mixed | mixed ✓ | mixed | 50% | none / none |  |  |
| quake-champions-goes-paid-2026 (seed 9) | mixed | negative ✗ | negative | 25% | none / none |  |  |
| reddit-ends-rss-and-public-api-2026 (seed 7) | negative | negative ✓ | negative | 62% | none / none |  |  |
| reddit-ends-rss-and-public-api-2026 (seed 8) | negative | negative ✓ | negative | 50% | none / none |  |  |
| reddit-ends-rss-and-public-api-2026 (seed 9) | negative | negative ✓ | negative | 62% | none / partial |  |  |
| runescape-reignited-fresh-start-mode-2026 (seed 7) | negative | mixed ✗ | mixed | 38% | full / none |  |  |
| runescape-reignited-fresh-start-mode-2026 (seed 8) | negative | mixed ✗ | mixed | 50% | full / none |  |  |
| runescape-reignited-fresh-start-mode-2026 (seed 9) | negative | mixed ✗ | mixed | 50% | full / partial |  |  |
| samsung-galaxy-z-fold-8-launch-2026 (seed 7) | positive | negative ✗ | negative | 38% | none / none |  |  |
| samsung-galaxy-z-fold-8-launch-2026 (seed 8) | positive | negative ✗ | negative | 50% | none / none |  |  |
| samsung-galaxy-z-fold-8-launch-2026 (seed 9) | positive | negative ✗ | negative | 38% | none / none |  |  |
| sfusd-neighborhood-school-assignment-2026 (seed 7) | mixed | negative ✗ | negative | 50% | none / partial |  |  |
| sfusd-neighborhood-school-assignment-2026 (seed 8) | mixed | negative ✗ | negative | 50% | none / partial |  |  |
| sfusd-neighborhood-school-assignment-2026 (seed 9) | mixed | negative ✗ | negative | 30% | none / partial |  |  |
| sony-playstation-ends-physical-discs-2026 (seed 7) | negative | negative ✓ | negative | 62% | none / none |  |  |
| sony-playstation-ends-physical-discs-2026 (seed 8) | negative | negative ✓ | negative | 62% | none / none |  |  |
| sony-playstation-ends-physical-discs-2026 (seed 9) | negative | negative ✓ | negative | 75% | none / none |  |  |
| uk-electricity-vat-cut-2026 (seed 7) | positive | positive ✓ | positive | 50% | none / none |  |  |
| uk-electricity-vat-cut-2026 (seed 8) | positive | mixed ✗ | mixed | 0% | none / none |  |  |
| uk-electricity-vat-cut-2026 (seed 9) | positive | positive ✓ | positive | 50% | none / none |  |  |
| uk-two-pound-bus-fare-cap-2026 (seed 7) | positive | positive ✓ | positive | 38% | none / none |  |  |
| uk-two-pound-bus-fare-cap-2026 (seed 8) | positive | positive ✓ | positive | 50% | none / none |  |  |
| uk-two-pound-bus-fare-cap-2026 (seed 9) | positive | positive ✓ | positive | 75% | none / none |  |  |
| umich-first-semester-pass-fail-2026 (seed 7) | mixed | mixed ✓ | mixed | 40% | partial / partial |  |  |
| umich-first-semester-pass-fail-2026 (seed 8) | mixed | mixed ✓ | mixed | 50% | partial / none |  |  |
| umich-first-semester-pass-fail-2026 (seed 9) | mixed | positive ✗ | positive | 10% | partial / partial |  |  |
| united-mileageplus-spend-only-status-2026 (seed 7) | negative | negative ✓ | negative | 70% | none / partial |  |  |
| united-mileageplus-spend-only-status-2026 (seed 8) | negative | negative ✓ | negative | 90% | none / none |  |  |
| united-mileageplus-spend-only-status-2026 (seed 9) | negative | negative ✓ | negative | 70% | none / none |  |  |
| ut-austin-ethnic-gender-majors-2026 (seed 7) | negative | negative ✓ | negative | 30% | none / none |  |  |
| ut-austin-ethnic-gender-majors-2026 (seed 8) | negative | negative ✓ | negative | 50% | none / none |  |  |
| ut-austin-ethnic-gender-majors-2026 (seed 9) | negative | negative ✓ | negative | 40% | none / none |  |  |
| witcher-3-free-remaster-upgrade-2026 (seed 7) | positive | positive ✓ | positive | 38% | none / none |  |  |
| witcher-3-free-remaster-upgrade-2026 (seed 8) | positive | positive ✓ | positive | 25% | none / none |  |  |
| witcher-3-free-remaster-upgrade-2026 (seed 9) | positive | positive ✓ | positive | 25% | none / none |  |  |
| wow-forever-skyborne-paid-race-2026 (seed 7) | mixed | negative ✗ | negative | 30% | none / none |  |  |
| wow-forever-skyborne-paid-race-2026 (seed 8) | mixed | mixed ✓ | mixed | 20% | none / none |  |  |
| wow-forever-skyborne-paid-race-2026 (seed 9) | mixed | negative ✗ | negative | 50% | none / none |  |  |
| xbox-cloud-gaming-monthly-caps-2026 (seed 7) | negative | negative ✓ | negative | 50% | none / none |  |  |
| xbox-cloud-gaming-monthly-caps-2026 (seed 8) | negative | negative ✓ | negative | 50% | none / none |  |  |
| xbox-cloud-gaming-monthly-caps-2026 (seed 9) | negative | negative ✓ | negative | 50% | none / none |  |  |

Usage: {"simulation_calls": 4850, "judge": {"calls": 129, "input_tokens": 1133283, "output_tokens": 282821, "cost_usd": 6.7449818}, "zero_shot": null}
