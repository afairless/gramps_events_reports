//! Property tests (plan §11): the writers' files read back exactly what
//! was written — csv and json through serde parse-back, parquet through
//! `ParquetRecordBatchReaderBuilder` — over arbitrary `EventRow`s, for
//! every `Option` position.
//!
//! The csv and parquet strategies never generate `Some("")` for `Option`
//! fields: CSV cannot distinguish an empty field from `None` (its inherent
//! limitation, asserted in a unit test), while parquet can and a dedicated
//! unit test covers the distinction. Everything generated here mirrors the
//! rows the views actually produce.

use event_core::EventRow;
use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;
use proptest::prelude::*;
use std::fs::{self, File};
use tempfile::TempDir;
use writers::{CsvWriter, EventWriter, JsonWriter, ParquetWriter};

// An arbitrary row the four writers must round-trip losslessly. String
// `Option`s are never empty (see the module docs); every other field
// takes its full value domain so the schema (null vs present, integer
// and boolean ranges) is exercised end to end.
prop_compose! {
    fn event_row()(
        person_id in prop::option::of("[A-Za-z0-9]{1,8}"),
        person_id_2 in prop::option::of("[A-Za-z0-9]{1,8}"),
        person_name in "[A-Za-z0-9 ]{0,24}",
        event_id in prop::option::of("[A-Za-z0-9]{1,8}"),
        event_type in "[A-Za-z]{1,16}",
        event_date in prop::option::of("[0-9]{4}-[0-9]{2}-[0-9]{2}"),
        event_date_text in "[A-Za-z0-9 ,:\\-]{0,32}",
        event_date_stop in prop::option::of("[0-9]{4}-[0-9]{2}-[0-9]{2}"),
        date_is_range in any::<bool>(),
        year in prop::option::of(any::<i32>()),
        month in prop::option::of(0u32..=12),
        day in prop::option::of(0u32..=31),
        anniversary_month in prop::option::of(0u32..=12),
        anniversary_day in prop::option::of(0u32..=31),
        leap_day_folded in any::<bool>(),
        place in prop::option::of("[A-Za-z0-9 /]{1,24}"),
        role in "[A-Za-z]{0,16}",
        age_at_event in prop::option::of("[0-9]{1,3}y [0-9]{1,2}m"),
        reference_year in any::<i32>(),
        elapsed_years in prop::option::of(any::<i32>()),
        private in any::<bool>(),
    ) -> EventRow {
        EventRow {
            person_id,
            person_id_2,
            person_name,
            event_id,
            event_type,
            event_date,
            event_date_text,
            event_date_stop,
            date_is_range,
            year,
            month,
            day,
            anniversary_month,
            anniversary_day,
            leap_day_folded,
            place,
            role,
            age_at_event,
            reference_year,
            elapsed_years,
            private,
        }
    }
}

fn row_lists() -> impl Strategy<Value = Vec<EventRow>> {
    prop::collection::vec(event_row(), 0..16)
}

proptest! {
    /// CSV: typed parse-back equals the input rows, field by field.
    #[test]
    fn csv_round_trip(rows in row_lists()) {
        let td = TempDir::new().unwrap();
        let dest = td.path().join("events.csv");
        CsvWriter.write(&rows, &dest).unwrap();
        let mut reader = csv::Reader::from_path(&dest).unwrap();
        let parsed: Vec<EventRow> = reader.deserialize().collect::<Result<_, _>>().unwrap();
        prop_assert_eq!(parsed, rows);
    }

    /// JSON: lossless round-trip, including every null position.
    #[test]
    fn json_round_trip(rows in row_lists()) {
        let td = TempDir::new().unwrap();
        let dest = td.path().join("events.json");
        JsonWriter.write(&rows, &dest).unwrap();
        let content = fs::read_to_string(&dest).unwrap();
        let parsed: Vec<EventRow> = serde_json::from_str(&content).unwrap();
        prop_assert_eq!(parsed, rows);
    }

    /// Parquet: read-back via ParquetRecordBatchReaderBuilder equals the
    /// written rows, with nulls and values preserved exactly.
    #[test]
    fn parquet_round_trip(rows in row_lists()) {
        let td = TempDir::new().unwrap();
        let dest = td.path().join("events.parquet");
        ParquetWriter.write(&rows, &dest).unwrap();

        let file = File::open(&dest).unwrap();
        let reader = ParquetRecordBatchReaderBuilder::try_new(file)
            .unwrap()
            .build()
            .unwrap();
        let batches: Vec<arrow::record_batch::RecordBatch> =
            reader.collect::<Result<_, _>>().unwrap();
        let parsed = batches_to_rows(&batches);
        prop_assert_eq!(parsed, rows);
    }
}

/// Convert read-back arrow batches back into `EventRow`s, column by
/// column, so the round-trip comparison runs on the contract values
/// (including the null-vs-present distinction) rather than arrow layouts.
fn batches_to_rows(batches: &[arrow::record_batch::RecordBatch]) -> Vec<EventRow> {
    let mut rows = Vec::new();
    for batch in batches {
        let opt_str = |i: usize| {
            batch
                .column(i)
                .as_any()
                .downcast_ref::<arrow::array::StringArray>()
                .expect("utf8 column")
                .iter()
                .map(|v| v.map(str::to_string))
                .collect::<Vec<Option<String>>>()
        };
        let plain_str = |i: usize| {
            batch
                .column(i)
                .as_any()
                .downcast_ref::<arrow::array::StringArray>()
                .expect("utf8 column")
                .iter()
                .map(|v| v.unwrap_or_default().to_string())
                .collect::<Vec<String>>()
        };
        let opt_i32 = |i: usize| {
            batch
                .column(i)
                .as_any()
                .downcast_ref::<arrow::array::Int32Array>()
                .expect("int32 column")
                .iter()
                .collect::<Vec<Option<i32>>>()
        };
        let opt_u32 = |i: usize| {
            batch
                .column(i)
                .as_any()
                .downcast_ref::<arrow::array::UInt32Array>()
                .expect("uint32 column")
                .iter()
                .collect::<Vec<Option<u32>>>()
        };
        let plain_bool = |i: usize| {
            batch
                .column(i)
                .as_any()
                .downcast_ref::<arrow::array::BooleanArray>()
                .expect("boolean column")
                .iter()
                .map(|v| v.unwrap_or_default())
                .collect::<Vec<bool>>()
        };
        let plain_i32 = |i: usize| {
            batch
                .column(i)
                .as_any()
                .downcast_ref::<arrow::array::Int32Array>()
                .expect("int32 column")
                .iter()
                .map(|v| v.unwrap_or_default())
                .collect::<Vec<i32>>()
        };

        let person_id = opt_str(0);
        let person_id_2 = opt_str(1);
        let person_name = plain_str(2);
        let event_id = opt_str(3);
        let event_type = plain_str(4);
        let event_date = opt_str(5);
        let event_date_text = plain_str(6);
        let event_date_stop = opt_str(7);
        let date_is_range = plain_bool(8);
        let year = opt_i32(9);
        let month = opt_u32(10);
        let day = opt_u32(11);
        let anniversary_month = opt_u32(12);
        let anniversary_day = opt_u32(13);
        let leap_day_folded = plain_bool(14);
        let place = opt_str(15);
        let role = plain_str(16);
        let age_at_event = opt_str(17);
        let reference_year = plain_i32(18);
        let elapsed_years = opt_i32(19);
        let private = plain_bool(20);

        for i in 0..batch.num_rows() {
            rows.push(EventRow {
                person_id: person_id[i].clone(),
                person_id_2: person_id_2[i].clone(),
                person_name: person_name[i].clone(),
                event_id: event_id[i].clone(),
                event_type: event_type[i].clone(),
                event_date: event_date[i].clone(),
                event_date_text: event_date_text[i].clone(),
                event_date_stop: event_date_stop[i].clone(),
                date_is_range: date_is_range[i],
                year: year[i],
                month: month[i],
                day: day[i],
                anniversary_month: anniversary_month[i],
                anniversary_day: anniversary_day[i],
                leap_day_folded: leap_day_folded[i],
                place: place[i].clone(),
                role: role[i].clone(),
                age_at_event: age_at_event[i].clone(),
                reference_year: reference_year[i],
                elapsed_years: elapsed_years[i],
                private: private[i],
            });
        }
    }
    rows
}
