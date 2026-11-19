use std::future::Future;

/// How an agent's turn ended: it answered, or a cancel cut it short. A cancelled turn is no failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ended {
    Answered,
    Cancelled,
}

/// What a turn sends while it runs: the reply's text as it is written, then its end, once.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentEvent {
    Delta(String),
    Done(Ended),
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AgentError {
    #[error("cannot start the agent: {0}")]
    Start(String),
    #[error("the agent is not the one asked for: {0}")]
    Refused(String),
    #[error("agent protocol: {0}")]
    Protocol(String),
    #[error("the agent failed its turn: {0}")]
    Agent(String),
    #[error("the agent is gone")]
    Gone,
}

/// The capability of answering what is said to it.
pub trait Agent: Send + Sync + 'static {
    type Turn: Turn;

    /// Sends one user turn; the reply arrives through the returned turn. One turn at a time: a second
    /// waits until the first is dropped.
    fn turn(&self, text: String) -> impl Future<Output = Result<Self::Turn, AgentError>> + Send;
}

/// One turn being answered.
pub trait Turn: Send + 'static {
    /// The next event; `None` once the turn has ended. Cancel-safe: a `select!` may drop it and call it
    /// again without losing what arrived.
    fn next(&mut self) -> impl Future<Output = Option<Result<AgentEvent, AgentError>>> + Send;

    /// Stops the turn; what the agent had not sent yet is never sent. The turn has ended when it returns.
    fn cancel(&mut self) -> impl Future<Output = Result<(), AgentError>> + Send;
}
