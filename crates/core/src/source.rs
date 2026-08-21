use crate::ProxyRecord;
use std::collections::HashSet;
use tracing::{info, warn};

const PROXYSCRAPE_URL: &str =
    "https://cdn.jsdelivr.net/gh/proxyscrape/free-proxy-list@main/proxies/all/data.json";
const THESPEEDX_HTTP: &str =
    "https://cdn.jsdelivr.net/gh/TheSpeedX/PROXY-List@master/http.txt";

pub async fn fetch_all(countries: &[String]) -> anyhow::Result<Vec<ProxyRecord>> {
    let mut all = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();

    // ProxyScrape JSON
    match reqwest::get(PROXYSCRAPE_URL).await {
        Ok(resp) => {
            if let Ok(raw) = resp.json::<Vec<serde_json::Value>>().await {
                let cc_set: HashSet<String> = countries.iter().map(|s| s.to_uppercase()).collect();
                for v in &raw {
                    let ip = v["ip"].as_str().unwrap_or_default();
                    let port = v["port"].as_u64().unwrap_or(0) as u16;
                    let protocol = v["protocol"].as_str().unwrap_or("http");
                    let cc = v["country_code"].as_str().unwrap_or("");
                    let anon = v["anonymity"].as_str().unwrap_or("");
                    let latency = v["latency_ms"].as_f64().map(|f| f as u64);

                    if (protocol == "http" || protocol == "https" || protocol == "socks5")
                        && cc_set.contains(cc)
                        && !ip.is_empty()
                        && port > 0
                    {
                        let k = format!("{}:{}", ip, port);
                        if seen.insert(k) {
                            all.push(ProxyRecord {
                                ip: ip.into(),
                                port,
                                protocol: protocol.into(),
                                country: Some(cc.into()),
                                anonymity: Some(anon.into()),
                                latency_ms: latency,
                                last_verified: None,
                            });
                        }
                    }
                }
                info!("proxyscrape: fetched {} proxies", all.len());
            }
        }
        Err(e) => warn!("proxyscrape fetch failed: {}", e),
    }

    // TheSpeedX HTTP
    match reqwest::get(THESPEEDX_HTTP).await {
        Ok(resp) => {
            if let Ok(body) = resp.text().await {
                for line in body.lines() {
                    if let Some((ip, port_str)) = line.trim().rsplit_once(':') {
                        if let Ok(port) = port_str.parse::<u16>() {
                            if !ip.is_empty() && port > 0 {
                                let k = format!("{}:{}", ip, port);
                                if seen.insert(k) {
                                    all.push(ProxyRecord {
                                        ip: ip.into(),
                                        port,
                                        protocol: "http".into(),
                                        country: None,
                                        anonymity: None,
                                        latency_ms: None,
                                        last_verified: None,
                                    });
                                }
                            }
                        }
                    }
                }
                info!("thespeedx: total now {}", all.len());
            }
        }
        Err(e) => warn!("thespeedx fetch failed: {}", e),
    }

    Ok(all)
}
