//! CSV row writer (plan §6.3): serde-driven records over the [`EventRow`]
//! contract, one row per (event, subject) — family/couple events collapse
//! to one row per event carrying both spouses (plan §3.4).

use std::path::Path;

use event_core::EventRow;

use crate::EventWriter;
use crate::atomic::write_atomically;
use crate::error::WriterError;

/// Writes [`EventRow`]s as RFC 4180 CSV.
///
/// The header row is derived from `EventRow`'s field names — the same
/// names the JSON keys and Parquet columns use (locked by the milestone-8
/// golden test) — and every row is one record. `Option` fields serialize
/// as empty fields (the CSV convention); note this collides with
/// `Some("")` on read-back, which is inherent to the format (see the
/// `some_empty_string_collides_with_none` test). With zero rows the file
/// is entirely empty (the header is emitted with the first record, so an
/// empty event list is still a valid, empty CSV).
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct CsvWriter;

impl EventWriter for CsvWriter {
    fn write(&self, rows: &[EventRow], dest: &Path) -> Result<(), WriterError> {
        write_atomically(dest, |mut file| {
            let mut wtr = csv::Writer::from_writer(&mut file);
            for row in rows {
                wtr.serialize(row)?;
            }
            // csv::Writer::flush pushes to the OS before the rename; this
            // is the boundary that makes the temp-file rename safe.
            wtr.flush().map_err(|e| WriterError::io(dest, e))?;
            Ok(())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::CsvWriter;
    use crate::EventWriter;
    use event_core::EventRow;
    use std::fs;
    use tempfile::TempDir;

    /// The field names the milestone-8 golden test locks as the JSON/CSV
    /// schema — mirrored here so the header test stays readable.
    const HEADER: [&str; 21] = [
        "person_id",
        "person_id_2",
        "person_name",
        "event_id",
        "event_type",
        "event_date",
        "event_date_text",
        "event_date_stop",
        "date_is_range",
        "year",
        "month",
        "day",
        "anniversary_month",
        "anniversary_day",
        "leap_day_folded",
        "place",
        "role",
        "age_at_event",
        "reference_year",
        "elapsed_years",
        "private",
    ];

    fn row(person_id: Option<&str>) -> EventRow {
        EventRow {
            person_id: person_id.map(str::to_string),
            person_id_2: None,
            person_name: "Abraham Meowser".to_string(),
            event_id: Some("E0005".to_string()),
            event_type: "Death".to_string(),
            event_date: Some("2020-12-03".to_string()),
            event_date_text: "2020-12-03".to_string(),
            event_date_stop: None,
            date_is_range: false,
            year: Some(2020),
            month: Some(12),
            day: Some(3),
            anniversary_month: Some(12),
            anniversary_day: Some(3),
            leap_day_folded: false,
            place: Some("Ur, Mesopotamia".to_string()),
            role: "Primary".to_string(),
            age_at_event: Some("82y 0m".to_string()),
            reference_year: 2026,
            elapsed_years: Some(6),
            private: false,
        }
    }

    #[test]
    fn writes_field_names_as_header_then_one_record_per_row() {
        let td = TempDir::new().unwrap();
        let dest = td.path().join("events.csv");
        let rows = vec![row(Some("I0004")), row(None)];
        CsvWriter.write(&rows, &dest).unwrap();

        let mut reader = csv::Reader::from_path(&dest).unwrap();
        let records = reader.records().collect::<Result<Vec<_>, _>>().unwrap();
        assert_eq!(reader.headers().unwrap().iter().collect::<Vec<_>>(), HEADER);
        assert_eq!(records.len(), 2, "one record per row after the header");
    }

    #[test]
    fn round_trip_preserves_values_and_nulls() {
        let td = TempDir::new().unwrap();
        let dest = td.path().join("events.csv");
        let mut rows = vec![row(Some("I0004")), row(None)];

        // Rows such as the real views produce: fields with commas, quotes
        // and spaces must survive the RFC 4180 quoting round-trip.
        rows[1].person_name = "Einstein, Albert \"the elder\"".to_string();
        rows[1].event_date = None;
        rows[1].year = None;
        rows[1].elapsed_years = None;
        rows[1].place = None;
        rows[1].anniversary_month = None;
        rows[1].anniversary_day = None;

        CsvWriter.write(&rows, &dest).unwrap();
        let mut reader = csv::Reader::from_path(&dest).unwrap();
        let parsed: Vec<EventRow> = reader.deserialize().collect::<Result<_, _>>().unwrap();
        assert_eq!(parsed, rows);
    }

    #[test]
    fn couple_row_round_trips_person_id_2_value_or_empty() {
        // A collapsed couple row serializes its second id as the
        // `person_id_2` field; a single-person row serializes an empty
        // field (the CSV convention for None).
        let td = TempDir::new().unwrap();
        let dest = td.path().join("events.csv");
        let mut rows = vec![row(Some("I0000"))];
        rows[0].person_name = "Adam Uplands ⚭ Eve Uplands".to_string();
        rows[0].person_id_2 = Some("I0001".to_string());
        rows.push(row(Some("I0004")));
        CsvWriter.write(&rows, &dest).unwrap();

        let mut reader = csv::Reader::from_path(&dest).unwrap();
        let parsed: Vec<EventRow> = reader.deserialize().collect::<Result<_, _>>().unwrap();
        assert_eq!(parsed, rows);
        assert_eq!(parsed[0].person_id_2, Some("I0001".to_string()));
        assert_eq!(parsed[1].person_id_2, None);
    }

    #[test]
    fn some_empty_string_collides_with_none() {
        // CSV has no way to distinguish an empty field from None: writing
        // Some("") yields the same empty field as None writes. Documented
        // here because the writers never produce Some("") for Option
        // fields (the views emit None or a non-empty value), so the
        // collision cannot occur in real rows.
        let td = TempDir::new().unwrap();
        let dest = td.path().join("events.csv");
        let mut rows = vec![row(None)];
        rows[0].place = Some(String::new());
        CsvWriter.write(&rows, &dest).unwrap();

        let mut reader = csv::Reader::from_path(&dest).unwrap();
        let parsed: Vec<EventRow> = reader.deserialize().collect::<Result<_, _>>().unwrap();
        assert_eq!(parsed[0].place, None);
    }

    #[test]
    fn empty_rows_write_an_empty_file() {
        // csv's writer emits the header lazily with the first record, so a
        // run whose filters leave no events produces a valid, completely
        // empty CSV (no records to read back) rather than a header-only
        // file. Consumers parse it as zero rows.
        let td = TempDir::new().unwrap();
        let dest = td.path().join("events.csv");
        CsvWriter.write(&[], &dest).unwrap();
        assert_eq!(fs::read_to_string(&dest).unwrap(), "");
        let mut reader = csv::Reader::from_path(&dest).unwrap();
        assert_eq!(reader.records().count(), 0);
    }
}
