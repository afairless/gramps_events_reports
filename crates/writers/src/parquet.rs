//! Parquet row writer (plan §6.3): a fixed flat arrow-rs 60 schema
//! mirroring the [`EventRow`] contract, written via `StructArray →
//! RecordBatch → ArrowWriter`.

use std::path::Path;
use std::sync::Arc;

use arrow::array::{ArrayRef, BooleanArray, Int32Array, StringArray, UInt32Array};
use arrow::datatypes::{DataType, Field, Schema, SchemaRef};
use arrow::record_batch::RecordBatch;
use event_core::EventRow;
use parquet::arrow::arrow_writer::ArrowWriter;

use crate::EventWriter;
use crate::atomic::write_atomically;
use crate::error::WriterError;

/// The fixed flat Parquet schema, in `EventRow` field order (the same
/// names the JSON keys and CSV header use, locked by the milestone-8
/// golden test). Field *order* and nullability are part of the Parquet
/// contract: readers consuming these files depend on the column layout, so
/// it must not drift from `EventRow`'s Serialize shape.
fn schema() -> SchemaRef {
    Arc::new(Schema::new(vec![
        Field::new("person_id", DataType::Utf8, true),
        Field::new("person_id_2", DataType::Utf8, true),
        Field::new("person_name", DataType::Utf8, false),
        Field::new("event_id", DataType::Utf8, true),
        Field::new("event_type", DataType::Utf8, false),
        Field::new("event_date", DataType::Utf8, true),
        Field::new("event_date_text", DataType::Utf8, false),
        Field::new("event_date_stop", DataType::Utf8, true),
        Field::new("date_is_range", DataType::Boolean, false),
        Field::new("year", DataType::Int32, true),
        Field::new("month", DataType::UInt32, true),
        Field::new("day", DataType::UInt32, true),
        Field::new("anniversary_month", DataType::UInt32, true),
        Field::new("anniversary_day", DataType::UInt32, true),
        Field::new("leap_day_folded", DataType::Boolean, false),
        Field::new("place", DataType::Utf8, true),
        Field::new("role", DataType::Utf8, false),
        Field::new("age_at_event", DataType::Utf8, true),
        Field::new("reference_year", DataType::Int32, false),
        Field::new("elapsed_years", DataType::Int32, true),
        Field::new("private", DataType::Boolean, false),
    ]))
}

/// Build a single flat `RecordBatch` carrying every row as one column per
/// `EventRow` field, `None` values as arrow nulls.
fn record_batch(rows: &[EventRow]) -> Result<RecordBatch, WriterError> {
    let columns: Vec<ArrayRef> = vec![
        Arc::new(StringArray::from(
            rows.iter()
                .map(|r| r.person_id.as_deref())
                .collect::<Vec<Option<&str>>>(),
        )),
        Arc::new(StringArray::from(
            rows.iter()
                .map(|r| r.person_id_2.as_deref())
                .collect::<Vec<Option<&str>>>(),
        )),
        Arc::new(StringArray::from(
            rows.iter()
                .map(|r| r.person_name.as_str())
                .collect::<Vec<&str>>(),
        )),
        Arc::new(StringArray::from(
            rows.iter()
                .map(|r| r.event_id.as_deref())
                .collect::<Vec<Option<&str>>>(),
        )),
        Arc::new(StringArray::from(
            rows.iter()
                .map(|r| r.event_type.as_str())
                .collect::<Vec<&str>>(),
        )),
        Arc::new(StringArray::from(
            rows.iter()
                .map(|r| r.event_date.as_deref())
                .collect::<Vec<Option<&str>>>(),
        )),
        Arc::new(StringArray::from(
            rows.iter()
                .map(|r| r.event_date_text.as_str())
                .collect::<Vec<&str>>(),
        )),
        Arc::new(StringArray::from(
            rows.iter()
                .map(|r| r.event_date_stop.as_deref())
                .collect::<Vec<Option<&str>>>(),
        )),
        Arc::new(BooleanArray::from(
            rows.iter().map(|r| r.date_is_range).collect::<Vec<bool>>(),
        )),
        Arc::new(Int32Array::from(
            rows.iter().map(|r| r.year).collect::<Vec<Option<i32>>>(),
        )),
        Arc::new(UInt32Array::from(
            rows.iter().map(|r| r.month).collect::<Vec<Option<u32>>>(),
        )),
        Arc::new(UInt32Array::from(
            rows.iter().map(|r| r.day).collect::<Vec<Option<u32>>>(),
        )),
        Arc::new(UInt32Array::from(
            rows.iter()
                .map(|r| r.anniversary_month)
                .collect::<Vec<Option<u32>>>(),
        )),
        Arc::new(UInt32Array::from(
            rows.iter()
                .map(|r| r.anniversary_day)
                .collect::<Vec<Option<u32>>>(),
        )),
        Arc::new(BooleanArray::from(
            rows.iter()
                .map(|r| r.leap_day_folded)
                .collect::<Vec<bool>>(),
        )),
        Arc::new(StringArray::from(
            rows.iter()
                .map(|r| r.place.as_deref())
                .collect::<Vec<Option<&str>>>(),
        )),
        Arc::new(StringArray::from(
            rows.iter().map(|r| r.role.as_str()).collect::<Vec<&str>>(),
        )),
        Arc::new(StringArray::from(
            rows.iter()
                .map(|r| r.age_at_event.as_deref())
                .collect::<Vec<Option<&str>>>(),
        )),
        Arc::new(Int32Array::from(
            rows.iter().map(|r| r.reference_year).collect::<Vec<i32>>(),
        )),
        Arc::new(Int32Array::from(
            rows.iter()
                .map(|r| r.elapsed_years)
                .collect::<Vec<Option<i32>>>(),
        )),
        Arc::new(BooleanArray::from(
            rows.iter().map(|r| r.private).collect::<Vec<bool>>(),
        )),
    ];
    RecordBatch::try_new(schema(), columns).map_err(WriterError::from)
}

/// Writes [`EventRow`]s as one Parquet file (arrow-rs 60, single row
/// group). `ArrowWriter::close` finalizes (and flushes) before the temp
/// file is renamed over the destination. An empty row set writes a valid
/// schema-only file with no row groups (readers see zero batches).
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct ParquetWriter;

impl EventWriter for ParquetWriter {
    fn write(&self, rows: &[EventRow], dest: &Path) -> Result<(), WriterError> {
        let batch = record_batch(rows)?;
        write_atomically(dest, |file| {
            let mut writer = ArrowWriter::try_new(file, schema(), None)?;
            writer.write(&batch)?;
            writer.close()?;
            Ok(())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{ParquetWriter, record_batch};
    use crate::EventWriter;
    use arrow::array::{Array, StringArray};
    use event_core::EventRow;
    use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;
    use std::fs::{self, File};
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

    fn read_batches(path: &std::path::Path) -> Vec<arrow::record_batch::RecordBatch> {
        let file = File::open(path).unwrap();
        ParquetRecordBatchReaderBuilder::try_new(file)
            .unwrap()
            .build()
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap()
    }

    #[test]
    fn writes_par1_magic_and_reads_back_identical_batches() {
        let td = TempDir::new().unwrap();
        let dest = td.path().join("events.parquet");
        let rows = vec![row(), row()];
        ParquetWriter.write(&rows, &dest).unwrap();

        // Parquet magic header (plan §11.1: correct magic headers).
        let head = &fs::read(&dest).unwrap()[..4];
        assert_eq!(head, b"PAR1", "parquet footer/header magic");

        let batches = read_batches(&dest);
        assert_eq!(batches, vec![record_batch(&rows).unwrap()]);
    }

    #[test]
    fn nulls_stay_nulls_and_empty_strings_stay_empty() {
        let td = TempDir::new().unwrap();
        let dest = td.path().join("events.parquet");
        let mut rows = vec![row()];
        rows[0].event_date = None;
        rows[0].place = Some(String::new());
        rows[0].elapsed_years = None;
        ParquetWriter.write(&rows, &dest).unwrap();

        let batches = read_batches(&dest);
        assert_eq!(batches, vec![record_batch(&rows).unwrap()]);

        // Spot-check the distinction the format must preserve: a null
        // Utf8 cell versus an empty but present string.
        let batch = &batches[0];
        let event_date = batch
            .column(5)
            .as_any()
            .downcast_ref::<StringArray>()
            .unwrap();
        assert!(event_date.is_null(0), "None must read back as null");
        let place = batch
            .column(15)
            .as_any()
            .downcast_ref::<StringArray>()
            .unwrap();
        assert_eq!(
            place.value(0),
            "",
            "Some(\"\") must read back as present-empty"
        );
        assert!(!place.is_null(0));
    }

    #[test]
    fn person_id_2_round_trips_preserved() {
        // The couple-column (D-e) is part of the Parquet contract: a
        // collapsed couple row's second id round-trips as the
        // `person_id_2` cell, and a single-person row's absence stays null.
        let td = TempDir::new().unwrap();
        let dest = td.path().join("events.parquet");
        let mut rows = vec![row()];
        rows[0].person_name = "Adam Uplands ⚭ Eve Uplands".to_string();
        rows[0].person_id_2 = Some("I0001".to_string());
        rows.push(row());
        ParquetWriter.write(&rows, &dest).unwrap();

        let batches = read_batches(&dest);
        let batch = &batches[0];
        let ids = batch
            .column(1)
            .as_any()
            .downcast_ref::<StringArray>()
            .unwrap();
        assert_eq!(ids.value(0), "I0001");
        assert!(ids.is_null(1), "absent second id must read back as null");
        assert_eq!(batches, vec![record_batch(&rows).unwrap()]);
    }

    #[test]
    fn column_schema_mirrors_the_row_contract() {
        let td = TempDir::new().unwrap();
        let dest = td.path().join("events.parquet");
        ParquetWriter.write(&[row()], &dest).unwrap();

        let schema = record_batch(&[row()]).unwrap().schema();
        let names: Vec<&str> = schema.fields().iter().map(|f| f.name().as_str()).collect();
        assert_eq!(
            names,
            [
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
            ]
        );
    }

    #[test]
    fn empty_rows_produce_a_valid_empty_file() {
        // ArrowWriter writes no row group for a zero-row batch, so readers
        // observe zero batches but the full contract schema: an empty
        // event list still yields a valid, schema-described parquet file.
        let td = TempDir::new().unwrap();
        let dest = td.path().join("events.parquet");
        ParquetWriter.write(&[], &dest).unwrap();
        assert_eq!(&fs::read(&dest).unwrap()[..4], b"PAR1");
        let file = File::open(&dest).unwrap();
        let builder = ParquetRecordBatchReaderBuilder::try_new(file).unwrap();
        assert_eq!(
            builder.schema().fields().len(),
            21,
            "schema present, no rows"
        );
        let batches: Vec<_> = builder.build().unwrap().collect::<Result<_, _>>().unwrap();
        assert!(batches.is_empty(), "no row groups for zero rows");
    }
}
