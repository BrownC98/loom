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
- **Web version (Linux, WSL, macOS, Windows, no app needed):** see "Web version" below.

Install the agent CLIs (Claude Code, Codex, OpenCode 2, Cursor CLI) yourself. loom finds them on `PATH`.

### Web version (every OS)

One daemon also serves the screen. You use it in a browser. You can install it without the app. If you also use the app, the app window and the browser tab show the same daemon: the same nodes, card places, settings and terminals.

1. Run one line. It downloads loom, puts a loom icon on the desktop, and opens loom in the browser.
   - Linux, WSL and macOS: in a terminal (on WSL, a WSL terminal)

     ```sh
     curl -fsSL https://raw.githubusercontent.com/BrownC98/loom/main/scripts/install.sh | sh
     ```

     If the loom app is on the Mac, the script uses the loom in the app and downloads nothing.
   - Windows: in PowerShell

     ```powershell
     irm https://raw.githubusercontent.com/BrownC98/loom/main/scripts/install.ps1 | iex
     ```

     If you installed the Windows app, **loom (browser)** in the Start menu does the same.
2. After that, click the **loom** icon on the desktop. If loom is not running, the icon starts it and opens the browser. You do not need a terminal.
3. When you close the browser tab, loom continues to run. To stop it, run `loom stop`. Nodes (agents) do not stop, and loom attaches to them again at the next start.
4. Add repositories in the left sidebar, and start agents with [New node] on a worktree group.
5. Click a card to type into its terminal. When an agent asks you something, the question shows in the **Questions** pill at the top of the graph and in the **Questions** tab of the right panel. Click it to go to that node, and answer in its terminal.

Manual install: download the file for your OS from the release (`loom-web-linux-x64`, `loom-web-macos-arm64`, `loom-web-windows-x64.exe`), rename it to `loom` (`loom.exe` on Windows), then run `loom open`. The name must be `loom`, because agents in nodes call the `loom` command. Do not end all `loom` processes: each node runs in its own `loom pty-host` process, and that would end the nodes too.

- **Data:** `~/.loom` (`%APPDATA%\loom` on Windows) holds the database, logs and built-in skills. If you delete it, the node history is lost. Only one daemon runs on one machine.
- **Update:** when a new version is out, a card shows at the bottom right. Click [Update]: loom downloads it, replaces itself and starts again. **Settings › About** shows the version, and [Check for updates] checks now. If the daemon is the loom inside the app, the app installs the update. Nodes (agent processes) live apart from the daemon, and the new daemon attaches to them again. You can also run the line in step 1 again.
- **Address:** by default only this machine can open it (loopback). loom assumes one user and has no sign-in, so do not open it to other machines with `--listen`.

Design and decisions (Korean): [docs/design.md](docs/design.md).

## Privacy policy

loom sends no data to a place that you did not choose. These are the only requests that go out:

- **Update check (desktop app):** when the window opens, loom reads `latest.json` from the releases of this repository on GitHub. It sends no personal data.
- **Usage display:** for a Claude account that you signed in to in loom settings, loom asks Anthropic (`api.anthropic.com`) for the usage of that account, with the token of that account. It sends this to no other place.
- **Agent CLIs:** the agents that you install and choose (Claude Code, Codex and others) communicate under the policy of each provider. loom adds no data to that communication.

## License

[MIT](LICENSE)
