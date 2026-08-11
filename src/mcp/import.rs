//! MCP 工具导入卫生：命名、清洗、上限、冲突拒绝。
//!
//! server 提供的名称、描述与 schema 都是不受信输入,且会进入模型的工具声明
//! (提示注入面)。本层在导入时一次性完成清洗与限额,任何拒绝都产出一条
//! 人类可读的 warning,绝不静默缺席。

use serde_json::Value;

use super::client::RawTool;
use crate::util;

/// 完整公开名(`mcp__{server}__{tool}`)的上限,对齐 provider 工具名限制。
const MAX_PUBLIC_NAME_CHARS: usize = 64;
/// 原始工具名按规范允许 1..=128 字符。
const MAX_RAW_NAME_CHARS: usize = 128;
const MAX_DESCRIPTION_CHARS: usize = 1024;
/// schema 序列化后的字节上限,防止单个工具声明撑爆上下文。
const MAX_SCHEMA_BYTES: usize = 64 * 1024;
/// 每 server 默认导入的工具数上限(config include/exclude 可先行过滤)。
pub(crate) const MAX_TOOLS_PER_SERVER: usize = 64;

/// 通过全部卫生检查、可注册进 ToolRegistry 的工具。
#[derive(Debug, Clone)]
pub(crate) struct ImportedTool {
    /// 注册与模型可见的名字:`mcp__{server}__{tool}`(`.` 已替换为 `_`)。
    pub public_name: String,
    /// server 侧的原始名字,tools/call 使用。
    pub original_name: String,
    /// 已清洗、限长并带来源前缀的描述。
    pub description: String,
    /// 原样透传给 provider 的 inputSchema(校验由 server 权威执行)。
    pub schema: Value,
}

pub(crate) struct ImportReport {
    pub tools: Vec<ImportedTool>,
    pub warnings: Vec<String>,
}

/// 对一个 server 的原始工具做导入。`name_taken` 查询公开名冲突(内置工具与
/// 先注册的 server);处理按原始名排序,保证截断与冲突判定确定性。
pub(crate) fn import_tools(
    server: &str,
    raw: Vec<RawTool>,
    include: Option<&[String]>,
    exclude: &[String],
    name_taken: &mut dyn FnMut(&str) -> bool,
) -> ImportReport {
    let mut warnings = Vec::new();
    let mut sorted = raw;
    sorted.sort_by(|a, b| a.name.cmp(&b.name));
    sorted.dedup_by(|a, b| {
        let duplicate = a.name == b.name;
        if duplicate {
            warnings.push(format!("[{server}] 工具 {:?} 重复声明,仅保留一条", b.name));
        }
        duplicate
    });

    let mut tools = Vec::new();
    for tool in sorted {
        if let Some(include) = include {
            if !include.iter().any(|name| name == &tool.name) {
                continue;
            }
        }
        if exclude.iter().any(|name| name == &tool.name) {
            continue;
        }
        match import_one(server, &tool, name_taken) {
            Ok(imported) => {
                if tools.len() >= MAX_TOOLS_PER_SERVER {
                    warnings.push(format!(
                        "[{server}] 工具数超过 {} 上限,{:?} 及之后的工具未导入;可用 include_tools/exclude_tools 过滤",
                        MAX_TOOLS_PER_SERVER, tool.name
                    ));
                    break;
                }
                tools.push(imported);
            }
            Err(reason) => warnings.push(format!("[{server}] 拒绝工具 {:?}: {reason}", tool.name)),
        }
    }
    ImportReport { tools, warnings }
}

fn import_one(
    server: &str,
    tool: &RawTool,
    name_taken: &mut dyn FnMut(&str) -> bool,
) -> Result<ImportedTool, String> {
    let raw_name = tool.name.as_str();
    let chars = raw_name.chars().count();
    if chars == 0 || chars > MAX_RAW_NAME_CHARS {
        return Err(format!("名字长度须在 1..={} 字符", MAX_RAW_NAME_CHARS));
    }
    if !raw_name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
    {
        return Err("名字含规范字符集([A-Za-z0-9_.-])之外的字符".into());
    }
    let mangled = raw_name.replace('.', "_");
    let public_name = format!("mcp__{server}__{mangled}");
    if public_name.chars().count() > MAX_PUBLIC_NAME_CHARS {
        return Err(format!(
            "带前缀后超过 {} 字符上限({})",
            MAX_PUBLIC_NAME_CHARS, public_name
        ));
    }
    if name_taken(&public_name) {
        return Err(format!("公开名 {public_name} 已被占用"));
    }

    let schema = match &tool.input_schema {
        Some(schema) if schema.is_object() => schema.clone(),
        Some(_) => return Err("inputSchema 不是 JSON object".into()),
        None => return Err("缺少 inputSchema".into()),
    };
    let schema_bytes = serde_json::to_string(&schema)
        .map(|text| text.len())
        .unwrap_or(usize::MAX);
    if schema_bytes > MAX_SCHEMA_BYTES {
        return Err(format!(
            "inputSchema 序列化 {} 字节,超过 {} 上限",
            schema_bytes, MAX_SCHEMA_BYTES
        ));
    }

    let described = tool
        .description
        .as_deref()
        .filter(|text| !text.trim().is_empty())
        .or(tool.title.as_deref())
        .unwrap_or("(server 未提供描述)");
    let description = format!(
        "[mcp:{server}] {}",
        bound_chars(&util::sanitize(described), MAX_DESCRIPTION_CHARS)
    );

    Ok(ImportedTool {
        public_name,
        original_name: raw_name.to_string(),
        description,
        schema,
    })
}

fn bound_chars(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let mut out: String = text.chars().take(max_chars).collect();
    out.push_str("…(已截断)");
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn raw(name: &str) -> RawTool {
        RawTool {
            name: name.into(),
            title: None,
            description: Some(format!("desc of {name}")),
            input_schema: Some(json!({"type": "object"})),
        }
    }

    fn import_simple(server: &str, tools: Vec<RawTool>) -> ImportReport {
        import_tools(server, tools, None, &[], &mut |_| false)
    }

    #[test]
    fn prefixes_and_mangles_names() {
        let report = import_simple("browser", vec![raw("admin.tools.list")]);
        assert!(report.warnings.is_empty(), "{:?}", report.warnings);
        assert_eq!(
            report.tools[0].public_name,
            "mcp__browser__admin_tools_list"
        );
        assert_eq!(report.tools[0].original_name, "admin.tools.list");
        assert!(report.tools[0].description.starts_with("[mcp:browser] "));
    }

    #[test]
    fn rejects_invalid_names_oversize_and_missing_schema() {
        let cases = vec![
            RawTool {
                input_schema: Some(json!("not object")),
                ..raw("bad_schema")
            },
            RawTool {
                input_schema: None,
                ..raw("no_schema")
            },
            raw("空格 名字"),
            raw(&"x".repeat(200)),
            raw(&"y".repeat(60)), // 前缀后超过 64
        ];
        let report = import_simple("srv", cases);
        assert!(report.tools.is_empty(), "{:?}", report.tools);
        assert_eq!(report.warnings.len(), 5, "{:?}", report.warnings);
    }

    #[test]
    fn rejects_collisions_and_duplicates_deterministically() {
        let mut taken = |name: &str| name == "mcp__srv__occupied";
        let report = import_tools(
            "srv",
            vec![raw("occupied"), raw("free"), raw("free")],
            None,
            &[],
            &mut taken,
        );
        assert_eq!(report.tools.len(), 1);
        assert_eq!(report.tools[0].public_name, "mcp__srv__free");
        assert_eq!(report.warnings.len(), 2, "{:?}", report.warnings);
    }

    #[test]
    fn include_exclude_filter_by_original_name() {
        let tools = vec![raw("keep"), raw("drop"), raw("banned")];
        let include = vec!["keep".to_string(), "banned".to_string()];
        let exclude = vec!["banned".to_string()];
        let report = import_tools("srv", tools, Some(&include), &exclude, &mut |_| false);
        assert_eq!(report.tools.len(), 1);
        assert_eq!(report.tools[0].original_name, "keep");
    }

    #[test]
    fn sanitizes_and_bounds_descriptions() {
        let mut tool = raw("styled");
        tool.description = Some(format!("\u{1b}[31m红色\u{1b}[0m {}", "长".repeat(3000)));
        let report = import_simple("srv", vec![tool]);
        let description = &report.tools[0].description;
        assert!(!description.contains('\u{1b}'));
        assert!(description.chars().count() < 1100, "{}", description.len());
        assert!(description.ends_with("…(已截断)"));
    }

    #[test]
    fn caps_tools_per_server() {
        let tools: Vec<RawTool> = (0..80).map(|i| raw(&format!("tool_{i:03}"))).collect();
        let report = import_simple("srv", tools);
        assert_eq!(report.tools.len(), MAX_TOOLS_PER_SERVER);
        assert!(report
            .warnings
            .iter()
            .any(|warning| warning.contains("上限")));
    }
}
