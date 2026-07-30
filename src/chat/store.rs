use super::model::{ApprovalResolution, ChatBlock, ToolStatus};
use std::sync::RwLock;

static CHAT_BLOCKS: RwLock<Vec<ChatBlock>> = RwLock::new(Vec::new());

pub(crate) fn snapshot() -> Vec<ChatBlock> {
    CHAT_BLOCKS.read().unwrap().clone()
}

pub(crate) fn len() -> usize {
    CHAT_BLOCKS.read().unwrap().len()
}

pub(crate) fn push(block: ChatBlock) {
    CHAT_BLOCKS.write().unwrap().push(block);
}

pub(crate) fn push_after_finalizing_response(block: ChatBlock) {
    finalize_streaming_response();
    push(block);
}

pub(crate) fn append_streaming_response(delta: &str) {
    let mut blocks = CHAT_BLOCKS.write().unwrap();
    match blocks.last_mut() {
        Some(ChatBlock::ResponseStreaming(text)) => text.push_str(delta),
        _ => blocks.push(ChatBlock::ResponseStreaming(delta.to_string())),
    }
}

pub(crate) fn finalize_streaming_response() {
    let mut blocks = CHAT_BLOCKS.write().unwrap();
    if let Some(ChatBlock::ResponseStreaming(text)) = blocks.last_mut() {
        let owned = std::mem::take(text);
        *blocks.last_mut().unwrap() = ChatBlock::Response { text: owned, expanded: false };
    }
}

pub(crate) fn append_streaming_reasoning(delta: &str) {
    let mut blocks = CHAT_BLOCKS.write().unwrap();
    match blocks.last_mut() {
        Some(ChatBlock::ReasoningStreaming { text }) => text.push_str(delta),
        _ => blocks.push(ChatBlock::ReasoningStreaming {
            text: delta.to_string(),
        }),
    }
}

pub(crate) fn finalize_reasoning(elapsed_secs: String) {
    let mut blocks = CHAT_BLOCKS.write().unwrap();
    if let Some(ChatBlock::ReasoningStreaming { text }) = blocks.last_mut() {
        let owned = std::mem::take(text);
        *blocks.last_mut().unwrap() = ChatBlock::Reasoning {
            text: owned,
            elapsed_secs,
        };
    }
}

pub(crate) fn push_final_response_if_missing(reply: String) {
    if reply.is_empty() {
        return;
    }
    let mut blocks = CHAT_BLOCKS.write().unwrap();
    if !matches!(blocks.last(), Some(ChatBlock::Response { .. })) {
        blocks.push(ChatBlock::Response { text: reply, expanded: false });
    }
}

pub(crate) fn push_pending_tool_call(name: String, args: String) {
    push(ChatBlock::ToolCall {
        name,
        args,
        status: ToolStatus::Pending,
        output: None,
        expanded: false,
    });
}

pub(crate) fn finalize_tool_call(name: &str, status: ToolStatus, output: Option<String>) {
    let mut blocks = CHAT_BLOCKS.write().unwrap();
    for block in blocks.iter_mut().rev() {
        if let ChatBlock::ToolCall {
            name: block_name,
            status: block_status,
            output: block_output,
            ..
        } = block
        {
            if block_name == name && matches!(block_status, ToolStatus::Pending) {
                *block_status = status;
                *block_output = output;
                return;
            }
        }
    }
    blocks.push(ChatBlock::ToolCall {
        name: name.to_string(),
        args: String::new(),
        status,
        output,
        expanded: false,
    });
}

pub(crate) fn push_approval(tool: String, arguments: String, risk: String) {
    push(ChatBlock::Approval {
        tool,
        arguments,
        risk,
        resolution: ApprovalResolution::Pending,
    });
}

pub(crate) fn resolve_approval(index: usize, resolution: ApprovalResolution) {
    if let Some(ChatBlock::Approval {
        resolution: current,
        ..
    }) = CHAT_BLOCKS.write().unwrap().get_mut(index)
    {
        *current = resolution;
    }
}

pub(crate) fn toggle_expand(index: usize) {
    let mut blocks = CHAT_BLOCKS.write().unwrap();
    if let Some(block) = blocks.get_mut(index) {
        match block {
            ChatBlock::ToolCall { expanded, .. } => *expanded = !*expanded,
            ChatBlock::Response { expanded, .. } => *expanded = !*expanded,
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    static TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[must_use]
    fn reset() -> std::sync::MutexGuard<'static, ()> {
        let guard = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
        CHAT_BLOCKS.write().unwrap().clear();
        guard
    }

    #[test]
    fn streaming_response_accumulates_and_finalizes() {
        let _guard = reset();
        append_streaming_response("hello");
        append_streaming_response(" world");
        finalize_streaming_response();
        assert!(matches!(&snapshot()[0], ChatBlock::Response { text, .. } if text == "hello world"));
    }

    #[test]
    fn streaming_reasoning_accumulates_and_finalizes() {
        let _guard = reset();
        append_streaming_reasoning("deep");
        append_streaming_reasoning(" thoughts");
        finalize_reasoning("5.2s".into());
        assert!(matches!(
            &snapshot()[0],
            ChatBlock::Reasoning { text, elapsed_secs }
                if text == "deep thoughts" && elapsed_secs == "5.2s"
        ));
    }

    #[test]
    fn finalizing_unknown_tool_pushes_fallback() {
        let _guard = reset();
        finalize_tool_call("unknown", ToolStatus::Error, Some("failed".into()));
        assert!(matches!(
            &snapshot()[0],
            ChatBlock::ToolCall { name, args, status: ToolStatus::Error, output: Some(output), .. }
                if name == "unknown" && args.is_empty() && output == "failed"
        ));
    }

    #[test]
    fn tool_and_approval_mutations_use_indices() {
        let _guard = reset();
        push_pending_tool_call("read_file".into(), "{}".into());
        toggle_expand(0);
        push_approval("run_command".into(), "cargo test".into(), "write".into());
        resolve_approval(1, ApprovalResolution::Approved);
        let blocks = snapshot();
        assert!(matches!(&blocks[0], ChatBlock::ToolCall { expanded: true, .. }));
        assert!(matches!(&blocks[1], ChatBlock::Approval { resolution: ApprovalResolution::Approved, .. }));
    }
}