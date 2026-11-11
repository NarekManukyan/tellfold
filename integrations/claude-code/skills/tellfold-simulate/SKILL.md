---
name: tellfold-simulate
description: Simulate how users, customers or a community will react to a text before it ships, using the Tellfold MCP server. Use when the user asks how people will take a changelog, release notes, a pricing change, a deprecation notice, a launch post, an email or any announcement; when they want to compare two drafts of one; or when they ask "how will users react to this", "test this announcement", "which version lands better".
---

# Simulate a reaction with Tellfold

Tellfold builds a small crowd of simulated people who would care about a text, lets them post and reply to each
other for a few rounds, and writes a report: the likely reaction, how confident it is, who pushes back and why, each
claim citing the posts behind it. The `tellfold` MCP server must be connected (its tools are `start_simulation`,
`wait_for_run`, `get_report` and so on). If it is not, tell the user to set it up with the steps in
`integrations/claude-code/README.md` of the Tellfold repository and stop.

## 1. Gather the text and the question

- **The text.** Use exactly what the user will publish: the changelog entry, the release notes, the email. If it is in
  a file, pass the path in `files` (relative paths start at the working folder). If it is in the conversation, or you
  are writing it with them, pass it as `text` with a short `title` such as `changelog`.
- **Context the crowd needs.** The people only know what they are given plus general knowledge. If the text relies on
  something they would already know (current prices, who the product is for, what changed last time), add a short
  paragraph or a second file with it. Do not add opinions about how the text will be received.
- **The question.** One concrete question the report must answer, phrased around the audience, for example "How will
  existing paid users react to the new per-seat pricing?" Ask the user if the audience is unclear.

## 2. Pick a size

Simulations take minutes and use the user's AI plan. Start small unless the user asks otherwise:

| purpose | agents | rounds |
|---|---|---|
| quick read of a changelog or email | 8 | 3 |
| a pricing change or launch that matters | 16 | 5 |
| the engine defaults | 20 | 6 |

Pass `language` when the audience writes in another language.

## 3. Run it and wait

1. Call `start_simulation` with `question`, `text` or `files`, `agents` and `rounds`. It returns a `run_id` at once.
2. Call `wait_for_run` with that `run_id` (`timeout_seconds` up to 120). While `finished` is false, tell the user the
   stage and progress in one line and call it again. Do not start other runs meanwhile unless asked.
3. If the state is `failed`, show the `error`. A missing or logged-out AI tool is the usual cause: the user can run
   `tellfold runtimes` or open Tellfold's Settings › Agents.

## 4. Report back

Call `get_report` (format `markdown`). Then give the user:

- the prediction and its confidence, in a sentence or two;
- the main camps and the strongest objection, quoting a cited post or two;
- the report's recommendations that apply to the text, as concrete edits you can make.

Keep it short and link nothing outside the report. Say plainly that the crowd is simulated: it tends to be more polite
and reasonable than a real one, so treat the result as a rehearsal, not a survey.

Follow-ups that need no new run: `ask_crowd` for a question about the outcome ("Why did the power users push back?"),
`list_people` and `interview_person` to ask one person why they said what they said.

## Comparing two drafts

When the user has two versions, or you have just rewritten the text, call `compare_drafts` with `question`, `draft_a`
and `draft_b` (or `baseline_run_id` set to a finished run plus `draft_b`). Draft B is shown to the same people as
draft A, so the difference comes from the text.

If `run_b` and `study_id` come back null, draft A is still making its people and draft B starts once it has them.
Call `get_study` with `run_id` set to `run_a` and `wait_seconds: 60`, again until it returns a `study_id` and the
runs. Then wait for both runs with `wait_for_run`, call `get_study` with the `study_id` and `wait_seconds: 60` until
`ready` is true, and summarise which draft landed better and why.

If a run's status has `pause` set, it is waiting for its cast to be checked in the Tellfold app: tell the user to
open the app and confirm the cast, then keep waiting.

## Checking a result that matters

Results vary between runs. Before a decision that matters, call `check_run` on the finished run (`runs: 3`), wait
with `get_study` and report what held across the repeats and what split.
