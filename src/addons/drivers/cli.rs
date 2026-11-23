//! The add-on's command-line flags: `--drivers-scope <scope>` (or
//! `--drivers-scope=<scope>`) opens the page on Kernel modules with that
//! scope.

use super::engine::KernelScope;

/// The raw value of `--drivers-scope` among the app's arguments; None when
/// the flag is missing.
fn scope_value(args: &[String]) -> Option<&str> {
    args.iter().skip(1).enumerate().find_map(|(i, a)| {
        if a == "--drivers-scope" {
            return Some(args.get(i + 2).map(String::as_str).unwrap_or(""));
        }
        a.strip_prefix("--drivers-scope=")
    })
}

/// The kernel scope asked for by `--drivers-scope` among the app's
/// arguments (a [`KernelScope::name`]); None when the flag is missing or
/// its value is not a valid scope.
pub fn kernel_scope(args: &[String]) -> Option<KernelScope> {
    KernelScope::parse(scope_value(args)?)
}

/// What to tell the user when `--drivers-scope` has a value that is not a
/// scope: the value, then the valid ones. None when the flag is missing or
/// valid.
pub fn scope_error(args: &[String]) -> Option<String> {
    let value = scope_value(args)?;
    if KernelScope::parse(value).is_some() {
        return None;
    }
    let valid: Vec<&str> = KernelScope::ALL.iter().map(|s| s.name()).collect();
    Some(format!("{value} (valid scopes: {})", valid.join(", ")))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(a: &[&str]) -> Vec<String> {
        std::iter::once("app.exe").chain(a.iter().copied()).map(String::from).collect()
    }

    #[test]
    fn every_valid_scope_is_read_in_both_spellings() {
        use KernelScope::*;
        for (value, scope) in [("all", All), ("loaded", Loaded), ("not-loaded", NotLoaded), ("third-party", ThirdParty), ("file-system", FileSystem)] {
            assert_eq!(kernel_scope(&args(&["--drivers-scope", value])), Some(scope), "{value}");
            assert_eq!(kernel_scope(&args(&["--page", "Drivers", &format!("--drivers-scope={value}")])), Some(scope), "{value}");
        }
        assert_eq!(kernel_scope(&args(&["--drivers-scope", "Not-Loaded"])), Some(NotLoaded), "case does not matter");
    }

    #[test]
    fn an_invalid_scope_is_reported_with_the_valid_ones() {
        let valid = "(valid scopes: all, loaded, not-loaded, third-party, file-system)";
        assert_eq!(scope_error(&args(&["--drivers-scope", "kernel"])), Some(format!("kernel {valid}")));
        assert_eq!(scope_error(&args(&["--drivers-scope=bogus"])), Some(format!("bogus {valid}")));
        assert_eq!(scope_error(&args(&["--drivers-scope", "Loaded"])), None, "valid in any case");
        assert_eq!(scope_error(&args(&["--page", "Drivers"])), None, "no flag, no error");
    }

    #[test]
    fn a_missing_or_invalid_scope_is_ignored() {
        assert_eq!(kernel_scope(&args(&[])), None);
        assert_eq!(kernel_scope(&args(&["--page", "Drivers"])), None);
        assert_eq!(kernel_scope(&args(&["--drivers-scope"])), None, "no value");
        assert_eq!(kernel_scope(&args(&["--drivers-scope", "--page"])), None, "the next flag is not a value");
        assert_eq!(kernel_scope(&args(&["--drivers-scope", "kernel"])), None);
        assert_eq!(kernel_scope(&args(&["--drivers-scope="])), None);
        assert_eq!(kernel_scope(&args(&["--drivers-scopes", "loaded"])), None, "only the exact flag");
        assert_eq!(kernel_scope(&["--drivers-scope".to_string(), "loaded".to_string()]), None, "the program name is not a flag");
    }
}
