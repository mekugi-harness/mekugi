use codex_code_mode_protocol::EnabledToolMetadata;
use codex_code_mode_protocol::normalize_code_mode_identifier;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::PoisonError;
use std::time::Duration;

use codex_code_mode_protocol::CellId;
use codex_code_mode_protocol::CodeModeNestedToolCall;
use codex_code_mode_protocol::CodeModeOutputTruncation;
use codex_code_mode_protocol::CodeModeSession;
use codex_code_mode_protocol::CodeModeSessionCellExecutionLimits;
use codex_code_mode_protocol::CodeModeSessionDelegate;
use codex_code_mode_protocol::CodeModeSessionOptions;
use codex_code_mode_protocol::CodeModeSessionResultFuture;
use codex_code_mode_protocol::CodeModeToolKind;
use codex_code_mode_protocol::DEFAULT_EXEC_YIELD_TIME_MS;
use codex_code_mode_protocol::ExecuteRequest;
use codex_code_mode_protocol::ExecuteToPendingOutcome;
use codex_code_mode_protocol::FunctionCallOutputContentItem;
use codex_code_mode_protocol::RuntimeResponse;
use codex_code_mode_protocol::StartedCell;
use codex_code_mode_protocol::TerminateRequest;
use codex_code_mode_protocol::WaitOutcome;
use codex_code_mode_protocol::WaitRequest;
use codex_code_mode_protocol::WaitToPendingOutcome;
use codex_code_mode_protocol::WaitToPendingRequest;
use serde_json::Value as JsonValue;
use tokio_util::sync::CancellationToken;

use crate::TaskFailureHandler;
use crate::cell_actor::take_completed_event;
use crate::output::OutputCollector;
use crate::output::RetainedCell;
use crate::output::ServerOutput;
use crate::session_runtime as runtime;
use crate::session_runtime::SessionRuntime;

const YIELD_GRACE_PERIOD: Duration = Duration::from_secs(1);
const MIN_YIELD_TIME_FOR_GRACE: Duration = Duration::from_secs(10);

/// Reports a timed observation's pause, its continued liveness, or its resumption.
pub type ObservationLiveness = Box<dyn Fn(bool) + Send + Sync>;

pub struct InProcessCodeModeSession {
    runtime: SessionRuntime,
    catalog: Mutex<(u64, runtime::ToolCatalog)>,
    pages: Arc<Mutex<CellPages>>,
    options: CodeModeSessionOptions,
}

impl InProcessCodeModeSession {
    pub fn new() -> Self {
        Self::with_limits(CodeModeSessionCellExecutionLimits::default())
    }

    pub fn with_limits(cell_execution_limits: CodeModeSessionCellExecutionLimits) -> Self {
        Self::with_options(
            CodeModeSessionOptions {
                cell_execution_limits,
                ..CodeModeSessionOptions::default()
            },
            /*task_failure_handler*/ None,
        )
    }

    /// Select output handling at creation. A session's mode must remain fixed
    /// for all of its cells and observations.
    pub fn with_options(
        options: CodeModeSessionOptions,
        task_failure_handler: Option<TaskFailureHandler>,
    ) -> Self {
        Self {
            runtime: SessionRuntime::new_with_task_failure_handler(task_failure_handler),
            catalog: Default::default(),
            pages: Default::default(),
            options: CodeModeSessionOptions {
                cell_execution_limits: CodeModeSessionCellExecutionLimits {
                    max_heap_size_bytes: None,
                    ..options.cell_execution_limits
                },
                ..options
            },
        }
    }

    fn runtime_request(
        &self,
        request: ExecuteRequest,
        revision: Option<u64>,
    ) -> Result<runtime::CreateCellRequest, String> {
        let tools: Vec<EnabledToolMetadata> = request
            .enabled_tools
            .into_iter()
            .map(|tool| EnabledToolMetadata {
                pauses_cell_deadline: tool.pauses_cell_deadline,
                global_name: normalize_code_mode_identifier(&tool.name),
                tool_name: tool.tool_name,
                description: tool.description,
                kind: tool.kind,
            })
            .collect();
        let mut tools: runtime::ToolCatalog = if revision.is_some() && !tools.is_empty() {
            EnabledToolMetadata::shared_catalog(tools).into()
        } else {
            tools.into_iter().map(Arc::new).collect()
        };
        if let Some(revision) = revision {
            let mut cache = self.catalog.lock().unwrap_or_else(PoisonError::into_inner);
            if revision == 0 {
                return Err("invalid code-mode catalog revision".into());
            }
            if tools.is_empty() {
                if revision != cache.0 {
                    return Err("unknown code-mode catalog revision".into());
                }
                tools = Arc::clone(&cache.1);
            } else if revision > cache.0 {
                *cache = (revision, Arc::clone(&tools));
            } else if revision == cache.0 && tools != cache.1 {
                return Err("conflicting code-mode catalog revision".into());
            }
            // A late full request runs with its own tools after cancellation.
        }
        Ok(runtime::CreateCellRequest {
            output_truncation: self.options.output_truncation,
            tool_call_id: request.tool_call_id,
            enabled_tools: tools,
            source: request.source,
        })
    }

    pub async fn execute(
        &self,
        request: ExecuteRequest,
        delegate: Arc<dyn CodeModeSessionDelegate>,
        preempt: Option<CancellationToken>,
    ) -> Result<StartedCell, String> {
        self.execute_with_catalog(request, /*catalog_revision*/ None, delegate, preempt)
            .await
    }

    /// A nonempty catalog installs a session-local revision. Subsequent cells
    /// retain that immutable snapshot while newer revisions can change the tools.
    pub async fn execute_with_catalog(
        &self,
        request: ExecuteRequest,
        catalog_revision: Option<u64>,
        delegate: Arc<dyn CodeModeSessionDelegate>,
        preempt: Option<CancellationToken>,
    ) -> Result<StartedCell, String> {
        self.execute_with_observed_catalog(request, catalog_revision, delegate, preempt, None)
            .await
    }

    pub async fn execute_with_observed_catalog(
        &self,
        request: ExecuteRequest,
        catalog_revision: Option<u64>,
        delegate: Arc<dyn CodeModeSessionDelegate>,
        preempt: Option<CancellationToken>,
        liveness: Option<ObservationLiveness>,
    ) -> Result<StartedCell, String> {
        let yield_time_ms = request.yield_time_ms.unwrap_or(DEFAULT_EXEC_YIELD_TIME_MS);
        let max_output_tokens = request.max_output_tokens;
        let started = self
            .runtime
            .execute(
                self.runtime_request(request, catalog_revision)?,
                runtime::ObserveMode::YieldAfter(self.resolve_yield_timeout(yield_time_ms))
                    .with_yield_signal(preempt.unwrap_or_default())
                    .with_liveness(liveness),
                Arc::new(ProtocolDelegate { delegate }),
            )
            .await
            .map_err(|error| error.to_string())?;
        let cell_id = protocol_cell_id(&started.cell_id);
        let mode = self.options.output_truncation;
        if let Some(cell) = started.retained.as_ref() {
            let mut pages = self.pages.lock().unwrap_or_else(PoisonError::into_inner);
            for evicted in self.runtime.take_evicted_outputs() {
                pages.remove(&CellId::new(evicted.to_string()));
            }
            if cell.is_retained() {
                pages.insert(
                    cell_id.clone(),
                    CellPagesEntry {
                        cell: Arc::clone(cell),
                        output: cell.batch(),
                        terminal: None,
                    },
                );
            }
        }
        let owner = InitialOutputOwner {
            pages: Arc::clone(&self.pages),
            cell_id: Some(cell_id.clone()),
        };
        let response_cell_id = cell_id.clone();
        Ok(StartedCell::from_future(cell_id, async move {
            let mut owner = owner;
            let event = started.initial_event().await;
            // Once polled to delivery, only the cell/page lifecycle owns the output.
            owner.cell_id = None;
            let mut pages = owner.pages.lock().unwrap_or_else(PoisonError::into_inner);
            match event {
                Ok(event) => {
                    observe_output(&mut pages, &response_cell_id, event, max_output_tokens)
                        .and_then(|page| {
                            page.map_or_else(|| missing_cell_response(response_cell_id, mode), Ok)
                        })
                }
                // A concurrent terminate can still own the event.
                Err(error @ runtime::Error::ClosedCell(_)) => Err(error.to_string()),
                Err(error) => {
                    pages.remove(&response_cell_id);
                    Err(error.to_string())
                }
            }
        }))
    }

    pub async fn execute_to_pending(
        &self,
        request: ExecuteRequest,
        delegate: Arc<dyn CodeModeSessionDelegate>,
    ) -> Result<ExecuteToPendingOutcome, String> {
        let started = self
            .runtime
            .execute(
                self.runtime_request(request, /*revision*/ None)?,
                runtime::ObserveMode::PendingFrontier,
                Arc::new(ProtocolDelegate { delegate }),
            )
            .await
            .map_err(|error| error.to_string())?;
        let cell_id = protocol_cell_id(&started.cell_id);
        let event = started
            .initial_event()
            .await
            .map_err(|error| error.to_string())?;
        pending_outcome(&cell_id, event)
    }

    pub async fn wait(
        &self,
        request: WaitRequest,
        preempt: Option<CancellationToken>,
    ) -> Result<WaitOutcome, String> {
        self.wait_observed(request, preempt, None).await
    }

    pub async fn wait_observed(
        &self,
        request: WaitRequest,
        preempt: Option<CancellationToken>,
        liveness: Option<ObservationLiveness>,
    ) -> Result<WaitOutcome, String> {
        self.begin_wait(request, preempt, liveness).await.await
    }

    async fn begin_wait(
        &self,
        request: WaitRequest,
        preempt: Option<CancellationToken>,
        liveness: Option<ObservationLiveness>,
    ) -> CodeModeSessionResultFuture<'static, WaitOutcome> {
        let WaitRequest {
            cell_id,
            yield_time_ms,
            max_output_tokens,
        } = request;
        if let Some(response) = next_retained_page(
            &mut self.pages.lock().unwrap_or_else(PoisonError::into_inner),
            &cell_id,
            max_output_tokens,
        ) {
            return Box::pin(async move { Ok(WaitOutcome::LiveCell(response)) });
        }
        let pages = Arc::clone(&self.pages);
        let output_truncation = self.options.output_truncation;
        let runtime_cell_id = runtime_cell_id(&cell_id);
        match self
            .runtime
            .begin_observe(
                &runtime_cell_id,
                runtime::ObserveMode::YieldAfter(self.resolve_yield_timeout(yield_time_ms))
                    .with_yield_signal(preempt.unwrap_or_default())
                    .with_liveness(liveness),
            )
            .await
        {
            Ok(pending_event) => Box::pin(async move {
                finish_wait(
                    &pages,
                    cell_id,
                    max_output_tokens,
                    output_truncation,
                    pending_event.event().await,
                )
            }),
            Err(error) => Box::pin(async move {
                finish_wait(
                    &pages,
                    cell_id,
                    max_output_tokens,
                    output_truncation,
                    Err(error),
                )
            }),
        }
    }

    pub async fn terminate(&self, request: TerminateRequest) -> Result<WaitOutcome, String> {
        let TerminateRequest {
            cell_id,
            max_output_tokens,
            discard_output,
        } = request;
        if let Some(response) = release_completed_pages(&self.pages, &cell_id) {
            return Ok(WaitOutcome::LiveCell(response));
        }
        if discard_output {
            let discarded = discard_cell(&self.pages, &cell_id);
            // Cancel through the runtime as well for unpaged Client sessions.
            let event = self.runtime.terminate(&runtime_cell_id(&cell_id)).await;
            return match event {
                Ok(event) => Ok(WaitOutcome::LiveCell(runtime_response(&cell_id, event)?.0)),
                Err(runtime::Error::MissingCell(_) | runtime::Error::ClosedCell(_))
                    if discarded =>
                {
                    Ok(WaitOutcome::LiveCell(RuntimeResponse::Terminated {
                        cell_id,
                        content_items: Vec::new(),
                        code_mode_host_duration: None,
                    }))
                }
                Err(error) => finish_wait(
                    &self.pages,
                    cell_id,
                    max_output_tokens,
                    self.options.output_truncation,
                    Err(error),
                ),
            };
        }
        let event = self.runtime.terminate(&runtime_cell_id(&cell_id)).await;
        if let Some(response) = release_completed_pages(&self.pages, &cell_id) {
            return Ok(WaitOutcome::LiveCell(response));
        }
        finish_wait(
            &self.pages,
            cell_id,
            max_output_tokens,
            self.options.output_truncation,
            event,
        )
    }

    pub async fn wait_to_pending(
        &self,
        request: WaitToPendingRequest,
    ) -> Result<WaitToPendingOutcome, String> {
        let cell_id = request.cell_id;
        match self
            .runtime
            .observe(
                &runtime_cell_id(&cell_id),
                runtime::ObserveMode::PendingFrontier,
            )
            .await
        {
            Ok(event) => Ok(WaitToPendingOutcome::LiveCell(pending_outcome(
                &cell_id, event,
            )?)),
            Err(runtime::Error::MissingCell(_) | runtime::Error::ClosedCell(_)) => {
                Ok(WaitToPendingOutcome::MissingCell(missing_cell_response(
                    cell_id,
                    self.options.output_truncation,
                )?))
            }
            Err(error) => Err(error.to_string()),
        }
    }

    pub async fn shutdown(&self) -> Result<(), String> {
        self.pages
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clear();
        self.runtime
            .shutdown()
            .await
            .map_err(|error| error.to_string())
    }

    fn resolve_yield_timeout(&self, yield_time_ms: u64) -> Duration {
        let yield_time = Duration::from_millis(yield_time_ms);
        let timeout = if yield_time >= MIN_YIELD_TIME_FOR_GRACE {
            yield_time.saturating_add(YIELD_GRACE_PERIOD)
        } else {
            yield_time
        };

        self.options
            .cell_execution_limits
            .max_yield_time_ms
            .map(Duration::from_millis)
            .map_or(timeout, |limit| timeout.min(limit))
    }
}

impl Default for InProcessCodeModeSession {
    fn default() -> Self {
        Self::new()
    }
}

impl CodeModeSession for InProcessCodeModeSession {
    fn output_truncation(&self) -> CodeModeOutputTruncation {
        self.options.output_truncation
    }

    fn execute<'a>(
        &'a self,
        request: ExecuteRequest,
        delegate: Arc<dyn CodeModeSessionDelegate>,
        preempt: Option<CancellationToken>,
    ) -> CodeModeSessionResultFuture<'a, StartedCell> {
        Box::pin(InProcessCodeModeSession::execute(
            self, request, delegate, preempt,
        ))
    }

    fn wait<'a>(
        &'a self,
        request: WaitRequest,
        preempt: Option<CancellationToken>,
    ) -> CodeModeSessionResultFuture<'a, WaitOutcome> {
        Box::pin(InProcessCodeModeSession::wait(self, request, preempt))
    }

    fn terminate<'a>(
        &'a self,
        request: TerminateRequest,
    ) -> CodeModeSessionResultFuture<'a, WaitOutcome> {
        Box::pin(InProcessCodeModeSession::terminate(self, request))
    }

    fn shutdown<'a>(&'a self) -> CodeModeSessionResultFuture<'a, ()> {
        Box::pin(InProcessCodeModeSession::shutdown(self))
    }
}

struct ProtocolDelegate {
    delegate: Arc<dyn CodeModeSessionDelegate>,
}

impl runtime::SessionRuntimeDelegate for ProtocolDelegate {
    #[tracing::instrument(
        name = "code_mode.runtime.invoke_tool",
        level = "info",
        skip_all,
        fields(
            cell.id = %invocation.cell_id,
            runtime_tool_call_id = invocation.runtime_tool_call_id.as_str(),
            tool_name = invocation.tool_name.name.as_str(),
            tool_namespace = invocation.tool_name.namespace.as_deref(),
        )
    )]
    async fn invoke_tool(
        &self,
        invocation: runtime::NestedToolCall,
        cancellation_token: CancellationToken,
    ) -> Result<JsonValue, String> {
        self.delegate
            .invoke_tool(
                CodeModeNestedToolCall {
                    cell_id: protocol_cell_id(&invocation.cell_id),
                    runtime_tool_call_id: invocation.runtime_tool_call_id,
                    tool_name: codex_protocol::ToolName {
                        name: invocation.tool_name.name,
                        namespace: invocation.tool_name.namespace,
                    },
                    tool_kind: match invocation.tool_kind {
                        runtime::ToolKind::Function => CodeModeToolKind::Function,
                        runtime::ToolKind::Freeform => CodeModeToolKind::Freeform,
                    },
                    input: invocation.input,
                },
                cancellation_token,
            )
            .await
    }

    async fn notify(
        &self,
        call_id: String,
        cell_id: runtime::CellId,
        text: String,
        cancellation_token: CancellationToken,
    ) -> Result<(), String> {
        self.delegate
            .notify(
                call_id,
                protocol_cell_id(&cell_id),
                text,
                cancellation_token,
            )
            .await
    }

    fn cell_closed(&self, cell_id: &runtime::CellId) {
        self.delegate.cell_closed(&protocol_cell_id(cell_id));
    }
}

fn runtime_cell_id(cell_id: &CellId) -> runtime::CellId {
    runtime::CellId::new(cell_id.as_str())
}

fn protocol_cell_id(cell_id: &runtime::CellId) -> CellId {
    CellId::new(cell_id.as_str().to_string())
}

type CellPages = HashMap<CellId, CellPagesEntry>;

struct CellPagesEntry {
    cell: Arc<RetainedCell>,
    output: ServerOutput,
    terminal: Option<RuntimeResponse>,
}

struct InitialOutputOwner {
    pages: Arc<Mutex<CellPages>>,
    cell_id: Option<CellId>,
}

impl Drop for InitialOutputOwner {
    fn drop(&mut self) {
        if let Some(cell_id) = &self.cell_id {
            discard_cell(&self.pages, cell_id);
        }
    }
}

fn discard_cell(pages: &Mutex<CellPages>, cell_id: &CellId) -> bool {
    let entry = pages
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .remove(cell_id);
    if let Some(entry) = entry {
        entry.cell.discard();
        true
    } else {
        false
    }
}

fn release_completed_pages(pages: &Mutex<CellPages>, cell_id: &CellId) -> Option<RuntimeResponse> {
    let mut pages = pages.lock().unwrap_or_else(PoisonError::into_inner);
    let entry = pages.get_mut(cell_id)?;
    if !entry.cell.is_retained() {
        pages.remove(cell_id);
        return None;
    }
    let mut terminal = entry.terminal.take()?;
    pages.remove(cell_id);
    terminal.set_content_items(vec![FunctionCallOutputContentItem::PageBoundary {
        more_output: false,
        continuation: true,
    }]);
    Some(terminal)
}

fn finish_wait(
    pages: &Mutex<CellPages>,
    cell_id: CellId,
    max_output_tokens: Option<usize>,
    mode: CodeModeOutputTruncation,
    event: Result<runtime::CellEvent, runtime::Error>,
) -> Result<WaitOutcome, String> {
    let mut pages = pages.lock().unwrap_or_else(PoisonError::into_inner);
    let response = match event {
        Ok(event) => observe_output(&mut pages, &cell_id, event, max_output_tokens)?,
        Err(runtime::Error::MissingCell(_) | runtime::Error::ClosedCell(_)) => {
            // The terminate caller may still be publishing the terminal event.
            next_retained_page(&mut pages, &cell_id, max_output_tokens)
        }
        Err(error) => return Err(error.to_string()),
    };
    match response {
        Some(response) => Ok(WaitOutcome::LiveCell(response)),
        None => Ok(WaitOutcome::MissingCell(missing_cell_response(
            cell_id, mode,
        )?)),
    }
}

fn observe_output(
    pages: &mut CellPages,
    cell_id: &CellId,
    event: runtime::CellEvent,
    limit: Option<usize>,
) -> Result<Option<RuntimeResponse>, String> {
    let (mut response, output) = runtime_response(cell_id, event)?;
    let OutputCollector::Server(output) = output else {
        response.set_content_items(output.into_items());
        return Ok(Some(response));
    };
    let Some(entry) = pages.get_mut(cell_id) else {
        return Ok(None);
    };
    entry.output.append(output);
    let Some((mut page, more_output)) = entry.output.next(limit, /*require_output*/ false) else {
        pages.remove(cell_id);
        return Ok(None);
    };
    if more_output {
        page.push(FunctionCallOutputContentItem::PageBoundary {
            more_output,
            continuation: false,
        });
    }
    if !matches!(response, RuntimeResponse::Yielded { .. }) {
        if more_output {
            let mut terminal = response.clone();
            if let RuntimeResponse::Result {
                error_text: Some(error),
                ..
            } = &mut terminal
            {
                *error = String::new(); // The failure diagnostic is already queued.
            }
            entry.terminal = Some(terminal);
        } else {
            pages.remove(cell_id);
        }
    }
    response.set_content_items(page);
    Ok(Some(response))
}

fn next_retained_page(
    pages: &mut CellPages,
    cell_id: &CellId,
    limit: Option<usize>,
) -> Option<RuntimeResponse> {
    let entry = pages.get_mut(cell_id)?;
    let (mut page, more_output) = entry.output.next(limit, /*require_output*/ true)?;
    let continuation = entry.terminal.is_some();
    if more_output || continuation {
        page.push(FunctionCallOutputContentItem::PageBoundary {
            more_output,
            continuation,
        });
    }
    let response = if let Some(mut terminal) = entry.terminal.clone() {
        if !more_output {
            pages.remove(cell_id);
        }
        terminal.set_content_items(page);
        terminal
    } else {
        RuntimeResponse::Yielded {
            cell_id: cell_id.clone(),
            content_items: page,
            code_mode_host_duration: None,
        }
    };
    Some(response)
}

fn pending_outcome(
    cell_id: &CellId,
    event: runtime::CellEvent,
) -> Result<ExecuteToPendingOutcome, String> {
    match event {
        runtime::CellEvent::Pending {
            content_items,
            pending_tool_call_ids,
        } => Ok(ExecuteToPendingOutcome::Pending {
            cell_id: cell_id.clone(),
            content_items: content_items.into_items(),
            pending_tool_call_ids,
        }),
        event => {
            let (mut response, output) = runtime_response(cell_id, event)?;
            response.set_content_items(output.into_items());
            Ok(ExecuteToPendingOutcome::Completed(response))
        }
    }
}

fn runtime_response(
    cell_id: &CellId,
    event: runtime::CellEvent,
) -> Result<(RuntimeResponse, OutputCollector), String> {
    match event {
        runtime::CellEvent::Yielded { content_items } => Ok((
            RuntimeResponse::Yielded {
                cell_id: cell_id.clone(),
                content_items: Vec::new(),
                code_mode_host_duration: None,
            },
            content_items,
        )),
        runtime::CellEvent::Completed {
            content_items,
            error_text,
            result_buffered_for,
        } => Ok((
            RuntimeResponse::Result {
                cell_id: cell_id.clone(),
                content_items: Vec::new(),
                error_text,
                result_buffered_for,
                code_mode_host_duration: None,
            },
            content_items,
        )),
        runtime::CellEvent::Terminated { content_items } => Ok((
            RuntimeResponse::Terminated {
                cell_id: cell_id.clone(),
                content_items: Vec::new(),
                code_mode_host_duration: None,
            },
            content_items,
        )),
        runtime::CellEvent::Pending { .. } => {
            Err("cell returned a pending frontier unexpectedly".to_string())
        }
    }
}

fn missing_cell_response(
    cell_id: CellId,
    mode: CodeModeOutputTruncation,
) -> Result<RuntimeResponse, String> {
    let error_text = format!("exec cell {cell_id} not found");
    let mut output = OutputCollector::new(mode);
    let event = take_completed_event(&mut output, Some(error_text));
    let (mut response, output) = runtime_response(&cell_id, event)?;
    response.set_content_items(output.into_items());
    Ok(response)
}

#[cfg(test)]
#[path = "service_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "service_contract_tests.rs"]
mod contract_tests;

#[cfg(test)]
#[path = "service_output_collection_tests.rs"]
mod output_collection_tests;
