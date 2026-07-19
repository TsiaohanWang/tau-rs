//! Summarization prompts for context-window compaction.
//!
//! Ported from `src/tau_coding/context_window.py` (Python Tau). These string
//! literals are fed to the active model provider as the system + user messages
//! when compacting conversation history. See ADR-P5-3 in `docs/phase-5.md`.

/// System prompt prepended to every summarization call.
pub const SUMMARIZATION_SYSTEM_PROMPT: &str = "\
You are a context summarization assistant. Your task is to read a conversation \
between a user and an AI coding assistant, then produce a structured summary \
following the exact format specified.

Do NOT continue the conversation. Do NOT respond to any questions in the \
conversation. ONLY output the structured summary.";

/// User prompt for the initial (first-time) compaction of a conversation.
pub const SUMMARIZATION_PROMPT: &str = "\
The messages above are a conversation to summarize. Create a structured context \
checkpoint summary that another LLM will use to continue the work.

Use this EXACT format:

## Goal
[What is the user trying to accomplish? Can be multiple items if the session \
covers different tasks.]

## Constraints & Preferences
- [Any constraints, preferences, or requirements mentioned by user]
- [Or \"(none)\" if none were mentioned]

## Progress
### Done
- [x] [Completed tasks/changes]

### In Progress
- [ ] [Current work]

### Blocked
- [Issues preventing progress, if any]

## Key Decisions
- **[Decision]**: [Brief rationale]

## Next Steps
1. [Ordered list of what should happen next]

## Critical Context
- [Any data, examples, or references needed to continue]
- [Or \"(none)\" if not applicable]

Keep each section concise. Preserve exact file paths, function names, and error \
messages.";

/// User prompt for updating an existing compaction summary with new messages.
pub const UPDATE_SUMMARIZATION_PROMPT: &str = "\
The messages above are NEW conversation messages to incorporate into the existing \
summary provided in <previous-summary> tags.

Update the existing structured summary with new information. RULES:
- PRESERVE all existing information from the previous summary
- ADD new progress, decisions, and context from the new messages
- UPDATE the Progress section: move items from \"In Progress\" to \"Done\" when \
completed
- UPDATE \"Next Steps\" based on what was accomplished
- PRESERVE exact file paths, function names, and error messages
- If something is no longer relevant, you may remove it

Use this EXACT format:

## Goal
[Preserve existing goals, add new ones if the task expanded]

## Constraints & Preferences
- [Preserve existing, add new ones discovered]

## Progress
### Done
- [x] [Include previously done items AND newly completed items]

### In Progress
- [ ] [Current work - update based on progress]

### Blocked
- [Current blockers - remove if resolved]

## Key Decisions
- **[Decision]**: [Brief rationale] (preserve all previous, add new)

## Next Steps
1. [Update based on current state]

## Critical Context
- [Preserve important context, add new if needed]

Keep each section concise. Preserve exact file paths, function names, and error \
messages.";

/// Prefix used to detect an existing compaction summary as the first user
/// message in the history. Matches Python's `COMPACTION_SUMMARY_PREFIX`.
pub const COMPACTION_SUMMARY_PREFIX: &str = "Previous conversation summary:\n";

/// Maximum number of characters for a single serialized message inside the
/// compaction prompt. Matches Python's `SUMMARY_MESSAGE_CHAR_LIMIT = 500`.
pub const SUMMARY_MESSAGE_CHAR_LIMIT: usize = 500;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summarization_system_prompt_is_nonempty_and_no_trailing_newline() {
        assert!(!SUMMARIZATION_SYSTEM_PROMPT.is_empty());
        assert!(
            !SUMMARIZATION_SYSTEM_PROMPT.ends_with('\n'),
            "prompt should not end with a bare newline"
        );
    }

    #[test]
    fn summarization_prompt_mentions_exact_format() {
        assert!(SUMMARIZATION_PROMPT.contains("EXACT format"));
        assert!(SUMMARIZATION_PROMPT.contains("## Goal"));
        assert!(SUMMARIZATION_PROMPT.contains("## Critical Context"));
    }

    #[test]
    fn update_prompt_mentions_previous_summary_tag() {
        assert!(UPDATE_SUMMARIZATION_PROMPT.contains("<previous-summary>"));
        assert!(UPDATE_SUMMARIZATION_PROMPT.contains("PRESERVE all existing"));
    }

    #[test]
    fn compaction_summary_prefix_ends_with_newline() {
        assert!(
            COMPACTION_SUMMARY_PREFIX.ends_with('\n'),
            "prefix must end with newline so summary text starts on the next line"
        );
    }
}
