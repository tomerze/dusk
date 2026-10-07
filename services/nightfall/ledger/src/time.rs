use time::OffsetDateTime;
use time::macros::format_description;

pub fn now() -> String {
    format(OffsetDateTime::now_utc())
}

pub fn format(moment: OffsetDateTime) -> String {
    moment
        .to_offset(time::UtcOffset::UTC)
        .format(format_description!(
            "[year]-[month]-[day]T[hour]:[minute]:[second].[subsecond digits:9]Z"
        ))
        .expect("every UTC moment between years 0 and 9999 formats")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_utc_with_nine_fractional_digits() {
        let moment = OffsetDateTime::from_unix_timestamp_nanos(1_791_278_043_512_408_117).unwrap();
        assert_eq!(format(moment), "2026-10-06T09:14:03.512408117Z");
        assert_eq!(now().len(), 30);
    }
}
