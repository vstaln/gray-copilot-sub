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

`gray-copilot-sub` turns an existing **GitHub Copilot subscription** into a
gray provider. GitHub's OAuth device flow gets you in; the plugin mints
short-lived Copilot session tokens on demand and relays chat through
`https://api.githubcopilot.net/chat/completions`.

## Auth

```sh
copilot-sub login     # device flow: prints a code + github.com/login/device
copilot-sub status    # oauth token + session-token state
copilot-sub logout    # revoke locally
```

GitHub device flow: the plugin prints a one-time code and the verification
URL — paste the code, authorize, and the OAuth token lands in
`~/.gray/copilot/config.json` (0600). Copilot session tokens
(~30 min TTL) are minted from it per need, never stored long-term, and
never logged.

The same flow is exposed through `provider/auth/*` so the host can drive
login interactively; `refresh`/`revoke` answer honestly (`unsupported`
where Copilot's API has no equivalent).

## Models

`copilot-sub models` lists what your Copilot entitlement exposes
(`api.githubcopilot.net/models`) — typically the GPT-4o / GPT-5 family and
friends. The catalog is fetched live, not pinned.

## How it works

Same shared relay stack as the other `*-sub` providers: the host POSTs an
OpenAI-Responses request to a per-turn loopback relay; the plugin
translates to Copilot's chat-completions shape, attaches the session
token, and folds the answer back into a Responses SSE stream.

## Build from source

```sh
cargo build --release
gray plugin check ./target/release/copilot-sub
```

## License

MIT
