//! Display-string formatting, mirroring Gramps 5.1.6 `Date.__str__`.
//!
//! Gramps renders a date as `[qual ][pref ]value[ (calendar-suffix)]`, where
//! `qual` is `est`/`calc`, `pref` is `bef`/`aft`/`abt`, `value` is the
//! ISO-ish date (`YYYY-MM-DD`, month/day `0` for partials, negative years
//! for BC), a dual-dated slash form, or the full `start - stop` for ranges
//! and spans. Non-Gregorian calendars (and any non-default new-year start)
//! append a parenthesised suffix, e.g. `1900-01-01 (Julian,Mar25)`.
//!
//! The values below are golden-captured from Gramps 5.1.6 (`Date.__str__`):
//!
//! ```text
//! year-only     1900-00-00          about    abt 1900-00-00
//! year-month    1822-11-00          before   bef 1914-01-01
//! full          1900-01-01          after    aft 1945-01-01
//! estimated     est 1900-01-01      BC       -550-04-22
//! calculated    calc 1900-01-01     Julian   1900-01-01 (Julian)
//! range         1914-01-01 - 1918-04-01
//! span          1822-11-01 - 1823-04-01
//! dual-dated    1582/3-10-04 (Julian)
//! ```
//!
//! **Deviation:** for *dual-dated ranges/spans* Gramps' `__str__` prints only
//! the start date in slash form (a known quirk; `get_slash()` wins over the
//! compound branch). We instead render both endpoints in slash form so the
//! full range stays visible, honoring the range-display rule of the project
//! plan (§8 rule 2).

use crate::model::{Calendar, GrampsDate, Modifier, NewYear, Quality};

impl GrampsDate {
    /// Recompute `self.display` from the structured fields, exactly as
    /// Gramps `Date.__str__` formats it.
    ///
    /// For [`Modifier::TextOnly`] dates the display is the verbatim text and
    /// is returned untouched (parsers set it first).
    pub fn compute_display(&self) -> String {
        let mut out = String::new();
        match self.quality {
            Quality::Estimated => out.push_str("est "),
            Quality::Calculated => out.push_str("calc "),
            Quality::None => {}
        }
        match self.modifier {
            Modifier::Before => out.push_str("bef "),
            Modifier::After => out.push_str("aft "),
            Modifier::About => out.push_str("abt "),
            Modifier::None | Modifier::Range | Modifier::Span | Modifier::TextOnly => {}
        }
        match self.modifier {
            Modifier::TextOnly => return self.display.clone(),
            Modifier::Range | Modifier::Span => {
                if self.dual_dated {
                    out.push_str(&slash_ymd(self.ymd));
                    out.push_str(" - ");
                    if let Some(stop) = self.stop {
                        out.push_str(&slash_ymd(stop));
                    }
                } else {
                    out.push_str(&iso_ymd(self.ymd));
                    out.push_str(" - ");
                    out.push_str(&iso_ymd(self.stop.unwrap_or(self.ymd)));
                }
            }
            Modifier::None | Modifier::Before | Modifier::After | Modifier::About => {
                if self.dual_dated {
                    out.push_str(&slash_ymd(self.ymd));
                } else {
                    out.push_str(&iso_ymd(self.ymd));
                }
            }
        }
        // Calendar / new-year suffix. Dual-dated dates are Julian (Gramps
        // coerces the calendar in `Date.set`); the suffix mirrors that.
        let calendar = if self.dual_dated {
            Calendar::Julian
        } else {
            self.calendar
        };
        let newyear = newyear_str(self.new_year);
        match calendar {
            Calendar::Gregorian if newyear.is_empty() => {}
            Calendar::Gregorian => {
                out.push(' ');
                out.push('(');
                out.push_str(newyear);
                out.push(')');
            }
            other => {
                out.push(' ');
                out.push('(');
                out.push_str(other.as_str());
                if !newyear.is_empty() {
                    out.push(',');
                    out.push_str(newyear);
                }
                out.push(')');
            }
        }
        out
    }
}

/// `YYYY-MM-DD` with `%04d`-style zero padding; BC years keep their sign.
fn iso_ymd(ymd: (i32, u32, u32)) -> String {
    format!("{:04}-{:02}-{:02}", ymd.0, ymd.1, ymd.2)
}

/// Dual-dated slash form `%04d/%d-%02d-%02d`: `(year-1)/(year%10)-MM-DD`.
fn slash_ymd(ymd: (i32, u32, u32)) -> String {
    format!(
        "{:04}/{}-{:02}-{:02}",
        ymd.0 - 1,
        ymd.0.rem_euclid(10),
        ymd.1,
        ymd.2
    )
}

fn newyear_str(ny: NewYear) -> &'static str {
    match ny {
        NewYear::Jan1 => "",
        NewYear::Mar1 => "Mar1",
        NewYear::Mar25 => "Mar25",
        NewYear::Sep1 => "Sep1",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{GrampsDate, Modifier, NewYear, Quality};

    fn date(
        ymd: (i32, u32, u32),
        modifier: Modifier,
        quality: Quality,
        calendar: Calendar,
        new_year: NewYear,
        dual_dated: bool,
        stop: Option<(i32, u32, u32)>,
    ) -> GrampsDate {
        GrampsDate {
            calendar,
            modifier,
            quality,
            ymd,
            stop,
            dual_dated,
            new_year,
            display: String::new(),
        }
    }

    fn disp(d: &GrampsDate) -> String {
        d.compute_display()
    }

    /// Golden strings captured from Gramps 5.1.6 `Date.__str__`.
    #[test]
    fn golden_display_strings_match_gramps() {
        let cases: &[(&str, GrampsDate)] = &[
            (
                "1900-00-00",
                date(
                    (1900, 0, 0),
                    Modifier::None,
                    Quality::None,
                    Calendar::Gregorian,
                    NewYear::Jan1,
                    false,
                    None,
                ),
            ),
            (
                "1822-11-00",
                date(
                    (1822, 11, 0),
                    Modifier::None,
                    Quality::None,
                    Calendar::Gregorian,
                    NewYear::Jan1,
                    false,
                    None,
                ),
            ),
            (
                "1900-01-01",
                date(
                    (1900, 1, 1),
                    Modifier::None,
                    Quality::None,
                    Calendar::Gregorian,
                    NewYear::Jan1,
                    false,
                    None,
                ),
            ),
            (
                "abt 1900-00-00",
                date(
                    (1900, 0, 0),
                    Modifier::About,
                    Quality::None,
                    Calendar::Gregorian,
                    NewYear::Jan1,
                    false,
                    None,
                ),
            ),
            (
                "bef 1914-01-01",
                date(
                    (1914, 1, 1),
                    Modifier::Before,
                    Quality::None,
                    Calendar::Gregorian,
                    NewYear::Jan1,
                    false,
                    None,
                ),
            ),
            (
                "aft 1945-01-01",
                date(
                    (1945, 1, 1),
                    Modifier::After,
                    Quality::None,
                    Calendar::Gregorian,
                    NewYear::Jan1,
                    false,
                    None,
                ),
            ),
            (
                "est 1900-01-01",
                date(
                    (1900, 1, 1),
                    Modifier::None,
                    Quality::Estimated,
                    Calendar::Gregorian,
                    NewYear::Jan1,
                    false,
                    None,
                ),
            ),
            (
                "calc 1900-01-01",
                date(
                    (1900, 1, 1),
                    Modifier::None,
                    Quality::Calculated,
                    Calendar::Gregorian,
                    NewYear::Jan1,
                    false,
                    None,
                ),
            ),
            (
                "-550-04-22",
                date(
                    (-550, 4, 22),
                    Modifier::None,
                    Quality::None,
                    Calendar::Gregorian,
                    NewYear::Jan1,
                    false,
                    None,
                ),
            ),
            (
                "abt -550-00-00",
                date(
                    (-550, 0, 0),
                    Modifier::About,
                    Quality::None,
                    Calendar::Gregorian,
                    NewYear::Jan1,
                    false,
                    None,
                ),
            ),
            (
                "1900-01-01 (Julian)",
                date(
                    (1900, 1, 1),
                    Modifier::None,
                    Quality::None,
                    Calendar::Julian,
                    NewYear::Jan1,
                    false,
                    None,
                ),
            ),
            (
                "1900-01-01 (Julian,Mar25)",
                date(
                    (1900, 1, 1),
                    Modifier::None,
                    Quality::None,
                    Calendar::Julian,
                    NewYear::Mar25,
                    false,
                    None,
                ),
            ),
            (
                "1900-01-01 (Mar1)",
                date(
                    (1900, 1, 1),
                    Modifier::None,
                    Quality::None,
                    Calendar::Gregorian,
                    NewYear::Mar1,
                    false,
                    None,
                ),
            ),
            (
                "1914-01-01 - 1918-04-01",
                date(
                    (1914, 1, 1),
                    Modifier::Range,
                    Quality::None,
                    Calendar::Gregorian,
                    NewYear::Jan1,
                    false,
                    Some((1918, 4, 1)),
                ),
            ),
            (
                "1822-11-01 - 1823-04-01",
                date(
                    (1822, 11, 1),
                    Modifier::Span,
                    Quality::None,
                    Calendar::Gregorian,
                    NewYear::Jan1,
                    false,
                    Some((1823, 4, 1)),
                ),
            ),
            (
                "1822-00-00 - 1824-00-00",
                date(
                    (1822, 0, 0),
                    Modifier::Range,
                    Quality::None,
                    Calendar::Gregorian,
                    NewYear::Jan1,
                    false,
                    Some((1824, 0, 0)),
                ),
            ),
            (
                "est 1914-01-01 - 1918-04-01",
                date(
                    (1914, 1, 1),
                    Modifier::Range,
                    Quality::Estimated,
                    Calendar::Gregorian,
                    NewYear::Jan1,
                    false,
                    Some((1918, 4, 1)),
                ),
            ),
            (
                "1914-01-01 - 1918-04-01 (Julian)",
                date(
                    (1914, 1, 1),
                    Modifier::Range,
                    Quality::None,
                    Calendar::Julian,
                    NewYear::Jan1,
                    false,
                    Some((1918, 4, 1)),
                ),
            ),
            (
                "-044-03-15",
                date(
                    (-44, 3, 15),
                    Modifier::None,
                    Quality::None,
                    Calendar::Gregorian,
                    NewYear::Jan1,
                    false,
                    None,
                ),
            ),
            (
                "-044-03-15 - -030-08-01",
                date(
                    (-44, 3, 15),
                    Modifier::Range,
                    Quality::None,
                    Calendar::Gregorian,
                    NewYear::Jan1,
                    false,
                    Some((-30, 8, 1)),
                ),
            ),
            (
                "1582/3-10-04 (Julian)",
                date(
                    (1583, 10, 4),
                    Modifier::None,
                    Quality::None,
                    Calendar::Gregorian,
                    NewYear::Jan1,
                    true,
                    None,
                ),
            ),
            (
                "1699/0-02-18 (Julian)",
                date(
                    (1700, 2, 18),
                    Modifier::None,
                    Quality::None,
                    Calendar::Julian,
                    NewYear::Jan1,
                    true,
                    None,
                ),
            ),
            (
                "-045/6-03-15 (Julian)",
                date(
                    (-44, 3, 15),
                    Modifier::None,
                    Quality::None,
                    Calendar::Julian,
                    NewYear::Jan1,
                    true,
                    None,
                ),
            ),
            (
                "est abt 1700-02-18 (Julian,Mar1)",
                date(
                    (1700, 2, 18),
                    Modifier::About,
                    Quality::Estimated,
                    Calendar::Julian,
                    NewYear::Mar1,
                    false,
                    None,
                ),
            ),
            (
                "0008-11-05 (French Republican)",
                date(
                    (8, 11, 5),
                    Modifier::None,
                    Quality::None,
                    Calendar::FrenchRepublican,
                    NewYear::Jan1,
                    false,
                    None,
                ),
            ),
            (
                "0000-05-15",
                date(
                    (0, 5, 15),
                    Modifier::None,
                    Quality::None,
                    Calendar::Gregorian,
                    NewYear::Jan1,
                    false,
                    None,
                ),
            ),
        ];
        for (expected, d) in cases {
            assert_eq!(&disp(d), expected, "date={d:?}");
        }
    }

    #[test]
    fn text_only_display_is_the_verbatim_text() {
        let mut d = date(
            (0, 0, 0),
            Modifier::TextOnly,
            Quality::None,
            Calendar::Gregorian,
            NewYear::Jan1,
            false,
            None,
        );
        d.display = "circa the harvest festival".to_string();
        assert_eq!(d.compute_display(), "circa the harvest festival");
        assert_eq!(d.text(), Some("circa the harvest festival"));
    }

    #[test]
    fn dual_dated_range_renders_both_endpoints_in_slash_form() {
        // Documented deviation: Gramps' __str__ shows only the start here.
        let d = date(
            (1850, 1, 1),
            Modifier::Span,
            Quality::Calculated,
            Calendar::Gregorian,
            NewYear::Jan1,
            true,
            Some((1850, 6, 30)),
        );
        assert_eq!(disp(&d), "calc 1849/0-01-01 - 1849/0-06-30 (Julian)");
    }

    #[test]
    fn iso_ymd_pads_like_python_percent_04d() {
        let d = date(
            (55, 1, 1),
            Modifier::None,
            Quality::None,
            Calendar::Gregorian,
            NewYear::Jan1,
            false,
            None,
        );
        assert_eq!(disp(&d), "0055-01-01");
        let d = date(
            (-550, 1, 1),
            Modifier::None,
            Quality::None,
            Calendar::Gregorian,
            NewYear::Jan1,
            false,
            None,
        );
        assert_eq!(disp(&d), "-550-01-01");
    }
}
