# Tellfold accuracy benchmark

A small, open test of Tellfold against real announcements whose public reaction is on record. The method, the case
list and the limits are written up in [docs/ACCURACY.md](../docs/ACCURACY.md).

```
bench/
  cases/<slug>/input.md      what was known on the day: the only seed the simulation gets
  cases/<slug>/outcome.md    what happened, with dated sources (never shown to the simulation or the judge)
  cases/<slug>/rubric.json   direction, top objections, backdown, groups that pushed back
  judge_prompt.md            the grading prompt (the judge never sees the true direction or backdown)
  zero_shot_prompt.md        the no-crowd baseline prompt (--zero-shot)
  results/                   one JSON and one Markdown file per real pass: <date>-<label>.json and .md
```

The harness that runs these cases is part of the app's source, which is not public. Every case file and every pass it produced is here, so each score in the write-up can be checked against the case and the judge's notes.
