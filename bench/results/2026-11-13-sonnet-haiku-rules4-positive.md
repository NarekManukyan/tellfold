# Tellfold accuracy: sonnet-haiku

Date 2026-11-13 · commit `e54bdd5` · 10 cases × 3 run(s) · agents haiku, planner sonnet, judge sonnet

| | graded | direction | objection recall | backdown | groups named | direction from sentiment |
|---|---|---|---|---|---|---|
| all cases | 30 | 10% | 56% | 87% | 52% | 28% |
| after the model's cutoff | 30 | 10% | 56% | 87% | 52% | 28% |
| known to the model | 0 | – | – | – | – | – |
| planner's stance mix alone | 30 | 47% | – | – | – | – |

The crowd's final direction moved away from the planner's mix in 13 of 30 runs: 0 to the right answer, 11 away from it, 2 from one wrong answer to another. Tellfold agrees with the zero-shot answer on direction in – of runs; the planner's mix alone agrees with it in –.

Ignoring the announcement: always "negative" gets the direction right 0% of the time; always "no backdown" gets the backdown right 100%.

## Cases

| case | actual | predicted | planner's mix | objections | backdown (actual / predicted) | zero-shot | error |
|---|---|---|---|---|---|---|---|
| barnet-mays-lane-parking-2026 (seed 7) | positive | positive ✓ | positive | 33% | none / none |  |  |
| barnet-mays-lane-parking-2026 (seed 8) | positive | negative ✗ | negative | 33% | none / partial |  |  |
| barnet-mays-lane-parking-2026 (seed 9) | positive | mixed ✗ | positive | 33% | none / none |  |  |
| bungie-destiny-2-unvaulting-2026 (seed 7) | positive | mixed ✗ | mixed | 75% | none / none |  |  |
| bungie-destiny-2-unvaulting-2026 (seed 8) | positive | mixed ✗ | mixed | 62% | none / none |  |  |
| bungie-destiny-2-unvaulting-2026 (seed 9) | positive | mixed ✗ | mixed | 62% | none / none |  |  |
| delta-medallion-thresholds-unchanged-2026 (seed 7) | positive | negative ✗ | negative | 17% | none / partial |  |  |
| delta-medallion-thresholds-unchanged-2026 (seed 8) | positive | mixed ✗ | mixed | 67% | none / none |  |  |
| delta-medallion-thresholds-unchanged-2026 (seed 9) | positive | mixed ✗ | positive | 33% | none / none |  |  |
| grossmont-librarians-reinstated-2026 (seed 7) | positive | negative ✗ | mixed | 88% | none / none |  |  |
| grossmont-librarians-reinstated-2026 (seed 8) | positive | mixed ✗ | mixed | 88% | none / none |  |  |
| grossmont-librarians-reinstated-2026 (seed 9) | positive | mixed ✗ | mixed | 88% | none / none |  |  |
| mbta-fare-pilots-2026 (seed 7) | positive | mixed ✗ | positive | 83% | none / none |  |  |
| mbta-fare-pilots-2026 (seed 8) | positive | mixed ✗ | positive | 83% | none / none |  |  |
| mbta-fare-pilots-2026 (seed 9) | positive | negative ✗ | positive | 100% | none / none |  |  |
| minneapolis-school-closures-pause-2026 (seed 7) | positive | mixed ✗ | positive | 50% | none / none |  |  |
| minneapolis-school-closures-pause-2026 (seed 8) | positive | mixed ✗ | positive | 50% | none / none |  |  |
| minneapolis-school-closures-pause-2026 (seed 9) | positive | mixed ✗ | positive | 50% | none / none |  |  |
| samsung-galaxy-z-fold-8-launch-2026 (seed 7) | positive | mixed ✗ | negative | 38% | none / none |  |  |
| samsung-galaxy-z-fold-8-launch-2026 (seed 8) | positive | negative ✗ | negative | 38% | none / none |  |  |
| samsung-galaxy-z-fold-8-launch-2026 (seed 9) | positive | negative ✗ | negative | 50% | none / none |  |  |
| uk-electricity-vat-cut-2026 (seed 7) | positive | mixed ✗ | mixed | 38% | none / none |  |  |
| uk-electricity-vat-cut-2026 (seed 8) | positive | negative ✗ | positive | 38% | none / none |  |  |
| uk-electricity-vat-cut-2026 (seed 9) | positive | mixed ✗ | mixed | 38% | none / none |  |  |
| uk-two-pound-bus-fare-cap-2026 (seed 7) | positive | mixed ✗ | positive | 75% | none / none |  |  |
| uk-two-pound-bus-fare-cap-2026 (seed 8) | positive | negative ✗ | negative | 62% | none / partial |  |  |
| uk-two-pound-bus-fare-cap-2026 (seed 9) | positive | mixed ✗ | positive | 62% | none / none |  |  |
| witcher-3-free-remaster-upgrade-2026 (seed 7) | positive | negative ✗ | negative | 62% | none / partial |  |  |
| witcher-3-free-remaster-upgrade-2026 (seed 8) | positive | positive ✓ | positive | 62% | none / none |  |  |
| witcher-3-free-remaster-upgrade-2026 (seed 9) | positive | positive ✓ | positive | 38% | none / none |  |  |

Usage: {"simulation_calls": 926, "judge": {"calls": 30, "input_tokens": 282071, "output_tokens": 60994, "cost_usd": 1.531087}, "zero_shot": null}
