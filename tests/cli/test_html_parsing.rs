use eddie::parse::strip_markdown;

#[test]
fn strips_inline_html_tags() {
    let input = "<h2>Heading</h2><p>Hello <strong>world</strong>.</p>";
    let output = strip_markdown(input);

    assert_eq!(output, "HeadingHello world.");
}
