use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::AtomicBool;

use serde_json::{json, Value};

use super::http_client;
use super::{
    clean_inline, clean_source_title, domain_allowed, normalize_citation_url, source_kind, Source,
    WebSearchBackend, WebSearchBackendKind, WebSearchContextSize, WebSearchError,
    WebSearchErrorKind, WebSearchRequest, WebSearchResponse, MAX_SOURCES, MAX_SOURCE_PREVIEW_CHARS,
};

const EXA_SEARCH_ENDPOINT: &str = "https://api.exa.ai/search";
const MAX_REQUEST_ID_CHARS: usize = 128;

pub(super) struct ExaBackend {
    agent: ureq::Agent,
    api_key: String,
    endpoint: String,
}

impl ExaBackend {
    pub(super) fn new(api_key: String) -> Self {
        Self {
            agent: http_client::agent(),
            api_key,
            endpoint: EXA_SEARCH_ENDPOINT.into(),
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

impl WebSearchBackend for ExaBackend {
    fn kind(&self) -> WebSearchBackendKind {
        WebSearchBackendKind::Exa
    }

    fn search(
        &self,
        request: &WebSearchRequest,
        cancel: &AtomicBool,
    ) -> Result<WebSearchResponse, WebSearchError> {
        let (search_type, num_results, highlight_chars) =
            request_parameters(request.settings.context_size);
        let mut body = json!({
            "query": request.query,
            "type": search_type,
            "numResults": num_results,
            "moderation": true,
            "contents": {
                "highlights": {"maxCharacters": highlight_chars}
            }
        });
        if !request.settings.allowed_domains.is_empty() {
            body["includeDomains"] = json!(request.settings.allowed_domains);
        }
        if let Some(country) = request
            .settings
            .location
            .as_ref()
            .and_then(|location| location.country.as_deref())
        {
            body["userLocation"] = json!(country);
        }
        let http_request = self
            .agent
            .post(&self.endpoint)
            .set("content-type", "application/json")
            .set("accept", "application/json")
            .set("x-api-key", &self.api_key);
        let value = http_client::execute_json(
            http_request,
            Some(&body),
            cancel,
            WebSearchBackendKind::Exa,
        )?;
        parse_response(&value, request)
    }
}

fn request_parameters(context_size: Option<WebSearchContextSize>) -> (&'static str, usize, usize) {
    match context_size.unwrap_or(WebSearchContextSize::Medium) {
        WebSearchContextSize::Low => ("fast", 3, 600),
        WebSearchContextSize::Medium => ("auto", 5, 1_000),
        WebSearchContextSize::High => ("auto", 10, 1_500),
    }
}

fn parse_response(
    value: &Value,
    request: &WebSearchRequest,
) -> Result<WebSearchResponse, WebSearchError> {
    let results = value["results"].as_array().ok_or_else(|| WebSearchError {
        backend: WebSearchBackendKind::Exa,
        kind: WebSearchErrorKind::InvalidResponse,
        message: "Exa response did not contain a results array".into(),
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
        let highlights = result["highlights"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .collect::<Vec<_>>();
        let preview = if highlights.is_empty() {
            result["text"]
                .as_str()
                .or_else(|| result["summary"].as_str())
        } else {
            None
        };
        let content_preview = if highlights.is_empty() {
            preview.map(|value| clean_inline(value, MAX_SOURCE_PREVIEW_CHARS))
        } else {
            Some(clean_inline(
                &highlights.join(" [...] "),
                MAX_SOURCE_PREVIEW_CHARS,
            ))
        }
        .filter(|preview| !preview.is_empty());
        let mut metadata = BTreeMap::new();
        metadata.insert("provider".into(), "exa".into());
        metadata.insert("citation_type".into(), "search_result".into());
        for (field, key) in [("author", "author"), ("publishedDate", "published_date")] {
            if let Some(value) = result[field]
                .as_str()
                .map(|value| clean_inline(value, 240))
                .filter(|value| !value.is_empty())
            {
                metadata.insert(key.into(), value);
            }
        }
        if let Some(score) = result["score"].as_f64().filter(|score| score.is_finite()) {
            metadata.insert("score".into(), format!("{score:.6}"));
        }
        let id = result["id"]
            .as_str()
            .map(|id| clean_inline(id, 500))
            .filter(|id| !id.is_empty())
            .map(|id| format!("exa:{id}"))
            .unwrap_or_else(|| format!("exa:{url}"));
        sources.push(Source {
            id,
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
    let request_id = value["requestId"]
        .as_str()
        .map(|id| clean_inline(id, MAX_REQUEST_ID_CHARS))
        .filter(|id| !id.is_empty());
    let mut metadata = BTreeMap::new();
    if let Some(search_type) = value["resolvedSearchType"]
        .as_str()
        .or_else(|| value["searchType"].as_str())
        .map(|value| clean_inline(value, 64))
        .filter(|value| !value.is_empty())
    {
        metadata.insert("resolved_search_type".into(), search_type);
    }
    if let Some(cost) = value
        .pointer("/costDollars/total")
        .and_then(Value::as_f64)
        .filter(|cost| cost.is_finite())
    {
        metadata.insert("cost_dollars".into(), format!("{cost:.6}"));
    }
    Ok(WebSearchResponse {
        backend: WebSearchBackendKind::Exa,
        query: request.query.clone(),
        sources,
        request_id,
        response_time_seconds: None,
        metadata,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::web::test_support::serve_json_once;
    use crate::web::WebSearchSettings;

    #[test]
    fn parses_exa_highlights_ids_and_cost_metadata() {
        let request = WebSearchRequest {
            query: "rust agents".into(),
            settings: WebSearchSettings::default(),
        };
        let response = parse_response(
            &json!({
                "requestId": "exa-request",
                "resolvedSearchType": "neural",
                "costDollars": {"total": 0.007},
                "results": [{
                    "id": "result-1",
                    "title": "Agent Guide",
                    "url": "https://example.com/agent",
                    "publishedDate": "2026-08-10T00:00:00Z",
                    "author": "Example Author",
                    "highlights": ["First excerpt", "Second excerpt"],
                    "score": 0.91
                }]
            }),
            &request,
        )
        .unwrap();
        assert_eq!(response.request_id.as_deref(), Some("exa-request"));
        assert_eq!(response.sources[0].id, "exa:result-1");
        assert_eq!(
            response.sources[0].content_preview.as_deref(),
            Some("First excerpt [...] Second excerpt")
        );
        assert_eq!(response.sources[0].metadata["author"], "Example Author");
        assert_eq!(response.metadata["cost_dollars"], "0.007000");
    }

    #[test]
    fn wire_request_uses_exa_header_and_bounded_contents() {
        let (endpoint, captured) =
            serve_json_once("/search", json!({"requestId": "test", "results": []}));
        let backend = ExaBackend::with_endpoint("test-exa-key".into(), endpoint);
        let request = WebSearchRequest {
            query: "rust agents".into(),
            settings: WebSearchSettings {
                context_size: Some(WebSearchContextSize::Low),
                allowed_domains: vec!["example.com".into()],
                location: None,
            },
        };
        backend.search(&request, &AtomicBool::new(false)).unwrap();
        let raw = captured
            .recv_timeout(std::time::Duration::from_secs(2))
            .unwrap();
        assert!(raw.starts_with("POST /search HTTP/1.1"));
        assert!(raw.to_ascii_lowercase().contains("x-api-key: test-exa-key"));
        let body: Value = serde_json::from_str(raw.split("\r\n\r\n").nth(1).unwrap()).unwrap();
        assert_eq!(body["query"], "rust agents");
        assert_eq!(body["type"], "fast");
        assert_eq!(body["numResults"], 3);
        assert_eq!(body["contents"]["highlights"]["maxCharacters"], 600);
        assert_eq!(body["includeDomains"], json!(["example.com"]));
    }
}
