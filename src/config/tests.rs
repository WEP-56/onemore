use super::{
    parse_permission_rule, ActiveModelSelection, ApiKind, Config, ProviderProfile,
    ReasoningEffortPolicy, EXAMPLE_CONFIG,
};
use crate::permission::PermissionRule;
use crate::web::WebCapabilityBinding;
use std::time::Duration;

fn load_config(text: &str) -> anyhow::Result<Config> {
    let root = std::env::temp_dir().join(format!(
        "onemore-config-{}-{}",
        std::process::id(),
        uuid::Uuid::new_v4()
    ));
    std::fs::create_dir_all(&root)?;
    let path = root.join("config.toml");
    std::fs::write(&path, text)?;
    let result = Config::load(&path);
    let _ = std::fs::remove_dir_all(root);
    result
}

#[test]
fn bundled_example_matches_checked_in_file() {
    assert_eq!(
        EXAMPLE_CONFIG.trim_end(),
        include_str!("../../config.example.toml")
            .replace("\r\n", "\n")
            .trim_end()
    );
    load_config(EXAMPLE_CONFIG).unwrap();
}

#[test]
fn compaction_settings_have_defaults_and_allow_an_explicit_disable() {
    let default = load_config(
        r#"
[agent]
provider = "mock"
[providers.mock]
api = "responses"
base_url = "https://example.invalid/v1"
api_key = ""
model = "model"
"#,
    )
    .unwrap();
    assert!(default.compaction.enabled);
    assert_eq!(default.compaction.reserve_tokens, 16_384);
    assert_eq!(default.compaction.keep_recent_tokens, 20_000);

    let disabled = load_config(
        r#"
[agent]
provider = "mock"
[compaction]
enabled = false
reserve_tokens = 1234
keep_recent_tokens = 5678
[providers.mock]
api = "responses"
base_url = "https://example.invalid/v1"
api_key = ""
model = "model"
"#,
    )
    .unwrap();
    assert!(!disabled.compaction.enabled);
    assert_eq!(disabled.compaction.reserve_tokens, 1234);
    assert_eq!(disabled.compaction.keep_recent_tokens, 5678);
}

#[test]
fn web_settings_are_normalized_and_bound_only_to_openai_responses() {
    let config = load_config(
        r#"
[agent]
provider = "openai"

[web]
mode = "native"
context_size = "high"
allowed_domains = ["EXAMPLE.com.", "docs.example.com"]

[web.location]
country = "us"
region = "California"
city = "San Francisco"
timezone = "America/Los_Angeles"

[providers.openai]
api = "responses"
profile = "openai"
base_url = "https://example.invalid/v1"
api_key = ""
model = "model"

[providers.anthropic]
api = "messages"
profile = "anthropic"
base_url = "https://example.invalid"
api_key = ""
model = "model"
"#,
    )
    .unwrap();

    let openai = config.resolve_provider("openai").unwrap();
    let tool = openai.web.openai_native_tool().unwrap();
    assert_eq!(tool["type"], "web_search");
    assert_eq!(tool["search_context_size"], "high");
    assert_eq!(
        tool["filters"]["allowed_domains"],
        serde_json::json!(["example.com", "docs.example.com"])
    );
    assert_eq!(tool["user_location"]["country"], "US");
    assert_eq!(
        config.resolve_provider("anthropic").unwrap().web,
        WebCapabilityBinding::Disabled
    );
}

#[test]
fn invalid_web_settings_are_rejected() {
    let base = r#"
[agent]
provider = "mock"
[web]
WEB_SETTING
[providers.mock]
api = "responses"
profile = "openai"
base_url = "https://example.invalid/v1"
api_key = ""
model = "model"
"#;
    for (setting, expected) in [
        ("mode = \"unsupported\"", "[web].mode"),
        ("context_size = \"huge\"", "context_size"),
        (
            "allowed_domains = [\"https://example.com\"]",
            "allowed_domains",
        ),
        ("[web.location]\ncountry = \"USA\"", "location.country"),
    ] {
        let error = load_config(&base.replace("WEB_SETTING", setting)).unwrap_err();
        assert!(format!("{error:#}").contains(expected), "{error:#}");
    }
}

#[test]
fn retry_and_turn_limits_have_stable_defaults_and_accept_overrides() {
    let default = load_config(
        r#"
[agent]
provider = "mock"
[providers.mock]
api = "responses"
base_url = "https://example.invalid/v1"
api_key = ""
model = "model"
"#,
    )
    .unwrap();
    assert_eq!(default.max_turns, 200);
    assert_eq!(default.retry_policy.max_attempts, 8);
    assert_eq!(default.retry_policy.base_delay, Duration::from_secs(1));
    assert_eq!(default.retry_policy.max_delay, Duration::from_secs(10));
    assert_eq!(
        default.retry_policy.max_retry_after,
        Duration::from_secs(60)
    );

    let custom = load_config(
        r#"
[agent]
provider = "mock"
max_turns = 37
[retry]
max_attempts = 4
base_delay_ms = 25
max_delay_ms = 250
max_retry_after_ms = 3000
[providers.mock]
api = "responses"
base_url = "https://example.invalid/v1"
api_key = ""
model = "model"
"#,
    )
    .unwrap();
    assert_eq!(custom.max_turns, 37);
    assert_eq!(custom.retry_policy.max_attempts, 4);
    assert_eq!(custom.retry_policy.base_delay, Duration::from_millis(25));
    assert_eq!(custom.retry_policy.max_delay, Duration::from_millis(250));
    assert_eq!(custom.retry_policy.max_retry_after, Duration::from_secs(3));
}

#[test]
fn invalid_retry_policies_are_rejected() {
    let base = r#"
[agent]
provider = "mock"
[retry]
RETRY_SETTING
[providers.mock]
api = "responses"
base_url = "https://example.invalid/v1"
api_key = ""
model = "model"
"#;
    for (setting, expected) in [
        ("max_attempts = 0", "max_attempts"),
        ("base_delay_ms = 0", "base_delay_ms"),
        ("base_delay_ms = 100\nmax_delay_ms = 99", "max_delay_ms"),
        ("max_retry_after_ms = 0", "max_retry_after_ms"),
    ] {
        let error = load_config(&base.replace("RETRY_SETTING", setting)).unwrap_err();
        assert!(format!("{error:#}").contains(expected), "{error:#}");
    }
}

#[test]
fn permission_rules_reject_unknown_values() {
    assert_eq!(
        parse_permission_rule(Some("deny"), PermissionRule::Allow, "commands").unwrap(),
        PermissionRule::Deny
    );
    assert!(parse_permission_rule(Some("maybe"), PermissionRule::Allow, "commands").is_err());
}

#[test]
fn chat_completions_is_not_a_supported_api_kind() {
    assert!(ApiKind::parse("chat").is_err());
}

#[test]
fn provider_profiles_are_family_checked() {
    assert_eq!(
        ProviderProfile::parse(None, ApiKind::Responses).unwrap(),
        ProviderProfile::OpenAiResponses
    );
    assert!(ProviderProfile::parse(Some("anthropic"), ApiKind::Responses).is_err());
}

#[test]
fn multi_model_catalog_resolves_limits_and_reasoning() {
    let config = load_config(
        r#"
[agent]
provider = "mock"

[providers.mock]
api = "responses"
profile = "openai"
base_url = "https://example.invalid/v1"
api_key = ""
default_model = "gpt.main/v1"

[providers.mock.models."gpt.main/v1"]
context_window = 400000
max_tokens = 128000
efforts = ["none", "medium", "vendor_ultra"]
default_effort = "vendor_ultra"

[providers.mock.models."small:model"]
context_window = 64000
max_tokens = 8000
efforts = []
"#,
    )
    .unwrap();

    let catalog = config.provider_catalog();
    assert_eq!(catalog.len(), 1);
    assert_eq!(catalog[0].default_model, "gpt.main/v1");
    assert_eq!(catalog[0].models.len(), 2);
    let main = catalog[0]
        .models
        .iter()
        .find(|model| model.id == "gpt.main/v1")
        .unwrap();
    assert_eq!(main.context_window, Some(400000));
    assert_eq!(main.max_tokens, Some(128000));
    assert_eq!(main.efforts, ["none", "medium", "vendor_ultra"]);
    assert_eq!(main.default_effort, "vendor_ultra");
    assert!(main.sends_effort);
    assert_eq!(
        config.default_selection("mock").unwrap().effort,
        "vendor_ultra"
    );

    let settings = config
        .resolve_selection(&ActiveModelSelection {
            provider: "mock".into(),
            model: "gpt.main/v1".into(),
            effort: "vendor_ultra".into(),
        })
        .unwrap();
    assert_eq!(settings.context_window, Some(400000));
    assert_eq!(settings.max_tokens, Some(128000));
    assert_eq!(
        settings.reasoning_effort,
        ReasoningEffortPolicy::Send("vendor_ultra".into())
    );

    let small = config
        .resolve_selection(&ActiveModelSelection {
            provider: "mock".into(),
            model: "small:model".into(),
            effort: "medium".into(),
        })
        .unwrap();
    assert_eq!(small.context_window, Some(64000));
    assert_eq!(small.reasoning_effort, ReasoningEffortPolicy::Omit);
}

#[test]
fn legacy_single_model_config_still_loads() {
    let config = load_config(
        r#"
[agent]
provider = "legacy"
[providers.legacy]
api = "responses"
base_url = "https://example.invalid/v1"
api_key = ""
model = "old-model"
context_window = 32000
max_tokens = 4096
"#,
    )
    .unwrap();
    let settings = config.resolve_provider("legacy").unwrap();
    assert_eq!(settings.model, "old-model");
    assert_eq!(settings.context_window, Some(32000));
    assert_eq!(
        settings.reasoning_effort,
        ReasoningEffortPolicy::Send("medium".into())
    );
    assert_eq!(
        config.provider_catalog()[0].models[0].efforts,
        ["none", "minimal", "low", "medium", "high", "xhigh", "max"]
    );
}

#[test]
fn rejects_mixed_formats_and_unknown_reasoning_fields() {
    let mixed = r#"
[agent]
provider = "mock"
[providers.mock]
api = "responses"
base_url = "https://example.invalid/v1"
api_key = ""
model = "old"
default_model = "new"
[providers.mock.models.new]
context_window = 32000
"#;
    assert!(format!("{:#}", load_config(mixed).unwrap_err()).contains("不能混用"));

    let removed_send_effort = r#"
[agent]
provider = "mock"
[providers.mock]
api = "responses"
base_url = "https://example.invalid/v1"
api_key = ""
default_model = "new"
[providers.mock.models.new]
context_window = 32000
send_effort = true
efforts = ["low", "high"]
"#;
    assert!(format!("{:#}", load_config(removed_send_effort).unwrap_err()).contains("send_effort"));
}

#[test]
fn profile_defaults_and_custom_efforts_do_not_require_medium() {
    let config = load_config(
        r#"
[agent]
provider = "openai"

[providers.openai]
api = "responses"
profile = "openai"
base_url = "https://example.invalid/v1"
api_key = ""
default_model = "custom"

[providers.openai.models.custom]
context_window = 400000
efforts = ["low", "high", "max"]

[providers.anthropic]
api = "messages"
profile = "anthropic"
base_url = "https://example.invalid"
api_key = ""
default_model = "standard"

[providers.anthropic.models.standard]
context_window = 200000
"#,
    )
    .unwrap();

    let catalog = config.provider_catalog();
    let openai = catalog
        .iter()
        .find(|provider| provider.name == "openai")
        .unwrap();
    assert_eq!(openai.models[0].efforts, ["low", "high", "max"]);
    assert_eq!(openai.models[0].default_effort, "low");
    assert_eq!(config.default_selection("openai").unwrap().effort, "low");
    assert_eq!(
        config
            .resolve_selection(&ActiveModelSelection {
                provider: "openai".into(),
                model: "custom".into(),
                effort: "max".into(),
            })
            .unwrap()
            .reasoning_effort,
        ReasoningEffortPolicy::Send("max".into())
    );
    let anthropic = catalog
        .iter()
        .find(|provider| provider.name == "anthropic")
        .unwrap();
    assert_eq!(
        anthropic.models[0].efforts,
        ["low", "medium", "high", "xhigh", "max"]
    );
    assert_eq!(anthropic.models[0].default_effort, "medium");
}

#[test]
fn rejects_unsafe_or_ambiguous_basic_configuration() {
    let invalid_shell = r#"
[agent]
provider = "mock"
shell = "bash"
[providers.mock]
api = "responses"
base_url = "https://example.invalid/v1"
api_key = ""
model = "model"
context_window = 32000
"#;
    assert!(format!("{:#}", load_config(invalid_shell).unwrap_err()).contains("shell"));

    let zero_turns = invalid_shell.replace("shell = \"bash\"", "max_turns = 0");
    assert!(format!("{:#}", load_config(&zero_turns).unwrap_err()).contains("max_turns"));

    let zero_compaction_reserve = invalid_shell.replace("shell = \"bash\"", "").replace(
        "[providers.mock]",
        "[compaction]\nreserve_tokens = 0\n[providers.mock]",
    );
    assert!(
        format!("{:#}", load_config(&zero_compaction_reserve).unwrap_err())
            .contains("compaction.reserve_tokens")
    );

    let duplicate_key_sources = r#"
[agent]
provider = "mock"
[providers.mock]
api = "responses"
base_url = "https://example.invalid/v1"
api_key = ""
api_key_env = "OPENAI_API_KEY"
model = "model"
context_window = 32000
"#;
    assert!(
        format!("{:#}", load_config(duplicate_key_sources).unwrap_err())
            .contains("只能配置 api_key 或 api_key_env")
    );
}
