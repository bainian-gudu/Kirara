use std::collections::HashMap;
use std::sync::Arc;

use serde_json::{json, Value};

use super::{HostCtx, HostHandle, UiAction};
use crate::utils::error::TACommandError;

#[derive(Debug, serde::Deserialize)]
struct InvokeMessage {
    id: u64,
    kind: String,
    cmd: String,
    #[serde(default)]
    args: Value,
}

pub fn on_message(ctx: &Arc<HostCtx>, handle: &HostHandle, json: &str) {
    let Ok(message) = serde_json::from_str::<InvokeMessage>(json) else {
        return;
    };
    if message.kind != "invoke" {
        return;
    }

    let ctx = ctx.clone();
    let handle = handle.clone();
    tokio::spawn(async move {
        let closing = matches!(message.cmd.as_str(), "window_close" | "launch_and_exit");
        let result = dispatch(&ctx, &handle, &message.cmd, message.args).await;
        let (ok, data) = match result {
            Ok(data) => (true, data),
            Err(error) => (false, error_payload(&error)),
        };
        handle.send(UiAction::Reply {
            id: message.id,
            ok,
            data,
        });
        if closing && ok {
            handle.send(UiAction::Close);
        }
    });
}

fn error_payload(error: &TACommandError) -> Value {
    json!({
        "message": format!("{:#}", error.error),
        "insight": error.insight,
    })
}

async fn dispatch(
    ctx: &HostCtx,
    handle: &HostHandle,
    cmd: &str,
    args: Value,
) -> Result<Value, TACommandError> {
    match cmd {
        "get_installer_config" => {
            let scan_exe = req_bool(&args, &["scanExe", "scan_exe"])?;
            ok(crate::installer::config::get_installer_config(&ctx.args, scan_exe).await?)
        }
        "select_dir" => {
            let path = req_str(&args, &["path"])?;
            let exe_name = req_str(&args, &["exeName", "exe_name"])?;
            let legacy_exe_names = opt_string_vec(&args, &["legacyExeNames", "legacy_exe_names"]);
            let silent = req_bool(&args, &["silent"])?;
            let parent = handle.dialog_parent();
            ok(crate::installer::select_dir(
                path,
                exe_name,
                legacy_exe_names,
                silent,
                Some(&parent),
            )
            .await)
        }
        "get_dirs" => {
            let elevated = req_bool(&args, &["elevated"])?;
            ok(crate::installer::lnk::get_dirs(elevated).await?)
        }
        "read_uninstall_metadata" => {
            let reg_name = req_str(&args, &["regName", "reg_name"])?;
            ok(crate::installer::registry::read_uninstall_metadata(reg_name).await?)
        }
        "get_exe_version" => {
            let exe_name = req_str(&args, &["exeName", "exe_name"])?;
            ok(crate::installer::get_exe_version(exe_name).await?)
        }
        "get_mirrorc_status" => {
            let resource_id = req_str(&args, &["resourceId", "resource_id"])?;
            let current_version = req_str(&args, &["currentVersion", "current_version"])?;
            let cdk = req_str(&args, &["cdk"])?;
            let channel = req_str(&args, &["channel"])?;
            let arch = opt_str(&args, &["arch"]);
            let os = opt_str(&args, &["os"]);
            ok(crate::thirdparty::mirrorc::get_mirrorc_status(
                &resource_id,
                &current_version,
                &cdk,
                &channel,
                arch.as_deref(),
                os.as_deref(),
            )
            .await?)
        }
        "get_dfs" => {
            let url = req_str(&args, &["url"])?;
            let range = opt_str(&args, &["range"]);
            let extras = opt_str(&args, &["extras"]);
            ok(crate::dfs::get_dfs(url, range, extras)
                .await
                .map_err(string_error)?)
        }
        "get_http_with_range" => {
            let url = req_str(&args, &["url"])?;
            let offset = req_u64(&args, &["offset"])?;
            let size = req_u64(&args, &["size"])?;
            ok(crate::dfs::get_http_with_range(url, offset, size).await?)
        }
        "http_get_request" => {
            let url = req_str(&args, &["url"])?;
            let ignore_redirects = opt_bool(&args, &["ignoreRedirects", "ignore_redirects"]);
            let headers = opt_headers(&args);
            let timeout_ms = opt_u64(&args, &["timeoutMs", "timeout_ms"]);
            ok(
                crate::dfs::http_get_request(url, ignore_redirects, headers, timeout_ms)
                    .await
                    .map_err(string_error)?,
            )
        }
        "get_dfs2_metadata" => {
            let api_url = req_str(&args, &["apiUrl", "api_url"])?;
            ok(crate::dfs::get_dfs2_metadata(api_url)
                .await
                .map_err(string_error)?)
        }
        "create_dfs2_session" => {
            let api_url = req_str(&args, &["apiUrl", "api_url"])?;
            let chunks = opt_string_vec_opt(&args, &["chunks"]);
            let version = opt_str(&args, &["version"]);
            let challenge_response = opt_str(&args, &["challengeResponse", "challenge_response"]);
            let session_id = opt_str(&args, &["sessionId", "session_id"]);
            let extras = args.get("extras").cloned();
            ok(crate::dfs::create_dfs2_session(
                api_url,
                chunks,
                version,
                challenge_response,
                session_id,
                extras,
            )
            .await
            .map_err(string_error)?)
        }
        "get_dfs2_chunk_url" => {
            let session_api_url = req_str(&args, &["sessionApiUrl", "session_api_url"])?;
            let range = req_str(&args, &["range"])?;
            ok(crate::dfs::get_dfs2_chunk_url(session_api_url, range)
                .await
                .map_err(string_error)?)
        }
        "get_dfs2_batch_chunk_urls" => {
            let session_api_url = req_str(&args, &["sessionApiUrl", "session_api_url"])?;
            let chunks = req_string_vec(&args, &["chunks"])?;
            ok(
                crate::dfs::get_dfs2_batch_chunk_urls(session_api_url, chunks)
                    .await
                    .map_err(string_error)?,
            )
        }
        "end_dfs2_session" => {
            let session_api_url = req_str(&args, &["sessionApiUrl", "session_api_url"])?;
            let insights = args
                .get("insights")
                .cloned()
                .map(serde_json::from_value)
                .transpose();
            let insights = match insights {
                Ok(value) => value,
                Err(error) => {
                    return Err(TACommandError::new(anyhow::anyhow!(
                        "invalid DFS2 insights: {error}"
                    )))
                }
            };
            ok(crate::dfs::end_dfs2_session(session_api_url, insights)
                .await
                .map_err(string_error)?)
        }
        "solve_dfs2_challenge" => {
            let challenge_type = req_str(&args, &["challengeType", "challenge_type"])?;
            let data = req_str(&args, &["data"])?;
            ok(crate::dfs::solve_dfs2_challenge(challenge_type, data)
                .await
                .map_err(string_error)?)
        }
        "is_dir_empty" => {
            let path = req_str(&args, &["path"])?;
            let exe_name = opt_str(&args, &["exeName", "exe_name"]).unwrap_or_default();
            ok(crate::fs::is_dir_empty(path, exe_name).await)
        }
        "ensure_dir" => {
            let path = req_str(&args, &["path"])?;
            ok(crate::fs::ensure_dir(path).await?)
        }
        "wincred_write" => {
            let target = req_str(&args, &["target"])?;
            let token = req_str(&args, &["token"])?;
            let comment = req_str(&args, &["comment"])?;
            ok(crate::utils::wincred::wincred_write(
                &target, &token, &comment,
            )?)
        }
        "wincred_read" => {
            let target = req_str(&args, &["target"])?;
            ok(crate::utils::wincred::wincred_read(&target)?)
        }
        "wincred_delete" => {
            let target = req_str(&args, &["target"])?;
            ok(crate::utils::wincred::wincred_delete(&target)?)
        }
        "managed_operation" => {
            let legacy = args
                .get("ipc")
                .cloned()
                .ok_or_else(|| missing("ipc"))
                .and_then(|value| {
                    serde_json::from_value::<crate::ipc_v2::legacy::LegacyIpcOperation>(value)
                        .map_err(|error| {
                            TACommandError::new(anyhow::anyhow!("invalid IPC operation: {error}"))
                        })
                })?;
            let id = req_str(&args, &["id"])?;
            let elevate = req_bool(&args, &["elevate"])?;
            let progress_kind = legacy.progress_kind();
            let ipc = legacy.into_v2();
            let handle = handle.clone();
            let progress_id = id.clone();
            let progress = crate::ipc_v2::progress_notify(move |progress| {
                handle.emit(
                    &progress_id,
                    crate::ipc_v2::legacy::progress_payload(progress_kind, progress),
                );
            });
            match ctx.elevate.run(ipc, elevate, progress).await {
                Ok(result) => Ok(json!({
                    "Ok": crate::ipc_v2::legacy::result_payload(result)?,
                })),
                Err(error) => Ok(json!({ "Err": error })),
            }
        }
        "launch" => {
            crate::installer::launch(req_str(&args, &["path"])?).await;
            ok(())
        }
        "launch_and_exit" => {
            crate::installer::launch_and_exit(req_str(&args, &["path"])?).await;
            ok(())
        }
        "error_dialog" => {
            let parent = handle.dialog_parent();
            crate::installer::error_dialog(
                req_str(&args, &["title"])?,
                req_str(&args, &["message"])?,
                &ctx.args,
                Some(&parent),
            )
            .await
            .map_err(string_error)?;
            ok(())
        }
        "confirm_dialog" => {
            let parent = handle.dialog_parent();
            let confirmed = crate::installer::confirm_dialog(
                req_str(&args, &["title"])?,
                req_str(&args, &["message"])?,
                &ctx.args,
                Some(&parent),
            )
            .await
            .map_err(string_error)?;
            ok(confirmed)
        }
        "log" => {
            crate::installer::log(string_arg(&args, "data"));
            ok(())
        }
        "warn" => {
            crate::installer::warn(string_arg(&args, "data"));
            ok(())
        }
        "error" => {
            crate::installer::error(string_arg(&args, "data"));
            ok(())
        }
        "window_show" => {
            handle.send(UiAction::Show);
            ok(())
        }
        "window_close" => ok(()),
        "window_minimize" => {
            handle.send(UiAction::Minimize);
            ok(())
        }
        "window_set_title" => {
            handle.send(UiAction::SetTitle(req_str(&args, &["title"])?));
            ok(())
        }
        "window_set_decorations" => {
            handle.send(UiAction::SetDecorations(req_bool(&args, &["decorations"])?));
            ok(())
        }
        other => Err(TACommandError::new(anyhow::anyhow!(
            "unknown host command: {other}"
        ))),
    }
}

fn field<'a>(args: &'a Value, keys: &[&str]) -> Option<&'a Value> {
    keys.iter().find_map(|key| args.get(*key))
}

fn req_str(args: &Value, keys: &[&str]) -> Result<String, TACommandError> {
    field(args, keys)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| missing(keys[0]))
}

fn opt_str(args: &Value, keys: &[&str]) -> Option<String> {
    field(args, keys).and_then(Value::as_str).map(str::to_owned)
}

fn req_bool(args: &Value, keys: &[&str]) -> Result<bool, TACommandError> {
    field(args, keys)
        .and_then(Value::as_bool)
        .ok_or_else(|| missing(keys[0]))
}

fn opt_bool(args: &Value, keys: &[&str]) -> Option<bool> {
    field(args, keys).and_then(Value::as_bool)
}

fn req_u64(args: &Value, keys: &[&str]) -> Result<u64, TACommandError> {
    field(args, keys)
        .and_then(Value::as_u64)
        .ok_or_else(|| missing(keys[0]))
}

fn opt_u64(args: &Value, keys: &[&str]) -> Option<u64> {
    field(args, keys).and_then(Value::as_u64)
}

fn req_string_vec(args: &Value, keys: &[&str]) -> Result<Vec<String>, TACommandError> {
    field(args, keys)
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .ok_or_else(|| missing(keys[0]))
}

fn opt_string_vec(args: &Value, keys: &[&str]) -> Vec<String> {
    field(args, keys)
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

fn opt_string_vec_opt(args: &Value, keys: &[&str]) -> Option<Vec<String>> {
    field(args, keys).and_then(Value::as_array).map(|items| {
        items
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_owned)
            .collect()
    })
}

fn opt_headers(args: &Value) -> Option<HashMap<String, String>> {
    args.get("headers")?.as_object().map(|headers| {
        headers
            .iter()
            .filter_map(|(key, value)| value.as_str().map(|value| (key.clone(), value.to_owned())))
            .collect()
    })
}

fn string_arg(args: &Value, key: &str) -> String {
    args.get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned()
}

fn missing(field: &str) -> TACommandError {
    TACommandError::new(anyhow::anyhow!("missing field: {field}"))
}

/// DFS 命令的错误类型在新版 `dfs.rs` 里从 `String` 换成了 `anyhow::Error`。
/// 这里接受任何可显示的错误（`String` 与 `anyhow::Error` 都算），
/// 两种形状的调用点都不用改。
fn string_error<E: std::fmt::Display>(error: E) -> TACommandError {
    TACommandError::new(anyhow::anyhow!("{error:#}"))
}

fn ok<T: serde::Serialize>(value: T) -> Result<Value, TACommandError> {
    serde_json::to_value(value).map_err(|error| TACommandError::new(anyhow::anyhow!(error)))
}
