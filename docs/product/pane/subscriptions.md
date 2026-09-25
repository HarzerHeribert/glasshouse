# Subscriptions Pane can sign in to

Read 2026-09-25. Pane signs in through CLIProxyAPI (pinned by the gateway);
the risk is the provider's terms, and CLIProxyAPI presents itself as each
vendor's own client, which Anthropic and Kimi name as the offence. The
wizard (`/login` → subscription) shows each warning before signing in and
signs in only after "Sign in anyway" (`session/controls.rs::SUBSCRIPTIONS`).

| Subscription | Login | Risk | Terms, with source |
|---|---|---|---|
| ChatGPT | `-codex-login`, `-codex-device-login` | low | OpenAI endorses third-party coding tools ([Codex for OSS](https://developers.openai.com/community/codex-for-oss)) |
| Grok | `-xai-login` | low | xAI supports SuperGrok / X Premium+ sign-in in third-party tools ([x.ai, 2026-05-21](https://x.ai/news/grok-opencode)) |
| Kimi | `-kimi-login` | low–medium | personal use in third-party tools; guides use an API key ([Kimi help](https://www.kimi.com/en/help/kimi-code/third-party-agents)) |
| Claude | `-claude-login` | high | forbidden, and enforced since 2026-01 ([legal and compliance](https://code.claude.com/docs/en/legal-and-compliance), [VentureBeat](https://venturebeat.com/technology/anthropic-cracks-down-on-unauthorized-claude-usage-by-third-party-harnesses)) |
| Gemini (Antigravity) | `-antigravity-login` | high | forbidden; suspension, permanent ban on a second strike ([Antigravity FAQ](https://antigravity.google/docs/faq/), [The Register](https://www.theregister.com/2026/02/23/google_antigravity_compute_burden/)) |
| Devin | `-devin-login` | medium | not addressed ([Cognition terms](https://cognition.com/legal/platform-terms-of-service)) |
| Muse Code | `-meta-login` | medium | terms not found; a data-sharing tier may train on code (unverified) |

Removed from CLIProxyAPI: Gemini CLI (2026-06-19), Qwen Code (2026-04-15),
iFlow (2026-04-17). Never supported: GitHub Copilot, Kiro.
