use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::AtomicBool;

use serde_json::Value;

use super::http_client;
use super::{
    clean_inline, clean_source_title, domain_allowed, normalize_citation_url, source_kind, Source,
    WebSearchBackend, WebSearchBackendKind, WebSearchContextSize, WebSearchError,
    WebSearchErrorKind, WebSearchRequest, WebSearchResponse, MAX_SOURCES, MAX_SOURCE_PREVIEW_CHARS,
};

const BRAVE_SEARCH_ENDPOINT: &str = "https://api.search.brave.com/res/v1/web/search";

pub(super) struct BraveBackend {
    agent: ureq::Agent,
    api_key: String,
    endpoint: String,
}

impl BraveBackend {
    pub(super) fn new(api_key: String) -> Self {
        Self {
            agent: http_client::agent(),
            api_key,
            endpoint: BRAVE_SEARCH_ENDPOINT.into(),
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

impl WebSearchBackend for BraveBackend {
    fn kind(&self) -> WebSearchBackendKind {
        WebSearchBackendKind::Brave
    }

    fn search(
        &self,
        request: &WebSearchRequest,
        cancel: &AtomicBool,
    ) -> Result<WebSearchResponse, WebSearchError> {
        let count = result_limit(request.settings.context_size).to_string();
        let mut http_request = self
            .agent
            .get(&self.endpoint)
            .set("accept", "application/json")
            .set("x-subscription-token", &self.api_key)
            .query("q", &request.query)
            .query("count", &count)
            .query("extra_snippets", "true")
            .query("safesearch", "moderate");
        if let Some(country) = request
            .settings
            .location
            .as_ref()
            .and_then(|location| location.country.as_deref())
        {
            http_request = http_request.query("country", country);
        }
        let value =
            http_client::execute_json(http_request, None, cancel, WebSearchBackendKind::Brave)?;
        parse_response(&value, request)
    }
}

fn result_limit(context_size: Option<WebSearchContextSize>) -> usize {
    match context_size.unwrap_or(WebSearchContextSize::Medium) {
        WebSearchContextSize::Low => 3,
        WebSearchContextSize::Medium => 5,
        WebSearchContextSize::High => 10,
    }
}

fn parse_response(
    value: &Value,
    request: &WebSearchRequest,
) -> Result<WebSearchResponse, WebSearchError> {
    let results = value
        .pointer("/web/results")
        .and_then(Value::as_array)
        .ok_or_else(|| WebSearchError {
            backend: WebSearchBackendKind::Brave,
            kind: WebSearchErrorKind::InvalidResponse,
            message: "Brave Search response did not contain web.results".into(),
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
        let mut excerpts = Vec::new();
        if let Some(description) = result["description"].as_str() {
            excerpts.push(description);
        }
        excerpts.extend(
            result["extra_snippets"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str),
        );
        let content_preview = (!excerpts.is_empty())
            .then(|| clean_inline(&excerpts.join(" [...] "), MAX_SOURCE_PREVIEW_CHARS))
            .filter(|preview| !preview.is_empty());
        let mut metadata = BTreeMap::new();
        metadata.insert("provider".into(), "brave".into());
        metadata.insert("citation_type".into(), "search_result".into());
        for (field, key) in [
            ("age", "age"),
            ("page_age", "page_age"),
            ("language", "language"),
        ] {
            if let Some(value) = result[field]
                .as_str()
                .map(|value| clean_inline(value, 100))
                .filter(|value| !value.is_empty())
            {
                metadata.insert(key.into(), value);
            }
        }
        sources.push(Source {
            id: format!("brave:{url}"),
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
    if let Some(more) = value
        .pointer("/query/more_results_available")
        .and_then(Value::as_bool)
    {
        metadata.insert("more_results_available".into(), more.to_string());
    }
    Ok(WebSearchResponse {
        backend: WebSearchBackendKind::Brave,
        query: request.query.clone(),
        sources,
        request_id: None,
        response_time_seconds: None,
        metadata,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::web::test_support::serve_json_once;
    use crate::web::{WebSearchLocation, WebSearchSettings};

    #[test]
    fn parses_brave_web_results_and_combines_bounded_snippets() {
        let request = WebSearchRequest {
            query: "rust agents".into(),
            settings: WebSearchSettings {
                context_size: Some(WebSearchContextSize::Low),
                allowed_domains: vec!["example.com".into()],
                location: Some(WebSearchLocation {
                    country: Some("US".into()),
                    region: None,
                    city: None,
                    timezone: None,
                }),
            },
        };
        let response = parse_response(
            &serde_json::json!({
                "query": {"original": "rust agents", "more_results_available": true},
                "web": {"results": [
                    {
                        "title": "Guide",
                        "url": "https://docs.example.com/guide#part",
                        "description": "Main snippet",
                        "extra_snippets": ["More context", "Final context"],
                        "age": "1 day ago",
                        "language": "en"
                    },
                    {"title": "Outside", "url": "https://other.test", "description": "drop"}
                ]}
            }),
            &request,
        )
        .unwrap();
        assert_eq!(response.sources.len(), 1);
        assert_eq!(response.sources[0].title.as_deref(), Some("Guide"));
        assert_eq!(
            response.sources[0].content_preview.as_deref(),
            Some("Main snippet [...] More context [...] Final context")
        );
        assert_eq!(response.sources[0].metadata["language"], "en");
        assert_eq!(response.metadata["more_results_available"], "true");
    }

    #[test]
    fn wire_request_uses_brave_endpoint_header_and_bounded_query_parameters() {
        let (endpoint, captured) = serve_json_once(
            "/res/v1/web/search",
            serde_json::json!({"web": {"results": []}}),
        );
        let backend = BraveBackend::with_endpoint("test-brave-key".into(), endpoint);
        let request = WebSearchRequest {
            query: "rust agents".into(),
            settings: WebSearchSettings {
                context_size: Some(WebSearchContextSize::Low),
                allowed_domains: Vec::new(),
                location: Some(WebSearchLocation {
                    country: Some("US".into()),
                    region: None,
                    city: None,
                    timezone: None,
                }),
            },
        };
        backend.search(&request, &AtomicBool::new(false)).unwrap();
        let raw = captured
            .recv_timeout(std::time::Duration::from_secs(2))
            .unwrap();
        let lowercase = raw.to_ascii_lowercase();
        assert!(raw.starts_with("GET /res/v1/web/search?"));
        assert!(lowercase.contains("x-subscription-token: test-brave-key"));
        let request_target = raw
            .lines()
            .next()
            .unwrap()
            .split_whitespace()
            .nth(1)
            .unwrap();
        let url = url::Url::parse(&format!("http://localhost{request_target}")).unwrap();
        let query = url.query_pairs().collect::<BTreeMap<_, _>>();
        assert_eq!(query["q"], "rust agents");
        assert_eq!(query["count"], "3");
        assert_eq!(query["extra_snippets"], "true");
        assert_eq!(query["country"], "US");
    }
}
