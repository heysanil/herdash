//! Which endpoint to talk to, and in which dialect.
//!
//! A **preset** is static table data with holes; a **resolved provider** has
//! none left. They are separate types because `herdash init` exists precisely
//! to hold the incomplete middle state.

/// The two request/response shapes. Selects a codec, nothing more.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Wire {
    OpenAi,
    Anthropic,
}

/// What actually varies between providers.
///
/// Three of the four dialects share `Wire::OpenAi` yet disagree on the
/// token-cap field, whether `temperature` is accepted, and how reasoning is
/// expressed — none of which the wire specifies. So the dialect is the
/// parameter and the wire only picks which codec consumes it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dialect {
    OpenRouter,
    OpenAiDirect,
    OpenAiGeneric,
    Anthropic,
}

/// How much chain-of-thought to ask for.
///
/// Summarization is extraction, not deduction, so reasoning buys nothing and
/// costs a great deal. But no single setting works everywhere — `Disabled` is
/// the only thing that makes some models return content at all, while other
/// endpoints reject it outright. So the client starts at the dialect's own
/// rung and escalates only on an explicit refusal, caching the answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ReasoningMode {
    Disabled,
    LowEffort,
    ProviderDefault,
}

impl ReasoningMode {
    /// The next rung to try when a provider refuses this one.
    pub fn escalate(self) -> Option<Self> {
        match self {
            Self::Disabled => Some(Self::LowEffort),
            Self::LowEffort => Some(Self::ProviderDefault),
            Self::ProviderDefault => None,
        }
    }

    /// Ordered index, so the shared cache can advance with `fetch_max`.
    pub fn index(self) -> u8 {
        match self {
            Self::Disabled => 0,
            Self::LowEffort => 1,
            Self::ProviderDefault => 2,
        }
    }

    pub fn from_index(i: u8) -> Self {
        match i {
            0 => Self::Disabled,
            1 => Self::LowEffort,
            _ => Self::ProviderDefault,
        }
    }
}

impl Dialect {
    pub fn wire(self) -> Wire {
        match self {
            Self::Anthropic => Wire::Anthropic,
            _ => Wire::OpenAi,
        }
    }

    /// gpt-5, the o-series and gpt-4.1 reject `max_tokens` with
    /// "Unsupported parameter: use 'max_completion_tokens' instead".
    /// `max_completion_tokens` also works on gpt-4o, so for OpenAI direct it
    /// is correct unconditionally. Third-party OpenAI-compatible servers
    /// generally know only `max_tokens`, so the substitution stops here.
    pub fn token_cap_field(self) -> &'static str {
        match self {
            Self::OpenAiDirect => "max_completion_tokens",
            _ => "max_tokens",
        }
    }

    /// The o-series rejects any temperature but 1, and current Claude models
    /// removed sampling parameters entirely.
    pub fn sends_temperature(self) -> bool {
        !matches!(self, Self::OpenAiDirect | Self::Anthropic)
    }

    /// Where to begin the ladder. `reasoning` is an OpenRouter extension that
    /// OpenAI direct rejects as an unknown argument, and llama.cpp-backed
    /// servers have no reasoning knob at all — starting below
    /// `ProviderDefault` there only buys a wasted round trip.
    pub fn start_rung(self) -> ReasoningMode {
        match self {
            Self::OpenRouter | Self::Anthropic => ReasoningMode::Disabled,
            Self::OpenAiDirect => ReasoningMode::LowEffort,
            Self::OpenAiGeneric => ReasoningMode::ProviderDefault,
        }
    }

    /// Whether this rung leaves the model free to think, which decides the
    /// token budget: reasoning tokens are billed against the output cap.
    pub fn reasons_at(self, rung: ReasoningMode) -> bool {
        rung != ReasoningMode::Disabled
    }
}

/// Whether a preset can run without an API key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyRequirement {
    Required,
    None,
    Optional,
}

/// Selectable on the command line.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum ProviderId {
    Openrouter,
    Openai,
    Anthropic,
    Ollama,
    Lmstudio,
    #[value(name = "openai-compatible")]
    OpenaiCompatible,
    #[value(name = "anthropic-compatible")]
    AnthropicCompatible,
}

impl ProviderId {
    /// The environment variable this provider's vendor key lives in.
    pub fn env_var(self) -> Option<&'static str> {
        match self {
            Self::Openrouter => Some("OPENROUTER_API_KEY"),
            Self::Openai => Some("OPENAI_API_KEY"),
            Self::Anthropic => Some("ANTHROPIC_API_KEY"),
            _ => None,
        }
    }

    /// Lowercase name used in messages and, later, as a credentials key.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Openrouter => "openrouter",
            Self::Openai => "openai",
            Self::Anthropic => "anthropic",
            Self::Ollama => "ollama",
            Self::Lmstudio => "lmstudio",
            Self::OpenaiCompatible => "openai-compatible",
            Self::AnthropicCompatible => "anthropic-compatible",
        }
    }

    /// How to discover model names, for the error shown when `--model` is
    /// required but absent.
    pub fn model_hint(self) -> &'static str {
        match self {
            Self::Ollama => "List installed models with `ollama list`.",
            Self::Lmstudio => "List loaded models with `lms ls`.",
            _ => "Pass --model with the provider's own model name.",
        }
    }
}

/// Static table data. `None` means the user must supply it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProviderPreset {
    pub id: ProviderId,
    pub dialect: Dialect,
    pub default_base_url: Option<&'static str>,
    pub default_model: Option<&'static str>,
    pub key: KeyRequirement,
}

/// The single source of truth for provider defaults.
pub fn preset(id: ProviderId) -> ProviderPreset {
    use Dialect as D;
    use KeyRequirement as K;
    let (dialect, base, model, key) = match id {
        ProviderId::Openrouter => (
            D::OpenRouter,
            Some("https://openrouter.ai/api/v1"),
            // Chosen by examples/bench.rs; see docs/benchmark.md.
            Some("openai/gpt-oss-120b"),
            K::Required,
        ),
        ProviderId::Openai => (
            D::OpenAiDirect,
            Some("https://api.openai.com/v1"),
            Some("gpt-5-mini"),
            K::Required,
        ),
        ProviderId::Anthropic => (
            D::Anthropic,
            Some("https://api.anthropic.com"),
            Some("claude-haiku-4-5"),
            K::Required,
        ),
        // Local model names are whatever the user pulled, so there is nothing
        // sane to guess.
        ProviderId::Ollama => (
            D::OpenAiGeneric,
            Some("http://localhost:11434/v1"),
            None,
            K::None,
        ),
        ProviderId::Lmstudio => (
            D::OpenAiGeneric,
            Some("http://localhost:1234/v1"),
            None,
            K::None,
        ),
        ProviderId::OpenaiCompatible => (D::OpenAiGeneric, None, None, K::Optional),
        ProviderId::AnthropicCompatible => (D::Anthropic, None, None, K::Optional),
    };
    ProviderPreset {
        id,
        dialect,
        default_base_url: base,
        default_model: model,
        key,
    }
}

/// Everything the client needs, with nothing left to resolve.
#[derive(Clone, PartialEq, Eq)]
pub struct ResolvedProvider {
    pub id: ProviderId,
    pub dialect: Dialect,
    pub base_url: String,
    pub api_key: Option<String>,
    pub model: String,
}

/// Manual, not derived: `api_key` must never appear in an error, notice, or
/// `Debug` output. There is no leak today — nothing formats this struct —
/// but `Settings` (`config.rs`) holds one and derives `Debug` too, and a
/// startup diagnostic is exactly the kind of addition someone reaches for
/// `Settings` to build.
impl std::fmt::Debug for ResolvedProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        /// Prints as `<redacted>` with no surrounding quotes, so
        /// `Option<Redacted>` reads as `Some(<redacted>)` / `None`.
        struct Redacted;
        impl std::fmt::Debug for Redacted {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                write!(f, "<redacted>")
            }
        }
        f.debug_struct("ResolvedProvider")
            .field("id", &self.id)
            .field("dialect", &self.dialect)
            .field("base_url", &self.base_url)
            .field("api_key", &self.api_key.as_ref().map(|_| Redacted))
            .field("model", &self.model)
            .finish()
    }
}

impl ResolvedProvider {
    fn join(&self, suffix: &str) -> String {
        format!("{}/{}", self.base_url.trim_end_matches('/'), suffix)
    }

    /// The completions endpoint. OpenAI-style base URLs already carry `/v1`;
    /// Anthropic-style ones do not.
    pub fn completions_url(&self) -> String {
        match self.dialect.wire() {
            Wire::OpenAi => self.join("chat/completions"),
            Wire::Anthropic => self.join("v1/messages"),
        }
    }

    /// The model-list endpoint, used by `herdash init` and as a liveness probe.
    pub fn models_url(&self) -> String {
        match self.dialect.wire() {
            Wire::OpenAi => self.join("models"),
            Wire::Anthropic => self.join("v1/models"),
        }
    }

    /// True only when transcripts genuinely never leave this machine.
    pub fn is_loopback(&self) -> bool {
        is_loopback_url(&self.base_url)
    }
}

/// Host-based loopback test.
///
/// The `(local)` badge is a claim about network egress, so it must be derived
/// from the resolved URL and never from a preset's name — `--provider ollama
/// --base-url http://192.168.1.50:11434/v1` is a remote endpoint wearing a
/// local preset's name.
pub fn is_loopback_url(url: &str) -> bool {
    let Some(host) = host_of(url) else {
        return false;
    };
    if host == "localhost" || host == "::1" {
        return true;
    }
    host.parse::<std::net::IpAddr>()
        .map(|ip| ip.to_canonical().is_loopback())
        .unwrap_or(false)
}

/// Scheme + host + port, ignoring path and trailing slash. Userinfo (if any)
/// is also part of the comparison, so `https://api.openai.com/v1` and
/// `https://user:pw@api.openai.com/v1` are treated as different origins.
/// That is deliberately fail-closed: when in doubt, the comparison simply
/// refuses to match rather than risk attaching a vendor key to the wrong
/// origin.
pub fn same_origin(a: &str, b: &str) -> bool {
    match (origin_of(a), origin_of(b)) {
        (Some(x), Some(y)) => x == y,
        _ => false,
    }
}

/// Minimal URL splitting. Avoids taking a `url` crate dependency for two
/// fields; anything malformed simply fails the comparisons above.
fn origin_of(url: &str) -> Option<String> {
    let (scheme, rest) = url.split_once("://")?;
    let authority = rest.split(['/', '?', '#']).next()?;
    if authority.is_empty() {
        return None;
    }
    Some(format!(
        "{}://{}",
        scheme.to_ascii_lowercase(),
        authority.to_ascii_lowercase()
    ))
}

fn host_of(url: &str) -> Option<String> {
    let (_, rest) = url.split_once("://")?;
    let authority = rest.split(['/', '?', '#']).next()?;
    let authority = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
    // Bracketed IPv6 literal, e.g. [::1]:11434. The bracket must close the
    // host: anything between `]` and the port is malformed, and accepting it
    // would let `[::1].evil.test` masquerade as loopback.
    if let Some(inner) = authority.strip_prefix('[') {
        let (host, rest) = inner.split_once(']')?;
        if !(rest.is_empty() || rest.starts_with(':')) {
            return None;
        }
        return Some(host.to_ascii_lowercase());
    }
    let host = authority.split(':').next()?;
    if host.is_empty() {
        None
    } else {
        Some(host.to_ascii_lowercase())
    }
}
