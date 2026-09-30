//! writers — the output backends (plan §6.3 / §7.4).
//!
//! The `EventWriter` trait turns `[EventRow]`s — event-core's flat output
//! contract — into csv / json / parquet files behind one interface, with
//! the `Formats` bitflag selecting any combination and [`WriterError`]
//! covering every failure mode (io, csv, json, arrow, parquet).
//!
//! Every writer is atomic (plan §7.4): the payload is rendered into a
//! uniquely named temp file in the destination directory and renamed over
//! the destination only on success, so an aborted export never leaves a
//! partial file at the final path. See [`atomic`] for the machinery.
//!
//! The `PDF` format bit is already part of the shared `Formats` surface
//! (the CLI and GUI pass every combination in one value); the `PdfBackend`
//! trait and the Typst renderer ship in milestone 11.

pub mod csv;
pub mod error;
pub mod json;
pub mod parquet;

mod atomic;

use event_core::EventRow;
use std::ops::{BitAnd, BitAndAssign, BitOr, BitOrAssign};
use std::path::Path;

pub use csv::CsvWriter;
pub use error::WriterError;
pub use json::JsonWriter;
pub use parquet::ParquetWriter;

/// Output formats the row writers can produce (plan §6.3).
///
/// A bitflag, so any combination (`csv | json | parquet | pdf`) travels
/// through the CLI and GUI as one value. `PDF` is part of the shared
/// surface from here on; the PDF backend lands in milestone 11.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Formats(u8);

impl Formats {
    /// No format selected.
    pub const NONE: Self = Self(0);
    /// Comma-separated values (`CsvWriter`).
    pub const CSV: Self = Self(1 << 0);
    /// JSON array of objects (`JsonWriter`).
    pub const JSON: Self = Self(1 << 1);
    /// Apache Parquet via arrow-rs (`ParquetWriter`).
    pub const PARQUET: Self = Self(1 << 2);
    /// PDF (`PdfBackend`, milestone 11).
    pub const PDF: Self = Self(1 << 3);
    /// Every known format at once.
    pub const ALL: Self = Self(Self::CSV.0 | Self::JSON.0 | Self::PARQUET.0 | Self::PDF.0);

    /// True when every bit of `other` is set in `self`.
    pub fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }

    /// True when no format bit is set.
    pub fn is_empty(self) -> bool {
        self.0 == 0
    }

    /// The enabled formats in deterministic order: csv, json, parquet,
    /// pdf — never hash-dependent (plan §8 rule 12).
    pub fn iter(self) -> impl Iterator<Item = Self> {
        [Self::CSV, Self::JSON, Self::PARQUET, Self::PDF]
            .into_iter()
            .filter(move |f| self.contains(*f))
    }
}

impl BitOr for Formats {
    type Output = Self;

    fn bitor(self, rhs: Self) -> Self {
        Self(self.0 | rhs.0)
    }
}

impl BitOrAssign for Formats {
    fn bitor_assign(&mut self, rhs: Self) {
        self.0 |= rhs.0;
    }
}

impl BitAnd for Formats {
    type Output = Self;

    fn bitand(self, rhs: Self) -> Self {
        Self(self.0 & rhs.0)
    }
}

impl BitAndAssign for Formats {
    fn bitand_assign(&mut self, rhs: Self) {
        self.0 &= rhs.0;
    }
}

/// A row writer: renders `[EventRow]`s into the file at `dest`.
///
/// Implementations serialize the flat contract exactly — the header names
/// / keys / columns match `EventRow`'s Serialize shape (milestone-8 golden
/// test) — and always write atomically (temp file → rename), so `dest` is
/// either absent or complete, never partial. Errors surface as
/// [`WriterError`].
pub trait EventWriter {
    /// Write `rows` to `dest`, replacing any existing file atomically.
    fn write(&self, rows: &[EventRow], dest: &Path) -> Result<(), WriterError>;
}

/// Canonical crate name — smoke tests (and later workspace integration
/// tests) use this to assert crate linkage.
pub const CRATE_NAME: &str = "writers";

#[cfg(test)]
mod tests {
    use super::{CRATE_NAME, Formats};

    #[test]
    fn smoke_crate_builds() {
        assert_eq!(CRATE_NAME, "writers");
    }

    #[test]
    fn singleton_bits_are_distinct_and_stable() {
        let bits = [Formats::CSV, Formats::JSON, Formats::PARQUET, Formats::PDF];
        for (i, a) in bits.iter().enumerate() {
            for (j, b) in bits.iter().enumerate() {
                if i != j {
                    assert_ne!(a.0, b.0, "format bits must be distinct");
                }
            }
        }
        assert!(Formats::ALL.contains(Formats::CSV));
        assert!(Formats::ALL.contains(Formats::JSON));
        assert!(Formats::ALL.contains(Formats::PARQUET));
        assert!(Formats::ALL.contains(Formats::PDF));
        assert!(Formats::NONE.is_empty());
        assert!(!Formats::ALL.is_empty());
    }

    #[test]
    fn union_and_intersection_are_bitwise() {
        let combo = Formats::CSV | Formats::PARQUET;
        assert!(combo.contains(Formats::CSV));
        assert!(combo.contains(Formats::PARQUET));
        assert!(!combo.contains(Formats::JSON));
        assert_eq!(combo & Formats::CSV, Formats::CSV);
        assert_eq!(combo & Formats::JSON, Formats::NONE);
        assert_eq!(combo & Formats::PARQUET, Formats::PARQUET);
        assert_eq!(Formats::NONE | Formats::CSV, Formats::CSV);
        assert_eq!(Formats::ALL & Formats::ALL, Formats::ALL);
    }

    #[test]
    fn iter_yields_enabled_formats_in_contract_order() {
        let all: Vec<Formats> = Formats::ALL.iter().collect();
        assert_eq!(
            all,
            vec![Formats::CSV, Formats::JSON, Formats::PARQUET, Formats::PDF]
        );
        assert!(Formats::NONE.iter().next().is_none());
        let subset: Vec<Formats> = (Formats::JSON | Formats::PDF).iter().collect();
        assert_eq!(subset, vec![Formats::JSON, Formats::PDF]);
    }

    proptest::proptest! {
        /// The bitflag operations follow boolean algebra over the four
        /// bits — the property a bit-level format flag must always hold.
        #[test]
        fn bitflag_algebra(a in 0u8..16, b in 0u8..16, c in 0u8..16) {
            let fa = Formats(a);
            let fb = Formats(b);
            let fc = Formats(c);

            // contains iff every bit is set.
            assert_eq!(fa.contains(fb), fa.0 & fb.0 == fb.0);
            // | and & are idempotent, absorbing, commutative, associative,
            // and together distributive.
            assert_eq!(fa | fa, fa);
            assert_eq!(fa & fa, fa);
            assert_eq!(fa | fb, fb | fa);
            assert_eq!(fa & fb, fb & fa);
            assert_eq!((fa | fb) | fc, fa | (fb | fc));
            assert_eq!((fa & fb) & fc, fa & (fb & fc));
            assert_eq!(fa & (fb | fc), (fa & fb) | (fa & fc));
            // iter() reconstructs exactly the set bits, in contract order,
            // and never yields a format outside the enabled set.
            let bits = Formats(fa.0);
            let restored: Formats = bits.iter().fold(Formats::NONE, |acc, f| acc | f);
            assert_eq!(restored, bits);
            assert!(bits.iter().all(|f| bits.contains(f)));
            // is_empty mirrors the zero bit mask (Default is NONE).
            assert_eq!(bits.is_empty(), fa.0 == 0);
            assert_eq!(Formats::default(), Formats::NONE);
            assert_eq!(fc | Formats::NONE, fc);
        }
    }
}
