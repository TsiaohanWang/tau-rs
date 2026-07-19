//! The portable agent loop: `run_agent_loop`.
//!
//! A pure function that drives the provider/tool loop and emits Pi
//! `AgentEvent`s, mirroring `tau_agent.loop.run_agent_loop`. It is the single
//! most complex async generator in the project; the translation preserves its
//! control flow shape (turn / steering / follow-up) and pull-based streaming
//! semantics via `async_stream::stream!` (see ADR-5, ADR-4).

use std::collections::HashMap;

use async_stream::stream;
use futures::StreamExt;
use tokio_util::sync::CancellationToken;

use tau_types::{
    AgentEndEvent, AgentEvent, AgentMessage, AgentStartEvent, AgentToolResult, AssistantMessage,
    AssistantMessageEvent as ProviderEv, MessageEndEvent, MessageStartEvent, MessageUpdateEvent,
    TextContent, ToolCall, ToolExecutionEndEvent, ToolExecutionStartEvent,
    ToolExecutionUpdateEvent, ToolResultMessage, TurnEndEvent, TurnStartEvent,
};

use crate::provider::{ModelProvider, StreamRequest};
use crate::tool::{AfterToolCall, AgentTool, BeforeToolCall};

pub struct LoopArgs<'a> {
    pub provider: &'a (dyn ModelProvider + Send + Sync),
    pub model: &'a str,
    pub system: &'a str,
    pub messages: &'a mut Vec<AgentMessage>,
    pub tools: &'a [AgentTool],
    pub prompts: &'a [AgentMessage],
    pub max_turns: Option<u32>,
    pub signal: Option<CancellationToken>,
    pub thinking_level: Option<&'a str>,
    pub get_steering_messages: Option<&'a mut (dyn FnMut() -> Vec<AgentMessage> + Send)>,
    pub get_follow_up_messages: Option<&'a mut (dyn FnMut() -> Vec<AgentMessage> + Send)>,
    pub before_tool_call: Option<&'a dyn BeforeToolCall>,
    pub after_tool_call: Option<&'a dyn AfterToolCall>,
}

pub fn run_agent_loop(mut args: LoopArgs<'_>) -> impl futures::Stream<Item = AgentEvent> + Send {
    stream! {
        let mut new_messages: Vec<AgentMessage> = args.prompts.to_vec();
        args.messages.extend(args.prompts.iter().cloned());

        yield AgentEvent::AgentStart(AgentStartEvent {});
        yield AgentEvent::TurnStart(TurnStartEvent {});
        for prompt in args.prompts {
            yield AgentEvent::MessageStart(MessageStartEvent { message: prompt.clone() });
            yield AgentEvent::MessageEnd(MessageEndEvent { message: prompt.clone() });
        }

        if args.max_turns.is_some_and(|m| m < 1) {
            let error = error_message(args.model, "max_turns must be at least 1");
            args.messages.push(AgentMessage::Assistant(error.clone()));
            new_messages.push(AgentMessage::Assistant(error.clone()));
            yield AgentEvent::MessageStart(MessageStartEvent { message: AgentMessage::Assistant(error.clone()) });
            yield AgentEvent::MessageEnd(MessageEndEvent { message: AgentMessage::Assistant(error.clone()) });
            yield AgentEvent::TurnEnd(TurnEndEvent { message: AgentMessage::Assistant(error), tool_results: Vec::new() });
            yield AgentEvent::AgentEnd(AgentEndEvent { messages: new_messages });
            return;
        }

        let tool_by_name: HashMap<&str, &AgentTool> =
            args.tools.iter().map(|t| (t.name.as_ref(), t)).collect();

        let mut turn: u32 = 1;
        let mut first_turn = true;
        let mut pending: Vec<AgentMessage> = drain(&mut args.get_steering_messages);

        'outer: loop {
            let mut has_more_tools = true;
            while has_more_tools || !pending.is_empty() {
                if !first_turn {
                    yield AgentEvent::TurnStart(TurnStartEvent {});
                }
                first_turn = false;

                for message in &pending {
                    args.messages.push(message.clone());
                    new_messages.push(message.clone());
                    yield AgentEvent::MessageStart(MessageStartEvent { message: message.clone() });
                    yield AgentEvent::MessageEnd(MessageEndEvent { message: message.clone() });
                }
                pending.clear();

                if args.max_turns.is_some_and(|m| turn > m) {
                    let error = error_message(
                        args.model,
                        &format!("Agent stopped after max_turns={}", m_of(args.max_turns)),
                    );
                    args.messages.push(AgentMessage::Assistant(error.clone()));
                    new_messages.push(AgentMessage::Assistant(error.clone()));
                    yield AgentEvent::MessageStart(MessageStartEvent { message: AgentMessage::Assistant(error.clone()) });
                    yield AgentEvent::MessageEnd(MessageEndEvent { message: AgentMessage::Assistant(error.clone()) });
                    yield AgentEvent::TurnEnd(TurnEndEvent { message: AgentMessage::Assistant(error.clone()), tool_results: Vec::new() });
                    yield AgentEvent::AgentEnd(AgentEndEvent { messages: new_messages });
                    return;
                }

                // Drive the provider stream inline.
                let mut assistant: Option<AssistantMessage> = None;
                {
                    let request = StreamRequest {
                        model: args.model,
                        system: args.system,
                        messages: &*args.messages,
                        tools: args.tools,
                        signal: args.signal.clone(),
                        thinking_level: args.thinking_level,
                    };
                    let mut source = args.provider.stream_response(&request);
                    let mut started = false;
                    while let Some(event) = source.next().await {
                        match event {
                            ProviderEv::Start(s) => {
                                started = true;
                                yield AgentEvent::MessageStart(MessageStartEvent {
                                    message: AgentMessage::Assistant((*s.partial).clone()),
                                });
                            }
                            ProviderEv::Done(d) => {
                                if !started {
                                    yield AgentEvent::MessageStart(MessageStartEvent {
                                        message: AgentMessage::Assistant(d.message.clone()),
                                    });
                                }
                                assistant = Some(d.message.clone());
                                yield AgentEvent::MessageEnd(MessageEndEvent {
                                    message: AgentMessage::Assistant(d.message),
                                });
                            }
                            ProviderEv::Error(e) => {
                                if !started {
                                    yield AgentEvent::MessageStart(MessageStartEvent {
                                        message: AgentMessage::Assistant(e.error.clone()),
                                    });
                                }
                                assistant = Some(e.error.clone());
                                yield AgentEvent::MessageEnd(MessageEndEvent {
                                    message: AgentMessage::Assistant(e.error),
                                });
                            }
                            other => {
                                let partial = partial_of(&other).clone();
                                yield AgentEvent::MessageUpdate(Box::new(MessageUpdateEvent {
                                    message: AgentMessage::Assistant((*partial).clone()),
                                    assistant_message_event: other,
                                }));
                            }
                        }
                    }
                }

                let assistant = match assistant {
                    Some(a) => a,
                    None => {
                        let m = error_message(args.model, "Provider produced no assistant message");
                        yield AgentEvent::MessageStart(MessageStartEvent { message: AgentMessage::Assistant(m.clone()) });
                        yield AgentEvent::MessageEnd(MessageEndEvent { message: AgentMessage::Assistant(m.clone()) });
                        m
                    }
                };

                args.messages.push(AgentMessage::Assistant(assistant.clone()));
                new_messages.push(AgentMessage::Assistant(assistant.clone()));

                if matches!(assistant.stop_reason, tau_types::StopReason::Error)
                    || matches!(assistant.stop_reason, tau_types::StopReason::Aborted)
                {
                    yield AgentEvent::TurnEnd(TurnEndEvent {
                        message: AgentMessage::Assistant(assistant.clone()),
                        tool_results: Vec::new(),
                    });
                    yield AgentEvent::AgentEnd(AgentEndEvent { messages: new_messages });
                    return;
                }

                // Execute tool calls sequentially.
                let calls: Vec<ToolCall> = assistant.tool_calls().cloned().collect();
                has_more_tools = !calls.is_empty();
                let mut tool_results: Vec<ToolResultMessage> = Vec::new();
                for call in calls {
                    yield AgentEvent::ToolExecutionStart(ToolExecutionStartEvent {
                        tool_call_id: call.id.clone(),
                        tool_name: call.name.clone(),
                        args: call.arguments.clone(),
                    });

                    let (result, is_error): (AgentToolResult, bool) = {
                        let (blocked, block_reason) = if let Some(before) = args.before_tool_call {
                            before.call(&call).await
                        } else {
                            (false, None)
                        };
                        if blocked {
                            (error_result(&block_reason.unwrap_or_else(|| "Tool execution was blocked".to_string())), true)
                        } else if args.signal.as_ref().is_some_and(|s| s.is_cancelled()) {
                            (error_result("Operation aborted"), true)
                        } else {
                            match tool_by_name.get(call.name.as_str()) {
                                None => (error_result(&format!("Tool {} not found", call.name)), true),
                                Some(tool) => {
                                    let (r, ie, updates) = run_tool(tool, &call, args.signal.clone()).await;
                                    for update in &updates {
                                        yield AgentEvent::ToolExecutionUpdate(ToolExecutionUpdateEvent {
                                            tool_call_id: call.id.clone(),
                                            tool_name: call.name.clone(),
                                            args: call.arguments.clone(),
                                            partial_result: update.clone(),
                                        });
                                    }
                                    (r, ie)
                                }
                            }
                        }
                    };

                    let (result, is_error) = if let Some(after) = args.after_tool_call {
                        after.call(&call, result, is_error).await
                    } else {
                        (result, is_error)
                    };

                    yield AgentEvent::ToolExecutionEnd(ToolExecutionEndEvent {
                        tool_call_id: call.id.clone(),
                        tool_name: call.name.clone(),
                        result: result.clone(),
                        is_error,
                    });

                    let message = ToolResultMessage {
                        role: tau_types::MessageRole::ToolResult,
                        tool_call_id: call.id.clone(),
                        tool_name: call.name.clone(),
                        content: result.content.clone(),
                        details: result.details.clone(),
                        added_tool_names: result.added_tool_names.clone(),
                        is_error,
                        timestamp: tau_types::current_timestamp_ms(),
                    };
                    tool_results.push(message.clone());
                    args.messages.push(AgentMessage::ToolResult(message.clone()));
                    new_messages.push(AgentMessage::ToolResult(message.clone()));
                    yield AgentEvent::MessageStart(MessageStartEvent {
                        message: AgentMessage::ToolResult(message.clone()),
                    });
                    yield AgentEvent::MessageEnd(MessageEndEvent {
                        message: AgentMessage::ToolResult(message),
                    });
                }

                yield AgentEvent::TurnEnd(TurnEndEvent {
                    message: AgentMessage::Assistant(assistant.clone()),
                    tool_results,
                });
                turn += 1;
                pending = drain(&mut args.get_steering_messages);
            }

            let follow_ups = drain(&mut args.get_follow_up_messages);
            if !follow_ups.is_empty() {
                pending = follow_ups;
                continue 'outer;
            }
            break 'outer;
        }

        yield AgentEvent::AgentEnd(AgentEndEvent { messages: new_messages });
    }
}

fn m_of(max_turns: Option<u32>) -> u32 {
    max_turns.unwrap_or(0)
}

fn drain(cb: &mut Option<&mut (dyn FnMut() -> Vec<AgentMessage> + Send)>) -> Vec<AgentMessage> {
    match cb {
        Some(f) => f(),
        None => Vec::new(),
    }
}

fn partial_of(ev: &ProviderEv) -> &std::sync::Arc<AssistantMessage> {
    match ev {
        ProviderEv::Start(s) => &s.partial,
        ProviderEv::TextStart(s) => &s.partial,
        ProviderEv::TextDelta(s) => &s.partial,
        ProviderEv::TextEnd(s) => &s.partial,
        ProviderEv::ThinkingStart(s) => &s.partial,
        ProviderEv::ThinkingDelta(s) => &s.partial,
        ProviderEv::ThinkingEnd(s) => &s.partial,
        ProviderEv::ToolCallStart(s) => &s.partial,
        ProviderEv::ToolCallDelta(s) => &s.partial,
        ProviderEv::ToolCallEnd(s) => &s.partial,
        ProviderEv::Done(_) | ProviderEv::Error(_) => {
            unreachable!("partial_of called on terminal event")
        }
    }
}

async fn run_tool(
    tool: &AgentTool,
    call: &ToolCall,
    signal: Option<CancellationToken>,
) -> (AgentToolResult, bool, Vec<AgentToolResult>) {
    let updates: std::sync::Arc<std::sync::Mutex<Vec<AgentToolResult>>> =
        std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let update_cb = {
        let updates = updates.clone();
        std::sync::Arc::new(move |r: AgentToolResult| {
            updates.lock().expect("update mutex poisoned").push(r);
        }) as std::sync::Arc<dyn Fn(AgentToolResult) + Send + Sync>
    };
    let result = tool
        .executor
        .execute(&call.id, &call.arguments, signal, Some(update_cb.as_ref()))
        .await;
    let collected = updates.lock().expect("update mutex poisoned").clone();
    match result {
        Ok(r) => (r, false, collected),
        Err(e) => (error_result(&e.message), true, collected),
    }
}

fn error_result(message: &str) -> AgentToolResult {
    let mut r = AgentToolResult::default();
    r.content
        .push(tau_types::ToolResultContent::Text(TextContent::new(
            message,
        )));
    r
}

fn error_message(model: &str, message: &str) -> AssistantMessage {
    AssistantMessage {
        model: model.to_string(),
        stop_reason: tau_types::StopReason::Error,
        error_message: Some(message.to_string()),
        ..Default::default()
    }
}
