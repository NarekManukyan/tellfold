# Tellfold for Claude Code

Two pieces let Claude Code ask Tellfold how people will react to a changelog, a pricing change or any announcement:

- the **MCP server** (`tellfold mcp`), which gives Claude the tools to start a simulation, wait for it and read the
  report;
- the **`tellfold-simulate` skill**, which tells Claude when to use them and how: what to pass, what size of run to
  start, and how to summarise the result.

The full tool list and the setup for Claude Desktop and Cursor are in [docs/MCP.md](../../docs/MCP.md).

## 1. Get the `tellfold` binary

The `mcp` command ships in the Tellfold releases after 0.4.0. The Mac app carries the binary:

```sh
/Applications/Tellfold.app/Contents/Resources/engine/tellfold --version
```

Until that release, or on Linux, build it from this repository:

```sh
cd engine-rs
cargo build --release -p tellfold-cli
# the binary is engine-rs/target/release/tellfold
```

Tellfold runs the simulated people on an AI tool you already have installed and signed in to, such as the Claude Code
CLI. `tellfold runtimes` lists what it finds; the choice is saved in the Mac app's Settings › Agents (or
`~/Library/Application Support/Tellfold/settings.json`).

## 2. Add the MCP server

For every project (user scope):

```sh
claude mcp add --scope user tellfold -- /Applications/Tellfold.app/Contents/Resources/engine/tellfold mcp
```

Use the path of your own build if you built it. `claude mcp list` should then show `tellfold` as connected. Runs
are saved in the Mac app's runs folder, so they also show up in the app.

## 3. Add the skill

Copy the skill folder into your personal skills (or into a project's `.claude/skills/` to share it with the team):

```sh
mkdir -p ~/.claude/skills
cp -R integrations/claude-code/skills/tellfold-simulate ~/.claude/skills/
```

## 4. Try it

Start Claude Code in a project with a changelog and ask:

> Simulate how our users will react to the 3.0 entry in CHANGELOG.md.

Claude starts a small run, waits for it (a few minutes), and sums up the report: the likely reaction, the main
objection with the posts behind it, and edits worth making. Then try:

> Rewrite it to lead with what stays free, and compare the two versions on the same crowd.
