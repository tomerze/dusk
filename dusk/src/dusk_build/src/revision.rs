pub(crate) fn short_revision(revision: &str) -> Option<String> {
    (revision.len() >= 16 && revision.bytes().all(|byte| byte.is_ascii_hexdigit()))
        .then(|| revision[..16].to_ascii_lowercase())
}

pub(crate) fn fail_without_revision(reason: &str) -> ! {
    eprintln!(
        "error: couldn't tell which git revision is being built: {reason}. Set DUSK_GIT_REV to \
         the revision, at least 16 hex digits, e.g. DUSK_GIT_REV=$(git rev-parse HEAD)"
    );
    std::process::exit(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_revision_is_cut_to_16_lowercase_hex_digits() {
        assert_eq!(
            short_revision("E7AE8B44f1c2d3e4a5b6c7d8e9f00112233445566").as_deref(),
            Some("e7ae8b44f1c2d3e4")
        );
        assert_eq!(
            short_revision("0123456789abcdef").as_deref(),
            Some("0123456789abcdef")
        );
    }

    #[test]
    fn anything_but_16_or_more_hex_digits_is_refused() {
        for revision in [
            "",
            "0123456789abcde",
            "main",
            "0123456789abcdefg",
            "0123456789abcde f",
        ] {
            assert_eq!(short_revision(revision), None, "{revision:?}");
        }
    }
}
