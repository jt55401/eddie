use eddie::parse::strip_markdown;

#[test]
fn strips_links_but_preserves_heading_markers_for_the_chunker() {
    let input = "# Title\n\nUse [Eddie](https://example.com) to search docs.";
    let output = strip_markdown(input);

    // Headings are deliberately NOT stripped here: the chunker consumes them
    // to find section boundaries and folds the heading text into the chunk
    // body itself. Stripping them here was the root cause of the critical
    // "section metadata is always None" bug.
    assert!(output.contains('#'));
    assert!(!output.contains("https://example.com"));
    assert!(output.contains("Title"));
    assert!(output.contains("Use Eddie to search docs."));
}

#[test]
fn drops_images_entirely_without_alt_text() {
    let input = "Look: ![a diagram](diagram.png) here.";
    let output = strip_markdown(input);

    assert!(!output.contains("diagram"));
    assert!(output.contains("Look:"));
    assert!(output.contains("here."));
}
