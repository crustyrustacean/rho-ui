use crate::util::json::{jbool, jf64, jstr, ju64};

#[allow(dead_code)]
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum RequestKind {
    GetState,
    GetSessionStats,
    ListModels,
    ListProviders,
    ListSessions,
    SetModel,
    ResumeSession,
    ReloadExtensions,
}

#[derive(Clone, Debug)]
pub(crate) enum RhoEvent {
    Ready,
    AgentStart,
    AgentEnd {
        reply: String,
        duration_ms: u64,
    },
    AgentError {
        error: String,
    },
    MessageDelta {
        delta: String,
    },
    ReasoningDelta {
        delta: String,
    },
    StateChange {
        state: String,
    },
    ToolCall {
        name: String,
        arguments: String,
    },
    ToolResult {
        name: String,
        is_error: bool,
        output: String,
    },
    ToolDenied {
        name: String,
    },
    ApprovalRequest {
        tool: String,
        arguments: String,
        risk: String,
    },
    Usage {
        input_tokens: u64,
        output_tokens: u64,
        cached_tokens: u64,
        cost: f64,
        context_used: u64,
        context_window: u64,
        utilization: u8,
    },
    Response {
        kind: RequestKind,
        result: serde_json::Value,
    },
    RequestError {
        kind: RequestKind,
        error: String,
    },
    Closed,
}

/// Parse a JSON-RPC notification (a line with a `method` field, no `id`).
/// Returns `None` for malformed JSON, missing `method`, or unknown methods.
/// This is the pure, testable half of `RhoAgent::parse_stdout`.
pub(crate) fn parse_notification(v: &serde_json::Value) -> Option<RhoEvent> {
    let method = v.get("method")?.as_str()?;
    let p = v.get("params");
    Some(match method {
        "ready" => RhoEvent::Ready,
        "agent/start" => RhoEvent::AgentStart,
        "agent/end" => RhoEvent::AgentEnd {
            reply: jstr(p, "reply"),
            duration_ms: ju64(p, "durationMs"),
        },
        "agent/error" => RhoEvent::AgentError {
            error: jstr(p, "error"),
        },
        "message/delta" => RhoEvent::MessageDelta {
            delta: jstr(p, "delta"),
        },
        "reasoning/delta" => RhoEvent::ReasoningDelta {
            delta: jstr(p, "delta"),
        },
        "state/change" => RhoEvent::StateChange {
            state: jstr(p, "state"),
        },
        "tool/call" => RhoEvent::ToolCall {
            name: jstr(p, "name"),
            arguments: jstr(p, "arguments"),
        },
        "tool/result" => RhoEvent::ToolResult {
            name: jstr(p, "name"),
            is_error: jbool(p, "isError"),
            output: jstr(p, "output"),
        },
        "tool/denied" => RhoEvent::ToolDenied {
            name: jstr(p, "name"),
        },
        "approval/request" => RhoEvent::ApprovalRequest {
            tool: jstr(p, "tool"),
            arguments: jstr(p, "arguments"),
            risk: jstr(p, "risk"),
        },
        "usage" => {
            let u = p.and_then(|p| p.get("usage"));
            let c = p.and_then(|p| p.get("context"));
            RhoEvent::Usage {
                input_tokens: ju64(u, "inputTokens"),
                output_tokens: ju64(u, "outputTokens"),
                cached_tokens: ju64(u, "cachedTokens"),
                cost: jf64(u, "cost"),
                context_used: ju64(c, "estimatedUsed"),
                context_window: ju64(c, "contextWindow"),
                utilization: ju64(c, "utilizationPercent").min(255) as u8,
            }
        }
        _ => return None,
    })
}

/// Parse the `error.message` from a JSON-RPC error response object.
pub(crate) fn parse_response_error(v: &serde_json::Value) -> String {
    v.get("error")
        .and_then(|err| err.get("message"))
        .and_then(|x| x.as_str())
        .unwrap_or("request failed")
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(line: &str) -> Option<RhoEvent> {
        let v: serde_json::Value = serde_json::from_str(line).ok()?;
        parse_notification(&v)
    }

    #[test]
    fn ready() {
        assert!(matches!(
            parse(r#"{"method":"ready","params":{}}"#),
            Some(RhoEvent::Ready)
        ));
    }

    #[test]
    fn agent_start() {
        assert!(matches!(
            parse(r#"{"method":"agent/start","params":{}}"#),
            Some(RhoEvent::AgentStart)
        ));
    }

    #[test]
    fn agent_end() {
        match parse(r#"{"method":"agent/end","params":{"reply":"hi","durationMs":3200}}"#) {
            Some(RhoEvent::AgentEnd { reply, duration_ms }) => {
                assert_eq!(reply, "hi");
                assert_eq!(duration_ms, 3200);
            }
            other => panic!("expected AgentEnd, got {other:?}"),
        }
    }

    #[test]
    fn agent_error() {
        match parse(r#"{"method":"agent/error","params":{"error":"boom"}}"#) {
            Some(RhoEvent::AgentError { error }) => assert_eq!(error, "boom"),
            other => panic!("expected AgentError, got {other:?}"),
        }
    }

    #[test]
    fn message_delta() {
        match parse(r#"{"method":"message/delta","params":{"delta":"world"}}"#) {
            Some(RhoEvent::MessageDelta { delta }) => assert_eq!(delta, "world"),
            other => panic!("expected MessageDelta, got {other:?}"),
        }
    }

    #[test]
    fn reasoning_delta() {
        match parse(r#"{"method":"reasoning/delta","params":{"delta":"..."}}"#) {
            Some(RhoEvent::ReasoningDelta { delta }) => assert_eq!(delta, "..."),
            other => panic!("expected ReasoningDelta, got {other:?}"),
        }
    }

    #[test]
    fn state_change() {
        match parse(r#"{"method":"state/change","params":{"state":"idle"}}"#) {
            Some(RhoEvent::StateChange { state }) => assert_eq!(state, "idle"),
            other => panic!("expected StateChange, got {other:?}"),
        }
    }

    #[test]
    fn tool_call() {
        match parse(
            r#"{"method":"tool/call","params":{"name":"read_file","arguments":"{}"}}"#,
        ) {
            Some(RhoEvent::ToolCall { name, arguments }) => {
                assert_eq!(name, "read_file");
                assert_eq!(arguments, "{}");
            }
            other => panic!("expected ToolCall, got {other:?}"),
        }
    }

    #[test]
    fn tool_result_success() {
        match parse(r#"{"method":"tool/result","params":{"name":"r","isError":false,"output":"ok"}}"#)
        {
            Some(RhoEvent::ToolResult { name, is_error, output }) => {
                assert_eq!(name, "r");
                assert!(!is_error);
                assert_eq!(output, "ok");
            }
            other => panic!("expected ToolResult, got {other:?}"),
        }
    }

    #[test]
    fn tool_result_error() {
        match parse(r#"{"method":"tool/result","params":{"name":"w","isError":true,"output":"bad"}}"#)
        {
            Some(RhoEvent::ToolResult { is_error, .. }) => assert!(is_error),
            other => panic!("expected ToolResult, got {other:?}"),
        }
    }

    #[test]
    fn tool_denied() {
        match parse(r#"{"method":"tool/denied","params":{"name":"run"}}"#) {
            Some(RhoEvent::ToolDenied { name }) => assert_eq!(name, "run"),
            other => panic!("expected ToolDenied, got {other:?}"),
        }
    }

    #[test]
    fn approval_request() {
        match parse(r#"{"method":"approval/request","params":{"tool":"t","arguments":"a","risk":"low"}}"#)
        {
            Some(RhoEvent::ApprovalRequest { tool, arguments, risk }) => {
                assert_eq!(tool, "t");
                assert_eq!(arguments, "a");
                assert_eq!(risk, "low");
            }
            other => panic!("expected ApprovalRequest, got {other:?}"),
        }
    }

    #[test]
    fn usage() {
        let line = r#"{"method":"usage","params":{"usage":{"inputTokens":1000,"outputTokens":500,"cachedTokens":200,"cost":0.0042},"context":{"estimatedUsed":8000,"contextWindow":200000,"utilizationPercent":4}}}"#;
        match parse(line) {
            Some(RhoEvent::Usage {
                input_tokens,
                output_tokens,
                cached_tokens,
                cost,
                context_used,
                context_window,
                utilization,
            }) => {
                assert_eq!(input_tokens, 1000);
                assert_eq!(output_tokens, 500);
                assert_eq!(cached_tokens, 200);
                assert!((cost - 0.0042).abs() < 1e-9);
                assert_eq!(context_used, 8000);
                assert_eq!(context_window, 200_000);
                assert_eq!(utilization, 4);
            }
            other => panic!("expected Usage, got {other:?}"),
        }
    }

    #[test]
    fn usage_utilization_clamped() {
        let line = r#"{"method":"usage","params":{"usage":{"inputTokens":0,"outputTokens":0,"cachedTokens":0,"cost":0.0},"context":{"estimatedUsed":0,"contextWindow":0,"utilizationPercent":300}}}"#;
        match parse(line) {
            Some(RhoEvent::Usage { utilization, .. }) => assert_eq!(utilization, 255),
            other => panic!("expected Usage, got {other:?}"),
        }
    }

    #[test]
    fn unknown_method() {
        assert!(parse(r#"{"method":"unknown","params":{}}"#).is_none());
    }

    #[test]
    fn invalid_json() {
        assert!(parse("not json").is_none());
    }

    #[test]
    fn missing_method() {
        let v: serde_json::Value = serde_json::from_str(r#"{"params":{}}"#).unwrap();
        assert!(parse_notification(&v).is_none());
    }

    #[test]
    fn response_error_message() {
        let v: serde_json::Value =
            serde_json::from_str(r#"{"error":{"message":"denied"}}"#).unwrap();
        assert_eq!(parse_response_error(&v), "denied");
    }

    #[test]
    fn response_error_fallback() {
        let v: serde_json::Value = serde_json::json!({});
        assert_eq!(parse_response_error(&v), "request failed");
    }
}
