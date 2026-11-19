//! Offline GeoIP enrichment for exit IPs.
//!
//! Proxy exit addresses are looked up in MaxMind format (`.mmdb`) databases
//! such as GeoLite2 Country and ASN, entirely offline. The two databases ship
//! as separate files, so `--geo-db` accepts both and each lookup is tried
//! against every loaded reader.

use std::net::IpAddr;
use std::path::Path;

use anyhow::{Context, Result};
use maxminddb::Reader;

/// Country and ASN of an exit IP.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize)]
pub struct GeoInfo {
    /// English country name, e.g. `Germany`.
    pub country: Option<String>,
    /// Two letter ISO 3166-1 code, e.g. `DE`.
    pub code: Option<String>,
    pub asn: Option<u32>,
    pub as_org: Option<String>,
}

impl GeoInfo {
    /// Compact label for table cells, e.g. `DE · AS3320`.
    pub fn label(&self) -> String {
        let mut parts = Vec::new();
        if let Some(code) = &self.code {
            parts.push(code.clone());
        }
        if let Some(asn) = self.asn {
            parts.push(format!("AS{asn}"));
        }
        if parts.is_empty() {
            "-".into()
        } else {
            parts.join(" · ")
        }
    }

    pub fn is_empty(&self) -> bool {
        self.country.is_none() && self.code.is_none() && self.asn.is_none()
    }
}

impl std::fmt::Display for GeoInfo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.label())
    }
}

/// One or more opened `.mmdb` databases.
#[derive(Debug, Default)]
pub struct GeoDb {
    readers: Vec<Reader<Vec<u8>>>,
}

impl GeoDb {
    /// Opens every given database file. An empty list yields a database that
    /// reports nothing.
    pub fn open<I, P>(paths: I) -> Result<Self>
    where
        I: IntoIterator<Item = P>,
        P: AsRef<Path>,
    {
        let mut readers = Vec::new();
        for path in paths {
            let path = path.as_ref();
            let reader = Reader::open_readfile(path)
                .with_context(|| format!("cannot open GeoIP database `{}`", path.display()))?;
            readers.push(reader);
        }
        Ok(Self { readers })
    }

    /// Looks up an address in every loaded database and merges the results.
    pub fn lookup(&self, address: IpAddr) -> GeoInfo {
        let mut info = GeoInfo::default();

        for reader in &self.readers {
            let Ok(lookup) = reader.lookup(address) else {
                continue;
            };

            if info.code.is_none()
                && let Ok(Some(record)) = lookup.decode::<maxminddb::geoip2::Country>()
            {
                info.code = record.country.iso_code.map(str::to_string);
                info.country = record.country.names.english.map(str::to_string);
            }

            if info.asn.is_none()
                && let Ok(Some(record)) = lookup.decode::<maxminddb::geoip2::Asn>()
            {
                info.asn = record.autonomous_system_number;
                info.as_org = record.autonomous_system_organization.map(str::to_string);
            }
        }

        info
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn label_joins_code_and_asn() {
        let info = GeoInfo {
            country: Some("Germany".into()),
            code: Some("DE".into()),
            asn: Some(3320),
            as_org: Some("Deutsche Telekom".into()),
        };
        assert_eq!(info.label(), "DE · AS3320");
        assert!(!info.is_empty());
    }

    #[test]
    fn label_of_missing_data_is_a_dash() {
        assert_eq!(GeoInfo::default().label(), "-");
        assert!(GeoInfo::default().is_empty());
    }

    #[test]
    fn missing_database_file_is_an_error() {
        assert!(GeoDb::open([&Path::new("/nonexistent/GeoLite2-Country.mmdb")]).is_err());
    }

    #[test]
    fn empty_database_reports_nothing() {
        let db: GeoDb = GeoDb::open(Vec::<std::path::PathBuf>::new()).unwrap();
        let info = db.lookup(IpAddr::from([1, 1, 1, 1]));
        assert!(info.is_empty());
    }
}
