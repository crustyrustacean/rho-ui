#[derive(Clone, Debug)]
pub(crate) enum ChatBlock {
    /// A user-submitted message.
    User(String),
    /// A mid-turn steering message (sent while the agent was busy).
    Steer(String),
    /// A completed reasoning ("thinking") block with elapsed time.
    Reasoning { text: String, elapsed_secs: String },
    /// A streaming reasoning block (still thinking).
    ReasoningStreaming { text: String },
    /// A streaming response rendered as a cheap label until finalized.
    ResponseStreaming(String),
    /// A finalized response rendered as markdown (collapsible).
    Response { text: String, expanded: bool },
    /// A tool call block with status.
    ToolCall {
        name: String,
        args: String,
        status: ToolStatus,
        output: Option<String>,
        expanded: bool,
    },
    /// A tool-approval prompt from the agent.
    Approval {
        tool: String,
        arguments: String,
        risk: String,
        resolution: ApprovalResolution,
    },
    /// A system/info message.
    Info(String),
}

#[derive(Clone, Debug)]
pub(crate) enum ToolStatus {
    Pending,
    Success,
    Error,
    Denied,
}

#[derive(Clone, Debug)]
pub(crate) enum ApprovalResolution {
    Pending,
    Approved,
    Denied,
    Redirected(String),
}