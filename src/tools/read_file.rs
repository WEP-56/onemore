//! read_file:带行号读取文本文件,支持 offset/limit 分段读大文件。
//!
//! 行号输出(`   12 | 内容`)不只是好看:edit_file 要求模型提供精确的
//! 原文片段,行号能帮模型定位;行号格式用 " | " 分隔避免与内容混淆。

use base64::Engine;
use serde_json::{json, Value};

use crate::message::ImageContent;

use super::{
    image::{detect_image, DetectedImage},
    optional_u64, require_str, Tool, ToolCapabilities, ToolContext, ToolError, ToolErrorCode,
    ToolOutput, ToolPermissionSpec, ToolSpec,
};

/// 默认/最大单次读取行数。要更多就带 offset 再调一次(教模型分页)。
const DEFAULT_LIMIT: u64 = 1000;
const MAX_LIMIT: u64 = 4000;
/// 单行超过这个字符数会被折断显示(防止压缩过的 js 一行几万字符)。
const MAX_LINE_CHARS: usize = 500;
/// Keep the encoded payload below the common 5 MiB inline-image limit.
const MAX_INLINE_BASE64_BYTES: usize = 4_718_592;

pub struct ReadFile;

impl Tool for ReadFile {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "read_file".into(),
            description: "读取文本文件或图片。文本带行号返回,可用 offset(起始行号,从 1 开始)与 limit(行数,默认 1000)分段读取；JPEG、PNG、GIF、WebP 图片作为附件发送给模型，offset/limit 对图片无效。修改文件前应先用本工具查看现状。".into(),
            schema: json!({
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "path": { "type": "string", "minLength": 1, "description": "文件路径,相对工作目录或绝对路径" },
                    "offset": { "type": "integer", "minimum": 1, "description": "起始行号(1-based),默认 1" },
                    "limit": { "type": "integer", "minimum": 1, "maximum": 4000, "description": "最多读取的行数,默认 1000" }
                },
                "required": ["path"]
            }),
            capabilities: ToolCapabilities::READ_ONLY,
            permission: ToolPermissionSpec::paths(&["path"]),
        }
    }

    fn execute(&self, args: &Value, ctx: &mut ToolContext<'_>) -> Result<ToolOutput, ToolError> {
        let path = ctx.workspace.resolve(require_str(args, "path")?);
        let offset = optional_u64(args, "offset")?.unwrap_or(1).max(1);
        let limit = optional_u64(args, "limit")?
            .unwrap_or(DEFAULT_LIMIT)
            .clamp(1, MAX_LIMIT);

        let bytes = ctx.workspace.read_bytes(&path).map_err(ToolError::io)?;
        let display_path = ctx.workspace.display_model_path(&path);
        match detect_image(&bytes) {
            Some(DetectedImage::Supported(mime_type)) => {
                let encoded_len = bytes.len().div_ceil(3) * 4;
                if encoded_len > MAX_INLINE_BASE64_BYTES {
                    let text = format!(
                        "Read image file: {display_path} ({mime_type}, {} bytes)\n[Image omitted: encoded payload {encoded_len} bytes exceeds inline limit {MAX_INLINE_BASE64_BYTES}.]",
                        bytes.len()
                    );
                    return Ok(ToolOutput {
                        model_text: text.clone(),
                        images: Vec::new(),
                        ui_summary: Some(text),
                        details: Some(json!({
                            "path": display_path,
                            "mime_type": mime_type,
                            "bytes": bytes.len(),
                            "image_attached": false,
                            "reason": "inline_size_limit",
                        })),
                    });
                }
                let text = format!(
                    "Read image file: {display_path} ({mime_type}, {} bytes)",
                    bytes.len()
                );
                return Ok(ToolOutput {
                    model_text: text.clone(),
                    images: vec![ImageContent {
                        data: base64::engine::general_purpose::STANDARD.encode(&bytes),
                        mime_type: mime_type.into(),
                    }],
                    ui_summary: Some(text),
                    details: Some(json!({
                        "path": display_path,
                        "mime_type": mime_type,
                        "bytes": bytes.len(),
                        "image_attached": true,
                    })),
                });
            }
            Some(DetectedImage::Unsupported(mime_type)) => {
                let text = format!(
                    "Read image file: {display_path} ({mime_type}, {} bytes)\n[Image omitted: this image format is detected but not supported for inline model input.]",
                    bytes.len()
                );
                return Ok(ToolOutput {
                    model_text: text.clone(),
                    images: Vec::new(),
                    ui_summary: Some(text),
                    details: Some(json!({
                        "path": display_path,
                        "mime_type": mime_type,
                        "bytes": bytes.len(),
                        "image_attached": false,
                        "reason": "unsupported_image_format",
                    })),
                });
            }
            None => {}
        }

        let content = String::from_utf8(bytes).map_err(|_| {
            ToolError::io(format!(
                "{} is neither UTF-8 text nor a supported image (JPEG, PNG, GIF, WebP)",
                path.display()
            ))
        })?;
        // 统一按 \n 分行(\r 在显示层已无意义,这里直接修剪)
        let lines: Vec<&str> = content.split('\n').collect();
        let total = lines.len();

        if offset as usize > total {
            return Err(ToolError::new(
                ToolErrorCode::InvalidArguments,
                format!("offset={} 超出范围,文件共 {} 行", offset, total),
            ));
        }

        let start = (offset - 1) as usize;
        let end = (start + limit as usize).min(total);
        let mut out = String::new();
        for (idx, raw) in lines[start..end].iter().enumerate() {
            let line = raw.trim_end_matches('\r');
            let shown: String = if line.chars().count() > MAX_LINE_CHARS {
                let cut: String = line.chars().take(MAX_LINE_CHARS).collect();
                format!("{}……[本行截断]", cut)
            } else {
                line.to_string()
            };
            out.push_str(&format!("{:>6} | {}\n", start + idx + 1, shown));
        }
        if end < total {
            out.push_str(&format!(
                "……[仅显示第 {}-{} 行,共 {} 行;继续读请传 offset={}]",
                start + 1,
                end,
                total,
                end + 1
            ));
        }
        Ok(ToolOutput {
            model_text: out,
            images: Vec::new(),
            ui_summary: Some(format!(
                "已读取 {} 第 {}-{} 行",
                ctx.workspace.display(&path),
                start + 1,
                end
            )),
            details: Some(json!({
                "path": ctx.workspace.display(&path),
                "start_line": start + 1,
                "end_line": end,
                "total_lines": total,
                "truncated": end < total,
                "next_offset": if end < total { Some(end + 1) } else { None },
            })),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plan::PlanSnapshot;
    use crate::tools::{Tool, ToolContext};
    use crate::workspace::Workspace;
    use std::sync::atomic::AtomicBool;

    fn execute(path: &str, bytes: &[u8]) -> ToolOutput {
        let root = std::env::temp_dir().join(format!(
            "onemore-read-image-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let file = root.join(path);
        std::fs::write(&file, bytes).unwrap();
        let workspace = Workspace::new(root.clone());
        let cancel = AtomicBool::new(false);
        let mut progress = |_| {};
        let mut ctx = ToolContext {
            workspace: &workspace,
            cancel: &cancel,
            session_id: "test",
            current_plan: PlanSnapshot::default(),
            progress: &mut progress,
            effects: Vec::new(),
        };
        let output = ReadFile
            .execute(&serde_json::json!({"path": path}), &mut ctx)
            .unwrap();
        let _ = std::fs::remove_dir_all(root);
        output
    }

    #[test]
    fn returns_supported_image_as_attachment_and_path_text() {
        let output = execute("photo.any", &[0xff, 0xd8, 0xff, 0xe0]);
        assert_eq!(output.images.len(), 1);
        assert_eq!(output.images[0].mime_type, "image/jpeg");
        assert!(output.model_text.contains("photo.any"));
        assert!(output.model_text.contains("image/jpeg"));
        assert!(output.ui_text().contains("photo.any"));
    }

    #[test]
    fn rejects_unknown_binary_as_readable_error() {
        let root = std::env::temp_dir().join(format!(
            "onemore-read-binary-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let file = root.join("data.bin");
        std::fs::write(&file, [0xff, 0x00, 0x01]).unwrap();
        let workspace = Workspace::new(root.clone());
        let cancel = AtomicBool::new(false);
        let mut progress = |_| {};
        let mut ctx = ToolContext {
            workspace: &workspace,
            cancel: &cancel,
            session_id: "test",
            current_plan: PlanSnapshot::default(),
            progress: &mut progress,
            effects: Vec::new(),
        };
        let error = ReadFile
            .execute(&serde_json::json!({"path": "data.bin"}), &mut ctx)
            .unwrap_err();
        assert!(error
            .message
            .contains("neither UTF-8 text nor a supported image"));
        let _ = std::fs::remove_dir_all(root);
    }
}
