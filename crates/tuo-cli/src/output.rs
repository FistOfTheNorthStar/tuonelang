//! The output mode shared by every `tuo` command: how results reach the user.
//!
//! One global `--message-format` selects between the human renderer and the
//! two machine encodings of the [`crate::protocol`] event stream, and one
//! global `--log` flag gates internal logging on stderr. Every command takes
//! an [`OutputMode`] and either renders for a human or drives an
//! [`Emitter`](crate::protocol::Emitter); the mode is the single place the
//! stream discipline (stdout = protocol only in machine mode; stderr = logging
//! only when enabled) is decided.

use std::io::{self, Write};

use clap::ValueEnum;

use crate::protocol::{Emitter, MachineFormat, ProtocolCommand};

/// The `--message-format` choices, as parsed from the command line.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, ValueEnum)]
pub(crate) enum MessageFormat {
    /// Human-readable text for a terminal (the default). Not a stable format.
    #[default]
    Human,
    /// The whole run as one versioned JSON envelope on stdout.
    Json,
    /// One versioned JSON object per event, streamed a line at a time.
    #[value(name = "json-lines")]
    JsonLines,
}

impl MessageFormat {
    /// The machine encoding this format maps to, or `None` for [`Self::Human`].
    fn machine(self) -> Option<MachineFormat> {
        match self {
            Self::Human => None,
            Self::Json => Some(MachineFormat::Json),
            Self::JsonLines => Some(MachineFormat::JsonLines),
        }
    }
}

/// The resolved output mode for one command run: the message format plus
/// whether internal logging is enabled.
#[derive(Clone, Copy, Debug)]
pub(crate) struct OutputMode {
    format: MessageFormat,
    logging: bool,
}

impl OutputMode {
    /// Build the mode from the parsed global flags.
    pub(crate) fn new(format: MessageFormat, logging: bool) -> Self {
        Self { format, logging }
    }

    /// Is this a machine format (so stdout must carry protocol output only)?
    pub(crate) fn is_machine(self) -> bool {
        self.format.machine().is_some()
    }

    /// An [`Emitter`] over stdout for `command`, or `None` in human mode.
    ///
    /// The emitter owns a locked stdout handle so protocol output is the only
    /// thing written there for the duration of the run.
    pub(crate) fn emitter(self, command: ProtocolCommand) -> Option<Emitter<io::Stdout>> {
        self.format
            .machine()
            .map(|format| Emitter::new(io::stdout(), format, command))
    }

    /// Write an internal log line to stderr **iff** logging is enabled.
    ///
    /// This is the only path to stderr in machine mode; when logging is off it
    /// is a silent no-op, so a machine consumer sees a clean, empty stderr. In
    /// human mode commands write diagnostics to stderr directly and do not use
    /// this.
    pub(crate) fn log(self, message: &str) {
        if self.logging {
            // A best-effort diagnostic channel: a failed write to stderr must
            // not change the command's outcome, so the result is discarded.
            let _ = writeln!(io::stderr(), "{message}");
        }
    }
}
