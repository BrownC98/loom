<h1 align="center">
  <img src="docs/images/icon.png" alt="" width="64" valign="middle"> loom
</h1>

<p align="center">
  <a href="https://github.com/BrownC98/loom/releases/latest"><img src="https://img.shields.io/github/v/release/BrownC98/loom?style=flat&color=2E5BD6" alt="Latest release"></a>
  <img src="https://img.shields.io/badge/license-MIT-2E5BD6?style=flat" alt="License: MIT">
  <img src="https://img.shields.io/badge/Windows%20%7C%20macOS%20%7C%20Linux-2E5BD6?style=flat" alt="Platforms: Windows, macOS and Linux">
</p>

<p align="center">
  <b>English</b> · <a href="README.ko.md">한국어</a>
</p>

<p align="center">
  <strong>loom is a desktop and web app that runs AI coding agents (Claude Code, Codex, OpenCode 2, Cursor CLI) as one team across git worktrees, and shows them as a graph of live terminals.</strong><br>
  Each agent is a node card with its real terminal. Parents spawn children, children report back, and each message shows on the edge between them.
</p>

<p align="center">
  <a href="docs/images/standard.png"><img src="docs/images/standard.png" alt="loom in Standard view: a three-level graph of seven nodes, and each card is a live terminal. A Claude Code lead on main has two coordinators (Codex auth, Claude Code ui), each in its own worktree group with two workers (Claude Code, Codex, OpenCode 2). The repository tree is open on the left and the commit graph is open on the right." width="960"></a>
</p>

## Features

| Feature | What it does |
|---|---|
| **Live terminals as nodes** | Each agent runs in a real TUI. Click a card to type into it. Double-click to enlarge it. |
| **Mix agent CLIs** | Different agents work in one tree. Each card head shows the CLI and the model. |
| **Small messaging CLI** | Agents use `loom spawn`, `send`, `wait` and `done` to split work and report results. |
| **Worktree groups** | Nodes are grouped by git worktree. `--worktree <branch>` makes a new group. |
| **Commit graph and event console** | See the commits of all branches and each message between nodes. |

<p align="center">
  <a href="docs/images/hero.png"><img src="docs/images/hero.png" alt="loom in Simple view: the same seven nodes as compact cards with the summary, model and state. The edges show messages lead → auth, auth → tests and form → ui. The right panel shows the live terminal of auth with a loom message header." width="960"></a><br>
  <em>Simple view: compact cards show each summary, and the side panel shows one live terminal.</em>
</p>

## How agents talk

An agent runs these commands in its own terminal. loom gives each node its address and token.

```sh
loom spawn tests --agent codex --file brief.md   # make a child in this worktree
loom send tests "Please stand by for review."    # queue a message
loom wait --json                                 # wait for the next message
loom done "Added one toggle() test"              # report to the parent
```

## Built-in skills

loom ships five skills: `o-mode` (orchestration mode), `ste-100`, `user-doc`, `raptor-3` and `ask-human`. You do not install anything.

- Call a skill by name. Claude Code: `/loom:<name>`. Codex: `loom:<name>`. OpenCode 2: `<name>`.
- Always on, without a call: `ask-human` (how an agent asks you: one line in the question inbox), STE-100 and the raptor-3 work rules go to every node. You can turn off all but `ask-human` in **Settings › Agents › Built-in skills**. A node without a parent (a node you start) also gets the full `o-mode` procedure and acts as the lead. A child gets only the common brief rules of `o-mode`.
- A node reads the skill text when it starts. After you update loom, restart running nodes to give them the new text.

Details (Korean): [docs/design.md 「내장 스킬」](docs/design.md#내장-스킬).

## Install

Get the files from the [latest release](https://github.com/BrownC98/loom/releases/latest).

- **Windows:** run `loom_<version>_x64-setup.exe`. To remove loom, use Windows **Settings › Apps**.
- **macOS (Apple Silicon):** open `loom_<version>_aarch64.dmg` and drag loom to the Applications folder. The app has no Apple signature, so macOS says it "is damaged". Before you open it the first time, run this command in Terminal:

  ```sh
  xattr -cr /Applications/loom.app
  ```

  To remove loom, move it from the Applications folder to the Trash.
- **Linux and WSL (x86-64), web version:** see "Web version" below.

Install the agent CLIs (Claude Code, Codex, OpenCode 2, Cursor CLI) yourself. loom finds them on `PATH`.

### Web version (Linux and WSL)

One daemon also serves the screen. You use it in a browser. The release has only an x86-64 Linux binary; on macOS and Windows, use the desktop app.

1. Download the binary and save it with the name `loom`. Agents in nodes call the `loom` command, so the name must be `loom`. The daemon puts its own folder first on each node's `PATH`.

   ```sh
   mkdir -p ~/loom/bin
   curl -fL -o ~/loom/bin/loom https://github.com/BrownC98/loom/releases/latest/download/loom-x86_64-linux
   chmod +x ~/loom/bin/loom
   ```

2. Start the daemon in a repository folder. The first folder becomes the first repository.

   ```sh
   cd <repository folder>
   ~/loom/bin/loom serve            # default address 127.0.0.1:7420
   ```

   To keep it running after you close the terminal: `setsid nohup ~/loom/bin/loom serve >>~/.loom/serve.log 2>&1 </dev/null &`

3. Open http://127.0.0.1:7420 in a browser. On WSL, open the same address in a Windows browser.
4. Add repositories in the left sidebar, and start agents with [New node] on a worktree group.
5. Click a card to type into its terminal. When an agent asks you something, the question shows in the **Questions** pill at the top of the graph and in the **Questions** tab of the right panel. Click it to go to that node, and answer in its terminal.

To stop the daemon, press Ctrl+C in its terminal. If it runs in the background, find its PID with `ps -eo pid,args | grep 'loom serve'` and run `kill <PID>`. Do not end all `loom` processes: each node runs in its own `loom pty-host` process, and that would end the nodes too.

- **Data:** `~/.loom` (database, logs, built-in skills). If you delete it, the node history is lost.
- **Update:** download the file again (step 1) and restart only the daemon. Nodes (agent processes) live apart from the daemon, and the new daemon attaches to them again.
- **Address:** by default only this machine can open it (loopback). loom assumes one user and has no sign-in, so do not open it to other machines with `--listen`.

Design and decisions (Korean): [docs/design.md](docs/design.md).

## Privacy policy

loom sends no data to a place that you did not choose. These are the only requests that go out:

- **Update check (desktop app):** when the window opens, loom reads `latest.json` from the releases of this repository on GitHub. It sends no personal data.
- **Usage display:** for a Claude account that you signed in to in loom settings, loom asks Anthropic (`api.anthropic.com`) for the usage of that account, with the token of that account. It sends this to no other place.
- **Agent CLIs:** the agents that you install and choose (Claude Code, Codex and others) communicate under the policy of each provider. loom adds no data to that communication.

## License

[MIT](LICENSE)
