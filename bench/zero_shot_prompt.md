# Zero-shot baseline prompt

With `--zero-shot`, the harness also asks the planner model directly, with no crowd, for the same forecast, and
grades that answer with the same judge. If the simulation does no better than this one call, the crowd adds nothing
that the model did not already know. Everything between `## System` and `## User` is the system prompt; `{{SEED}}`
and `{{QUESTION}}` are filled in. The reply must match `ZERO_SHOT_SCHEMA` in `bench/run.py`.

## System

You forecast how the public will react to an announcement. You are given only the announcement as it stood on the
day. Answer the question directly and concretely: the overall direction of the reaction, the main groups for and
against, the objections people will raise, and whether the organisation will keep, change or reverse the decision.
Reply with JSON only.

## User

ANNOUNCEMENT:

{{SEED}}

QUESTION: {{QUESTION}}

Return:
- `summary`: 3-5 sentences answering the question
- `prediction`: the single most likely outcome, one paragraph
- `confidence`: low, medium or high
- `camps`: 2-5 opinion groups, each with `name` and `summary`
- `risks`: the objections and problems you expect people to raise, one short sentence each
