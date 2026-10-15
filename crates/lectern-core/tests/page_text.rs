//! HTML pages: the review text of a page is the text its bridge reports, block by block, and each
//! block's source line is found, as best it can be, where its leading text is in the page's HTML.

use lectern_core::review::text::{page_text_len, PageBlock, TextMap, MAX_PAGE_TEXT};

/// An invented report, as `fixtures/vault/artifacts/report.html` is laid out, but with a script
/// that builds its text from words, so no leading text of what it adds is in the source.
const REPORT: &str = r#"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<title>Weekly report</title>
<link rel="stylesheet" href="style.css">
</head>
<body>
<h1>Weekly report</h1>
<p>An invented report for Lectern's tests: the lanternfish survey, week by week.</p>
<h2>Results</h2>
<p>Sign-ups rose steadily, but conversion dipped in week 3 before recovering.</p>
<table>
<tr><th>Week</th><th>Sign-ups</th><th>Conversion</th></tr>
<tr><td>1</td><td>120</td><td>4.1%</td></tr>
<tr><td>2</td><td>134</td><td>4.3%</td></tr>
<tr><td>3</td><td>151</td><td>3.2%</td></tr>
</table>
<h2>Next steps</h2>
<p>Check the week 3 dip against the survey calendar, as the <a href="other.html">other page</a> does, or ask the <a href="https://example.com">survey site</a>.</p>
<script>
  var note = document.createElement("p");
  note.textContent = ["Added", "by", "a", "script"].join(" ");
  document.body.appendChild(note);
</script>
</body>
</html>
"#;

fn block(text: &str) -> PageBlock {
    PageBlock {
        text: text.to_owned(),
        heading_level: None,
    }
}

fn heading(level: u8, text: &str) -> PageBlock {
    PageBlock {
        text: text.to_owned(),
        heading_level: Some(level),
    }
}

/// The blocks the bridge reports for `REPORT`, its script's paragraph last.
fn report_blocks() -> Vec<PageBlock> {
    let mut blocks = vec![
        heading(1, "Weekly report"),
        block("An invented report for Lectern's tests: the lanternfish survey, week by week."),
        heading(2, "Results"),
        block("Sign-ups rose steadily, but conversion dipped in week 3 before recovering."),
    ];
    for cell in [
        "Week",
        "Sign-ups",
        "Conversion",
        "1",
        "120",
        "4.1%",
        "2",
        "134",
        "4.3%",
        "3",
        "151",
        "3.2%",
    ] {
        blocks.push(block(cell));
    }
    blocks.extend([
        heading(2, "Next steps"),
        block("Check the week 3 dip against the survey calendar, as the other page does, or ask the survey site."),
        block("Added by a script"),
    ]);
    blocks
}

/// Each block of the text map as `(start line, end line, text)`.
fn blocks(map: &TextMap) -> Vec<(u32, u32, &str)> {
    map.blocks()
        .iter()
        .map(|b| (b.start_line, b.end_line, map.block_text(b)))
        .collect()
}

/// The source line of each block, in order.
fn lines(map: &TextMap) -> Vec<u32> {
    map.blocks().iter().map(|b| b.start_line).collect()
}

#[test]
fn the_text_is_the_blocks_joined_as_the_bridge_joins_them() {
    let page = report_blocks();
    let map = TextMap::from_blocks(&page, REPORT);
    let joined: Vec<&str> = page.iter().map(|b| b.text.as_str()).collect();
    assert_eq!(map.text(), joined.join(" "));
    assert_eq!(map.blocks().len(), page.len());
}

/// Every block is on the line its leading text is first found on, after the block before it, and
/// the body's text is looked for in the body: the title repeating the first heading isn't it.
#[test]
fn each_block_is_on_the_line_its_leading_text_is_on() {
    let map = TextMap::from_blocks(&report_blocks(), REPORT);
    assert_eq!(
        blocks(&map)[..4],
        [
            (9, 9, "Weekly report"),
            (
                10,
                10,
                "An invented report for Lectern's tests: the lanternfish survey, week by week."
            ),
            (11, 11, "Results"),
            (
                12,
                12,
                "Sign-ups rose steadily, but conversion dipped in week 3 before recovering."
            ),
        ]
    );
    // Table cells, short as they are, follow one another along their rows.
    assert_eq!(
        lines(&map)[4..16],
        [14, 14, 14, 15, 15, 15, 16, 16, 16, 17, 17, 17]
    );
    // A link inside the paragraph comes after its first 40 characters.
    assert_eq!(lines(&map)[16..18], [19, 20]);
}

/// Text only a script makes isn't in the source: it takes the line of the block before it.
#[test]
fn a_block_not_in_the_source_takes_the_line_before_it() {
    let map = TextMap::from_blocks(&report_blocks(), REPORT);
    assert_eq!(blocks(&map).last(), Some(&(20, 20, "Added by a script")));
    // Nothing found before it: line 1.
    let map = TextMap::from_blocks(
        &[
            block("Made up by a script"),
            block("A paragraph in the page"),
        ],
        "<body>\n\n<p>A paragraph in the page</p>\n",
    );
    assert_eq!(lines(&map), [1, 3]);
    // A script holding the words as the page shows them is where they are found.
    let fixture = REPORT.replace(
        r#"["Added", "by", "a", "script"].join(" ")"#,
        r#""Added by a script""#,
    );
    let map = TextMap::from_blocks(&report_blocks(), &fixture);
    assert_eq!(lines(&map).last(), Some(&23));
}

/// Inline markup cuts the leading text short: the longest prefix found, at a word boundary and at
/// least 8 characters, places the block.
#[test]
fn inline_markup_leaves_a_shorter_prefix_to_find() {
    let source = "<body>\n<p>Intro.</p>\n\n<p>Sign-ups <strong>rose</strong> steadily.</p>\n<p>Tide <em>tables</em> by hour.</p>\n";
    let map = TextMap::from_blocks(
        &[
            block("Intro."),
            block("Sign-ups rose steadily."),
            // Markup inside its shortest prefix ("Tide tables"): not found, it takes the line
            // before it.
            block("Tide tables by hour."),
        ],
        source,
    );
    assert_eq!(lines(&map), [2, 4, 4]);
}

/// The source's `&amp;`, `&lt;`, `&gt;`, `&quot;` and `&#39;` are the characters the page shows.
#[test]
fn the_five_entities_are_read_as_their_characters() {
    let source = "<body>\n<p>Intro.</p>\n<p>Q&amp;A on &lt;tags&gt;, &quot;quotes&quot; and &#39;marks&#39;</p>\n<p>Salt &amp;amp; pepper</p>\n";
    let map = TextMap::from_blocks(
        &[
            block("Intro."),
            block("Q&A on <tags>, \"quotes\" and 'marks'"),
            // Decoded once: `&amp;amp;` is `&amp;`.
            block("Salt &amp; pepper"),
        ],
        source,
    );
    assert_eq!(lines(&map), [2, 3, 4]);
}

/// The search goes on from where the block before it was found, so lines never go back up, and a
/// phrase found twice is placed by its own turn.
#[test]
fn lines_follow_the_order_of_the_blocks() {
    let source = "<body>\n<p>Repeated line of text</p>\n<p>Middle paragraph here</p>\n<p>Repeated line of text</p>\n";
    let map = TextMap::from_blocks(
        &[
            block("Middle paragraph here"),
            block("Repeated line of text"),
        ],
        source,
    );
    assert_eq!(lines(&map), [3, 4]);
}

/// A page without a `<body>` tag is searched from its start; text with no word boundary in its
/// first 40 characters (one long word, or a language written without spaces) is looked for by
/// those 40.
#[test]
fn a_page_without_a_body_and_text_without_spaces() {
    let cjk = "東京の調査は週ごとに行われ、三週目に転換率が下がったが、その後は回復した。来週も同じ方法で続ける。";
    let source = format!("<p>Intro paragraph.</p>\n<p>{cjk}</p>\n");
    let map = TextMap::from_blocks(&[block("Intro paragraph."), block(cjk)], &source);
    assert_eq!(lines(&map), [1, 2]);
}

/// Headings build the heading path, as a note's do.
#[test]
fn headings_build_the_heading_path() {
    let map = TextMap::from_blocks(&report_blocks(), REPORT);
    let headings: Vec<(u8, &str, u32)> = map
        .headings()
        .iter()
        .map(|h| (h.level, h.text.as_str(), h.line))
        .collect();
    assert_eq!(
        headings,
        [
            (1, "Weekly report", 9),
            (2, "Results", 11),
            (2, "Next steps", 19)
        ]
    );
    assert_eq!(map.heading_path_at(10), ["Weekly report"]);
    assert_eq!(map.heading_path_at(12), ["Weekly report", "Results"]);
    assert_eq!(map.heading_path_at(20), ["Weekly report", "Next steps"]);
    // A level outside 1 to 6 makes no heading.
    let map = TextMap::from_blocks(&[heading(0, "Zero"), heading(7, "Seven")], "");
    assert!(map.headings().is_empty());
}

/// Whitespace is collapsed and trimmed, and an empty block is dropped, as the bridge does.
#[test]
fn blocks_are_normalised_and_empty_ones_dropped() {
    let map = TextMap::from_blocks(&[block("  Two\n  words "), block(" \t"), block("Next")], "");
    assert_eq!(map.text(), "Two words Next");
    assert_eq!(blocks(&map), [(1, 1, "Two words"), (1, 1, "Next")]);
}

/// What the cap measures: UTF-16 code units, as the bridge and the UI count, and a space between
/// each two blocks.
#[test]
fn the_page_text_is_measured_in_utf16_units() {
    assert_eq!(MAX_PAGE_TEXT, 2 * 1024 * 1024);
    assert_eq!(page_text_len(&[]), 0);
    assert_eq!(page_text_len(&[block("ab"), block("€𝄞")]), 2 + 1 + 3);
    assert_eq!(page_text_len(&[block(""), block("")]), 1);
}

/// The JSON the UI sends: `headingLevel` only for a heading.
#[test]
fn page_blocks_come_as_the_bridge_reports_them() {
    let blocks: Vec<PageBlock> =
        serde_json::from_str(r#"[{"text":"Results","headingLevel":2},{"text":"Body"}]"#).unwrap();
    assert_eq!(blocks, [heading(2, "Results"), block("Body")]);
    assert_eq!(
        serde_json::to_string(&blocks).unwrap(),
        r#"[{"text":"Results","headingLevel":2},{"text":"Body"}]"#
    );
}
