//! MCP 远端工具代理：把 server 声明的工具装进 [`Tool`] 平台。
//!
//! 代理层只做三件事:转发调用(取消/超时语义由 client 承担)、把内容块映射成
//! 有界的 `ToolOutput`(text 拼接、image 落盘、其余丢弃并注明)、把失败分类映射
//! 成稳定 `ToolError`。权限与调度语义固定:非只读、Sequential、未声明路径 →
//! "未声明目标的副作用" Ask,session 按工具名授权;`always_ask` 由 server 配置
//! 收紧。schema 由 server 权威校验,本地只保证参数是 object。

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use serde_json::{Map, Value};

use super::{
    SchemaValidation, Tool, ToolCapabilities, ToolContext, ToolError, ToolErrorCode,
    ToolExecutionMode, ToolOutput, ToolPermissionSpec, ToolSpec,
};
use crate::mcp::client::{CallFailure, CallOutcome, McpClient};
use crate::mcp::import::ImportedTool;
use crate::mcp::{decode_base64, McpToolSeed};

const MAX_IMAGE_BYTES: usize = 8 * 1024 * 1024;
const MAX_IMAGES_PER_CALL: usize = 4;
const MAX_STRUCTURED_BYTES: usize = 64 * 1024;

pub(crate) struct McpTool {
    client: Arc<McpClient>,
    tool: ImportedTool,
    always_ask: bool,
    /// 图像内容块的落盘根目录(`{data_root}/mcp-artifacts`)。内存宿主为 None,
    /// 图像块丢弃并在 details 注明。
    artifacts_dir: Option<PathBuf>,
}

impl McpTool {
    pub(crate) fn from_seed(seed: McpToolSeed, artifacts_dir: Option<PathBuf>) -> McpTool {
        McpTool {
            client: seed.client,
            tool: seed.tool,
            always_ask: seed.always_ask,
            artifacts_dir,
        }
    }

    fn map_failure(&self, failure: CallFailure) -> ToolError {
        let server = &self.client.name;
        match failure {
            CallFailure::Aborted => ToolError::new(
                ToolErrorCode::Aborted,
                "MCP 调用已取消,server 已收到 cancelled 通知",
            ),
            CallFailure::Timeout { limit } => ToolError::new(
                ToolErrorCode::Timeout,
                format!(
                    "MCP 调用超过 {:.0}s 上限,已通知 server 取消",
                    limit.as_secs_f64()
                ),
            ),
            CallFailure::ServerUnavailable { reason } => ToolError::new(
                ToolErrorCode::ExecutionFailed,
                format!("MCP server {server} 不可用: {reason};执行 /reload 可尝试恢复"),
            ),
            CallFailure::Rpc { code, message }
                if code == crate::mcp::protocol::ERROR_INVALID_PARAMS =>
            {
                ToolError::invalid_arguments(format!("server 拒绝参数: {message}"))
            }
            CallFailure::Rpc { code, message } => ToolError::new(
                ToolErrorCode::ExecutionFailed,
                format!("server 协议错误({code}): {message}"),
            ),
            CallFailure::Protocol { message } => {
                ToolError::new(ToolErrorCode::ExecutionFailed, message)
            }
        }
    }

    fn assemble_output(&self, outcome: CallOutcome, session_id: &str) -> ToolOutput {
        let CallOutcome {
            text,
            images,
            mut dropped,
            structured,
            is_error: _,
        } = outcome;
        let mut model_text = text;
        let mut saved_images: Vec<Value> = Vec::new();
        if !images.is_empty() {
            match &self.artifacts_dir {
                Some(root) => {
                    if images.len() > MAX_IMAGES_PER_CALL {
                        dropped.push(format!(
                            "图像块超过单次 {} 张上限,多余的已丢弃",
                            MAX_IMAGES_PER_CALL
                        ));
                    }
                    for image in images.into_iter().take(MAX_IMAGES_PER_CALL) {
                        match self.persist_image(root, session_id, &image.mime, &image.data_base64)
                        {
                            Ok((path, bytes)) => {
                                model_text.push_str(&format!(
                                    "\n[图像已保存: {} ({}, {} 字节)]",
                                    path.display(),
                                    image.mime,
                                    bytes
                                ));
                                saved_images.push(Value::String(path.display().to_string()));
                            }
                            Err(reason) => dropped.push(format!("图像块落盘失败: {reason}")),
                        }
                    }
                }
                None => dropped.push(format!(
                    "{} 个图像块已丢弃(内存宿主没有数据目录)",
                    images.len()
                )),
            }
        }
        if model_text.trim().is_empty() {
            model_text = "(MCP server 未返回文本内容)".into();
        }

        let mut details = Map::new();
        details.insert("server".into(), Value::String(self.client.name.clone()));
        details.insert(
            "tool".into(),
            Value::String(self.tool.original_name.clone()),
        );
        if !saved_images.is_empty() {
            details.insert("images".into(), Value::Array(saved_images));
        }
        if let Some(structured) = structured {
            let bytes = serde_json::to_string(&structured)
                .map(|text| text.len())
                .unwrap_or(usize::MAX);
            if bytes <= MAX_STRUCTURED_BYTES {
                details.insert("structured".into(), structured);
            } else {
                dropped.push(format!(
                    "structuredContent {} 字节超过 {} 上限,已丢弃",
                    bytes, MAX_STRUCTURED_BYTES
                ));
            }
        }
        if !dropped.is_empty() {
            details.insert(
                "dropped".into(),
                Value::Array(dropped.into_iter().map(Value::String).collect()),
            );
        }
        ToolOutput {
            model_text,
            ui_summary: None,
            details: Some(Value::Object(details)),
        }
    }

    fn persist_image(
        &self,
        root: &std::path::Path,
        session_id: &str,
        mime: &str,
        data_base64: &str,
    ) -> Result<(PathBuf, usize), String> {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let bytes = decode_base64(data_base64)?;
        if bytes.len() > MAX_IMAGE_BYTES {
            return Err(format!(
                "{} 字节超过单图 {} 上限",
                bytes.len(),
                MAX_IMAGE_BYTES
            ));
        }
        let dir = root.join(session_id);
        std::fs::create_dir_all(&dir).map_err(|error| error.to_string())?;
        let extension = match mime {
            "image/png" => "png",
            "image/jpeg" => "jpg",
            "image/webp" => "webp",
            "image/gif" => "gif",
            _ => "bin",
        };
        let sequence = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = dir.join(format!(
            "{}-{}-{}.{}",
            self.client.name, self.tool.original_name, sequence, extension
        ));
        std::fs::write(&path, &bytes).map_err(|error| error.to_string())?;
        Ok((path, bytes.len()))
    }
}

impl Tool for McpTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: self.tool.public_name.clone(),
            description: self.tool.description.clone(),
            schema: self.tool.schema.clone(),
            capabilities: ToolCapabilities {
                read_only: false,
                destructive: true,
                execution_mode: ToolExecutionMode::Sequential,
                supports_background: false,
            },
            permission: ToolPermissionSpec {
                path_arguments: Vec::new(),
                always_ask: self.always_ask,
                command: None,
                session_grant_by_name: true,
            },
        }
    }

    fn schema_validation(&self) -> SchemaValidation {
        SchemaValidation::ServerAuthoritative
    }

    fn execute(&self, args: &Value, ctx: &mut ToolContext<'_>) -> Result<ToolOutput, ToolError> {
        let outcome = self
            .client
            .call_tool(&self.tool.original_name, args, ctx.cancel)
            .map_err(|failure| self.map_failure(failure))?;
        if outcome.is_error {
            // 规范:工具执行错误应交给模型自纠,text 就是错误信息。
            let message = if outcome.text.trim().is_empty() {
                "(server 报告工具执行失败,未提供说明)".to_string()
            } else {
                outcome.text
            };
            return Err(ToolError {
                code: ToolErrorCode::ExecutionFailed,
                message,
                retryable: false,
                details: Some(serde_json::json!({
                    "server": self.client.name,
                    "tool": self.tool.original_name,
                    "mcp_is_error": true,
                })),
            });
        }
        Ok(self.assemble_output(outcome, ctx.session_id))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mcp::client::ImageBlock;

    fn seed(always_ask: bool) -> McpToolSeed {
        McpToolSeed {
            client: Arc::new(McpClient::test_stub("browser")),
            tool: ImportedTool {
                public_name: "mcp__browser__click".into(),
                original_name: "click".into(),
                description: "[mcp:browser] click something".into(),
                schema: serde_json::json!({ "type": "object" }),
            },
            always_ask,
        }
    }

    #[test]
    fn spec_declares_sequential_mutation_with_name_scoped_session_grant() {
        let tool = McpTool::from_seed(seed(false), None);
        let spec = tool.spec();
        assert_eq!(spec.name, "mcp__browser__click");
        assert!(!spec.capabilities.read_only);
        assert!(spec.capabilities.destructive);
        assert_eq!(
            spec.capabilities.execution_mode,
            ToolExecutionMode::Sequential
        );
        assert!(!spec.capabilities.supports_background);
        assert!(spec.permission.path_arguments.is_empty());
        assert!(!spec.permission.always_ask);
        assert!(spec.permission.session_grant_by_name);
        assert_eq!(
            tool.schema_validation(),
            SchemaValidation::ServerAuthoritative
        );

        // config 收紧开关直达权限声明。
        let forced = McpTool::from_seed(seed(true), None);
        assert!(forced.spec().permission.always_ask);
    }

    #[test]
    fn failure_mapping_produces_stable_error_codes() {
        let tool = McpTool::from_seed(seed(false), None);
        assert_eq!(
            tool.map_failure(CallFailure::Aborted).code,
            ToolErrorCode::Aborted
        );
        assert_eq!(
            tool.map_failure(CallFailure::Timeout {
                limit: std::time::Duration::from_secs(1)
            })
            .code,
            ToolErrorCode::Timeout
        );
        assert_eq!(
            tool.map_failure(CallFailure::ServerUnavailable { reason: "x".into() })
                .code,
            ToolErrorCode::ExecutionFailed
        );
        assert_eq!(
            tool.map_failure(CallFailure::Rpc {
                code: -32602,
                message: "bad".into()
            })
            .code,
            ToolErrorCode::InvalidArguments
        );
        assert_eq!(
            tool.map_failure(CallFailure::Rpc {
                code: -32000,
                message: "x".into()
            })
            .code,
            ToolErrorCode::ExecutionFailed
        );
        assert_eq!(
            tool.map_failure(CallFailure::Protocol {
                message: "m".into()
            })
            .code,
            ToolErrorCode::ExecutionFailed
        );
    }

    #[test]
    fn assemble_output_persists_images_and_bounds_details() {
        let root = std::env::temp_dir().join(format!(
            "onemore-mcp-proxy-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        let tool = McpTool::from_seed(seed(false), Some(root.clone()));
        let outcome = CallOutcome {
            text: "正文".into(),
            images: vec![ImageBlock {
                mime: "image/png".into(),
                data_base64: "aGk=".into(),
            }],
            dropped: vec!["audio 内容块(v1 不支持,已丢弃)".into()],
            structured: Some(serde_json::json!({ "k": 1 })),
            is_error: false,
        };
        let output = tool.assemble_output(outcome, "session-x");
        assert!(output.model_text.starts_with("正文"));
        assert!(output.model_text.contains("图像已保存"));
        let details = output.details.unwrap();
        assert_eq!(details["server"], "browser");
        assert_eq!(details["tool"], "click");
        assert_eq!(details["structured"], serde_json::json!({ "k": 1 }));
        let images = details["images"].as_array().unwrap();
        assert_eq!(images.len(), 1);
        let saved = std::path::PathBuf::from(images[0].as_str().unwrap());
        assert_eq!(std::fs::read(&saved).unwrap(), b"hi");
        assert!(!details["dropped"].as_array().unwrap().is_empty());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn images_without_data_dir_are_dropped_with_note() {
        let tool = McpTool::from_seed(seed(false), None);
        let outcome = CallOutcome {
            images: vec![ImageBlock {
                mime: "image/png".into(),
                data_base64: "aGk=".into(),
            }],
            ..CallOutcome::default()
        };
        let output = tool.assemble_output(outcome, "session-x");
        assert_eq!(output.model_text, "(MCP server 未返回文本内容)");
        let details = output.details.unwrap();
        let dropped = details["dropped"].as_array().unwrap();
        assert!(
            dropped
                .iter()
                .any(|note| note.as_str().unwrap().contains("图像块已丢弃")),
            "{dropped:?}"
        );
    }
}
