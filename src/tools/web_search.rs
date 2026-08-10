use std::sync::Arc;

use serde_json::{json, Value};

use super::{
    require_str, Tool, ToolCapabilities, ToolContext, ToolError, ToolErrorCode, ToolOutput,
    ToolPermissionSpec, ToolSpec,
};
use crate::util;
use crate::web::{
    build_external_backend, WebCapabilityBinding, WebSearchBackend, WebSearchError,
    WebSearchErrorKind, WebSearchRequest, WebSearchResponse, WebSearchSettings,
};

const MAX_QUERY_CHARS: usize = 500;

pub(super) struct WebSearchTool {
    backend: Arc<dyn WebSearchBackend>,
    settings: WebSearchSettings,
}

impl WebSearchTool {
    fn new(backend: Arc<dyn WebSearchBackend>, settings: WebSearchSettings) -> Self {
        Self { backend, settings }
    }
}

pub(super) fn from_binding(
    binding: &WebCapabilityBinding,
) -> Result<Option<Box<dyn Tool>>, String> {
    let WebCapabilityBinding::HarnessFunction { settings, .. } = binding else {
        return Ok(None);
    };
    let backend = build_external_backend(binding)?
        .ok_or_else(|| "external Web binding did not produce a backend".to_string())?;
    Ok(Some(Box::new(WebSearchTool::new(
        backend,
        settings.clone(),
    ))))
}

impl Tool for WebSearchTool {
    fn spec(&self) -> ToolSpec {
        let max_results = match self.settings.context_size {
            Some(crate::web::WebSearchContextSize::Low) => 3,
            Some(crate::web::WebSearchContextSize::High) => 10,
            Some(crate::web::WebSearchContextSize::Medium) | None => 5,
        };
        ToolSpec {
            name: "web_search".into(),
            description: format!(
                "Search the current public web using the frozen {} backend and return up to {} bounded sources with URLs and excerpts. The query is sent to an external service and requires approval. Host-configured domain filters cannot be changed by tool arguments.",
                self.backend.kind().label(),
                max_results
            ),
            schema: json!({
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "query": {
                        "type": "string",
                        "minLength": 1,
                        "maxLength": MAX_QUERY_CHARS,
                        "description": "Focused web search query"
                    }
                },
                "required": ["query"]
            }),
            capabilities: ToolCapabilities::READ_ONLY,
            permission: ToolPermissionSpec::opaque_side_effect(&[]),
        }
    }

    fn execute(&self, args: &Value, ctx: &mut ToolContext<'_>) -> Result<ToolOutput, ToolError> {
        let query = normalize_query(require_str(args, "query")?)?;
        ctx.report_progress(ToolOutput {
            model_text: format!("Searching the web for {query:?}"),
            ui_summary: Some(format!("Searching with {}", self.backend.kind().label())),
            details: Some(json!({
                "backend": self.backend.kind(),
                "status": "started"
            })),
        });
        let response = self
            .backend
            .search(
                &WebSearchRequest {
                    query,
                    settings: self.settings.clone(),
                },
                ctx.cancel,
            )
            .map_err(map_error)?;
        Ok(format_response(response))
    }
}

fn normalize_query(value: &str) -> Result<String, ToolError> {
    let query = util::sanitize(value)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    if query.is_empty() {
        return Err(ToolError::invalid_arguments(
            "web_search query must contain non-whitespace text",
        ));
    }
    if query.chars().count() > MAX_QUERY_CHARS {
        return Err(ToolError::invalid_arguments(format!(
            "web_search query exceeds {MAX_QUERY_CHARS} characters"
        )));
    }
    Ok(query)
}

fn format_response(response: WebSearchResponse) -> ToolOutput {
    let mut model_text = if response.sources.is_empty() {
        format!(
            "No web results found for {:?} using {}.",
            response.query,
            response.backend.label()
        )
    } else {
        format!(
            "Web results for {:?} using {} ({} sources):",
            response.query,
            response.backend.label(),
            response.sources.len()
        )
    };
    for (index, source) in response.sources.iter().enumerate() {
        let title = source.title.as_deref().unwrap_or("Untitled source");
        let locator = source.locator.as_deref().unwrap_or(&source.id);
        model_text.push_str(&format!("\n\n{}. {}\n   {}", index + 1, title, locator));
        if let Some(preview) = &source.content_preview {
            model_text.push_str(&format!("\n   {preview}"));
        }
    }
    let details = json!({
        "backend": response.backend,
        "query": response.query,
        "result_count": response.sources.len(),
        "request_id": response.request_id,
        "response_time_seconds": response.response_time_seconds,
        "metadata": response.metadata,
        "sources": response.sources
    });
    let result_count = details["result_count"].as_u64().unwrap_or(0);
    ToolOutput {
        model_text,
        ui_summary: Some(format!(
            "{} returned {} web source{}",
            response.backend.label(),
            result_count,
            if result_count == 1 { "" } else { "s" }
        )),
        details: Some(details),
    }
}

fn map_error(error: WebSearchError) -> ToolError {
    let code = match error.kind {
        WebSearchErrorKind::Aborted => ToolErrorCode::Aborted,
        WebSearchErrorKind::Timeout => ToolErrorCode::Timeout,
        WebSearchErrorKind::Network
        | WebSearchErrorKind::Http
        | WebSearchErrorKind::InvalidResponse => ToolErrorCode::ExecutionFailed,
    };
    ToolError {
        code,
        message: error.message,
        retryable: error.retryable,
        details: Some(json!({
            "backend": error.backend,
            "status": error.status,
            "retryable": error.retryable
        })),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::sync::atomic::AtomicBool;

    use super::*;
    use crate::plan::PlanSnapshot;
    use crate::web::{Source, SourceKind, WebSearchBackendKind};
    use crate::workspace::Workspace;

    struct FakeBackend {
        response: Result<WebSearchResponse, WebSearchError>,
    }

    impl WebSearchBackend for FakeBackend {
        fn kind(&self) -> WebSearchBackendKind {
            WebSearchBackendKind::Tavily
        }

        fn search(
            &self,
            _request: &WebSearchRequest,
            _cancel: &AtomicBool,
        ) -> Result<WebSearchResponse, WebSearchError> {
            self.response.clone()
        }
    }

    fn execute(tool: &WebSearchTool, args: Value) -> super::super::ToolOutcome {
        let workspace = Workspace::new(std::env::current_dir().unwrap());
        let cancel = AtomicBool::new(false);
        let mut progress = |_| {};
        let mut context = ToolContext {
            workspace: &workspace,
            cancel: &cancel,
            session_id: "test",
            current_plan: PlanSnapshot::default(),
            progress: &mut progress,
            effects: Vec::new(),
        };
        super::super::ToolRegistry::new(vec![Box::new(WebSearchTool::new(
            Arc::clone(&tool.backend),
            tool.settings.clone(),
        ))])
        .execute("web_search", &args, &mut context)
    }

    #[test]
    fn tool_is_bounded_read_only_and_always_requires_approval() {
        let tool = WebSearchTool::new(
            Arc::new(FakeBackend {
                response: Ok(WebSearchResponse {
                    backend: WebSearchBackendKind::Tavily,
                    query: "rust agents".into(),
                    sources: Vec::new(),
                    request_id: None,
                    response_time_seconds: None,
                    metadata: BTreeMap::new(),
                }),
            }),
            WebSearchSettings::default(),
        );
        let spec = tool.spec();
        assert_eq!(spec.name, "web_search");
        assert!(spec.capabilities.read_only);
        assert!(spec.permission.always_ask);
        assert_eq!(spec.schema["properties"]["query"]["maxLength"], 500);
    }

    #[test]
    fn successful_results_include_model_text_and_structured_sources() {
        let tool = WebSearchTool::new(
            Arc::new(FakeBackend {
                response: Ok(WebSearchResponse {
                    backend: WebSearchBackendKind::Tavily,
                    query: "rust agents".into(),
                    sources: vec![Source {
                        id: "tavily:one".into(),
                        kind: SourceKind::WebPage,
                        locator: Some("https://example.com/guide".into()),
                        title: Some("Guide".into()),
                        content_preview: Some("Useful context".into()),
                        metadata: BTreeMap::new(),
                    }],
                    request_id: Some("request-1".into()),
                    response_time_seconds: Some(0.5),
                    metadata: BTreeMap::new(),
                }),
            }),
            WebSearchSettings::default(),
        );
        let outcome = execute(&tool, json!({"query": " rust\n agents "}));
        assert!(!outcome.is_error());
        assert!(outcome.output.model_text.contains("1. Guide"));
        assert!(outcome.output.model_text.contains("Useful context"));
        assert_eq!(outcome.output.details.unwrap()["result_count"], 1);
    }

    #[test]
    fn backend_errors_keep_retryability_and_stable_details() {
        let tool = WebSearchTool::new(
            Arc::new(FakeBackend {
                response: Err(WebSearchError {
                    backend: WebSearchBackendKind::Tavily,
                    kind: WebSearchErrorKind::Http,
                    message: "Tavily web search failed with HTTP 429".into(),
                    retryable: true,
                    status: Some(429),
                }),
            }),
            WebSearchSettings::default(),
        );
        let outcome = execute(&tool, json!({"query": "rust"}));
        let error = outcome.error.unwrap();
        assert_eq!(error.code, ToolErrorCode::ExecutionFailed);
        assert!(error.retryable);
        assert_eq!(error.details.unwrap()["status"], 429);
    }
}
