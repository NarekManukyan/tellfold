# Tellfold

**See how people might react, before you tell them.**

Tellfold is a free Mac app. You describe something you are about to announce or change, and ask a question about it. Tellfold builds a small crowd of simulated people who would care, lets them talk about it with each other for a few rounds, and then writes you a short answer: how the reaction is likely to go, who pushes back, and why.

[**Download for Mac**](https://narekmanukyan.github.io/tellfold/) · [Website](https://narekmanukyan.github.io/tellfold/) · [Sponsor](https://github.com/sponsors/NarekManukyan)

<p align="center"><a href="media/video/out/tellfold-16x9.mp4"><img src="media/video/out/tellfold-preview.webp" width="800" alt="A crowd of eight simulated people talks over a café's price rise round by round while a mood line rises from −0.15 to +0.70. Then six people fold into the Tellfold fan, and the fan opens into a report whose claims cite the posts behind them."></a></p>

<p align="center"><sub>Real runs, played back. <a href="media/video/out/tellfold-16x9.mp4">Watch the 45-second film with sound</a>.</sub></p>

## What you can use it for

Anything where you want to know how a group of people will take news before they hear it. For example:

- **A small business** raising its prices, changing its opening hours or launching a loyalty card
- **A school** writing to parents about a new rule, a timetable change or a trip
- **A town council or club** proposing a new street layout, a fee or a change to the rules
- **A team or company** announcing a reorganisation, a return to the office or a new benefit
- **A landlord or housing group** explaining a rent increase or building works
- **A charity or campaign** testing how an appeal or a message will land
- **A product or app** changing its pricing or launching a feature

You can also add a twist partway through ("a local newspaper criticises the plan") and see whether the mood changes.

## How it works

1. **Describe it.** Paste your announcement, letter or plan, or add the document. Then type your question, such as "How will regular customers react to the new prices?"
2. **Watch the crowd.** Tellfold creates a mix of people (supporters, sceptics and people who don't care much) and lets them post, reply and like each other's posts, round by round. A run takes a few minutes.
3. **Read the answer.** You get a short report with the likely outcome, how confident it is, and the posts that back it up. You can then ask any of the simulated people why they said what they said.

## What you need

- A Mac with Apple silicon, running macOS 15 or later
- An AI tool on your Mac, installed and signed in: [Claude Code](https://claude.com/claude-code), [Codex](https://developers.openai.com/codex/cli), [Gemini CLI](https://github.com/google-gemini/gemini-cli), [Cursor](https://cursor.com/cli), [Ollama](https://ollama.com) and more. Tellfold uses it to run the simulated people, so there is no extra account. On first launch it finds the tools you have and suggests one; change it any time in Settings › Agents. Bigger runs use more of your plan, and Ollama runs free models on your Mac.

To install, download `Tellfold.dmg`, open it and drag Tellfold to Applications. The app updates itself (Tellfold › Check for Updates…).

## Your privacy

Tellfold has no account and no server. Your documents go only to the AI tool you choose, and every run stays on your Mac.

## Good to know

- The people are simulated by AI, not real. They tend to be more polite and more reasonable than a real crowd, so treat the result as a rehearsal, not a survey.
- A run is a small group, not a whole town or the internet.
- Results vary from run to run. If a decision matters, run it two or three times.
- The crowd only knows what you give it, plus general knowledge. If an important fact or group is missing from your text, it will be missing from the simulation.

## Support Tellfold

Tellfold is free to download and use. If it helped you, [sponsoring it on GitHub](https://github.com/sponsors/NarekManukyan) pays for the time that goes into it. Questions and bug reports are welcome in [Issues](https://github.com/NarekManukyan/tellfold/issues).

Developers who want to build from source or use the command line can read [docs/DEVELOPMENT.md](docs/DEVELOPMENT.md).

Tellfold can also be used from other tools. [docs/MCP.md](docs/MCP.md) sets it up as an MCP server for Claude Code, Claude Desktop and Cursor, so an assistant can test a changelog or an announcement for you, and [integrations/claude-code](integrations/claude-code/README.md) has a ready-made Claude Code skill. [docs/API.md](docs/API.md) documents the local HTTP API the app and the MCP server use.

## Licence

Copyright © 2026 Narek Manukyan. All rights reserved. The source is public so you can read and check it. No licence is granted to copy, change or redistribute it.
