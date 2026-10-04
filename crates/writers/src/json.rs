//! JSON row writer (plan §6.3): one compact array of [`EventRow`] objects,
//! `Option`s serialized as `null`.

use std::io::Write as _;
use std::path::Path;

use event_core::EventRow;

use crate::EventWriter;
use crate::atomic::write_atomically;
use crate::error::WriterError;

/// Writes [`EventRow`]s as a JSON array of objects (serde field names,
/// `null` for absent `Option`s). Compact output; a pretty/compact toggle
/// can be added when a consumer needs it.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct JsonWriter;

impl EventWriter for JsonWriter {
    fn write(&self, rows: &[EventRow], dest: &Path) -> Result<(), WriterError> {
        write_atomically(dest, |mut file| {
            serde_json::to_writer(&mut file, rows)?;
            file.flush().map_err(|e| WriterError::io(dest, e))?;
            Ok(())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::JsonWriter;
    use crate::EventWriter;
    use event_core::EventRow;
    use std::fs;
    use tempfile::TempDir;

    fn row() -> EventRow {
        EventRow {
            person_id: Some("I0004".to_string()),
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
            place: Some("Ur".to_string()),
            role: "Primary".to_string(),
            age_at_event: Some("82y 0m".to_string()),
            reference_year: 2026,
            elapsed_years: Some(6),
            private: false,
        }
    }

    #[test]
    fn array_of_objects_round_trips_exactly() {
        let td = TempDir::new().unwrap();
        let dest = td.path().join("events.json");
        let mut rows = vec![row(), row()];
        // Exercise every nullability: None stays null, and — unlike CSV —
        // Some("") survives the round-trip (JSON is lossless here).
        rows[1].person_id = None;
        rows[1].event_date = None;
        rows[1].year = None;
        rows[1].month = None;
        rows[1].day = None;
        rows[1].place = Some(String::new());
        rows[1].elapsed_years = None;

        JsonWriter.write(&rows, &dest).unwrap();
        let parsed: Vec<EventRow> =
            serde_json::from_str(&fs::read_to_string(&dest).unwrap()).unwrap();
        assert_eq!(parsed, rows);
    }

    #[test]
    fn output_starts_with_an_array_bracket() {
        let td = TempDir::new().unwrap();
        let dest = td.path().join("events.json");
        JsonWriter.write(&[row()], &dest).unwrap();
        let content = fs::read_to_string(&dest).unwrap();
        assert!(content.starts_with('['), "JSON output must be an array");
    }

    #[test]
    fn empty_rows_write_an_empty_array() {
        let td = TempDir::new().unwrap();
        let dest = td.path().join("events.json");
        JsonWriter.write(&[], &dest).unwrap();
        assert_eq!(fs::read_to_string(&dest).unwrap(), "[]");
    }
}
