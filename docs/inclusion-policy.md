# Inclusion policy

agent-radar lists a tool only if it meets every rule below. A suggestion that does not is closed with the rule it fails. The [suggestion form](https://github.com/Asugawara/agent-radar/issues/new?template=suggest-tool.yml) asks for what each rule needs.

日本語: [inclusion-policy.ja.md](inclusion-policy.ja.md)

## Required

1. **In scope.** A tool for building, running, extending or operating AI agents. A tool that lets agents drive a specific app (for example an MCP server for Blender) is in scope. A service for one industry or its data (for example data-center site selection) is not.
2. **Usable now.** The official site or repository is public and works. It is not waitlist-only or demo-only. If it has a repository, the repository is not archived and was updated within the last 12 months.
3. **Follows AI providers' terms.** The tool does not use an AI provider's subscription in a way the provider forbids. Anthropic ([terms](https://code.claude.com/docs/en/legal-and-compliance)) and Google for Gemini CLI ([terms](https://geminicli.com/docs/resources/tos-privacy/)) forbid third-party tools from doing any of the following. This rule covers the tool's official plugins too.
   - Offering its own sign-in to a Claude or Gemini CLI subscription.
   - Collecting, storing or relaying subscription tokens.
   - Posing as the official CLI: a borrowed OAuth client ID, or a faked user agent, system prompt or TLS fingerprint.

   These are fine: running the unmodified official CLI or SDK (`claude`, `codex`, `gemini`) under the user's own sign-in, API keys, and a provider's official program for third-party apps (for example Sign in with ChatGPT). Providers that have published no such rule (OpenAI as of November 2026) are not covered until they do.
4. **Safe to install.** No install-time scripts that do more than build the tool. No obfuscated code. No collection of credentials or data beyond what the tool is for. Documented telemetry is fine.
5. **Not already listed.** Not the same product as a tool already on agent-radar. Search the site first.

## Expected for new tools

6. **In use.** At least one of these: 100 or more GitHub stars, 1,000 or more weekly downloads of its package, or an official repository of an AI provider. A tool already listed is not removed for falling below this.

## Also

- You may suggest your own tool. Say so in the form; it is judged by the same rules.
- Any license is fine. A tool is marked open source (`open_source = true`) only with an OSI-approved license.
- Paid tools are fine if people can sign up now.
- Rules 1 to 5 apply to tools already listed, too. A listed tool found to break one is removed.
