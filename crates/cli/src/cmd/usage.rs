//! `pproxy usage`（M2 §4.5）。

use crate::client::AdminClient;
use crate::cmd::route::report_err;
use crate::cmd::service::tokio_block;
use crate::render::{human_bytes, Table};
use crate::EXIT_OK;

pub(crate) fn report(
    http: &AdminClient,
    hours: u64,
    route: Option<&str>,
    token_id: Option<i64>,
) -> Result<i32, String> {
    tokio_block(async {
        match http.usage(hours, route, token_id).await {
            Ok(rep) => {
                let mut t = Table::new(&["route", "token_id", "requests", "bytes_in", "bytes_out"]);
                for r in &rep.rows {
                    t.push(vec![
                        r.route.clone(),
                        r.token_id.to_string(),
                        r.requests.to_string(),
                        human_bytes(r.bytes_in),
                        human_bytes(r.bytes_out),
                    ]);
                }
                print!("{}", t.render());
                println!(
                    "total ({hours}h): requests={} bytes_in={} bytes_out={}",
                    rep.total.requests,
                    human_bytes(rep.total.bytes_in),
                    human_bytes(rep.total.bytes_out),
                );
                Ok(EXIT_OK)
            }
            Err(e) => Ok(report_err(e)),
        }
    })
}
