//! Driver signatures: who signed a driver file, and whether it is WHQL.
//!
//! The check is `WinVerifyTrust` with the driver policy, first on the
//! file's own (embedded) signature, then on the catalog that lists the
//! file's hash (`CryptCATAdmin*`), because most drivers are signed through
//! their package's catalog. It needs no administrator rights. WHQL is
//! decided from the signer chain by [`verdict`].

use std::collections::HashMap;

/// The signer of a driver Microsoft's hardware compatibility program
/// (WHQL) tested: when it is in the signer chain, the driver is WHQL.
pub const WHQL_PUBLISHER: &str = "Microsoft Windows Hardware Compatibility Publisher";

/// What was found about one driver file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Signature {
    /// Signed, and the signer chain has [`WHQL_PUBLISHER`].
    Whql { signer: String },
    /// Signed by someone else.
    Signed { signer: String },
    /// Neither the file nor a catalog carries a signature.
    Unsigned,
    /// The check itself failed: this says why, and it is not "Unsigned".
    CouldNotCheck(String),
}

impl Signature {
    /// What the column and the Signature row show.
    pub fn label(&self) -> String {
        match self {
            Signature::Whql { .. } => "WHQL".into(),
            Signature::Signed { .. } => "Signed".into(),
            Signature::Unsigned => "Unsigned".into(),
            Signature::CouldNotCheck(reason) => format!("Could not check: {reason}"),
        }
    }

    /// The signer's name, when there is one.
    pub fn signer(&self) -> Option<&str> {
        match self {
            Signature::Whql { signer } | Signature::Signed { signer } if !signer.is_empty() => Some(signer),
            _ => None,
        }
    }
}

/// The text of the Signature column and row for a file: its result, or
/// "Not checked" while there is none.
pub fn signature_text(found: Option<&Signature>) -> String {
    found.map_or_else(|| "Not checked".to_string(), Signature::label)
}

/// The verdict for a signature that verified: `chain` holds the names of the
/// certificates from the signer up to the root. An empty chain has no
/// signer, so it is unsigned; a chain with [`WHQL_PUBLISHER`] anywhere in it
/// is WHQL. The signer shown is the first name.
pub fn verdict(chain: &[String]) -> Signature {
    let Some(signer) = chain.first() else { return Signature::Unsigned };
    let signer = signer.clone();
    if chain.iter().any(|name| name.trim().eq_ignore_ascii_case(WHQL_PUBLISHER)) {
        Signature::Whql { signer }
    } else {
        Signature::Signed { signer }
    }
}

/// What the checks found, by file path (case does not matter). The hub
/// replaces the whole book when it grows, so the page holds a cheap
/// snapshot and can tell when it changed.
#[derive(Clone, Debug, Default)]
pub struct SignatureBook(HashMap<String, Signature>);

impl SignatureBook {
    fn key(path: &str) -> String {
        path.trim().to_lowercase()
    }

    /// The result for the file at `path`; None while it is not checked.
    pub fn of(&self, path: &str) -> Option<&Signature> {
        self.0.get(&Self::key(path))
    }

    pub fn insert(&mut self, path: &str, signature: Signature) {
        self.0.insert(Self::key(path), signature);
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(windows)]
mod windows;

#[cfg(target_os = "linux")]
use linux as system;
#[cfg(target_os = "macos")]
use macos as system;
#[cfg(windows)]
use windows as system;

/// The signature of the driver file at `path`, with normal rights.
pub use system::check;

#[cfg(test)]
mod tests {
    use super::*;

    fn names(chain: &[&str]) -> Vec<String> {
        chain.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn a_chain_with_the_hardware_compatibility_publisher_is_whql() {
        let chain = names(&[WHQL_PUBLISHER, "Microsoft Windows Third Party Component CA 2014", "Microsoft Root Authority"]);
        assert_eq!(verdict(&chain), Signature::Whql { signer: WHQL_PUBLISHER.into() });
    }

    #[test]
    fn the_publisher_anywhere_in_the_chain_makes_it_whql_and_the_signer_is_the_first_name() {
        let chain = names(&["Realtek Semiconductor Corp.", WHQL_PUBLISHER, "Microsoft Root Authority"]);
        assert_eq!(verdict(&chain), Signature::Whql { signer: "Realtek Semiconductor Corp.".into() });
        let lower = names(&["microsoft windows hardware compatibility publisher"]);
        assert!(matches!(verdict(&lower), Signature::Whql { .. }), "case does not matter");
    }

    #[test]
    fn any_other_chain_is_signed_with_its_signer() {
        let chain = names(&["WireGuard LLC", "DigiCert Trusted G4 Code Signing", "DigiCert Trusted Root G4"]);
        assert_eq!(verdict(&chain), Signature::Signed { signer: "WireGuard LLC".into() });
        // A look-alike name is not the publisher.
        let fake = names(&["Microsoft Windows Hardware Compatibility Publisher 2", "Some Root"]);
        assert!(matches!(verdict(&fake), Signature::Signed { .. }));
    }

    #[test]
    fn a_chain_without_a_signer_is_unsigned() {
        assert_eq!(verdict(&[]), Signature::Unsigned);
    }

    #[test]
    fn the_shown_values_are_fixed_and_a_failed_check_is_not_unsigned() {
        let whql = Signature::Whql { signer: WHQL_PUBLISHER.into() };
        let signed = Signature::Signed { signer: "Acme".into() };
        let failed = Signature::CouldNotCheck("the file is in use".into());
        assert_eq!(whql.label(), "WHQL");
        assert_eq!(signed.label(), "Signed");
        assert_eq!(Signature::Unsigned.label(), "Unsigned");
        assert_eq!(failed.label(), "Could not check: the file is in use");
        assert_ne!(failed.label(), Signature::Unsigned.label());
        assert_eq!(signature_text(None), "Not checked");
        assert_eq!(signature_text(Some(&signed)), "Signed");
        assert_eq!(whql.signer(), Some(WHQL_PUBLISHER));
        assert_eq!(signed.signer(), Some("Acme"));
        assert_eq!(Signature::Unsigned.signer(), None);
        assert_eq!(failed.signer(), None);
    }

    #[test]
    fn the_book_finds_a_path_whatever_its_case() {
        let mut book = SignatureBook::default();
        assert!(book.is_empty());
        book.insert(r"C:\Windows\System32\Drivers\Disk.sys", Signature::Unsigned);
        assert_eq!(book.of(r"c:\windows\system32\drivers\disk.SYS"), Some(&Signature::Unsigned));
        assert_eq!(book.of(r"C:\other.sys"), None);
        assert_eq!(book.len(), 1);
    }

    /// Catalog-signed Windows drivers have no embedded signature: they must
    /// not be called unsigned. The test binary is signed by nobody.
    #[cfg(windows)]
    #[test]
    #[ignore = "checks this PC's driver files"]
    fn this_pc_does_not_call_catalog_signed_drivers_unsigned() {
        let windir = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into());
        for file in ["disk.sys", "ntfs.sys", "tcpip.sys"] {
            let path = format!(r"{windir}\System32\drivers\{file}");
            let found = check(&path);
            println!("{file}: {found:?}");
            assert!(matches!(found, Signature::Signed { .. } | Signature::Whql { .. }), "{file} is signed through a catalog, not {found:?}");
        }
        let own = std::env::current_exe().unwrap();
        let found = check(&own.to_string_lossy());
        println!("test binary: {found:?}");
        assert_eq!(found, Signature::Unsigned, "a file nobody signed");
        let missing = check(&format!(r"{windir}\System32\drivers\no-such-driver.sys"));
        println!("missing: {missing:?}");
        assert!(matches!(missing, Signature::CouldNotCheck(_)), "a missing file is a failed check, not Unsigned");
    }

    /// Every kernel driver of this PC gets a result, and most are signed.
    #[cfg(windows)]
    #[test]
    #[ignore = "checks this PC's driver files"]
    fn this_pc_checks_every_kernel_driver() {
        use crate::addons::drivers::ListingReader;
        let list = crate::addons::drivers::SystemListing.kernel().expect("kernel drivers");
        let (mut whql, mut signed, mut unsigned, mut failed) = (0, 0, 0, 0);
        for d in list.iter().filter(|d| std::path::Path::new(&d.path).is_file()) {
            match check(&d.path) {
                Signature::Whql { .. } => whql += 1,
                Signature::Signed { .. } => signed += 1,
                Signature::Unsigned => {
                    println!("unsigned: {}", d.path);
                    unsigned += 1
                }
                Signature::CouldNotCheck(why) => {
                    println!("could not check {}: {why}", d.path);
                    failed += 1
                }
            }
        }
        println!("{whql} WHQL, {signed} signed, {unsigned} unsigned, {failed} could not be checked");
        assert!(whql + signed > unsigned + failed, "most of this PC's drivers are signed");
    }
}
