//! `pony token ...`（M2 §4.4）。

use std::io::Write as _;

use crate::client::{AdminClient, ApiError};
use crate::cmd::route::report_err;
use crate::cmd::service::tokio_block;
use crate::render::{fmt_ts, Table};
use crate::{EXIT_FAILURE, EXIT_OK};

pub(crate) fn create(
    http: &AdminClient,
    name: &str,
    expires_days: Option<u64>,
    data_plane_base: &str,
) -> Result<i32, String> {
    tokio_block(async {
        match http.create_token(name, expires_days).await {
            Ok(t) => {
                // 明文一次性高亮打印（M2 §4.4 安全约束允许的唯一输出点之一）
                println!("token created: id={} name={}", t.id, t.name);
                if let Some(exp) = t.expires_at {
                    println!("expires_at: {}", fmt_ts(Some(exp)));
                }
                println!();
                println!("  \x1b[1;32m{}\x1b[0m", t.token);
                println!();
                println!("仅此一次，请立即保存。base_url 片段：");
                println!("  {data_plane_base}/{}/<route>", t.token);
                Ok(EXIT_OK)
            }
            Err(e) => Ok(report_err(e)),
        }
    })
}

pub(crate) fn list(http: &AdminClient) -> Result<i32, String> {
    tokio_block(async {
        match http.list_tokens().await {
            Ok(rows) => {
                let mut t = Table::new(&[
                    "id",
                    "name",
                    "status",
                    "created_at",
                    "expires_at",
                    "last_used_at",
                ]);
                for r in &rows {
                    t.push(vec![
                        r.id.to_string(),
                        r.name.clone(),
                        r.status.clone().unwrap_or_else(|| "?".into()),
                        fmt_ts(r.created_at),
                        fmt_ts(r.expires_at),
                        fmt_ts(r.last_used_at),
                    ]);
                }
                print!("{}", t.render());
                Ok(EXIT_OK)
            }
            Err(e) => Ok(report_err(e)),
        }
    })
}

pub(crate) fn revoke(http: &AdminClient, id: i64) -> Result<i32, String> {
    tokio_block(async {
        match http.revoke_token(id).await {
            Ok(true) => {
                println!("revoked token {id}");
                Ok(EXIT_OK)
            }
            Ok(false) => {
                let _ = writeln!(std::io::stderr(), "error: revoke failed for token {id}");
                Ok(EXIT_FAILURE)
            }
            Err(ApiError::Status(400, m)) => {
                // admin 行防自锁拒绝原样透传（M2 §4.4）
                let _ = writeln!(std::io::stderr(), "error: {m}");
                Ok(EXIT_FAILURE)
            }
            Err(e) => Ok(report_err(e)),
        }
    })
}
