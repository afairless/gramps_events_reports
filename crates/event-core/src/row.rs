//! The flat writer contract — [`EventRow`].
//!
//! Every view builder produces [`EventRow`]s (plan §7.3): one row per
//! (event, subject) — an event referenced by two people yields one row per
//! person (plan §8.6) — while family/couple events collapse to **one** row
//! per event carrying both spouses' names and ids (plan §3.4, D-d). Each
//! row carries the person's id and display name, the event type and Gramps
//! display-date string, the normalized Gregorian
//! dates (`event_date` start / `event_date_stop` range end), the
//! anniversary anchor *after* the Feb 29 fold (decision D6), the place
//! chain, the subject's role, the age at the event and the elapsed years
//! against the run's reference year.
//!
//! The CSV / JSON / Parquet writers (milestones 9–10) serialize this struct
//! directly, so it IS the flat-file schema: adding or reordering fields
//! later is a breaking schema change for Parquet readers — any evolution
//! must bump a `schema_version` field or ship a new file format (plan
//! §7.3). The serialized shape (field names and types) is locked by a
//! golden test; `Deserialize` exists so the csv/json writers' parse-back
//! round-trip tests (plan §11) can read rows back exactly as written.

use serde::{Deserialize, Serialize};

/// One flat output row (plan §7.3): an event as seen by one of its
/// subjects — or, for a family/couple event, one row per event grouping
/// both spouses under `person_name` with `person_id` / `person_id_2`
/// (decision D-d/D-e).
///
/// `event_date` / `event_date_stop` are ISO strings of the *normalized*
/// Gregorian start date (and range/span stop) — `None` where no
/// normalization exists: partial dates, unknown years, the five
/// non-converted calendars (D4), undated and text-only events.
/// `event_date_text` always carries the Gramps display string (`"1900-00-00"`,
/// `"bef 1914-06-28"`, `"1822-11-00 - 1823-04-00"`, the verbatim text for a
/// `datestr`; empty for events with no date element). `date_is_range`
/// records a `daterange`/`datespan` of any quality (§8 rule 2) even when
/// its endpoints are not fully convertible.
///
/// `anniversary_month`/`anniversary_day` are the *effective* anchor — the
/// cell of the anniversary calendar the row lands in — after the Feb 29
/// fold (D6): a Feb 29 anchor in a non-leap reference year lands on
/// Feb 28 with `leap_day_folded = true`, while `event_date` keeps the true
/// Gregorian date. Rows without an anchor (year-only dates, non-convertible
/// calendars, undated events) carry `None` in both.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EventRow {
    /// The subject's Gramps id (`id="I0000"`), when the exporter set one;
    /// the first spouse's id on a collapsed couple row.
    pub person_id: Option<String>,
    /// The second spouse's Gramps id on a collapsed couple row (decision
    /// D-e); `None` for single-person events, single-spouse families and
    /// any row with fewer than two subjects.
    pub person_id_2: Option<String>,
    /// The subject's display name (`First Surname`), `"—"` for orphan
    /// events, `""` for a nameless person; both spouses' names joined
    /// with `" ⚭ "` on a collapsed couple row (decision D-c).
    pub person_name: String,
    /// The event's Gramps id (`id="E0000"`), when the exporter set one.
    pub event_id: Option<String>,
    /// The event `<type>` verbatim, e.g. `"Birth"` or any custom type.
    pub event_type: String,
    /// ISO of the normalized (Gregorian) start date, `None` when not
    /// normalizable.
    pub event_date: Option<String>,
    /// The Gramps display string (`` for events without a date element).
    pub event_date_text: String,
    /// ISO of the range/span stop date, `None` for single dates and for
    /// stops that do not normalize.
    pub event_date_stop: Option<String>,
    /// True for `daterange` / `datespan` dates of any quality (plan §8
    /// rule 2).
    pub date_is_range: bool,
    /// The start year (normalized Gregorian when convertible, else the
    /// stored calendar's own year); `None` when the year is unknown.
    pub year: Option<i32>,
    /// The start month (as `year`); `None` when the month is unknown.
    pub month: Option<u32>,
    /// The start day (as `year`); `None` when the day is unknown.
    pub day: Option<u32>,
    /// The effective anniversary month after the Feb 29 fold (D6); `None`
    /// when the date carries no anchor (year-only, non-converted
    /// calendars, undated).
    pub anniversary_month: Option<u32>,
    /// The effective anniversary day after the Feb 29 fold (D6); `None`
    /// together with `anniversary_month`.
    pub anniversary_day: Option<u32>,
    /// True when the Feb 29 fold applied for this row in this run (D6).
    pub leap_day_folded: bool,
    /// The event's place chain joined with `" / "` (event place first,
    /// ancestors after), `None` when the event has no resolvable place.
    pub place: Option<String>,
    /// The subject's role on the event: `"Primary"`, an eventref role
    /// (`"Witness"`, `"Family"`, ...), or `""` for the orphan placeholder.
    pub role: String,
    /// The subject's age (years/months) when the event happened, formatted
    /// `"{years}y {months}m"` (e.g. `"82y 0m"`); best-effort years-only
    /// ages also format months as `0m`. `None` when unknown.
    pub age_at_event: Option<String>,
    /// The run's report year — constant on every row of one run, so a
    /// single output file stays self-describing (plan §7.3).
    pub reference_year: i32,
    /// `reference_year − event start year` (plan §8.4); negative values
    /// mean the event postdates the reference year and render `"—"`.
    pub elapsed_years: Option<i32>,
    /// True when the event record carries `priv="1"` (plan §8.7) — shown
    /// only when the run opts in, but flagged here whenever present.
    pub private: bool,
}

/// Format an `(years, months)` age for the `age_at_event` column:
/// `"{years}y {months}m"` — deterministic, parseable by readers.
pub(crate) fn format_age(age: (i32, u32)) -> String {
    format!("{}y {}m", age.0, age.1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// The bare minimum row a view can produce — surfaces the serialized
    /// shape (the CSV/JSON/Parquet schema) and exercises every field's
    /// serialized JSON type.
    fn sample_row() -> EventRow {
        EventRow {
            person_id: Some("I0004".to_string()),
            person_id_2: Some("I0009".to_string()),
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
    fn serialized_shape_is_the_flat_contract() {
        let row = sample_row();
        let value: serde_json::Value = serde_json::to_value(&row).unwrap();
        // Every contract field serializes under its documented name.
        let expected = json!({
            "person_id": "I0004",
            "person_id_2": "I0009",
            "person_name": "Abraham Meowser",
            "event_id": "E0005",
            "event_type": "Death",
            "event_date": "2020-12-03",
            "event_date_text": "2020-12-03",
            "event_date_stop": null,
            "date_is_range": false,
            "year": 2020,
            "month": 12,
            "day": 3,
            "anniversary_month": 12,
            "anniversary_day": 3,
            "leap_day_folded": false,
            "place": "Ur",
            "role": "Primary",
            "age_at_event": "82y 0m",
            "reference_year": 2026,
            "elapsed_years": 6,
            "private": false,
        });
        assert_eq!(value, expected, "EventRow JSON schema must stay stable");
    }

    #[test]
    fn nullability_serializes_as_json_null() {
        let mut row = sample_row();
        row.person_id = None;
        row.person_id_2 = None;
        row.event_id = None;
        row.event_date = None;
        row.event_date_stop = None;
        row.year = None;
        row.month = None;
        row.day = None;
        row.anniversary_month = None;
        row.anniversary_day = None;
        row.place = None;
        row.age_at_event = None;
        row.elapsed_years = None;
        let value: serde_json::Value = serde_json::to_value(&row).unwrap();
        for key in [
            "person_id",
            "person_id_2",
            "event_id",
            "event_date",
            "event_date_stop",
            "year",
            "month",
            "day",
            "anniversary_month",
            "anniversary_day",
            "place",
            "age_at_event",
            "elapsed_years",
        ] {
            assert_eq!(value[key], serde_json::Value::Null, "{key} is null");
        }
        // The non-nullable members keep their literal types.
        assert_eq!(value["person_name"], "Abraham Meowser");
        assert_eq!(value["date_is_range"], false);
        assert_eq!(value["leap_day_folded"], false);
        assert_eq!(value["reference_year"], 2026);
    }

    #[test]
    fn age_format_is_years_and_months() {
        assert_eq!(format_age((82, 0)), "82y 0m");
        assert_eq!(format_age((0, 0)), "0y 0m");
        assert_eq!(format_age((98, 7)), "98y 7m");
    }
}
