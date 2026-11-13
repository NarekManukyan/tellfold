# Tellfold accuracy: sonnet-haiku

Date 2026-11-13 · commit `5ba314c` · 20 cases × 3 run(s) · agents haiku, planner sonnet, judge sonnet

| | graded | direction | objection recall | backdown | groups named | direction from sentiment |
|---|---|---|---|---|---|---|
| all cases | 60 | 78% | 50% | 67% | 52% | 72% |
| after the model's cutoff | 54 | 76% | 52% | 63% | 54% | 70% |
| known to the model | 6 | 100% | 36% | 100% | 33% | 83% |
| zero-shot baseline | 20 | 40% | 68% | 60% | 70% | – |
| planner's stance mix alone | 60 | 80% | – | – | – | – |

The crowd's final direction moved away from the planner's mix in 20 of 60 runs: 9 to the right answer, 10 away from it, 1 from one wrong answer to another. Tellfold agrees with the zero-shot answer on direction in 33% of runs; the planner's mix alone agrees with it in 57%.

Ignoring the announcement: always "negative" gets the direction right 75% of the time; always "no backdown" gets the backdown right 65%.

## Cases

| case | actual | predicted | planner's mix | objections | backdown (actual / predicted) | zero-shot | error |
|---|---|---|---|---|---|---|---|
| apple-music-black-and-white-rebrand-2026 (seed 7) | negative | negative ✓ | negative | 75% | none / none | mixed ✗, 100%, none |  |
| apple-music-black-and-white-rebrand-2026 (seed 8) | negative | mixed ✗ | negative | 62% | none / none | mixed ✗, 100%, none |  |
| apple-music-black-and-white-rebrand-2026 (seed 9) | negative | mixed ✗ | mixed | 50% | none / none | mixed ✗, 100%, none |  |
| barclays-three-day-office-2026 (seed 7) | negative | negative ✓ | negative | 60% | partial / partial | mixed ✗, 70%, partial |  |
| barclays-three-day-office-2026 (seed 8) | negative | negative ✓ | negative | 80% | partial / partial | mixed ✗, 70%, partial |  |
| barclays-three-day-office-2026 (seed 9) | negative | negative ✓ | negative | 70% | partial / partial | mixed ✗, 70%, partial |  |
| barnet-mays-lane-parking-2026 (seed 7) | positive | mixed ✗ | positive | 50% | none / none | positive ✓, 67%, none |  |
| barnet-mays-lane-parking-2026 (seed 8) | positive | mixed ✗ | positive | 33% | none / none | positive ✓, 67%, none |  |
| barnet-mays-lane-parking-2026 (seed 9) | positive | mixed ✗ | positive | 33% | none / none | positive ✓, 67%, none |  |
| dublin-7-bin-bag-ban-2026 (seed 7) | negative | negative ✓ | negative | 50% | partial / partial | mixed ✗, 62%, partial |  |
| dublin-7-bin-bag-ban-2026 (seed 8) | negative | negative ✓ | negative | 50% | partial / partial | mixed ✗, 62%, partial |  |
| dublin-7-bin-bag-ban-2026 (seed 9) | negative | negative ✓ | negative | 50% | partial / partial | mixed ✗, 62%, partial |  |
| garmin-cirqa-live-heart-rate-paywall-2026 (seed 7) | negative | negative ✓ | mixed | 67% | partial / none | mixed ✗, 83%, none |  |
| garmin-cirqa-live-heart-rate-paywall-2026 (seed 8) | negative | negative ✓ | mixed | 83% | partial / none | mixed ✗, 83%, none |  |
| garmin-cirqa-live-heart-rate-paywall-2026 (seed 9) | negative | negative ✓ | mixed | 67% | partial / none | mixed ✗, 83%, none |  |
| hillsborough-leggings-dress-code-2026 (seed 7) | negative | negative ✓ | mixed | 12% | full / partial | mixed ✗, 62%, none |  |
| hillsborough-leggings-dress-code-2026 (seed 8) | negative | negative ✓ | mixed | 25% | full / none | mixed ✗, 62%, none |  |
| hillsborough-leggings-dress-code-2026 (seed 9) | negative | negative ✓ | mixed | 12% | full / none | mixed ✗, 62%, none |  |
| hubspot-contact-data-sharing-terms-2026 (seed 7) | negative | mixed ✗ | negative | 62% | full / partial | mixed ✗, 50%, none |  |
| hubspot-contact-data-sharing-terms-2026 (seed 8) | negative | negative ✓ | negative | 38% | full / partial | mixed ✗, 50%, none |  |
| hubspot-contact-data-sharing-terms-2026 (seed 9) | negative | negative ✓ | negative | 50% | full / none | mixed ✗, 50%, none |  |
| instagram-wordmark-refresh-2026 (seed 7) | mixed | mixed ✓ | mixed | 50% | none / none | mixed ✓, 75%, none |  |
| instagram-wordmark-refresh-2026 (seed 8) | mixed | mixed ✓ | mixed | 0% | none / none | mixed ✓, 75%, none |  |
| instagram-wordmark-refresh-2026 (seed 9) | mixed | negative ✗ | mixed | 25% | none / none | mixed ✓, 75%, none |  |
| ireland-nta-fare-increase-2026 (seed 7) | negative | negative ✓ | negative | 50% | none / none | negative ✓, 50%, partial |  |
| ireland-nta-fare-increase-2026 (seed 8) | negative | negative ✓ | negative | 12% | none / partial | negative ✓, 50%, partial |  |
| ireland-nta-fare-increase-2026 (seed 9) | negative | negative ✓ | negative | 12% | none / none | negative ✓, 50%, partial |  |
| iron-county-four-day-week-2026 (seed 7) | mixed | negative ✗ | positive | 62% | none / none | positive ✗, 88%, none |  |
| iron-county-four-day-week-2026 (seed 8) | mixed | mixed ✓ | positive | 62% | none / partial | positive ✗, 88%, none |  |
| iron-county-four-day-week-2026 (seed 9) | mixed | mixed ✓ | positive | 62% | none / none | positive ✗, 88%, none |  |
| mbta-fare-pilots-2026 (seed 7) | positive | mixed ✗ | positive | 67% | none / none | positive ✓, 67%, none |  |
| mbta-fare-pilots-2026 (seed 8) | positive | mixed ✗ | positive | 100% | none / none | positive ✓, 67%, none |  |
| mbta-fare-pilots-2026 (seed 9) | positive | mixed ✗ | positive | 100% | none / none | positive ✓, 67%, none |  |
| meta-commercial-terms-website-crawling-2026 (seed 7) | negative | negative ✓ | negative | 50% | full / none | mixed ✗, 80%, partial |  |
| meta-commercial-terms-website-crawling-2026 (seed 8) | negative | negative ✓ | mixed | 10% | full / none | mixed ✗, 80%, partial |  |
| meta-commercial-terms-website-crawling-2026 (seed 9) | negative | negative ✓ | negative | 50% | full / none | mixed ✗, 80%, partial |  |
| natwest-office-attendance-2026 (seed 7) | negative | negative ✓ | negative | 88% | none / partial | mixed ✗, 88%, partial |  |
| natwest-office-attendance-2026 (seed 8) | negative | negative ✓ | negative | 88% | none / partial | mixed ✗, 88%, partial |  |
| natwest-office-attendance-2026 (seed 9) | negative | negative ✓ | negative | 75% | none / partial | mixed ✗, 88%, partial |  |
| nintendo-switch-2-price-rise-2026 (seed 7) | negative | negative ✓ | negative | 17% | none / none | negative ✓, 33%, none |  |
| nintendo-switch-2-price-rise-2026 (seed 8) | negative | negative ✓ | negative | 33% | none / none | negative ✓, 33%, none |  |
| nintendo-switch-2-price-rise-2026 (seed 9) | negative | negative ✓ | negative | 17% | none / none | negative ✓, 33%, none |  |
| olathe-school-closures-2026 (seed 7) | negative | negative ✓ | negative | 30% | none / partial | mixed ✗, 60%, partial |  |
| olathe-school-closures-2026 (seed 8) | negative | negative ✓ | negative | 50% | none / partial | mixed ✗, 60%, partial |  |
| olathe-school-closures-2026 (seed 9) | negative | negative ✓ | negative | 60% | none / none | mixed ✗, 60%, partial |  |
| openai-chatgpt-pro-200-usage-cut-2026 (seed 7) | negative | negative ✓ | negative | 50% | full / partial | mixed ✗, 88%, partial |  |
| openai-chatgpt-pro-200-usage-cut-2026 (seed 8) | negative | negative ✓ | negative | 62% | full / none | mixed ✗, 88%, partial |  |
| openai-chatgpt-pro-200-usage-cut-2026 (seed 9) | negative | mixed ✗ | negative | 75% | full / none | mixed ✗, 88%, partial |  |
| sony-playstation-ends-physical-discs-2026 (seed 7) | negative | negative ✓ | negative | 75% | none / none | mixed ✗, 75%, none |  |
| sony-playstation-ends-physical-discs-2026 (seed 8) | negative | negative ✓ | negative | 62% | none / none | mixed ✗, 75%, none |  |
| sony-playstation-ends-physical-discs-2026 (seed 9) | negative | negative ✓ | negative | 75% | none / none | mixed ✗, 75%, none |  |
| ut-austin-ethnic-gender-majors-2026 (seed 7) | negative | negative ✓ | negative | 60% | none / none | negative ✓, 70%, partial |  |
| ut-austin-ethnic-gender-majors-2026 (seed 8) | negative | negative ✓ | negative | 20% | none / none | negative ✓, 70%, partial |  |
| ut-austin-ethnic-gender-majors-2026 (seed 9) | negative | negative ✓ | negative | 60% | none / none | negative ✓, 70%, partial |  |
| witcher-3-free-remaster-upgrade-2026 (seed 7) | positive | negative ✗ | negative | 50% | none / none | positive ✓, 38%, none |  |
| witcher-3-free-remaster-upgrade-2026 (seed 8) | positive | positive ✓ | positive | 0% | none / none | positive ✓, 38%, none |  |
| witcher-3-free-remaster-upgrade-2026 (seed 9) | positive | positive ✓ | positive | 0% | none / none | positive ✓, 38%, none |  |
| xbox-cloud-gaming-monthly-caps-2026 (seed 7) | negative | negative ✓ | negative | 62% | none / none | negative ✓, 62%, partial |  |
| xbox-cloud-gaming-monthly-caps-2026 (seed 8) | negative | negative ✓ | negative | 50% | none / partial | negative ✓, 62%, partial |  |
| xbox-cloud-gaming-monthly-caps-2026 (seed 9) | negative | negative ✓ | negative | 50% | none / partial | negative ✓, 62%, partial |  |

Usage: {"simulation_calls": 1975, "judge": {"calls": 80, "input_tokens": 693669, "output_tokens": 187879, "cost_usd": 4.1031892}, "zero_shot": {"calls": 20, "input_tokens": 107303, "output_tokens": 62807, "cost_usd": 0.9341748000000001}}
