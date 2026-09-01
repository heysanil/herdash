//! Classifying a reasoning refusal. Getting this wrong is expensive in both
//! directions: too narrow and a provider never escalates; too broad and an
//! unrelated 400 is retried twice at cost, then reported as a reasoning
//! failure the user cannot act on.

use herdash::summary::client::is_reasoning_rejection;

#[test]
fn a_genuine_refusal_escalates() {
    assert!(is_reasoning_rejection(
        400,
        true,
        "Reasoning is mandatory for this endpoint"
    ));
    assert!(is_reasoning_rejection(
        400,
        true,
        "reasoning cannot be disabled for this model"
    ));
    assert!(is_reasoning_rejection(
        400,
        true,
        "Unrecognized request argument supplied: reasoning_effort"
    ));
    assert!(is_reasoning_rejection(
        400,
        true,
        "Unsupported parameter: 'reasoning_effort' is not supported with this model"
    ));
    assert!(is_reasoning_rejection(
        400,
        true,
        "thinking.type: disabled is not supported by this model"
    ));
    assert!(is_reasoning_rejection(
        400,
        true,
        "output_config.effort is not supported for claude-haiku-4-5"
    ));
}

#[test]
fn an_unrelated_400_is_not_retried() {
    // Without this gate, a typo'd model name would be retried twice at cost
    // and then reported as a reasoning problem.
    assert!(!is_reasoning_rejection(400, true, "model not found: gpt-o"));
    assert!(!is_reasoning_rejection(400, true, "invalid api key"));
    assert!(!is_reasoning_rejection(
        400,
        true,
        "max_tokens must be greater than 0"
    ));
}

#[test]
fn a_rung_that_sent_no_reasoning_field_cannot_have_been_refused_for_one() {
    assert!(!is_reasoning_rejection(
        400,
        false,
        "Reasoning is mandatory for this endpoint"
    ));
}

#[test]
fn server_errors_and_rate_limits_are_not_reasoning_refusals() {
    assert!(!is_reasoning_rejection(500, true, "reasoning is mandatory"));
    assert!(!is_reasoning_rejection(429, true, "reasoning is mandatory"));
    assert!(!is_reasoning_rejection(
        502,
        true,
        "reasoning not supported"
    ));
}
