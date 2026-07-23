//! An optional structured execution trace.
//!
//! Tracing is off by default (it costs time and memory). When enabled, the
//! interpreter records one [`TraceEvent`] per significant execution step —
//! calls, returns, block entries, statements, and traps — into an ordered
//! [`Trace`]. The trace is deterministic: the same program under the same
//! [`crate::Limits`] produces byte-identical trace output every run, which is
//! what makes it usable as a reference oracle for the native backends.

use tuo_source::Span;

use crate::value::Value;

/// One recorded step of execution, in occurrence order.
#[derive(Clone, Debug)]
pub enum TraceEvent {
    /// Entered a function call at the given stack depth.
    Call {
        /// The called function's name.
        function: String,
        /// The call-stack depth after entering (entry function is depth 1).
        depth: u32,
    },
    /// Entered a basic block within the current function.
    Block {
        /// The block index.
        block: u32,
    },
    /// Executed a statement; `detail` is its rendered form.
    Statement {
        /// A short rendering of the statement.
        detail: String,
    },
    /// Returned from the current function with a value.
    Return {
        /// The returned value.
        value: Value,
    },
    /// The program aborted with a trap.
    Trap {
        /// The trap's stable label.
        label: String,
        /// The span the trap is attributed to.
        span: Span,
    },
}

/// An ordered, deterministic record of an execution.
#[derive(Clone, Debug, Default)]
pub struct Trace {
    events: Vec<TraceEvent>,
}

impl Trace {
    /// A fresh, empty trace.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Append an event (called by the interpreter when tracing is on).
    pub fn push(&mut self, event: TraceEvent) {
        self.events.push(event);
    }

    /// The recorded events, in occurrence order.
    #[must_use]
    pub fn events(&self) -> &[TraceEvent] {
        &self.events
    }

    /// Render the trace as one deterministic line per event, for snapshot
    /// tests and debugging.
    #[must_use]
    pub fn render(&self) -> String {
        let mut out = String::new();
        for event in &self.events {
            match event {
                TraceEvent::Call { function, depth } => {
                    out.push_str(&format!("call {function} @depth {depth}\n"));
                }
                TraceEvent::Block { block } => {
                    out.push_str(&format!("  block bb{block}\n"));
                }
                TraceEvent::Statement { detail } => {
                    out.push_str(&format!("    {detail}\n"));
                }
                TraceEvent::Return { value } => {
                    out.push_str(&format!("  return {}\n", value.render()));
                }
                TraceEvent::Trap { label, .. } => {
                    out.push_str(&format!("  trap {label}\n"));
                }
            }
        }
        out
    }
}
