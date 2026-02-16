use anyhow::{Context, Result};
use reqwest::Url;
use std::time::Duration;

use crate::model::RunConfig;

#[derive(Clone)]
pub struct CloudflareClient {
    pub base_url: Url,
    pub meas_id: String,
    pub http: reqwest::Client,
}

impl CloudflareClient {
    pub fn new(cfg: &RunConfig) -> Result<Self> {
        let base_url = Url::parse(&cfg.base_url).context("invalid base_url")?;

        let mut default_headers = reqwest::header::HeaderMap::new();
        default_headers.insert(
            reqwest::header::REFERER,
            "https://speed.cloudflare.com/".parse().unwrap(),
        );

        let mut builder = reqwest::Client::builder()
            .user_agent(cfg.user_agent.clone())
            .default_headers(default_headers)
            .timeout(Duration::from_secs(30))
            .tcp_keepalive(Duration::from_secs(15));

        // Configure binding to interface or source IP if specified
        if let Some(ref iface) = cfg.interface {
            use crate::engine::network_bind;
            match network_bind::get_interface_ip(iface) {
                Ok(ip) => {
                    builder = builder.local_address(ip);
                    eprintln!(
                        "Binding HTTP connections to interface {} (IP: {})",
                        iface, ip
                    );
                }
                Err(e) => {
                    return Err(anyhow::anyhow!(
                        "Failed to get IP address for interface {}: {}",
                        iface,
                        e
                    ));
                }
            }
        } else if let Some(ref source_ip) = cfg.source_ip {
            // Bind to specific source IP address
            match source_ip.parse::<std::net::IpAddr>() {
                Ok(ip) => {
                    builder = builder.local_address(ip);
                    eprintln!("Binding HTTP connections to source IP: {}", ip);
                }
                Err(e) => {
                    return Err(anyhow::anyhow!(
                        "Invalid source IP address format '{}': {}",
                        source_ip,
                        e
                    ));
                }
            }
        }

        // Load custom certificate if provided
        if let Some(ref cert_path) = cfg.certificate_path {
            // Check file extension
            let ext = cert_path
                .extension()
                .and_then(|e| e.to_str())
                .map(|e| e.to_lowercase());

            let valid_extensions = ["pem", "crt", "cer", "der"];
            if let Some(ref ext) = ext {
                if !valid_extensions.contains(&ext.as_str()) {
                    return Err(anyhow::anyhow!(
                        "Invalid certificate file extension '{}'. Expected one of: {}",
                        ext,
                        valid_extensions.join(", ")
                    ));
                }
            } else {
                return Err(anyhow::anyhow!(
                    "Certificate file has no extension. Expected one of: {}",
                    valid_extensions.join(", ")
                ));
            }

            let cert_data = std::fs::read(cert_path).with_context(|| {
                format!("failed to read certificate from {}", cert_path.display())
            })?;

            // Parse based on file extension
            let cert = match ext.as_deref() {
                Some("der") => reqwest::Certificate::from_der(&cert_data).with_context(|| {
                    format!(
                        "failed to parse DER certificate from {}",
                        cert_path.display()
                    )
                })?,
                _ => reqwest::Certificate::from_pem(&cert_data).with_context(|| {
                    format!(
                        "failed to parse PEM certificate from {}",
                        cert_path.display()
                    )
                })?,
            };

            builder = builder.add_root_certificate(cert);
        }

        // Configure proxy if specified
        if let Some(ref proxy_url) = cfg.proxy {
            let proxy = reqwest::Proxy::all(proxy_url).with_context(|| {
                format!(
                    "invalid proxy URL '{}'. Expected format: [protocol://]host[:port]",
                    proxy_url
                )
            })?;
            builder = builder.proxy(proxy);
        }

        let http = builder.build().context("failed to build http client")?;

        Ok(Self {
            base_url,
            meas_id: cfg.meas_id.clone(),
            http,
        })
    }

    pub fn down_url(&self) -> Url {
        self.base_url.join("/__down").expect("join __down")
    }

    pub fn up_url(&self) -> Url {
        self.base_url.join("/__up").expect("join __up")
    }


    pub async fn probe_latency_ms(
        &self,
        during: Option<&str>,
        timeout_ms: u64,
    ) -> Result<(f64, Option<serde_json::Value>)> {
        let mut url = self.down_url();
        {
            let mut qp = url.query_pairs_mut();
            qp.append_pair("bytes", "0");
            if let Some(d) = during {
                qp.append_pair("during", d);
            } else {
                qp.append_pair("measId", &self.meas_id);
            }
        }

        let start = std::time::Instant::now();
        let resp = self
            .http
            .get(url)
            .timeout(Duration::from_millis(timeout_ms))
            .send()
            .await?;

        // Extract meta from headers before consuming body
        let meta = self.extract_meta_from_response(&resp);
        let has_meta = !meta.as_object().map(|m| m.is_empty()).unwrap_or(true);

        // Consume body to keep behavior consistent
        let _ = resp.bytes().await;
        let elapsed = start.elapsed().as_secs_f64() * 1000.0;
        Ok((elapsed, if has_meta { Some(meta) } else { None }))
    }

    pub fn extract_meta_from_response(&self, resp: &reqwest::Response) -> serde_json::Value {
        let mut meta = serde_json::Map::new();

        // Extract from cf-meta-* headers (preferred, contains all info)
        if let Some(ip) = resp
            .headers()
            .get("cf-meta-ip")
            .and_then(|h| h.to_str().ok())
        {
            meta.insert(
                "clientIp".to_string(),
                serde_json::Value::String(ip.to_string()),
            );
        }

        if let Some(colo) = resp
            .headers()
            .get("cf-meta-colo")
            .and_then(|h| h.to_str().ok())
        {
            meta.insert(
                "colo".to_string(),
                serde_json::Value::String(colo.to_string()),
            );
        }

        if let Some(city) = resp
            .headers()
            .get("cf-meta-city")
            .and_then(|h| h.to_str().ok())
        {
            meta.insert(
                "city".to_string(),
                serde_json::Value::String(city.to_string()),
            );
        }

        if let Some(country) = resp
            .headers()
            .get("cf-meta-country")
            .and_then(|h| h.to_str().ok())
        {
            meta.insert(
                "country".to_string(),
                serde_json::Value::String(country.to_string()),
            );
        }

        if let Some(asn) = resp
            .headers()
            .get("cf-meta-asn")
            .and_then(|h| h.to_str().ok())
        {
            // Try parsing as number first, fall back to string
            if let Ok(asn_num) = asn.parse::<i64>() {
                meta.insert("asn".to_string(), serde_json::Value::Number(asn_num.into()));
            } else {
                meta.insert(
                    "asn".to_string(),
                    serde_json::Value::String(asn.to_string()),
                );
            }
        }

        // Fallback to CF-Connecting-IP and CF-RAY if cf-meta-* headers not available
        if !meta.contains_key("clientIp") {
            if let Some(ip) = resp
                .headers()
                .get("cf-connecting-ip")
                .or_else(|| resp.headers().get("CF-Connecting-IP"))
                .and_then(|h| h.to_str().ok())
            {
                meta.insert(
                    "clientIp".to_string(),
                    serde_json::Value::String(ip.to_string()),
                );
            }
        }

        if !meta.contains_key("colo") {
            if let Some(ray) = resp
                .headers()
                .get("cf-ray")
                .or_else(|| resp.headers().get("CF-RAY"))
                .and_then(|h| h.to_str().ok())
            {
                if let Some(colo) = ray.split('-').nth(1) {
                    meta.insert(
                        "colo".to_string(),
                        serde_json::Value::String(colo.to_string()),
                    );
                }
            }
        }

        serde_json::Value::Object(meta)
    }
}

pub async fn fetch_meta_from_response(client: &CloudflareClient) -> Result<serde_json::Value> {
    // Try to get meta info from a test request response headers
    let mut url = client.down_url();
    url.query_pairs_mut()
        .append_pair("bytes", "0")
        .append_pair("measId", &client.meas_id);

    let resp = client.http.get(url).send().await?;

    Ok(client.extract_meta_from_response(&resp))
}

pub async fn fetch_meta(client: &CloudflareClient) -> Result<serde_json::Value> {
    let mut url = client.base_url.join("/meta").context("join /meta")?;
    // Try with measId parameter
    url.query_pairs_mut().append_pair("measId", &client.meas_id);
    let v: serde_json::Value = client.http.get(url).send().await?.json().await?;
    Ok(v)
}

/// Parse the /cdn-cgi/trace endpoint which returns key=value pairs
pub async fn fetch_trace(client: &CloudflareClient) -> Result<serde_json::Value> {
    let url = client
        .base_url
        .join("/cdn-cgi/trace")
        .context("join /cdn-cgi/trace")?;
    let text = client.http.get(url).send().await?.text().await?;

    let mut meta = serde_json::Map::new();
    for line in text.lines() {
        if let Some((key, value)) = line.split_once('=') {
            match key {
                "ip" => {
                    meta.insert(
                        "clientIp".to_string(),
                        serde_json::Value::String(value.to_string()),
                    );
                }
                "colo" => {
                    meta.insert(
                        "colo".to_string(),
                        serde_json::Value::String(value.to_string()),
                    );
                }
                "loc" => {
                    meta.insert(
                        "country".to_string(),
                        serde_json::Value::String(value.to_string()),
                    );
                }
                "tls" => {
                    meta.insert(
                        "tlsVersion".to_string(),
                        serde_json::Value::String(value.to_string()),
                    );
                }
                _ => {}
            }
        }
    }

    Ok(serde_json::Value::Object(meta))
}

pub async fn fetch_locations(client: &CloudflareClient) -> Result<serde_json::Value> {
    let url = client
        .base_url
        .join("/locations")
        .context("join /locations")?;
    let v: serde_json::Value = client.http.get(url).send().await?.json().await?;
    Ok(v)
}

pub fn map_colo_to_server(locations: &serde_json::Value, colo: &str) -> Option<String> {
    // Try to get location info from dynamic locations data
    fn visit(v: &serde_json::Value, colo: &str) -> Option<serde_json::Value> {
        match v {
            serde_json::Value::Array(a) => {
                for x in a {
                    if let Some(f) = visit(x, colo) {
                        return Some(f);
                    }
                }
                None
            }
            serde_json::Value::Object(m) => {
                let keys = ["iata", "colo", "code", "id"];
                let mut matched = false;
                for k in keys {
                    if m.get(k).and_then(|x| x.as_str()) == Some(colo) {
                        matched = true;
                        break;
                    }
                }
                if matched {
                    return Some(serde_json::Value::Object(m.clone()));
                }
                for (_, x) in m {
                    if let Some(f) = visit(x, colo) {
                        return Some(f);
                    }
                }
                None
            }
            _ => None,
        }
    }

    if let Some(obj) = visit(locations, colo) {
        if let Some(m) = obj.as_object() {
            let city = m
                .get("city")
                .and_then(|v| v.as_str())
                .or_else(|| m.get("name").and_then(|v| v.as_str()));
            let country = m
                .get("country")
                .and_then(|v| v.as_str())
                .or_else(|| m.get("countryName").and_then(|v| v.as_str()));

            let mut parts: Vec<String> = Vec::new();
            parts.push(colo.to_string());
            if let Some(c) = city {
                parts.push(c.to_string());
            }
            if let Some(c) = country {
                parts.push(c.to_string());
            }
            if parts.len() >= 2 {
                return Some(parts.join(" - "));
            }
        }
    }

    // Just return the colo code if no location data available
    Some(colo.to_string())
}
