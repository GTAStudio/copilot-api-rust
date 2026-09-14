# Client Configuration

Verified against the official protocol/configuration documentation on 2026-09-14.
Local HTTP contract tests are not a substitute for signing in and testing your
actual account, organization policies, model entitlements, and desktop version.

## Server

Build with Rust 1.92 or newer. From the repository root:

```powershell
cargo build --manifest-path rust-server/Cargo.toml --release --locked
cargo build --manifest-path gui-slint/Cargo.toml --release --locked
```

The second build embeds the first executable. For a different Cargo target
directory, set `COPILOT_SERVER_PATH` to the intended server executable before
building the GUI. Release GUI builds fail if no server is available.

Select **English** in the GUI's top-right language menu, then use
**Connection > Sign In to GitHub Copilot**, or:

```powershell
.\rust-server\target\release\copilot-api-server.exe auth
.\rust-server\target\release\copilot-api-server.exe start --host 127.0.0.1 --port 4141
```

Only use an account and models you are authorized to access. The Copilot upstream
uses internal GitHub endpoints and can change independently of this project.
This project does not provide subscriptions, bypass policy, or guarantee access
to every model listed in a client's built-in picker.

### Local Gateway Authentication

The default loopback listener permits native local clients without a gateway key.
Browser-origin requests are rejected, and raw upstream tokens are never served
over HTTP. This is not a security boundary against other programs running as you.

For authenticated operation, provide `COPILOT_API_KEY` to the server process and
the same value to your client using its secret storage. It must contain 16 to 512
printable ASCII characters without spaces. Generate a key with your secret
manager, or generate a local random value in PowerShell:

```powershell
$env:COPILOT_API_KEY = [Guid]::NewGuid().ToString('N')
```

Do not generate a different value for each client. The variable belongs to the
current shell and its child processes; a new terminal does not inherit it.
Never put real keys into tracked project settings. Both `Authorization: Bearer`
and `x-api-key` are accepted. When both are sent they must agree.

Non-loopback binding is refused without a key. For remote use, put the server
behind an authenticated HTTPS reverse proxy, enable a suitable `--rate-limit`,
and restrict network access. This server does not terminate TLS itself.

### Providers And Proxies

- `COPILOT_PROVIDER=copilot`: GitHub device authorization or `COPILOT_GITHUB_TOKEN`.
- `COPILOT_PROVIDER=anthropic`: `ANTHROPIC_API_KEY`; optional `ANTHROPIC_BASE_URL`.
- `COPILOT_PROVIDER=openai`: `OPENAI_API_KEY`; optional `OPENAI_BASE_URL`.
- `COPILOT_PROVIDER=azure`: `AZURE_OPENAI_ENDPOINT`, `AZURE_OPENAI_KEY`, and
  `AZURE_OPENAI_DEPLOYMENT`; optional `AZURE_OPENAI_API_VERSION`.

Messages clients require the Copilot or Anthropic provider. OpenAI and Azure use
Chat Completions, Responses, and Embeddings endpoints; their selection no longer
silently falls back to Copilot. Azure Responses uses `/openai/v1/responses`.

The GUI's **Connection / 服务连接** page has an explicit provider selector. Select
`anthropic` for a custom Anthropic gateway even when its hostname is not
`api.anthropic.com`; select `openai` for an OpenAI-compatible gateway. `auto`
retains legacy configuration inference and is not recommended for custom hosts.
The saved provider, upstream URL/key, and request interval control the child
server; unrelated inherited provider credentials are removed. Manual request
approval remains an interactive server CLI feature, not a GUI workflow.

`HTTP_PROXY`, `HTTPS_PROXY`, `ALL_PROXY`, and `NO_PROXY` are honored. HTTP and
SOCKS5 proxy support is compiled in. `COPILOT_DISABLE_PROXY=1` disables proxy use.
GUI proxy settings apply to both authentication and server processes. Redirects
are disabled for credential-bearing upstream requests. HTTP upstream base URLs
are allowed only for loopback hosts; other upstreams must use HTTPS.

Optional absolute directory overrides support isolated profiles and tests:
`COPILOT_DATA_DIR` (server credentials), `COPILOT_GUI_CONFIG_DIR` (GUI settings),
and `COPILOT_GUI_CACHE_DIR` (verified embedded-server cache). These do not move
existing files automatically. Credentials remain protected by local file/ACL
permissions, not an OS credential vault; keep these directories private.

`COPILOT_EDITOR_VERSION` and `COPILOT_EXTENSION_VERSION` can override client
compatibility headers. The editor version otherwise comes from Microsoft's
official stable-release endpoint with a bounded lookup and a fallback.

## GitHub Copilot In VS Code

Use **Chat: Manage Language Models > Add Models > Custom Endpoint**. The old
`github.copilot.chat.customOAIModels` setting is deprecated.

For a Claude model, choose the **Messages** API type and the full URL
`http://127.0.0.1:4141/v1/messages`. Use the exact model ID and limits returned by
`GET /v1/models`. Do not assert capabilities or context sizes that your upstream
does not provide. An example provider entry with secure key input is:

```json
[
  {
    "name": "Local Copilot Gateway",
    "vendor": "customendpoint",
    "apiKey": "${input:localGatewayKey}",
    "apiType": "messages",
    "url": "http://127.0.0.1:4141"
  }
]
```

When configuring models explicitly, use the model-level `url` and `apiType`.
OpenAI models can use `/v1/chat/completions` or `/v1/responses` as appropriate.
Organization BYOK policies still apply. Agent Host sessions may additionally
require `chat.agentHost.byokModels.enabled` in current VS Code versions.
BYOK model access does not replace all Copilot services such as inline completion.

## Claude Code CLI And VS Code Extension

Set the gateway address and local gateway credential, not your GitHub token:

```powershell
$env:ANTHROPIC_BASE_URL = 'http://127.0.0.1:4141'
$env:ANTHROPIC_AUTH_TOKEN = $env:COPILOT_API_KEY
$env:CLAUDE_CODE_ENABLE_GATEWAY_MODEL_DISCOVERY = '1'
claude
```

For keyless loopback mode only, use `ANTHROPIC_AUTH_TOKEN=local-only` to select
gateway authentication rather than a saved subscription credential. Do not send
a saved claude.ai OAuth token as the local gateway credential.

The GUI's **Models & Clients > Configure Claude Code** button explicitly merges gateway settings
into `CLAUDE_CONFIG_DIR/settings.json` or `~/.claude/settings.json`, preserving
unrelated keys and rejecting malformed existing documents. Starting or saving
the GUI does not rewrite Claude settings. The button intentionally updates the
Anthropic credential entries; review your configuration before using it.

For the Claude Code VS Code extension, place the variables in VS Code's
`claudeCode.environmentVariables` setting so its own login check sees them.
Values in a shell are not inherited by an editor launched from the Start menu.
Use `/status` to verify the gateway URL and credential source, then send a test
message with a model actually available to your account.

## Claude Desktop App

Current Claude Desktop third-party inference is configured separately from
Claude Code environment variables and settings files.

1. Enable Developer Mode through **Help > Troubleshooting > Enable Developer Mode**.
2. Open **Developer > Configure Third-Party Inference**.
3. Select **Gateway**, with base URL `http://127.0.0.1:4141`.
4. Select **Static API key** and **Bearer** or **x-api-key** authentication.
5. Enter the same local gateway key, or `local-only` for keyless loopback mode.
6. Use discovered Claude models, or configure exact upstream model IDs.

The relevant configuration keys are `inferenceProvider`,
`inferenceGatewayBaseUrl`, `inferenceGatewayApiKey`, and
`inferenceGatewayAuthScheme`. Administrator-managed settings take precedence.
OIDC/SSO gateway authentication is not implemented by this server; do not select
interactive gateway sign-in. Do not confuse GitHub device authorization with
Claude Desktop's gateway SSO protocol.

Do not assume Chat, Code, Cowork, remote execution, or a beta feature is enabled
merely because an HTTP request succeeds. These depend on the installed app,
upstream support, and organization configuration. No installed desktop settings
were changed and no real desktop workflow was exercised during this audit.

## Protocol Boundaries

- Native Copilot Messages is selected from upstream `supported_endpoints`.
  Anthropic Messages forwarding preserves body fields, `anthropic-*` headers,
  thinking/signature blocks, tool extensions, cache markers, and SSE pings.
  The current SDK's `system` message role is preserved. `max_tokens: 0` is forwarded
  for native cache prewarming, but is rejected by legacy generation conversion.
- Requested Claude models are never silently replaced by GPT models. Equivalent
  model spelling is matched only against the upstream model catalogue.
- Legacy conversion supports text, images, function tools, tool results, and
  streaming. Features it cannot represent are rejected instead of discarded.
  Named function choices are converted to Responses format and `strict` schemas
  survive Chat forwarding. Native cache/citation/tool-error extensions are not
  silently removed to make a legacy model appear compatible.
- `/v1/messages/count_tokens` does not require `max_tokens`. Anthropic mode uses
  native counting; other modes return an explicitly marked heuristic estimate
  (`x-token-count-estimated: true`), not an exact Claude tokenizer result.
- Safe upstream error statuses and retry delays are retained. Sensitive raw
  diagnostics are not exposed; common capability rejections use stable markers.
- Request bodies are limited to 32 MiB and translated SSE buffering to 16 MiB.
  Oversized request bodies return HTTP 413 with `request_too_large`.
- API version and model support can change upstream. Repeat client-level
  acceptance tests after updating either client or provider.

## Official References

- https://code.visualstudio.com/docs/agent-customization/language-models
- https://code.claude.com/docs/en/llm-gateway-protocol
- https://code.claude.com/docs/en/llm-gateway-connect
- https://claude.com/docs/third-party/claude-desktop/gateway
- https://platform.claude.com/docs/en/api/messages-count-tokens
- https://docs.github.com/en/apps/oauth-apps/building-oauth-apps/authorizing-oauth-apps
- https://github.com/microsoft/vscode-copilot-chat/blob/main/src/platform/endpoint/common/endpointProvider.ts
- https://github.com/anthropics/anthropic-sdk-typescript/blob/main/src/resources/messages/messages.ts