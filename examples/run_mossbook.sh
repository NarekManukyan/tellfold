#!/usr/bin/env sh
# Example: how will Mossbook's users react to the switch to subscription pricing?
# Roughly 60-80 Claude calls with these settings (mostly haiku). Run from the repository root.
set -e
uv run tellfold run examples/mossbook_pricing.md \
  -q "How will existing Mossbook buyers and the wider houseplant community react to the new subscription pricing during its first two weeks, and will Mossbook have to change the plan?" \
  --agents 16 --rounds 5 --batch 6 \
  --event "3:A popular plant YouTuber with 800k subscribers posts a video titled 'Mossbook just betrayed its users'" \
  --event "4:Mossbook announces that photo diagnosis will stay free for existing buyers for two years instead of one" \
  --open
