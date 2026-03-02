use eddie::parse::strip_markdown;

#[test]
fn strips_headings_and_links_from_markdown() {
    let input = "# Title\n\nUse [Eddie](https://example.com) to search docs.";
    let output = strip_markdown(input);

    assert!(!output.contains('#'));
    assert!(!output.contains("https://example.com"));
    assert!(output.contains("Title"));
    assert!(output.contains("Use Eddie to search docs."));
}
