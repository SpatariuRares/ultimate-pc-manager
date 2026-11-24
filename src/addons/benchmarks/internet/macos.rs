//! macOS: no system proxy is read yet.

/// The proxy of the `HTTPS_PROXY`-style environment variables.
pub fn env_proxy() -> Option<ureq::Proxy> {
    ureq::Proxy::try_from_env()
}

/// The system proxy: none is read yet.
pub fn system_proxy() -> Option<String> {
    None
}
