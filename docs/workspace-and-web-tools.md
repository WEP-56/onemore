# Workspace and Web tools

This phase follows the bounded-tool patterns used by mature Rust coding agents while keeping
Onemore's provider and runtime boundaries explicit.

## Workspace tools

The default registry includes `glob`, `search`, `repo_state`, and `git_diff`.

- `glob` and `search` use ripgrep's underlying Rust crates. They honor repository ignore files,
  avoid directory symlinks, skip common generated directories, sort deterministically, and report
  truncation and skipped-file counts in structured `details`. Model-facing paths always use `/`;
  `search.context` can include 0-10 adjacent lines while matching remains line-oriented.
- `repo_state` and `git_diff` invoke Git directly without a shell. Hooks, fsmonitor, color, and
  optional locks are disabled; execution time and captured output are bounded. `git_diff.path` is
  a real repository-relative pathspec, and every patch starts with a bounded `--numstat` file and
  line-count summary so a model can request a narrower follow-up diff.
- Tool descriptions state defaults, limits, matching semantics, and side effects. Model text stays
  concise while complete counters and machine-readable fields remain in `details`.
- All local tool output passes through the shared ANSI/control-character sanitizer and context-size
  bound before it becomes a model observation.

## Web search capability

`[web].mode` accepts `auto`, `native`, `external`, or `disabled`. The selected implementation and
its settings are frozen when the provider is constructed and rebuilt by `/reload` or model/provider
selection. Hosted search is not registered as a local function tool because it executes inside the
provider. Harness-owned search registers exactly one local `web_search` tool for the selected epoch.

OpenAI Responses native search currently supports these optional settings:

```toml
[web]
mode = "auto"
external_backends = ["tavily", "brave", "exa", "serper"]
context_size = "medium" # low | medium | high
allowed_domains = ["developers.openai.com"]

[web.location]
country = "US"
region = "California"
city = "San Francisco"
timezone = "America/Los_Angeles"
```

External backend order is deterministic. `auto` prefers OpenAI Responses native search, then picks
the first configured backend with a non-empty credential. `external` requires one available backend;
`native` never falls back to a harness backend. Credentials are read once while the tool is built:

- `tavily`: `TAVILY_API_KEY`
- `brave`: `BRAVE_SEARCH_API_KEY`
- `exa`: `EXA_API_KEY`
- `serper`: `SERPER_API_KEY`

The key value is held only by the backend instance; the frozen binding, prompt identity, tool output,
and persisted facts contain only the backend kind and environment-variable name. A runtime failure is
reported against the selected backend and never switches implementations inside the session.

External searches are remote disclosures, so `web_search` uses the harness `always_ask` permission
boundary even if ordinary command permissions are configured as `allow`. Requests and responses have
fixed HTTP timeouts and byte limits. Result URLs, titles, excerpts, provider metadata, and error bodies
are sanitized and bounded before becoming `Source` records or model text. Tavily and Exa receive the
configured domain allowlist directly; all four adapters also enforce it locally on normalized result
URLs. Context tiers cap the returned sources at 3, 5, or 10.

Domain names are normalized and validated, the allowlist is limited to 100 entries, and location is
host configuration rather than model-controlled input. Provider citations accept only bounded HTTP
or HTTPS URLs. Credentials and fragments are removed; titles are sanitized, collapsed to one line,
bounded, and URLs are deduplicated before the source list is appended to assistant text.

## Deferred work

- TODO: add stable SDK/RPC DTOs for Web search started/completed/failed events, normalized sources,
  and external-context provenance. The current JSONL RPC intentionally receives only the existing
  assistant text and generic runtime events.
- TODO: expose source metadata as first-class TUI/SDK data after the event contract is defined;
  provider-specific annotations must not leak into that public contract.
- TODO: implement provider-hosted adapters beyond OpenAI Responses and add general `web_fetch`,
  crawl caching, and browser control.
