<div align="center">
  <img alt="gray-copilot-sub" src="assets/icon.svg" width="120" height="120" />
  <h1>gray-copilot-sub</h1>
  <p><strong>Use your GitHub Copilot subscription as a gray model provider.</strong></p>
  <p>
    <a href="https://gray.alignment.id">Website</a> ·
    <a href="https://github.com/vstaln/gray-copilot-sub">Source</a> ·
    <a href="https://github.com/vstaln/gray">gray</a>
  </p>
  <p>
    <a href="LICENSE"><img alt="License: MIT" src="https://img.shields.io/badge/license-MIT-1c1c20?style=flat-square&labelColor=0a0a0b" /></a>
    <a href="https://www.rust-lang.org"><img alt="Built with Rust" src="https://img.shields.io/badge/built%20with-rust-1c1c20?style=flat-square&labelColor=0a0a0b&logo=rust&logoColor=d4a373" /></a>
    <img alt="gray plugin" src="https://img.shields.io/badge/gray-plugin-1c1c20?style=flat-square&labelColor=0a0a0b&color=7aa2f7" />
  </p>
</div>

<br/>
```bash
gray plugin install copilot
```

`gray-copilot-sub` runs your **GitHub Copilot subscription** through the
official **GitHub Copilot CLI**. The plugin starts `copilot --acp --stdio`, the
CLI's documented [Agent Client Protocol](https://docs.github.com/en/copilot/reference/copilot-cli-reference/acp-server)
server, and relays gray chat turns to it. The CLI does the sign-in and all
model traffic. The ACP server is in public preview and may change.

## Requirements

- GitHub Copilot CLI on `PATH` (npm package `@github/copilot`), or set
  `GRAY_COPILOT_SUB_COMMAND=/path/to/copilot`.
- Signed in with the CLI: run `copilot login` in a terminal (or `/login` inside
  Copilot CLI). The CLI stores its token in the OS keychain, or in
  `~/.copilot/config.json` when no keychain is available.

## What the plugin does and does not do

- **Reads no tokens.** It does not open the CLI's keychain entry or
  `~/.copilot/config.json`. It does not read `GH_TOKEN` / `GITHUB_TOKEN`,
  and it does not run its own OAuth or device login. `copilot login` runs
  only in your terminal.
- **Calls no internal endpoints.** Every request goes through the official
  `copilot --acp` process. The plugin never calls `api.githubcopilot.net`
  or `api.github.com/copilot_internal`.
- **Sends no spoofed client identity.** It sends no editor headers and
  does not pose as VS Code or Copilot Chat.
- **Keeps the model inert.** The CLI's built-in tools (shell, file edit and
  read, web fetch, subagents, and `ask_user`) are excluded at server start with
  `--excluded-tools=…` and `--no-ask-user`. Any permission request the CLI
  still raises is refused, except for gray's own `mcp__gray__*` tools.
  `COPILOT_ALLOW_ALL` is forced to `false` for the child.
- **Lists models from the CLI.** Models come from the ACP `session/new`
  response. `GRAY_COPILOT_SUB_MODELS` is an explicit fallback. Nothing is
  hardcoded.
- **Reports no quota.** The CLI publishes no usage counter over ACP, so the
  usage card shows connection status only.

## Commands

```sh
copilot-sub status    # CLI path and version, sign-in state, legacy-file notice
copilot-sub models    # models the CLI reports
copilot-sub login     # prints the command to run (does not sign in itself)
copilot-sub logout    # prints the sign-out instruction (/logout in Copilot CLI)
```

Inside gray: `/copilot status`.

## Environment

| Variable | Default | Meaning |
| --- | --- | --- |
| `GRAY_COPILOT_SUB_COMMAND` | `copilot` on `PATH` | Binary override |
| `GRAY_COPILOT_SUB_MODELS` | unset | Comma-separated model ids, used only when the CLI reports none |

## Legacy login (upgrade notice)

Versions before 0.1.1 ran their own GitHub device login with a VS Code client
ID and stored the resulting token in `~/.gray/copilot/config.json`. That
approach impersonated VS Code, so it is retired. The current version never
reads that file. `copilot-sub status` reports it while it is present. Delete the
file yourself, and remove the GitHub OAuth authorization you granted for that
login. Nothing is deleted automatically.

## Verification status

The tests use a fake `copilot --acp` (a Python stand-in) over the real ACP
transport, because the Copilot CLI was not installed when this was built. The
exact auth-error text and the `session/new` models shape of the real CLI are
therefore not verified. Run `copilot-sub status` and `copilot-sub models` against
the real CLI before relying on it.

## Build from source

```sh
cargo test
cargo build --release
gray plugin check ./target/release/copilot-sub
```

## License

MIT
