use std::collections::BTreeMap;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::MutexGuard;
use std::sync::PoisonError;

use tokio_util::sync::CancellationToken;

use super::MAX_RETAINED_BYTES;
use super::MAX_USER_BYTES;
use super::OutputItem;
use super::pages::MeasuredItem;
use super::pages::OutputBuffer;

const MAX_SESSION_BYTES: usize = 64 * 1024 * 1024;

/// Cells and their transferred batches share a single byte ledger. Keeping the
/// buffers here lets eviction reclaim bytes from both running and finished cells.
#[derive(Clone, Debug, Default)]
pub(crate) struct OutputRetention(Arc<Mutex<RetentionState>>);

#[derive(Debug, Default)]
struct RetentionState {
    bytes: usize,
    cells: BTreeMap<u64, CellStorage>,
    evicted: Vec<u64>,
}

#[derive(Debug, Default)]
struct CellStorage {
    bytes: usize,
    clipped: bool,
    next_batch: u64,
    batches: HashMap<u64, OutputBuffer>,
    cancel: CancellationToken,
}

impl OutputRetention {
    fn lock(&self) -> MutexGuard<'_, RetentionState> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub(crate) fn register(&self, id: u64, cancel: CancellationToken) -> Arc<RetainedCell> {
        self.lock().cells.insert(
            id,
            CellStorage {
                cancel,
                ..Default::default()
            },
        );
        Arc::new(RetainedCell {
            id,
            retention: self.clone(),
        })
    }

    pub(crate) fn take_evicted(&self) -> Vec<u64> {
        std::mem::take(&mut self.lock().evicted)
    }
}

impl RetentionState {
    fn remove_cell(&mut self, id: u64) -> Option<CellStorage> {
        let cell = self.cells.remove(&id)?;
        self.bytes -= cell.bytes;
        Some(cell)
    }

    fn enforce_session_limit(&mut self) {
        while self.bytes > MAX_SESSION_BYTES {
            let Some((&id, _)) = self.cells.iter().find(|(_, cell)| cell.bytes != 0) else {
                unreachable!("retained bytes belong to a cell");
            };
            if let Some(cell) = self.remove_cell(id) {
                self.evicted.push(id);
                cell.cancel.cancel();
            }
        }
    }
}

#[derive(Debug)]
pub(crate) struct RetainedCell {
    id: u64,
    retention: OutputRetention,
}

impl RetainedCell {
    pub(crate) fn is_retained(&self) -> bool {
        self.retention.lock().cells.contains_key(&self.id)
    }

    pub(crate) fn discard(&self) {
        let removed = self.retention.lock().remove_cell(self.id);
        if let Some(cell) = removed {
            cell.cancel.cancel();
        }
    }

    pub(crate) fn batch(self: &Arc<Self>) -> ServerOutput {
        let mut state = self.retention.lock();
        let id = state.cells.get_mut(&self.id).map(|cell| {
            let id = cell.next_batch;
            cell.next_batch += 1;
            id
        });
        ServerOutput(Arc::new(Batch {
            cell: Arc::clone(self),
            id,
        }))
    }
}

impl Drop for RetainedCell {
    fn drop(&mut self) {
        self.retention.lock().remove_cell(self.id);
    }
}

#[derive(Debug)]
struct Batch {
    cell: Arc<RetainedCell>,
    id: Option<u64>,
}

impl Batch {
    fn remove(&self) -> Option<OutputBuffer> {
        let mut state = self.cell.retention.lock();
        let cell = state.cells.get_mut(&self.cell.id)?;
        let buffer = cell.batches.remove(&self.id?)?;
        cell.bytes -= buffer.bytes;
        if buffer.bytes != 0 {
            cell.clipped = false;
        }
        state.bytes -= buffer.bytes;
        Some(buffer)
    }
}

impl Drop for Batch {
    fn drop(&mut self) {
        self.remove();
    }
}

#[derive(Clone, Debug)]
pub(crate) struct ServerOutput(Arc<Batch>);

impl ServerOutput {
    pub(crate) fn take(&mut self) -> Self {
        let next = self.0.cell.batch();
        std::mem::replace(self, next)
    }

    pub(super) fn push(&self, item: OutputItem) {
        self.push_item(MeasuredItem::new(item), 0, false);
    }

    pub(super) fn push_error(&self, text: String, lost: usize) {
        self.push_item(
            MeasuredItem::new(OutputItem::InputText { text }),
            lost,
            true,
        );
    }

    fn push_item(&self, mut item: MeasuredItem, mut lost: usize, diagnostic: bool) {
        let Some(batch) = self.0.id else { return };
        let mut state = self.0.cell.retention.lock();
        let Some(cell) = state.cells.get_mut(&self.0.cell.id) else {
            return;
        };
        let buffer = cell.batches.entry(batch).or_default();
        let capacity = if diagnostic {
            MAX_RETAINED_BYTES - cell.bytes
        } else if cell.clipped {
            0
        } else {
            MAX_USER_BYTES.saturating_sub(cell.bytes)
        };
        let original_bytes = item.bytes;
        let mut admitted = 0;
        let mut clipped = false;
        match &mut item.item {
            OutputItem::InputText { text } => {
                let packed = !diagnostic
                    && buffer.items.len() >= 1024
                    && buffer.items.back().is_some_and(|previous| {
                        matches!(previous.item, OutputItem::InputText { .. })
                            && previous.bytes + 1 + original_bytes <= 4096
                    });
                let available = capacity.saturating_sub(usize::from(packed));
                let end = text.floor_char_boundary(text.len().min(available));
                clipped = end < original_bytes;
                lost += original_bytes - end;
                if end != 0 {
                    admitted = end;
                    if packed {
                        let previous = buffer
                            .items
                            .back_mut()
                            .expect("packing requires a text item");
                        previous.append_text("\n");
                        previous.append_text(&text[..end]);
                        admitted += 1;
                    } else {
                        text.truncate(end);
                        text.shrink_to_fit();
                        buffer.items.push_back(MeasuredItem::new(item.item));
                    }
                }
            }
            _ if original_bytes <= capacity && (!cell.clipped || diagnostic) => {
                admitted = original_bytes;
                buffer.items.push_back(item);
            }
            _ => {
                clipped = true;
                lost += original_bytes;
            }
        }
        buffer.discarded = buffer.discarded.saturating_add(lost as u64);
        buffer.bytes += admitted;
        cell.bytes += admitted;
        cell.clipped |= clipped;
        state.bytes += admitted;
        state.enforce_session_limit();
    }

    pub(crate) fn append(&self, other: Self) {
        let (Some(batch), Some(source)) = (self.0.id, other.0.id) else {
            return;
        };
        debug_assert!(Arc::ptr_eq(&self.0.cell, &other.0.cell));
        let mut state = self.0.cell.retention.lock();
        if let Some(cell) = state.cells.get_mut(&self.0.cell.id)
            && let Some(buffer) = cell.batches.remove(&source)
        {
            cell.batches.entry(batch).or_default().append(buffer);
        }
    }

    pub(crate) fn next(
        &self,
        limit: Option<usize>,
        require_output: bool,
    ) -> Option<(Vec<OutputItem>, bool)> {
        let mut state = self.0.cell.retention.lock();
        let cell = state.cells.get_mut(&self.0.cell.id)?;
        let buffer = cell.batches.entry(self.0.id?).or_default();
        if require_output && !buffer.has_output() {
            return None;
        }
        let before = buffer.bytes;
        let page = buffer.next(limit);
        let drained = before - buffer.bytes;
        let more = buffer.has_output();
        if drained != 0 || !more {
            cell.clipped = false;
        }
        cell.bytes -= drained;
        state.bytes -= drained;
        Some((page, more))
    }

    pub(super) fn into_items(self) -> Vec<OutputItem> {
        self.0
            .remove()
            .map_or_else(Vec::new, OutputBuffer::into_items)
    }
}

#[cfg(test)]
impl PartialEq for ServerOutput {
    fn eq(&self, other: &Self) -> bool {
        let items = |output: &Self| {
            let state = output.0.cell.retention.lock();
            state
                .cells
                .get(&output.0.cell.id)
                .and_then(|cell| cell.batches.get(&output.0.id?))
                .filter(|buffer| buffer.has_output())
                .map(|buffer| buffer.clone())
        };
        items(self) == items(other)
    }
}
