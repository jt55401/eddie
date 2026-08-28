use eddie::parse::strip_markdown;

#[test]
fn strips_inline_html_tags_as_whitespace_not_deletion() {
    let input = "<h2>Heading</h2><p>Hello <strong>world</strong>.</p>";
    let output = strip_markdown(input);

    // Tags become whitespace (not deleted outright) so adjacent block text
    // doesn't fuse into a single token.
    assert_eq!(output, "Heading Hello world.");
}

#[test]
fn removes_script_and_style_bodies() {
    let input = "<p>One</p><p>Two</p>\n<script>var x = 1; alert('hi');</script>\n<style>.a{color:red}</style>\nDone.";
    let output = strip_markdown(input);

    assert!(output.contains("One"));
    assert!(output.contains("Two"));
    assert!(output.contains("Done."));
    assert!(!output.contains("var x"));
    assert!(!output.contains("color:red"));
}
