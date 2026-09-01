//! Wire-agnostic prompt text, response schema, and token budgets.
//!
//! Both codecs need these. Nothing here knows which provider it is talking
//! to — the vendor wrapper around the schema is the codec's job.

use serde_json::{Value, json};

/// Transcripts are clamped before sending. Generous enough for 200 lines of
/// agent output, small enough that a runaway pane cannot blow up a request.
pub const TRANSCRIPT_MAX_BYTES: usize = 12 * 1024;

pub const AGENT_SYSTEM: &str = "\
You summarize a coding agent's terminal transcript for a status dashboard. \
The transcript contains terminal chrome — status lines, progress bars, token \
counters, spinners, box-drawing characters. Ignore all of it and describe only \
the actual work. Write plainly, in the third person, with no preamble. \
\
Fields:\n\
- `headline`: at most 60 characters, the gist at a glance.\n\
- `task`: the overall objective and its context, as a short paragraph of two \
to four sentences. Say what is being built or fixed, in which area of the \
codebase, and why — enough that a reader who has not looked at this agent in \
an hour can pick the thread back up.\n\
- `now`: what the agent is doing at this exact moment, in one to three \
sentences. Be concrete: name the file, command, test or error it is on.\n\
- `recent`: up to six recently completed steps, newest first, one short \
clause each.\n\
- `needs_attention`: true ONLY if the agent is blocked on the human right now. \
That means it asked the human a direct question, is sitting at an approval or \
permission prompt, or hit an error it cannot resolve without a decision. \
\
It is FALSE in all of these cases, which look similar but are not: the agent \
is busy, thinking, or running a long command; the agent finished its work \
cleanly and the pane is merely idle; the agent proposed or suggested a next \
step without asking anything; the agent said no action is needed; the human \
has half-typed something into the prompt but not sent it. An idle pane is not \
by itself a request — most idle agents want nothing. Only flag an agent whose \
transcript contains an actual unanswered ask directed at the human.\n\
- `attention_reason`: if `needs_attention` is true, at most 70 characters \
addressed to the human as a direct instruction, starting with a verb. Say what \
they must do or decide. Do not begin with \"The agent\", do not describe the \
agent's state, and do not pad with words like \"needs\" or \"is waiting for\". \
Good: \"Approve the Figma authorization\". \"Decide whether to write to the \
seeded branch\". \"Answer its question about the summarize helper\". \
Bad: \"The agent needs Figma authorization to proceed with the task\". \
Empty string otherwise.\n\
\
Respond only with the JSON object.";

pub const FLEET_SYSTEM: &str = "\
You summarize a fleet of coding agents for a dashboard header. Given one line \
per agent, write one or two sentences describing what the fleet is collectively \
doing and naming any agent that is waiting on the human. Be specific and terse. \
No preamble, no bullet points, no markdown.";

/// Keep the tail of an over-long transcript, cut on a UTF-8 boundary and
/// preferably at a line break. Recent output is what matters.
pub fn clamp_transcript(text: &str, max_bytes: usize) -> &str {
    if text.len() <= max_bytes {
        return text;
    }
    let mut start = text.len() - max_bytes;
    while start < text.len() && !text.is_char_boundary(start) {
        start += 1;
    }
    let tail = &text[start..];
    match tail.find('\n') {
        Some(i) if tail.len() > i + 1 => &tail[i + 1..],
        _ => tail,
    }
}

/// The summary schema, with no vendor wrapper.
///
/// OpenAI nests this under `response_format.json_schema` and requires a
/// sibling `name` and `strict`; Anthropic nests it under
/// `output_config.format` and accepts neither. So the wrapper belongs to the
/// codec and only the schema itself is shared.
pub fn summary_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "headline": { "type": "string" },
            "task": { "type": "string" },
            "now": { "type": "string" },
            "recent": { "type": "array", "items": { "type": "string" } },
            "needs_attention": { "type": "boolean" },
            "attention_reason": { "type": "string" }
        },
        "required": [
            "headline", "task", "now", "recent",
            "needs_attention", "attention_reason"
        ],
        "additionalProperties": false
    })
}

/// Output budget for one agent summary.
///
/// `reasons` is true when the active reasoning rung lets the model think.
/// **Reasoning tokens are charged against this same budget**, so a cap tuned
/// for reasoning-disabled can be consumed entirely by thinking, returning
/// empty content with `finish_reason: "length"` — a call you still pay for.
/// That is the kimi-k2.6 failure `ReasoningMode` exists to avoid, and it
/// returns the moment a dialect starts above `Disabled`.
pub fn agent_max_tokens(reasons: bool) -> u32 {
    if reasons { 2400 } else { 900 }
}

/// Output budget for the fleet overview. The prose is two sentences, so the
/// reasoning-disabled cap is small — and correspondingly the most exposed to
/// being eaten whole by reasoning tokens.
pub fn fleet_max_tokens(reasons: bool) -> u32 {
    if reasons { 1200 } else { 200 }
}
