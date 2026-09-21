//! Python <-> Rust differential corpus (AGENTS §5).
//!
//! Replays `tests/fixtures/model_corpus.jsonl` through the Rust models and
//! compares each result with:
//!
//! 1. `tests/fixtures/model_expected.jsonl`, the committed pydantic output, and
//! 2. optionally a live `tools/gen_fixtures.py --stdout` run when the tau
//!    virtualenv is available (`TAU_PYTHON` overrides the interpreter path).
//!
//! Cases carrying a `divergence` field record an intentional strictness delta
//! (mostly pydantic's lax coercions, AGENTS ADR-007); those assert the recorded
//! `rust` expectation instead of parity. Everything else must match Python
//! byte for byte, including accept/reject decisions.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::Command;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::tau_agent::messages::{
    AgentMessage, AssistantDiagnosticError, AssistantMessageDiagnostic, ImageContent,
    ResponseTiming, TextContent, ThinkingContent, ToolCall, Usage, UsageCost,
};
use crate::tau_agent::provider_events::AssistantMessageEvent;
use crate::tau_agent::tools::AgentToolResult;

#[derive(Debug, Deserialize)]
struct CorpusCase {
    id: String,
    kind: String,
    input: Value,
    #[serde(default)]
    group: String,
    #[serde(default)]
    divergence: Option<String>,
    #[serde(default)]
    rust: Option<RustExpectation>,
}

#[derive(Debug, Deserialize)]
struct RustExpectation {
    accepted: bool,
    #[serde(default)]
    output: Option<String>,
}

#[derive(Debug, Deserialize)]
struct PythonExpectation {
    id: String,
    accepted: bool,
    #[serde(default)]
    output: Option<String>,
}

fn manifest_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn read_jsonl<T: DeserializeOwned>(path: &Path) -> Vec<T> {
    let text = std::fs::read_to_string(path)
        .unwrap_or_else(|error| panic!("cannot read {}: {error}", path.display()));
    text.lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            serde_json::from_str(line)
                .unwrap_or_else(|error| panic!("bad JSONL in {}: {error}\n{line}", path.display()))
        })
        .collect()
}

/// Run one corpus input through the matching Rust model.
fn run_case(kind: &str, input: &Value) -> Result<String, String> {
    fn check<T: DeserializeOwned + Serialize>(input: &Value) -> Result<String, String> {
        match serde_json::from_value::<T>(input.clone()) {
            Ok(value) => Ok(serde_json::to_string(&value).expect("serialize model")),
            Err(error) => Err(error.to_string()),
        }
    }

    match kind {
        "agent_message" => check::<AgentMessage>(input),
        "assistant_message_event" => check::<AssistantMessageEvent>(input),
        "agent_tool_result" => check::<AgentToolResult>(input),
        "usage" => check::<Usage>(input),
        "usage_cost" => check::<UsageCost>(input),
        "response_timing" => check::<ResponseTiming>(input),
        "text_content" => check::<TextContent>(input),
        "thinking_content" => check::<ThinkingContent>(input),
        "image_content" => check::<ImageContent>(input),
        "tool_call" => check::<ToolCall>(input),
        "assistant_diagnostic_error" => check::<AssistantDiagnosticError>(input),
        "assistant_message_diagnostic" => check::<AssistantMessageDiagnostic>(input),
        other => panic!("corpus case uses unknown kind {other:?}"),
    }
}

#[test]
fn differential_corpus_matches_python_fixture() {
    let corpus =
        read_jsonl::<CorpusCase>(&manifest_dir().join("tests/fixtures/model_corpus.jsonl"));
    let expectations = read_jsonl::<PythonExpectation>(
        &manifest_dir().join("tests/fixtures/model_expected.jsonl"),
    );

    assert_eq!(
        corpus.len(),
        expectations.len(),
        "corpus and expected fixture differ in case count"
    );

    let mut failures = Vec::new();
    let mut matched = 0usize;
    for case in &corpus {
        let python = expectations
            .iter()
            .find(|expectation| expectation.id == case.id)
            .unwrap_or_else(|| panic!("no Python expectation for case {:?}", case.id));

        let result = run_case(&case.kind, &case.input);
        let (accepted, output) = match result {
            Ok(output) => (true, Some(output)),
            Err(_) => (false, None),
        };

        if let Some(rust) = &case.rust {
            assert!(
                case.divergence.is_some(),
                "{}: a rust expectation requires a divergence reason",
                case.id
            );
            if let Some(expected) = &rust.output {
                if output.as_deref() != Some(expected.as_str()) {
                    failures.push(format!(
                        "{}: rust output mismatch\n  expected: {expected}\n  actual:   {output:?}",
                        case.id
                    ));
                }
            } else if accepted != rust.accepted {
                failures.push(format!(
                    "{}: rust accepted={accepted}, expectation={}",
                    case.id, rust.accepted
                ));
            }
            matched += 1;
            continue;
        }

        if python.accepted {
            let expected = python.output.as_deref().unwrap_or_default();
            match &output {
                Some(actual) if actual == expected => {}
                Some(actual) => failures.push(format!(
                    "{}: output differs\n  python: {expected}\n  rust:   {actual}",
                    case.id
                )),
                None => failures.push(format!(
                    "{}: python accepts but rust rejects\n  python: {expected}",
                    case.id
                )),
            }
        } else if accepted {
            failures.push(format!(
                "{}: python rejects but rust accepts: {output:?}",
                case.id
            ));
        }
        matched += 1;
    }

    assert!(matched == corpus.len(), "unmatched corpus cases");
    assert!(
        failures.is_empty(),
        "{} differential mismatch(es):\n{}",
        failures.len(),
        failures.join("\n")
    );
}

#[test]
fn differential_corpus_covers_every_kind_and_group() {
    let corpus =
        read_jsonl::<CorpusCase>(&manifest_dir().join("tests/fixtures/model_corpus.jsonl"));
    let mut groups: BTreeMap<(&str, &str), usize> = BTreeMap::new();
    for case in &corpus {
        *groups
            .entry((case.kind.as_str(), case.group.as_str()))
            .or_default() += 1;
    }

    let kinds: BTreeSet<&str> = corpus.iter().map(|case| case.kind.as_str()).collect();
    for kind in &kinds {
        for group in ["normal", "extreme", "edge"] {
            assert!(
                groups.get(&(kind, group)).copied().unwrap_or(0) > 0,
                "kind {kind:?} has no {group} cases"
            );
        }
    }
    assert!(kinds.len() >= 12, "only {} kinds covered", kinds.len());
    assert!(
        corpus.len() >= 150,
        "corpus shrank unexpectedly: {} cases",
        corpus.len()
    );
}

#[test]
fn differential_corpus_matches_live_python() {
    let Some(python) = python_interpreter() else {
        eprintln!(
            "skipping live Python check: no interpreter found (run tools/setup-reference.sh or set TAU_PYTHON)"
        );
        return;
    };
    let script = manifest_dir().join("tools/gen_fixtures.py");
    let output = Command::new(&python)
        .arg(&script)
        .arg("--stdout")
        .current_dir(manifest_dir())
        .output()
        .unwrap_or_else(|error| panic!("failed to run {}: {error}", python.display()));
    assert!(
        output.status.success(),
        "{} failed:\n{}",
        python.display(),
        String::from_utf8_lossy(&output.stderr)
    );

    let live = jsonl_by_id(&String::from_utf8_lossy(&output.stdout));
    let committed = jsonl_by_id(
        &std::fs::read_to_string(manifest_dir().join("tests/fixtures/model_expected.jsonl"))
            .expect("expected fixture"),
    );

    assert_eq!(
        live.len(),
        committed.len(),
        "live Python produced {} cases, committed fixture has {}",
        live.len(),
        committed.len()
    );
    for (id, live_line) in &live {
        let committed_line = committed
            .get(id)
            .unwrap_or_else(|| panic!("live case {id:?} missing from the committed fixture"));
        assert_eq!(
            live_line, committed_line,
            "case {id:?} drifted between tau and the committed fixture; run tools/fixtures.sh"
        );
    }
}

fn jsonl_by_id(text: &str) -> BTreeMap<String, String> {
    text.lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            let value: Value = serde_json::from_str(line).expect("expected JSONL");
            let id = value["id"].as_str().expect("id").to_owned();
            (id, line.to_owned())
        })
        .collect()
}

fn python_interpreter() -> Option<PathBuf> {
    if let Ok(path) = std::env::var("TAU_PYTHON") {
        let path = PathBuf::from(path);
        return path.exists().then_some(path);
    }
    let venv = manifest_dir().join("reference/tau/.venv/bin/python");
    venv.exists().then_some(venv)
}
