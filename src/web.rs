//! Provider-neutral Web capability selection and bounded, normalized citations.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use url::Url;

use crate::config::ProviderProfile;
use crate::util;

const MAX_ALLOWED_DOMAINS: usize = 100;
const MAX_DOMAIN_LENGTH: usize = 253;
const MAX_LOCATION_TEXT_CHARS: usize = 100;
const MAX_TIMEZONE_CHARS: usize = 64;
const MAX_SOURCES: usize = 20;
const MAX_URL_CHARS: usize = 2_048;
const MAX_SOURCE_TITLE_CHARS: usize = 240;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WebMode {
    Auto,
    Native,
    External,
    Disabled,
}

/// Controls the amount of web-search context a hosted provider may use.
/// The provider owns the exact token budgets for these stable tiers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WebSearchContextSize {
    Low,
    Medium,
    High,
}

impl WebSearchContextSize {
    pub(crate) fn parse(value: &str) -> Result<Self, String> {
        match value {
            "low" => Ok(Self::Low),
            "medium" => Ok(Self::Medium),
            "high" => Ok(Self::High),
            _ => Err("must be low | medium | high".into()),
        }
    }
}

/// An approximate location, passed only to providers that explicitly support
/// it. It is intentionally configuration rather than a model-controlled tool
/// argument.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WebSearchLocation {
    pub country: Option<String>,
    pub region: Option<String>,
    pub city: Option<String>,
    pub timezone: Option<String>,
}

/// Provider-independent hosted-search settings. These are normalized during
/// config loading, then frozen into a `WebCapabilityBinding` for the session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct WebSearchSettings {
    pub context_size: Option<WebSearchContextSize>,
    pub allowed_domains: Vec<String>,
    pub location: Option<WebSearchLocation>,
}

impl WebSearchSettings {
    pub(crate) fn new(
        context_size: Option<&str>,
        allowed_domains: Vec<String>,
        location: Option<WebSearchLocation>,
    ) -> Result<Self, String> {
        let context_size = context_size
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(WebSearchContextSize::parse)
            .transpose()
            .map_err(|error| format!("context_size {error}"))?;

        let mut normalized_domains = Vec::new();
        for domain in allowed_domains {
            let domain = normalize_domain(&domain)?;
            if normalized_domains
                .iter()
                .any(|existing| existing == &domain)
            {
                return Err(format!(
                    "allowed_domains contains duplicate domain {domain:?}"
                ));
            }
            if normalized_domains.len() == MAX_ALLOWED_DOMAINS {
                return Err(format!(
                    "allowed_domains supports at most {MAX_ALLOWED_DOMAINS} domains"
                ));
            }
            normalized_domains.push(domain);
        }

        Ok(Self {
            context_size,
            allowed_domains: normalized_domains,
            location: location.map(normalize_location).transpose()?,
        })
    }
}

impl WebMode {
    pub(crate) fn parse(value: &str) -> Result<Self, String> {
        match value {
            "auto" => Ok(Self::Auto),
            "native" => Ok(Self::Native),
            "external" => Ok(Self::External),
            "disabled" => Ok(Self::Disabled),
            _ => Err("must be auto | native | external | disabled".into()),
        }
    }
}

/// The implementation selected once for one Agent/session capability epoch.
/// It is intentionally not a local `ToolRegistry` entry: hosted tools execute
/// inside the provider, while a future external binding will be a real local
/// function tool with its own lifecycle.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WebCapabilityBinding {
    OpenAiNative(WebSearchSettings),
    Disabled,
}

impl WebCapabilityBinding {
    pub fn resolve(mode: WebMode, profile: ProviderProfile, settings: WebSearchSettings) -> Self {
        match mode {
            WebMode::Disabled | WebMode::External => Self::Disabled,
            WebMode::Auto | WebMode::Native if profile == ProviderProfile::OpenAiResponses => {
                Self::OpenAiNative(settings)
            }
            WebMode::Auto | WebMode::Native => Self::Disabled,
        }
    }

    /// Build the hosted OpenAI Responses API tool definition. Hosted web
    /// search deliberately remains outside the local `ToolRegistry`.
    pub(crate) fn openai_native_tool(&self) -> Option<Value> {
        let Self::OpenAiNative(settings) = self else {
            return None;
        };
        let mut tool = json!({ "type": "web_search" });
        if let Some(context_size) = settings.context_size {
            tool["search_context_size"] = json!(context_size);
        }
        if !settings.allowed_domains.is_empty() {
            tool["filters"] = json!({ "allowed_domains": settings.allowed_domains });
        }
        if let Some(location) = &settings.location {
            let mut value = json!({ "type": "approximate" });
            if let Some(country) = &location.country {
                value["country"] = json!(country);
            }
            if let Some(region) = &location.region {
                value["region"] = json!(region);
            }
            if let Some(city) = &location.city {
                value["city"] = json!(city);
            }
            if let Some(timezone) = &location.timezone {
                value["timezone"] = json!(timezone);
            }
            tool["user_location"] = value;
        }
        Some(tool)
    }

    pub fn label(&self) -> String {
        match self {
            Self::OpenAiNative(settings) => {
                let mut details = Vec::new();
                if let Some(context_size) = settings.context_size {
                    details.push(format!("context={context_size:?}").to_ascii_lowercase());
                }
                if !settings.allowed_domains.is_empty() {
                    details.push(format!("domains={}", settings.allowed_domains.len()));
                }
                if settings.location.is_some() {
                    details.push("approximate location".into());
                }
                if details.is_empty() {
                    "OpenAI hosted web search".into()
                } else {
                    format!("OpenAI hosted web search ({})", details.join(", "))
                }
            }
            Self::Disabled => "disabled".into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceKind {
    WebPage,
    Pdf,
    Document,
    BrowserSnapshot,
    Other(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Source {
    pub id: String,
    pub kind: SourceKind,
    pub locator: Option<String>,
    pub title: Option<String>,
    pub content_preview: Option<String>,
    pub metadata: BTreeMap<String, String>,
}

/// Extract and deduplicate Response API URL citations from a completed output
/// message. Citation annotations are provider-specific; all callers receive
/// the same `Source` representation.
pub fn sources_from_openai_message(item: &Value) -> Vec<Source> {
    let mut seen = BTreeSet::new();
    let mut sources = Vec::new();
    for part in item["content"].as_array().into_iter().flatten() {
        for annotation in part["annotations"].as_array().into_iter().flatten() {
            if annotation["type"].as_str() != Some("url_citation") {
                continue;
            }
            let Some(url) = annotation["url"].as_str().and_then(normalize_citation_url) else {
                continue;
            };
            if !seen.insert(url.clone()) {
                continue;
            }
            let title = annotation["title"].as_str().and_then(clean_source_title);
            let mut metadata = BTreeMap::new();
            metadata.insert("provider".into(), "openai".into());
            metadata.insert("citation_type".into(), "url_citation".into());
            sources.push(Source {
                id: format!("openai:{url}"),
                kind: source_kind(&url),
                locator: Some(url),
                title,
                content_preview: None,
                metadata,
            });
            if sources.len() == MAX_SOURCES {
                return sources;
            }
        }
    }
    sources
}

pub fn append_sources_to_text(mut text: String, sources: &[Source]) -> String {
    if sources.is_empty() {
        return text;
    }
    if !text.trim().is_empty() {
        text.push_str("\n\n");
    }
    text.push_str("Sources:");
    for source in sources {
        let title = source
            .title
            .as_deref()
            .and_then(clean_source_title)
            .unwrap_or_else(|| "Untitled source".into());
        let locator = source
            .locator
            .as_deref()
            .and_then(normalize_citation_url)
            .unwrap_or_else(|| clean_inline(&source.id, MAX_URL_CHARS));
        text.push_str(&format!("\n- {title}: {locator}"));
    }
    text
}

fn normalize_domain(value: &str) -> Result<String, String> {
    let domain = value.trim().trim_end_matches('.').to_ascii_lowercase();
    if domain.is_empty() || domain.len() > MAX_DOMAIN_LENGTH {
        return Err("allowed_domains contains an empty or oversized domain".into());
    }
    if domain.starts_with('.')
        || domain.ends_with('-')
        || domain.contains("..")
        || !domain.chars().all(|character| {
            character.is_ascii_lowercase()
                || character.is_ascii_digit()
                || character == '.'
                || character == '-'
        })
    {
        return Err(format!(
            "allowed_domains contains invalid domain {domain:?}"
        ));
    }
    if domain.split('.').any(|label| {
        label.is_empty() || label.starts_with('-') || label.ends_with('-') || label.len() > 63
    }) {
        return Err(format!(
            "allowed_domains contains invalid domain {domain:?}"
        ));
    }
    Ok(domain)
}

fn normalize_location(location: WebSearchLocation) -> Result<WebSearchLocation, String> {
    let country = clean_config_text(
        location.country,
        "location.country",
        MAX_LOCATION_TEXT_CHARS,
    )?;
    if country.as_ref().is_some_and(|country| {
        country.len() != 2
            || !country
                .chars()
                .all(|character| character.is_ascii_alphabetic())
    }) {
        return Err("location.country must be a two-letter ISO country code".into());
    }
    let region = clean_config_text(location.region, "location.region", MAX_LOCATION_TEXT_CHARS)?;
    let city = clean_config_text(location.city, "location.city", MAX_LOCATION_TEXT_CHARS)?;
    let timezone = clean_config_text(location.timezone, "location.timezone", MAX_TIMEZONE_CHARS)?;
    if timezone.as_ref().is_some_and(|timezone| {
        !timezone.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '/' | '_' | '-' | '+')
        })
    }) {
        return Err("location.timezone must be an IANA-style timezone identifier".into());
    }
    if country.is_none() && region.is_none() && city.is_none() && timezone.is_none() {
        return Err("location must include at least one field".into());
    }
    Ok(WebSearchLocation {
        country: country.map(|value| value.to_ascii_uppercase()),
        region,
        city,
        timezone,
    })
}

fn clean_config_text(
    value: Option<String>,
    field: &str,
    maximum: usize,
) -> Result<Option<String>, String> {
    let Some(value) = value else {
        return Ok(None);
    };
    let cleaned = util::sanitize(&value)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    if cleaned.is_empty() {
        return Err(format!("{field} must not be empty when specified"));
    }
    if cleaned.chars().count() > maximum {
        return Err(format!("{field} exceeds {maximum} characters"));
    }
    Ok(Some(cleaned))
}

fn normalize_citation_url(value: &str) -> Option<String> {
    let cleaned = util::sanitize(value);
    let mut url = Url::parse(cleaned.trim()).ok()?;
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
        return None;
    }
    let _ = url.set_username("");
    let _ = url.set_password(None);
    url.set_fragment(None);
    let normalized = url.to_string();
    (normalized.chars().count() <= MAX_URL_CHARS).then_some(normalized)
}

fn clean_source_title(value: &str) -> Option<String> {
    let cleaned = clean_inline(value, MAX_SOURCE_TITLE_CHARS);
    (!cleaned.is_empty()).then_some(cleaned)
}

fn clean_inline(value: &str, maximum: usize) -> String {
    let value = util::sanitize(value);
    let collapsed = value.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.chars().count() <= maximum {
        return collapsed;
    }
    let prefix = collapsed
        .chars()
        .take(maximum.saturating_sub(3))
        .collect::<String>();
    format!("{prefix}...")
}

fn source_kind(url: &str) -> SourceKind {
    let path = url
        .split(['?', '#'])
        .next()
        .unwrap_or(url)
        .to_ascii_lowercase();
    if path.ends_with(".pdf") {
        SourceKind::Pdf
    } else {
        SourceKind::WebPage
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auto_resolves_only_the_supported_native_provider() {
        let settings = WebSearchSettings::default();
        assert_eq!(
            WebCapabilityBinding::resolve(
                WebMode::Auto,
                ProviderProfile::OpenAiResponses,
                settings.clone(),
            ),
            WebCapabilityBinding::OpenAiNative(settings.clone())
        );
        assert_eq!(
            WebCapabilityBinding::resolve(
                WebMode::Auto,
                ProviderProfile::AnthropicMessages,
                settings.clone(),
            ),
            WebCapabilityBinding::Disabled
        );
        assert_eq!(
            WebCapabilityBinding::resolve(
                WebMode::External,
                ProviderProfile::OpenAiResponses,
                settings,
            ),
            WebCapabilityBinding::Disabled
        );
    }

    #[test]
    fn hosted_tool_uses_normalized_supported_parameters() {
        let settings = WebSearchSettings::new(
            Some("high"),
            vec!["EXAMPLE.com.".into(), "docs.example.com".into()],
            Some(WebSearchLocation {
                country: Some("us".into()),
                region: Some("California".into()),
                city: Some("San Francisco".into()),
                timezone: Some("America/Los_Angeles".into()),
            }),
        )
        .unwrap();
        let tool = WebCapabilityBinding::resolve(
            WebMode::Native,
            ProviderProfile::OpenAiResponses,
            settings,
        )
        .openai_native_tool()
        .unwrap();
        assert_eq!(tool["type"], "web_search");
        assert_eq!(tool["search_context_size"], "high");
        assert_eq!(
            tool["filters"]["allowed_domains"],
            json!(["example.com", "docs.example.com"])
        );
        assert_eq!(tool["user_location"]["country"], "US");
    }

    #[test]
    fn normalizes_and_deduplicates_openai_citations() {
        let item = serde_json::json!({
            "content": [{
                "annotations": [
                    {"type":"url_citation","url":"https://example.com/guide","title":"Guide"},
                    {"type":"url_citation","url":"https://example.com/guide","title":"Duplicate"},
                    {"type":"url_citation","url":"https://example.com/report.pdf","title":"Report"}
                ]
            }]
        });
        let sources = sources_from_openai_message(&item);
        assert_eq!(sources.len(), 2);
        assert_eq!(sources[0].kind, SourceKind::WebPage);
        assert_eq!(sources[1].kind, SourceKind::Pdf);
        let text = append_sources_to_text("Answer".into(), &sources);
        assert!(text.contains("Guide: https://example.com/guide"));
    }

    #[test]
    fn drops_unsafe_citations_and_cleans_source_text() {
        let item = serde_json::json!({
            "content": [{
                "annotations": [
                    {"type":"url_citation","url":"javascript:alert(1)","title":"Ignore"},
                    {"type":"url_citation","url":"https://user:secret@example.com/docs#heading","title":" Guide\n\u{001b}[31mTitle\u{001b}[0m "}
                ]
            }]
        });
        let sources = sources_from_openai_message(&item);
        assert_eq!(sources.len(), 1);
        assert_eq!(
            sources[0].locator.as_deref(),
            Some("https://example.com/docs")
        );
        assert_eq!(sources[0].title.as_deref(), Some("Guide Title"));
    }
}
