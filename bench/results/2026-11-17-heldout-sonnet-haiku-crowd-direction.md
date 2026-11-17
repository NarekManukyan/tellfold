# Tellfold accuracy: heldout-sonnet-haiku-crowd-direction

Date 2026-11-17 · commit `5db9d7e` · 13 cases × 3 run(s) · agents haiku, planner sonnet, judge sonnet

**Direction from the crowd.** Each report's direction comes from the crowd's first posts, not the planner's mix.

| | graded | direction | objection recall | backdown | groups named | direction from sentiment |
|---|---|---|---|---|---|---|
| all cases | 39 | 69% | 51% | 64% | 61% | 56% |
| after the model's cutoff | 36 | 67% | 52% | 61% | 61% | 52% |
| known to the model | 3 | 100% | 33% | 100% | 67% | 100% |
| zero-shot baseline | 13 | 85% | 71% | 92% | 88% | – |
| planner's stance mix alone | 39 | 77% | – | – | – | – |

The crowd's final direction moved away from the planner's mix in 10 of 39 runs: 3 to the right answer, 6 away from it, 1 from one wrong answer to another. Tellfold agrees with the zero-shot answer on direction in 67% of runs; the planner's mix alone agrees with it in 67%.

Ignoring the announcement: always "negative" gets the direction right 31% of the time; always "no backdown" gets the backdown right 77%.

## Cases

| case | actual | predicted | planner's mix | objections | backdown (actual / predicted) | zero-shot | error |
|---|---|---|---|---|---|---|---|
| alberta-bill-35-report-cards-2026 (seed 7) | mixed | mixed ✓ | mixed | 62% | none / partial | mixed ✓, 75%, none |  |
| alberta-bill-35-report-cards-2026 (seed 8) | mixed | mixed ✓ | mixed | 62% | none / partial | mixed ✓, 75%, none |  |
| alberta-bill-35-report-cards-2026 (seed 9) | mixed | mixed ✓ | mixed | 50% | none / partial | mixed ✓, 75%, none |  |
| au-veterans-allied-health-cap-dropped-2026 (seed 7) | positive | mixed ✗ | positive | 62% | none / none | positive ✓, 62%, none |  |
| au-veterans-allied-health-cap-dropped-2026 (seed 8) | positive | mixed ✗ | positive | 50% | none / none | positive ✓, 62%, none |  |
| au-veterans-allied-health-cap-dropped-2026 (seed 9) | positive | positive ✓ | positive | 62% | none / none | positive ✓, 62%, none |  |
| australia-working-holiday-visa-ballot-2026 (seed 7) | negative | negative ✓ | negative | 50% | partial / partial | negative ✓, 70%, partial |  |
| australia-working-holiday-visa-ballot-2026 (seed 8) | negative | negative ✓ | negative | 40% | partial / none | negative ✓, 70%, partial |  |
| australia-working-holiday-visa-ballot-2026 (seed 9) | negative | negative ✓ | negative | 40% | partial / partial | negative ✓, 70%, partial |  |
| camden-high-street-pedestrianisation-2026 (seed 7) | mixed | mixed ✓ | positive | 38% | none / none | mixed ✓, 38%, none |  |
| camden-high-street-pedestrianisation-2026 (seed 8) | mixed | mixed ✓ | mixed | 38% | none / partial | mixed ✓, 38%, none |  |
| camden-high-street-pedestrianisation-2026 (seed 9) | mixed | mixed ✓ | mixed | 38% | none / partial | mixed ✓, 38%, none |  |
| dallas-library-funding-restored-2026 (seed 7) | positive | positive ✓ | positive | 62% | none / none | positive ✓, 50%, none |  |
| dallas-library-funding-restored-2026 (seed 8) | positive | positive ✓ | positive | 50% | none / none | positive ✓, 50%, none |  |
| dallas-library-funding-restored-2026 (seed 9) | positive | positive ✓ | positive | 62% | none / none | positive ✓, 50%, none |  |
| glasgow-council-fire-and-rehire-2026 (seed 7) | negative | negative ✓ | negative | 50% | full / none | mixed ✗, 60%, partial |  |
| glasgow-council-fire-and-rehire-2026 (seed 8) | negative | negative ✓ | negative | 70% | full / none | mixed ✗, 60%, partial |  |
| glasgow-council-fire-and-rehire-2026 (seed 9) | negative | negative ✓ | negative | 70% | full / none | mixed ✗, 60%, partial |  |
| hawaii-free-school-meals-expansion-2026 (seed 7) | positive | mixed ✗ | positive | 50% | none / none | positive ✓, 100%, none |  |
| hawaii-free-school-meals-expansion-2026 (seed 8) | positive | positive ✓ | positive | 67% | none / none | positive ✓, 100%, none |  |
| hawaii-free-school-meals-expansion-2026 (seed 9) | positive | mixed ✗ | positive | 67% | none / partial | positive ✓, 100%, none |  |
| manitoba-permanent-daylight-time-2026 (seed 7) | mixed | mixed ✓ | positive | 38% | none / none | mixed ✓, 88%, none |  |
| manitoba-permanent-daylight-time-2026 (seed 8) | mixed | mixed ✓ | mixed | 62% | none / none | mixed ✓, 88%, none |  |
| manitoba-permanent-daylight-time-2026 (seed 9) | mixed | mixed ✓ | mixed | 75% | none / none | mixed ✓, 88%, none |  |
| mtg-god-packs-booster-changes-2026 (seed 7) | mixed | negative ✗ | negative | 75% | none / partial | mixed ✓, 62%, none |  |
| mtg-god-packs-booster-changes-2026 (seed 8) | mixed | negative ✗ | negative | 25% | none / partial | mixed ✓, 62%, none |  |
| mtg-god-packs-booster-changes-2026 (seed 9) | mixed | negative ✗ | negative | 62% | none / none | mixed ✓, 62%, none |  |
| nice-enhertu-her2-low-breast-cancer-2026 (seed 7) | positive | positive ✓ | positive | 25% | none / none | positive ✓, 62%, none |  |
| nice-enhertu-her2-low-breast-cancer-2026 (seed 8) | positive | positive ✓ | positive | 25% | none / none | positive ✓, 62%, none |  |
| nice-enhertu-her2-low-breast-cancer-2026 (seed 9) | positive | positive ✓ | positive | 25% | none / none | positive ✓, 62%, none |  |
| rogers-wireless-plan-rate-adjustment-2026 (seed 7) | negative | negative ✓ | negative | 75% | none / none | negative ✓, 75%, partial |  |
| rogers-wireless-plan-rate-adjustment-2026 (seed 8) | negative | negative ✓ | negative | 12% | none / none | negative ✓, 75%, partial |  |
| rogers-wireless-plan-rate-adjustment-2026 (seed 9) | negative | negative ✓ | negative | 12% | none / none | negative ✓, 75%, partial |  |
| sezane-rebrand-2026 (seed 7) | mixed | negative ✗ | negative | 100% | none / none | mixed ✓, 88%, none |  |
| sezane-rebrand-2026 (seed 8) | mixed | mixed ✓ | negative | 62% | none / none | mixed ✓, 88%, none |  |
| sezane-rebrand-2026 (seed 9) | mixed | negative ✗ | negative | 75% | none / none | mixed ✓, 88%, none |  |
| trinity-university-sun-logo-2026 (seed 7) | negative | positive ✗ | mixed | 25% | full / none | mixed ✗, 88%, partial |  |
| trinity-university-sun-logo-2026 (seed 8) | negative | mixed ✗ | negative | 50% | full / none | mixed ✗, 88%, partial |  |
| trinity-university-sun-logo-2026 (seed 9) | negative | mixed ✗ | negative | 12% | full / partial | mixed ✗, 88%, partial |  |

Usage: {"simulation_calls": 1380, "judge": {"calls": 52, "input_tokens": 397834, "output_tokens": 120184, "cost_usd": 2.5633604}, "zero_shot": {"calls": 13, "input_tokens": 45898, "output_tokens": 38617, "cost_usd": 0.5393704}}
