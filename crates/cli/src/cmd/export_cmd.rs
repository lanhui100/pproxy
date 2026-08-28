//! `pproxy config export <service>`（M2 §4.7）。纯本地渲染 + data_plane 推导。

use std::io::Write as _;

use crate::config::{self, PonyConfig};
use crate::export;
use crate::{EXIT_FAILURE, EXIT_LOCAL_CONFIG, EXIT_OK};

pub(crate) fn run(
    cfg: &PonyConfig,
    service: &str,
    route: Option<&str>,
    token: Option<&str>,
) -> Result<i32, String> {
    let Some(tpl) = export::lookup(service) else {
        let _ = writeln!(
            std::io::stderr(),
            "error: unknown service '{service}' — available: {}",
            export::available().join(", ")
        );
        return Ok(EXIT_FAILURE);
    };
    let route = route.unwrap_or(tpl.name);
    let dp = match cfg.data_plane.as_deref().filter(|s| !s.is_empty()) {
        Some(dp) => dp.to_string(),
        None => match config::derive_data_plane(cfg) {
            Ok(dp) => dp,
            Err(e) => {
                let _ = writeln!(std::io::stderr(), "error: {e}");
                return Ok(EXIT_LOCAL_CONFIG);
            }
        },
    };
    if token.is_some() {
        // 显式嵌入明文前 stderr 风险提示（M2 §6）
        let _ = writeln!(std::io::stderr(), "warning: 明文 token 已嵌入输出，注意终端/日志记录");
    }
    print!("{}", export::render(tpl, route, token, &dp));
    Ok(EXIT_OK)
}
