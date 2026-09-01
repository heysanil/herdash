# Configuring a provider

herdash speaks two wire protocols — OpenAI v1 Chat Completions and the
Anthropic Messages API — so it reaches any endpoint that implements either.
`--provider` picks a preset; `--base-url` overrides the endpoint.

| `--provider` | Endpoint | Key | Default model |
| --- | --- | --- | --- |
| `openrouter` (default) | `https://openrouter.ai/api/v1` | `$OPENROUTER_API_KEY` | `openai/gpt-oss-120b` |
| `openai` | `https://api.openai.com/v1` | `$OPENAI_API_KEY` | `gpt-5-mini` |
| `anthropic` | `https://api.anthropic.com` | `$ANTHROPIC_API_KEY` | `claude-haiku-4-5` |
| `ollama` | `http://localhost:11434/v1` | none | required |
| `lmstudio` | `http://localhost:1234/v1` | none | required |
| `openai-compatible` | `--base-url` required | optional | required |
| `anthropic-compatible` | `--base-url` required | optional | required |

`$HERDASH_API_KEY` overrides every provider-specific variable.

**Base URLs follow each vendor's own convention**: OpenAI-style URLs include
`/v1`, Anthropic-style URLs do not. This is deliberate, not an inconsistency
to fix — it matches each vendor's own docs, so a URL you paste from them
works unmodified.

## OpenRouter

```sh
export OPENROUTER_API_KEY=sk-or-...
herdash
```

The default, and the only provider whose models are benchmarked — see
[benchmark.md](benchmark.md). `--model openai/gpt-oss-20b:nitro` is 4.6x
cheaper with identical attention accuracy.

## OpenAI

```sh
export OPENAI_API_KEY=sk-...
herdash --provider openai --model gpt-5-mini
```

herdash sends `max_completion_tokens` rather than `max_tokens`, because
gpt-5, the o-series and gpt-4.1 all reject `max_tokens`. It also omits
`temperature` for this provider entirely: only the o-series actually rejects
it (any value but `1`), but sending it to some models and not others is more
surface than it's worth, so herdash leaves it out dialect-wide.

## Claude platform

```sh
export ANTHROPIC_API_KEY=sk-ant-...
herdash --provider anthropic --model claude-haiku-4-5
```

No sampling parameters are sent — they were removed on current models.
`claude-haiku-4-5` is the cheapest current model and ample for summarization.

## LM Studio (local)

Start the server (**Developer → Start Server**, or `lms server start`), load a
model, then:

```sh
herdash --provider lmstudio --model qwen/qwen3-8b
```

No key needed, and the header shows `summaries on · lmstudio (local)` —
transcripts never leave the machine, which is the strongest answer to the
[privacy warning](../README.md#privacy) a networked provider carries. Models
below roughly 7B are documented as unreliable at structured output; if
summaries fail to parse, try a larger one.

## Ollama (local)

```sh
ollama pull qwen3
herdash --provider ollama --model qwen3
```

Same zero-egress guarantee as LM Studio — the header shows
`summaries on · ollama (local)`.

Self-hosted Ollama 0.5.0+ enforces the JSON schema through llama.cpp's
grammar-constrained decoder. **Ollama Cloud accepts the schema but does not
enforce it**, so cloud models fail with `model did not return the requested
schema` — use a locally-pulled model.

## Any other endpoint

```sh
herdash --provider openai-compatible --base-url http://gateway.internal/v1 --model llama-70b
herdash --provider anthropic-compatible --base-url https://gateway.internal --model claude-proxy
```

Works with vLLM, LiteLLM, and corporate gateways. `openai-compatible` sends
no reasoning field — third-party OpenAI-wire servers generally have no such
knob. `anthropic-compatible` shares the Claude platform's dialect, so it
negotiates reasoning the same way: it starts each request with
`"thinking": {"type": "disabled"}` and only stops sending it if the endpoint
refuses that field outright.

**Credentials are bound to a provider's own origin.** If `--base-url` points a
vendor preset somewhere else, herdash will not forward the stored vendor key —
pass `HERDASH_API_KEY` to send a key to a custom endpoint deliberately.
