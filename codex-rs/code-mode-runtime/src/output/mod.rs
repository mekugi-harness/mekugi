//! Bound unread Server output across the actor, delivery events and wait pages.

use codex_code_mode_protocol::CodeModeOutputTruncation;
use codex_code_mode_protocol::FunctionCallOutputContentItem as OutputItem;
use codex_utils_string::truncate_middle_with_marker;

mod pages;
mod retention;

pub(crate) use retention::OutputRetention;
pub(crate) use retention::RetainedCell;
pub(crate) use retention::ServerOutput;

/// Reserve room for a bounded failure diagnostic even when user output filled the cell.
pub(crate) const MAX_SERVER_DIAGNOSTIC_BYTES: usize = 64 * 1024;
const ERROR_LABEL: &str = "Script error:\n";
const MAX_RETAINED_BYTES: usize = 16 * 1024 * 1024;
const MAX_USER_BYTES: usize = MAX_RETAINED_BYTES - MAX_SERVER_DIAGNOSTIC_BYTES;

#[derive(Clone, Debug)]
#[cfg_attr(test, derive(PartialEq))]
pub(crate) enum OutputCollector {
    Client(Vec<OutputItem>),
    Server(ServerOutput),
}

impl OutputCollector {
    pub(crate) fn new(mode: CodeModeOutputTruncation) -> Self {
        match mode {
            CodeModeOutputTruncation::Client => Self::Client(Vec::new()),
            CodeModeOutputTruncation::Server => {
                let cell = OutputRetention::default().register(1, Default::default());
                Self::Server(cell.batch())
            }
        }
    }

    pub(crate) fn push(&mut self, item: OutputItem) {
        match self {
            Self::Client(items) => items.push(item),
            Self::Server(output) => output.push(item),
        }
    }

    pub(crate) fn collect_error(&mut self, error: String) -> String {
        if let Self::Server(output) = self {
            let end = error.floor_char_boundary(
                error
                    .len()
                    .min(MAX_SERVER_DIAGNOSTIC_BYTES - ERROR_LABEL.len()),
            );
            output.push_error(format!("{ERROR_LABEL}{}", &error[..end]), error.len() - end);
            if error.capacity() > MAX_SERVER_DIAGNOSTIC_BYTES {
                return truncate_middle_with_marker(&error, MAX_SERVER_DIAGNOSTIC_BYTES, "…");
            }
        }
        error
    }

    pub(crate) fn take(&mut self) -> Self {
        match self {
            Self::Client(items) => Self::Client(std::mem::take(items)),
            Self::Server(output) => Self::Server(output.take()),
        }
    }

    pub(crate) fn try_deliver(&mut self, send: impl FnOnce(Self) -> Result<(), Self>) -> bool {
        if let Err(undelivered) = send(self.take()) {
            *self = undelivered;
            return false;
        }
        true
    }

    pub(crate) fn into_items(self) -> Vec<OutputItem> {
        match self {
            Self::Client(items) => items,
            Self::Server(output) => output.into_items(),
        }
    }
}

fn append_discard_notice(items: &mut Vec<OutputItem>, discarded: u64) {
    if discarded != 0 {
        items.push(OutputItem::InputText {
            text: format!("Code Mode output limit reached: discarded {discarded} bytes"),
        });
    }
}

#[cfg(test)]
#[path = "output_tests.rs"]
mod tests;
