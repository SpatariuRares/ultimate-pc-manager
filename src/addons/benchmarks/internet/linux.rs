//! Linux: the proxy of the standard environment variables.
//!
//! `https_proxy` / `HTTPS_PROXY`, then `http_proxy` / `HTTP_PROXY`, and the
//! `no_proxy` / `NO_PROXY` list. Desktop proxy settings are not read.

use super::{proxy_for, HOST};

/// No separate environment proxy: [`system_proxy`] reads the variables
/// itself (lower case wins, `no_proxy` honoured).
pub fn env_proxy() -> Option<ureq::Proxy> {
    None
}

/// The system proxy for [`HOST`], from the process environment.
pub fn system_proxy() -> Option<String> {
    proxy_from_env(|name| std::env::var(name).ok())
}

/// The proxy for [`HOST`] given an environment lookup. For each setting the
/// lower-case name wins over the upper-case one; empty values count as unset.
fn proxy_from_env(get: impl Fn(&str) -> Option<String>) -> Option<String> {
    let setting = |lower: &str| {
        [lower.to_string(), lower.to_ascii_uppercase()]
            .iter()
            .find_map(|name| get(name).map(|v| v.trim().to_string()).filter(|v| !v.is_empty()))
    };
    let server = setting("https_proxy").or_else(|| setting("http_proxy"))?;
    let bypass = setting("no_proxy").map(|list| bypass_list(&list)).unwrap_or_default();
    proxy_for(&server, &bypass, HOST)
}

/// A `no_proxy` list ("example.com,.corp,*") as the bypass list [`proxy_for`]
/// reads: a name covers itself and its subdomains, a leading dot only the
/// subdomains. Ports are ignored.
fn bypass_list(no_proxy: &str) -> String {
    let mut entries = Vec::new();
    for raw in no_proxy.split(',').map(str::trim).filter(|e| !e.is_empty()) {
        if raw == "*" {
            entries.push("*".to_string());
            continue;
        }
        let name = raw.rsplit_once(':').filter(|(_, port)| port.chars().all(|c| c.is_ascii_digit())).map_or(raw, |(n, _)| n);
        let name = name.trim_start_matches("*.");
        match name.strip_prefix('.') {
            Some(domain) => entries.push(format!("*.{domain}")),
            None => {
                entries.push(name.to_string());
                entries.push(format!("*.{name}"));
            }
        }
    }
    entries.join(";")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn proxy(vars: &[(&str, &str)]) -> Option<String> {
        let env: HashMap<String, String> = vars.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        proxy_from_env(|name| env.get(name).cloned())
    }

    #[test]
    fn no_variable_means_no_proxy() {
        assert_eq!(proxy(&[]), None);
        assert_eq!(proxy(&[("https_proxy", "")]), None, "an empty value is unset");
    }

    #[test]
    fn lower_and_upper_case_variables_are_both_read() {
        assert_eq!(proxy(&[("https_proxy", "p:1")]).as_deref(), Some("http://p:1"));
        assert_eq!(proxy(&[("HTTPS_PROXY", "p:2")]).as_deref(), Some("http://p:2"));
        assert_eq!(proxy(&[("http_proxy", "p:3")]).as_deref(), Some("http://p:3"));
        assert_eq!(proxy(&[("HTTP_PROXY", "p:4")]).as_deref(), Some("http://p:4"));
    }

    #[test]
    fn lower_case_wins_over_upper_case_and_https_over_http() {
        assert_eq!(proxy(&[("https_proxy", "low:1"), ("HTTPS_PROXY", "up:1")]).as_deref(), Some("http://low:1"));
        assert_eq!(proxy(&[("https_proxy", "s:1"), ("http_proxy", "h:1")]).as_deref(), Some("http://s:1"));
        assert_eq!(proxy(&[("HTTPS_PROXY", "p:1"), ("https_proxy", "")]).as_deref(), Some("http://p:1"), "empty lower case is unset");
    }

    #[test]
    fn a_full_proxy_url_is_kept() {
        assert_eq!(proxy(&[("https_proxy", "http://p.corp:3128")]).as_deref(), Some("http://p.corp:3128"));
    }

    #[test]
    fn no_proxy_entries_bypass_the_proxy_for_the_test_host() {
        let with = |no_proxy: &str| proxy(&[("https_proxy", "p:1"), ("no_proxy", no_proxy)]);
        assert_eq!(with("speed.cloudflare.com"), None);
        assert_eq!(with("localhost, cloudflare.com"), None, "a name covers its subdomains");
        assert_eq!(with(".cloudflare.com"), None, "a leading dot covers subdomains");
        assert_eq!(with("*.cloudflare.com"), None);
        assert_eq!(with("cloudflare.com:443"), None, "ports are ignored");
        assert_eq!(with("*"), None);
        assert_eq!(with("example.com,localhost").as_deref(), Some("http://p:1"), "other hosts do not bypass");
        assert_eq!(with("ed.cloudflare.com").as_deref(), Some("http://p:1"), "a partial label does not match");
        assert_eq!(proxy(&[("https_proxy", "p:1"), ("NO_PROXY", "cloudflare.com")]), None, "upper case NO_PROXY is read");
        assert_eq!(proxy(&[("https_proxy", "p:1"), ("no_proxy", "example.com"), ("NO_PROXY", "cloudflare.com")]).as_deref(), Some("http://p:1"), "lower case no_proxy wins");
    }
}
