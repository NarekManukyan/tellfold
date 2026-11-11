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
  run.py                     the harness (Python standard library only)
  results/                   one JSON and one Markdown file per real pass: <date>-<label>.json and .md
```

Run from the repository root:

```bash
python3 bench/run.py --validate          # check every case file
python3 bench/run.py --estimate          # calls, tokens and API-equivalent cost of a full pass
python3 bench/run.py --dry-run           # whole pass on the offline model: free, numbers meaningless,
                                         # results in bench/runs/results/ (ignored by git)
python3 bench/run.py --zero-shot         # a real pass, plus the no-crowd baseline
python3 bench/run.py --repeats 3         # three seeds per case (7, 8, 9)
python3 bench/run.py --cases a,b         # only these cases
```

The harness uses `engine-rs/target/release/tellfold` when it is built (`cargo build --release -p tellfold-cli` in
`engine-rs/`), else `tellfold` on your PATH; `--tellfold "uv run tellfold"` runs the Python engine instead. The
judge and the zero-shot baseline go through the Claude Code CLI (`--claude`, `--judge-model`).

Every case runs with the same settings: 20 agents, 6 rounds, batch 6, haiku agents, sonnet planner, seed 7 (8, 9
with `--repeats`), Claude Code for both roles. Simulation folders go to `bench/runs/` (ignored by git).

## Adding a case

1. Pick an announcement made after the cutoff of the model under test, with a reaction reported by at least two
   independent sources. Drop it if you can't verify it.
2. Write `input.md` in your own words, from what was public on the day only. Nothing about the reaction, the
   coverage or any later change, and nothing about the benchmark itself (that it is a test case, a control, or
   before or after a cutoff): that belongs in `rubric.json`. `--validate` rejects benchmark words and warns about
   words such as "backlash" or "petition".
3. Write `outcome.md` with a dated timeline and every source.
4. Write `rubric.json` (see any existing case; `validate_case` in `run.py` lists the rules). The question always
   follows the same template, so no case is asked a more leading question than another.
5. `python3 bench/run.py --validate`, then commit the folder.
