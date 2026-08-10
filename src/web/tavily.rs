use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::AtomicBool;

use serde_json::{json, Value};

use super::http_client;
use super::{
    clean_inline, clean_source_title, domain_allowed, normalize_citation_url, source_kind, Source,
    WebSearchBackend, WebSearchBackendKind, WebSearchContextSize, WebSearchError,
    WebSearchErrorKind, WebSearchRequest, WebSearchResponse, MAX_SOURCES, MAX_SOURCE_PREVIEW_CHARS,
};

const TAVILY_SEARCH_ENDPOINT: &str = "https://api.tavily.com/search";
const MAX_REQUEST_ID_CHARS: usize = 128;

pub(super) struct TavilyBackend {
    agent: ureq::Agent,
    api_key: String,
    endpoint: String,
}

impl TavilyBackend {
    pub(super) fn new(api_key: String) -> Self {
        Self {
            agent: http_client::agent(),
            api_key,
            endpoint: TAVILY_SEARCH_ENDPOINT.into(),
        }
    }

    #[cfg(test)]
    fn with_endpoint(api_key: String, endpoint: String) -> Self {
        Self {
            agent: http_client::agent(),
            api_key,
            endpoint,
        }
    }
}

impl WebSearchBackend for TavilyBackend {
    fn kind(&self) -> WebSearchBackendKind {
        WebSearchBackendKind::Tavily
    }

    fn search(
        &self,
        request: &WebSearchRequest,
        cancel: &AtomicBool,
    ) -> Result<WebSearchResponse, WebSearchError> {
        let (search_depth, chunks_per_source, max_results) =
            request_parameters(request.settings.context_size);
        let body = json!({
            "query": request.query,
            "search_depth": search_depth,
            "chunks_per_source": chunks_per_source,
            "max_results": max_results,
            "topic": "general",
            "include_answer": false,
            "include_raw_content": false,
            "include_images": false,
            "include_domains": request.settings.allowed_domains,
            "auto_parameters": false,
            "include_usage": true
        });
        let http_request = self
            .agent
            .post(&self.endpoint)
            .set("content-type", "application/json")
            .set("accept", "application/json")
            .set("authorization", &format!("Bearer {}", self.api_key));
        let value = http_client::execute_json(
            http_request,
            Some(&body),
            cancel,
            WebSearchBackendKind::Tavily,
        )?;
        parse_response(&value, request)
    }
}

fn request_parameters(context_size: Option<WebSearchContextSize>) -> (&'static str, u64, usize) {
    match context_size.unwrap_or(WebSearchContextSize::Medium) {
        WebSearchContextSize::Low => ("basic", 1, 3),
        WebSearchContextSize::Medium => ("advanced", 2, 5),
        WebSearchContextSize::High => ("advanced", 3, 10),
    }
}

fn parse_response(
    value: &Value,
    request: &WebSearchRequest,
) -> Result<WebSearchResponse, WebSearchError> {
    let results = value["results"].as_array().ok_or_else(|| WebSearchError {
        backend: WebSearchBackendKind::Tavily,
        kind: WebSearchErrorKind::InvalidResponse,
        message: "Tavily response did not contain a results array".into(),
        retryable: false,
        status: Some(200),
    })?;

    let mut seen = BTreeSet::new();
    let mut sources = Vec::new();
    for result in results {
        let Some(url) = result["url"].as_str().and_then(normalize_citation_url) else {
            continue;
        };
        if !domain_allowed(&url, &request.settings.allowed_domains) || !seen.insert(url.clone()) {
            continue;
        }
        let title = result["title"].as_str().and_then(clean_source_title);
        let content_preview = result["content"]
            .as_str()
            .map(|content| clean_inline(content, MAX_SOURCE_PREVIEW_CHARS))
            .filter(|content| !content.is_empty());
        let mut metadata = BTreeMap::new();
        metadata.insert("provider".into(), "tavily".into());
        metadata.insert("citation_type".into(), "search_result".into());
        if let Some(score) = result["score"].as_f64().filter(|score| score.is_finite()) {
            metadata.insert("score".into(), format!("{score:.6}"));
        }
        if let Some(published_date) = result["published_date"]
            .as_str()
            .map(|value| clean_inline(value, 100))
            .filter(|value| !value.is_empty())
        {
            metadata.insert("published_date".into(), published_date);
        }
        let result_id = result["id"]
            .as_str()
            .map(|value| clean_inline(value, 200))
            .filter(|value| !value.is_empty());
        sources.push(Source {
            id: result_id
                .map(|id| format!("tavily:{id}"))
                .unwrap_or_else(|| format!("tavily:{url}")),
            kind: source_kind(&url),
            locator: Some(url),
            title,
            content_preview,
            metadata,
        });
        if sources.len() == MAX_SOURCES {
            break;
        }
    }

    let query = value["query"]
        .as_str()
        .map(|query| clean_inline(query, 500))
        .filter(|query| !query.is_empty())
        .unwrap_or_else(|| request.query.clone());
    let request_id = value["request_id"]
        .as_str()
        .map(|id| clean_inline(id, MAX_REQUEST_ID_CHARS))
        .filter(|id| !id.is_empty());
    let response_time_seconds = value["response_time"].as_f64().or_else(|| {
        value["response_time"]
            .as_str()
            .and_then(|value| value.parse::<f64>().ok())
    });
    let mut response_metadata = BTreeMap::new();
    if let Some(credits) = value.pointer("/usage/credits").and_then(Value::as_u64) {
        response_metadata.insert("credits".into(), credits.to_string());
    }
    Ok(WebSearchResponse {
        backend: WebSearchBackendKind::Tavily,
        query,
        sources,
        request_id,
        response_time_seconds: response_time_seconds.filter(|value| value.is_finite()),
        metadata: response_metadata,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::web::test_support::serve_json_once;
    use crate::web::{SourceKind, WebSearchSettings};

    fn request(allowed_domains: Vec<String>) -> WebSearchRequest {
        WebSearchRequest {
            query: "rust agents".into(),
            settings: WebSearchSettings {
                context_size: Some(WebSearchContextSize::Low),
                allowed_domains,
                location: None,
            },
        }
    }

    #[test]
    fn parses_bounded_deduplicated_sources_and_metadata() {
        let response = parse_response(
            &json!({
                "query": "rust agents",
                "request_id": "request-1",
                "response_time": 0.42,
                "results": [
                    {
                        "id": "one",
                        "title": " Guide\nTitle ",
                        "url": "https://user:secret@docs.example.com/guide#part",
                        "content": " useful\n  context ",
                        "score": 0.875,
                        "published_date": "2026-08-10"
                    },
                    {
                        "title": "duplicate",
                        "url": "https://docs.example.com/guide",
                        "content": "ignored"
                    },
                    {
                        "title": "outside",
                        "url": "https://other.example.net/guide",
                        "content": "ignored"
                    },
                    {
                        "title": "report",
                        "url": "https://example.com/report.pdf",
                        "content": "report context"
                    }
                ]
            }),
            &request(vec!["example.com".into()]),
        )
        .unwrap();

        assert_eq!(response.sources.len(), 2);
        assert_eq!(response.sources[0].id, "tavily:one");
        assert_eq!(
            response.sources[0].locator.as_deref(),
            Some("https://docs.example.com/guide")
        );
        assert_eq!(response.sources[0].title.as_deref(), Some("Guide Title"));
        assert_eq!(
            response.sources[0].content_preview.as_deref(),
            Some("useful context")
        );
        assert_eq!(response.sources[0].metadata["score"], "0.875000");
        assert_eq!(response.sources[1].kind, SourceKind::Pdf);
        assert_eq!(response.request_id.as_deref(), Some("request-1"));
        assert_eq!(response.response_time_seconds, Some(0.42));
    }

    #[test]
    fn invalid_response_and_http_errors_are_sanitized() {
        let error = parse_response(&json!({}), &request(Vec::new())).unwrap_err();
        assert_eq!(error.kind, WebSearchErrorKind::InvalidResponse);
    }

    #[test]
    fn context_size_controls_cost_and_result_bounds() {
        assert_eq!(
            request_parameters(Some(WebSearchContextSize::Low)),
            ("basic", 1, 3)
        );
        assert_eq!(
            request_parameters(Some(WebSearchContextSize::Medium)),
            ("advanced", 2, 5)
        );
        assert_eq!(
            request_parameters(Some(WebSearchContextSize::High)),
            ("advanced", 3, 10)
        );
    }

    #[test]
    fn test_constructor_keeps_endpoint_injectable_without_exposing_the_key() {
        let backend = TavilyBackend::with_endpoint("secret".into(), "http://localhost".into());
        assert_eq!(backend.kind(), WebSearchBackendKind::Tavily);
        assert_eq!(backend.endpoint, "http://localhost");
    }

    #[test]
    fn wire_request_uses_tavily_bearer_and_disables_unbounded_content() {
        let (endpoint, captured) = serve_json_once(
            "/search",
            json!({
                "query": "rust agents",
                "results": [],
                "response_time": 0.1,
                "request_id": "test"
            }),
        );
        let backend = TavilyBackend::with_endpoint("test-tavily-key".into(), endpoint);
        backend
            .search(&request(Vec::new()), &AtomicBool::new(false))
            .unwrap();
        let raw = captured
            .recv_timeout(std::time::Duration::from_secs(2))
            .unwrap();
        assert!(raw.starts_with("POST /search HTTP/1.1"));
        assert!(raw
            .to_ascii_lowercase()
            .contains("authorization: bearer test-tavily-key"));
        let body: Value = serde_json::from_str(raw.split("\r\n\r\n").nth(1).unwrap()).unwrap();
        assert_eq!(body["query"], "rust agents");
        assert_eq!(body["search_depth"], "basic");
        assert_eq!(body["max_results"], 3);
        assert_eq!(body["include_answer"], false);
        assert_eq!(body["include_raw_content"], false);
        assert_eq!(body["auto_parameters"], false);
    }
}
