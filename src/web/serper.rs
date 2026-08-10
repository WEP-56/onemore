use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::AtomicBool;

use serde_json::{json, Value};

use super::http_client;
use super::{
    clean_inline, clean_source_title, domain_allowed, normalize_citation_url, source_kind, Source,
    WebSearchBackend, WebSearchBackendKind, WebSearchContextSize, WebSearchError, WebSearchRequest,
    WebSearchResponse, MAX_SOURCES, MAX_SOURCE_PREVIEW_CHARS,
};

const SERPER_SEARCH_ENDPOINT: &str = "https://google.serper.dev/search";

pub(super) struct SerperBackend {
    agent: ureq::Agent,
    api_key: String,
    endpoint: String,
}

impl SerperBackend {
    pub(super) fn new(api_key: String) -> Self {
        Self {
            agent: http_client::agent(),
            api_key,
            endpoint: SERPER_SEARCH_ENDPOINT.into(),
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

impl WebSearchBackend for SerperBackend {
    fn kind(&self) -> WebSearchBackendKind {
        WebSearchBackendKind::Serper
    }

    fn search(
        &self,
        request: &WebSearchRequest,
        cancel: &AtomicBool,
    ) -> Result<WebSearchResponse, WebSearchError> {
        let mut body = json!({
            "q": request.query,
            "num": result_limit(request.settings.context_size)
        });
        if let Some(country) = request
            .settings
            .location
            .as_ref()
            .and_then(|location| location.country.as_deref())
        {
            body["gl"] = json!(country.to_ascii_lowercase());
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
            WebSearchBackendKind::Serper,
        )?;
        Ok(parse_response(&value, request))
    }
}

fn result_limit(context_size: Option<WebSearchContextSize>) -> usize {
    match context_size.unwrap_or(WebSearchContextSize::Medium) {
        WebSearchContextSize::Low => 3,
        WebSearchContextSize::Medium => 5,
        WebSearchContextSize::High => 10,
    }
}

fn parse_response(value: &Value, request: &WebSearchRequest) -> WebSearchResponse {
    let empty = Vec::new();
    let results = value["organic"].as_array().unwrap_or(&empty);
    let mut seen = BTreeSet::new();
    let mut sources = Vec::new();
    for result in results {
        let Some(url) = result["link"].as_str().and_then(normalize_citation_url) else {
            continue;
        };
        if !domain_allowed(&url, &request.settings.allowed_domains) || !seen.insert(url.clone()) {
            continue;
        }
        let title = result["title"].as_str().and_then(clean_source_title);
        let content_preview = result["snippet"]
            .as_str()
            .map(|snippet| clean_inline(snippet, MAX_SOURCE_PREVIEW_CHARS))
            .filter(|snippet| !snippet.is_empty());
        let mut metadata = BTreeMap::new();
        metadata.insert("provider".into(), "serper".into());
        metadata.insert("citation_type".into(), "search_result".into());
        if let Some(position) = result["position"].as_u64() {
            metadata.insert("position".into(), position.to_string());
        }
        if let Some(date) = result["date"]
            .as_str()
            .map(|value| clean_inline(value, 100))
            .filter(|value| !value.is_empty())
        {
            metadata.insert("published_date".into(), date);
        }
        sources.push(Source {
            id: format!("serper:{url}"),
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
    let mut metadata = BTreeMap::new();
    if let Some(credits) = value["credits"].as_u64() {
        metadata.insert("credits".into(), credits.to_string());
    }
    WebSearchResponse {
        backend: WebSearchBackendKind::Serper,
        query: value
            .pointer("/searchParameters/q")
            .and_then(Value::as_str)
            .map(|query| clean_inline(query, 500))
            .filter(|query| !query.is_empty())
            .unwrap_or_else(|| request.query.clone()),
        sources,
        request_id: None,
        response_time_seconds: None,
        metadata,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::web::test_support::serve_json_once;
    use crate::web::WebSearchSettings;

    #[test]
    fn parses_serper_organic_results_and_credit_usage() {
        let request = WebSearchRequest {
            query: "rust agents".into(),
            settings: WebSearchSettings::default(),
        };
        let response = parse_response(
            &json!({
                "searchParameters": {"q": "rust agents", "type": "search"},
                "organic": [{
                    "title": "Agent Guide",
                    "link": "https://example.com/agent#section",
                    "snippet": "Useful search context",
                    "date": "Aug 10, 2026",
                    "position": 1
                }],
                "credits": 1
            }),
            &request,
        );
        assert_eq!(response.sources.len(), 1);
        assert_eq!(response.sources[0].id, "serper:https://example.com/agent");
        assert_eq!(
            response.sources[0].content_preview.as_deref(),
            Some("Useful search context")
        );
        assert_eq!(response.sources[0].metadata["position"], "1");
        assert_eq!(response.metadata["credits"], "1");
    }

    #[test]
    fn missing_organic_results_is_a_valid_empty_search() {
        let request = WebSearchRequest {
            query: "nothing".into(),
            settings: WebSearchSettings::default(),
        };
        assert!(parse_response(&json!({}), &request).sources.is_empty());
    }

    #[test]
    fn wire_request_uses_serper_endpoint_header_and_bounded_count() {
        let (endpoint, captured) = serve_json_once("/search", json!({"organic": []}));
        let backend = SerperBackend::with_endpoint("test-serper-key".into(), endpoint);
        let request = WebSearchRequest {
            query: "rust agents".into(),
            settings: WebSearchSettings {
                context_size: Some(WebSearchContextSize::Low),
                allowed_domains: Vec::new(),
                location: None,
            },
        };
        backend.search(&request, &AtomicBool::new(false)).unwrap();
        let raw = captured
            .recv_timeout(std::time::Duration::from_secs(2))
            .unwrap();
        assert!(raw.starts_with("POST /search HTTP/1.1"));
        assert!(raw
            .to_ascii_lowercase()
            .contains("x-api-key: test-serper-key"));
        let body: Value = serde_json::from_str(raw.split("\r\n\r\n").nth(1).unwrap()).unwrap();
        assert_eq!(body, json!({"q": "rust agents", "num": 3}));
    }
}
