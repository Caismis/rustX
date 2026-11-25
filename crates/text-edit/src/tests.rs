//! Representative regressions of the shared edit kernel — the same
//! cases the Agent's Edit tool and the Execution Client's
//! `PrepareEdit` both exercise through `apply`.

use super::*;

fn edit(old_text: &str, new_text: &str) -> Replacement {
    Replacement {
        old_text: old_text.to_owned(),
        new_text: new_text.to_owned(),
    }
}

#[test]
fn disjoint_replacements_apply_against_one_original_snapshot() {
    let updated = apply(
        "alpha\nbeta\ngamma",
        &[edit("alpha", "A"), edit("gamma", "G")],
    )
    .unwrap();
    assert_eq!(updated, "A\nbeta\nG");
}

#[test]
fn bom_and_each_line_ending_style_are_preserved() {
    for (original, expected) in [
        ("\u{FEFF}one\ntwo\n", "\u{FEFF}one\nTWO\n"),
        ("\u{FEFF}one\r\ntwo\r\n", "\u{FEFF}one\r\nTWO\r\n"),
        ("\u{FEFF}one\rtwo\r", "\u{FEFF}one\rTWO\r"),
    ] {
        assert_eq!(apply(original, &[edit("two", "TWO")]).unwrap(), expected);
    }
}

#[test]
fn edits_with_carriage_returns_normalize_to_the_snapshot() {
    // A model-supplied CRLF anchor still matches the LF-normalized
    // snapshot, and its replacement restores the file's style.
    let updated = apply("a\r\nb\r\n", &[edit("a\r\nb", "x\ny")]).unwrap();
    assert_eq!(updated, "x\r\ny\r\n");
}

#[test]
fn fuzzy_matching_normalizes_quotes_dashes_spaces_and_trailing_whitespace() {
    let original = "\u{2018}quote\u{2019} \u{2014} value\u{00A0}  \nnext";
    let updated = apply(original, &[edit("'quote' - value  ", "changed")]).unwrap();
    assert_eq!(
        updated, "changed\u{00A0}  \nnext",
        "fuzzy matching must not normalize source text outside the matched range"
    );
}

#[test]
fn exact_matching_wins_even_when_the_fuzzy_view_would_be_ambiguous() {
    let updated = apply("\u{2018}same\u{2019}\n'same'", &[edit("'same'", "changed")]).unwrap();
    assert_eq!(updated, "\u{2018}same\u{2019}\nchanged");
}

#[test]
fn exact_and_fuzzy_matches_resolve_independently_per_edit() {
    // Edit 1 has one exact hit; edit 2 only exists in the fuzzy view.
    // Each resolves on its own; both ranges come from the original.
    let original = "\u{2018}same\u{2019}\n'same'\n\u{2018}other\u{2019}";
    let updated = apply(original, &[edit("'same'", "one"), edit("'other'", "two")]).unwrap();
    assert_eq!(updated, "\u{2018}same\u{2019}\none\ntwo");
}

#[test]
fn a_missing_anchor_is_missing_not_ambiguous() {
    assert_eq!(
        apply("content", &[edit("absent", "x")]),
        Err(EditFailure::Missing { index: 0 })
    );
}

#[test]
fn an_ambiguous_anchor_reports_the_occurrence_count() {
    assert_eq!(
        apply("dup dup", &[edit("dup", "x")]),
        Err(EditFailure::Ambiguous {
            index: 0,
            occurrences: 2
        })
    );
}

#[test]
fn overlapping_edits_fail_atomically() {
    assert_eq!(
        apply("abcdef", &[edit("abc", "x"), edit("cde", "y")]),
        Err(EditFailure::Overlap {
            first: 0,
            second: 1
        })
    );
}

#[test]
fn a_noop_replacement_is_a_no_change_failure() {
    assert_eq!(
        apply("same", &[edit("same", "same")]),
        Err(EditFailure::NoChange)
    );
}

#[test]
fn an_empty_request_and_empty_anchor_are_typed() {
    assert_eq!(apply("x", &[]), Err(EditFailure::EmptyEdits));
    assert_eq!(
        apply("x", &[edit("", "y")]),
        Err(EditFailure::EmptyAnchor { index: 0 })
    );
}

#[test]
fn an_unsafe_partial_normalization_range_is_refused() {
    // `ﬃ` decomposes to `ffi`: an anchor of just `fi` selects part of
    // the ligature's source span — one outside normalized scalar claims
    // the remaining byte, so the range is unsafe rather than silently
    // widening.
    assert_eq!(
        apply("a\u{FB03}b", &[edit("fi", "x")]),
        Err(EditFailure::UnsafeNormalization { index: 0 })
    );
    // But the full ligature's normalized form is a safe range.
    assert_eq!(apply("a\u{FB03}b", &[edit("ffi", "x")]).unwrap(), "axb");
}

#[test]
fn fuzzy_anchors_still_apply_to_original_bytes() {
    // A combining-mark reorder must not corrupt the source: the planned
    // range is provenance-mapped back to the original snapshot.
    let original = "caf\u{0065}\u{0301} done";
    let updated = apply(original, &[edit("cafe\u{0301}", "cafe")]).unwrap();
    assert_eq!(updated, "cafe done");
}

#[test]
fn fuzzy_ambiguity_is_reported_with_its_occurrence_count() {
    // Neither is an exact anchor; in the fuzzy view both normalize
    // identically, so the single edit is ambiguous.
    assert_eq!(
        apply("\u{2018}a\u{2019}\n\u{2019}a\u{2018}", &[edit("'a'", "x")]),
        Err(EditFailure::Ambiguous {
            index: 0,
            occurrences: 2
        })
    );
}

#[test]
fn request_and_source_bounds_refuse_before_work() {
    // Too many replacements.
    let many: Vec<Replacement> = (0..=MAX_EDIT_REPLACEMENTS)
        .map(|i| edit(&format!("a{i}"), "b"))
        .collect();
    assert_eq!(apply("x", &many), Err(EditFailure::TooLarge));
    // Aggregate old+new over the request cap.
    let big = "x".repeat(MAX_EDIT_REQUEST_BYTES);
    assert_eq!(
        apply("x", &[edit("a", &big), edit("b", &big)]),
        Err(EditFailure::TooLarge)
    );
    // Source over the file cap — refused before any edit work.
    let source = "x".repeat(MAX_EDIT_FILE_BYTES + 1);
    assert_eq!(
        apply(&source, &[edit("x", "y")]),
        Err(EditFailure::TooLarge)
    );
    // Output over the file cap.
    let source = "x".repeat(MAX_EDIT_FILE_BYTES);
    assert_eq!(
        apply(&source, &[edit("x", &big)]),
        Err(EditFailure::TooLarge)
    );
}

#[test]
fn a_pathological_combining_run_is_bounded_not_unbounded() {
    // A long run of combining marks is the reorder-buffer stress case:
    // it stays under the provenance cap and simply works — the bound
    // exists so a deliberately hostile source refuses rather than grow.
    let mut source = String::from("base ");
    for _ in 0..64 {
        source.push('\u{0301}');
    }
    let updated = apply(&source, &[edit("base", "root")]).unwrap();
    assert!(updated.starts_with("root "));
}

#[test]
fn indices_report_the_failing_replacement() {
    assert_eq!(
        apply("abc", &[edit("abc", "ABC"), edit("zzz", "q")]),
        Err(EditFailure::Missing { index: 1 })
    );
}

#[test]
fn the_output_cap_counts_crlf_expansion_before_allocating() {
    // `K` "a\r\n" rows plus a "b" tail: the output at exactly
    // MAX_EDIT_FILE_BYTES is accepted; one byte over is TooLarge even
    // though the LF-normalized output is well under the cap.
    let k = (MAX_EDIT_FILE_BYTES - 2) / 3;
    let source = "a\r\n".repeat(k) + "b";
    assert_eq!(source.len(), MAX_EDIT_FILE_BYTES - 1);

    let updated = apply(&source, &[edit("b", "cc")]).unwrap();
    assert_eq!(updated.len(), MAX_EDIT_FILE_BYTES);
    assert!(updated.ends_with("cc"));

    // The same source with a one-byte-longer replacement: the LF-
    // normalized output would be 1 398 103 bytes — far under the cap —
    // but CRLF restoration pushes the real output one byte over.
    assert_eq!(
        apply(&source, &[edit("b", "ccc")]),
        Err(EditFailure::TooLarge)
    );
}
